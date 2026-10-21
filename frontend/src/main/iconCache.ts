/**
 * 站点 / 邮件图标缓存：`astorm-icon://` 协议 + 本地磁盘缓存。
 *
 * 背景：相关资料卡片、邮件头像各自用 `<img src="https://…">` 直取 favicon，只有进程内的
 * URL / 失败状态缓存，图片字节不落盘。应用重启、组件重挂载后又会重新请求；Google /
 * DuckDuckGo / Gravatar 在国内容易**挂起而不是快速失败**，重复请求的代价很高。
 *
 * 这里把图标请求收进主进程：
 *   - 按 hostname（固定域名）做 key，本地缓存 7 天；
 *   - 失败也写一个短 TTL 的 miss 标记，避免每次挂载都重新等超时；
 *   - 同一 key 的并发请求合并成一次，过期后抓取失败可以继续返回旧图（图标过期无害）；
 *   - 抓取走独立的内存 session，不带用户的浏览 Cookie。
 *
 * 协议形态（标准 scheme，注册在 index.ts 的 registerSchemesAsPrivileged）：
 *   astorm-icon://favicon/<host>?sz=64   站点图标（站点 /favicon.ico → Google 服务）
 *   astorm-icon://mail/<domain>?sz=68    邮件发件域图标（域名 → 二级域 → DuckDuckGo）
 *   astorm-icon://avatar/<sha256>?sz=68  Gravatar 头像
 *
 * 缓存文件直接落在 `<系统目录>/cache/icons/`，文件名是 `v1-<sha1>`，用文件 mtime 表示
 * 抓取时间；不维护 JSON 索引，避免索引与文件不一致。字节类型在读取时按 magic bytes 判定。
 */
import { protocol, session, type Session } from 'electron'
import { createHash } from 'node:crypto'
import { promises as fs } from 'node:fs'
import { join } from 'node:path'
import { systemDir } from './systemPaths'

const SCHEME = 'astorm-icon'
/** 成功图标缓存时长：7 天。 */
const TTL_MS = 7 * 24 * 60 * 60 * 1000
/** 失败标记缓存时长：1 天（网络抖动导致的失败不该锁死一周）。 */
const MISS_TTL_MS = 24 * 60 * 60 * 1000
/** 过期后重新抓取失败时，旧图最多再沿用 30 天。 */
const STALE_MAX_MS = 30 * 24 * 60 * 60 * 1000
/** 单张图标字节上限；超过按失败处理。 */
const MAX_BYTES = 512 * 1024
/** 单次上游请求超时。 */
const SITE_TIMEOUT_MS = 1500
const SERVICE_TIMEOUT_MS = 2500
/** 同时抓取的上游请求上限，避免一次打开面板打出几十个连接。 */
const MAX_CONCURRENCY = 6
/** 缓存文件上限；超出时按 mtime 删最旧的。 */
const MAX_ENTRIES = 1000
/** 启动清理时删除超过这么久没有更新过的文件。 */
const MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000

type IconKind = 'favicon' | 'mail' | 'avatar'

interface IconBytes {
  bytes: Buffer
  mime: string
}

type CacheHit =
  | { status: 'fresh'; ageMs: number; icon: IconBytes }
  /** `refresh` = 过期后需要尝试刷新；false 表示刚刷新失败过，先用旧图且暂时不再打网络 */
  | { status: 'stale'; ageMs: number; refresh: boolean; icon: IconBytes }
  | { status: 'miss'; ageMs: number }
  | { status: 'none' }

/** 独立 session：不落盘、不带用户浏览 Cookie（只用于抓图标）。 */
let iconSession: Session | null = null
function fetchSession(): Session {
  if (!iconSession) iconSession = session.fromPartition('astorm-icons')
  return iconSession
}

/** 同一 key 的并发抓取合并。 */
const inflight = new Map<string, Promise<IconBytes | null>>()

/** 并发槽位。 */
let activeFetches = 0
const fetchQueue: Array<() => void> = []

async function withSlot<T>(fn: () => Promise<T>): Promise<T> {
  if (activeFetches >= MAX_CONCURRENCY) {
    await new Promise<void>((resolve) => fetchQueue.push(resolve))
  }
  activeFetches += 1
  try {
    return await fn()
  } finally {
    activeFetches -= 1
    fetchQueue.shift()?.()
  }
}

/** 图标缓存根目录。 */
function cacheRoot(): string {
  return join(systemDir(), 'cache', 'icons')
}

function cacheKey(kind: IconKind, key: string): string {
  return createHash('sha1').update(`v1\0${kind}\0${key}`).digest('hex')
}

function cachePath(kind: IconKind, key: string): string {
  return join(cacheRoot(), cacheKey(kind, key))
}

/** 把任意输入归一成 hostname（允许传完整 URL）；不接受 IP 字面量、端口和空 host。 */
export function normalizeIconHost(input: string): string {
  const raw = input.trim()
  if (!raw) return ''
  let host = ''
  try {
    host = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(raw) ? raw : `https://${raw}`).hostname
  } catch {
    return ''
  }
  host = host.toLowerCase().replace(/\.$/, '')
  if (!host || host.includes('..') || host.startsWith('.') || host.endsWith('.')) return ''
  // 不抓本机 / IP 字面量；端口在 URL 解析时会被丢掉（这里始终重新拼 https://host）。
  if (host === 'localhost' || host.endsWith('.localhost')) return ''
  if (host.includes(':') || /^\d{1,3}(?:\.\d{1,3}){3}$/.test(host)) return ''
  if (!/^[a-z0-9.-]+$/.test(host)) return ''
  return host
}

/** 邮件域名的候选链：整域 → 末两段（沿用现有 MailAvatar 的语义）。 */
export function mailDomainCandidates(input: string): string[] {
  const domain = normalizeIconHost(input)
  if (!domain) return []
  const parts = domain.split('.')
  if (parts.length > 2) return [domain, parts.slice(-2).join('.')]
  return [domain]
}

function isHexHash(value: string): boolean {
  return /^[a-f0-9]{64}$/.test(value)
}

/** 按 magic bytes 判定图片类型；非图片（如 HTML 错误页）一律拒绝。 */
function sniffImage(buf: Buffer): string | null {
  if (buf.length >= 8 && buf.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) {
    return 'image/png'
  }
  if (buf.length >= 3 && buf[0] === 0xff && buf[1] === 0xd8 && buf[2] === 0xff) return 'image/jpeg'
  if (buf.length >= 6) {
    const head = buf.toString('ascii', 0, 6)
    if (head === 'GIF87a' || head === 'GIF89a') return 'image/gif'
  }
  if (
    buf.length >= 12 &&
    buf.toString('ascii', 0, 4) === 'RIFF' &&
    buf.toString('ascii', 8, 12) === 'WEBP'
  ) {
    return 'image/webp'
  }
  if (buf.length >= 4 && buf[0] === 0 && buf[1] === 0 && buf[2] === 1 && buf[3] === 0) {
    return 'image/x-icon'
  }
  if (buf.length >= 2 && buf.toString('ascii', 0, 2) === 'BM') return 'image/bmp'
  const text = buf.subarray(0, 512).toString('utf8').trimStart().toLowerCase()
  if (text.startsWith('<svg') || (text.startsWith('<?xml') && text.includes('<svg'))) {
    return 'image/svg+xml'
  }
  return null
}

async function readHit(kind: IconKind, key: string): Promise<CacheHit> {
  const path = cachePath(kind, key)
  const missPath = `${path}.miss`
  const now = Date.now()

  try {
    const stat = await fs.stat(path)
    if (!stat.isFile() || stat.size === 0 || stat.size > MAX_BYTES) {
      await fs.rm(path, { force: true })
      return { status: 'none' }
    }
    const ageMs = now - stat.mtimeMs
    const bytes = await fs.readFile(path)
    const mime = sniffImage(bytes)
    if (!mime) {
      await fs.rm(path, { force: true })
      return { status: 'none' }
    }
    if (ageMs <= TTL_MS) return { status: 'fresh', ageMs, icon: { bytes, mime } }
    // 过期：若刚刷新失败过（有新鲜的 miss 标记），先继续用旧图，不再反复打网络。
    let refresh = true
    try {
      const missStat = await fs.stat(missPath)
      if (now - missStat.mtimeMs <= MISS_TTL_MS) refresh = false
    } catch {
      /* 没有 miss 标记 → 正常刷新 */
    }
    return { status: 'stale', ageMs, refresh, icon: { bytes, mime } }
  } catch {
    // 没有成功缓存，再看失败标记。
  }

  try {
    const stat = await fs.stat(missPath)
    const ageMs = now - stat.mtimeMs
    if (ageMs <= MISS_TTL_MS) return { status: 'miss', ageMs }
    await fs.rm(missPath, { force: true })
  } catch {
    /* 没有失败标记 */
  }
  return { status: 'none' }
}

async function saveHit(kind: IconKind, key: string, icon: IconBytes): Promise<void> {
  await fs.mkdir(cacheRoot(), { recursive: true })
  const path = cachePath(kind, key)
  await fs.writeFile(path, icon.bytes)
  await fs.rm(`${path}.miss`, { force: true })
}

async function saveMiss(kind: IconKind, key: string): Promise<void> {
  await fs.mkdir(cacheRoot(), { recursive: true })
  const path = cachePath(kind, key)
  // **不删旧图**：过期刷新失败时继续用旧图兜底；miss 只负责阻止短时间内的重复抓取。
  await fs.writeFile(`${path}.miss`, '')
}

async function fetchWithTimeout(url: string, timeoutMs: number): Promise<Response> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)
  try {
    return await fetchSession().fetch(url, {
      method: 'GET',
      redirect: 'follow',
      credentials: 'omit',
      signal: controller.signal,
      headers: { accept: 'image/*,*/*;q=0.8' }
    })
  } finally {
    clearTimeout(timer)
  }
}

async function fetchImage(url: string, timeoutMs: number): Promise<IconBytes | null> {
  try {
    const res = await fetchWithTimeout(url, timeoutMs)
    if (!res.ok) return null
    const declared = Number(res.headers.get('content-length') ?? '')
    if (Number.isFinite(declared) && declared > MAX_BYTES) return null
    const bytes = Buffer.from(await res.arrayBuffer())
    if (!bytes.length || bytes.length > MAX_BYTES) return null
    const mime = sniffImage(bytes)
    return mime ? { bytes, mime } : null
  } catch {
    return null
  }
}

async function fetchIcon(kind: IconKind, key: string): Promise<IconBytes | null> {
  if (kind === 'avatar') {
    if (!isHexHash(key)) return null
    return fetchImage(`https://www.gravatar.com/avatar/${key}?d=404&s=128`, SERVICE_TIMEOUT_MS)
  }

  const domains = kind === 'mail' ? mailDomainCandidates(key) : [normalizeIconHost(key)]
  if (!domains[0]) return null

  // 顺序关键：站点直取放最前——第三方图标服务在国内容易长时间挂起。
  for (const domain of domains) {
    const got = await fetchImage(`https://${domain}/favicon.ico`, SITE_TIMEOUT_MS)
    if (got) return got
  }
  if (kind === 'mail') {
    for (const domain of domains) {
      const got = await fetchImage(
        `https://icons.duckduckgo.com/ip3/${encodeURIComponent(domain)}.ico`,
        SERVICE_TIMEOUT_MS
      )
      if (got) return got
    }
    return null
  }
  return fetchImage(
    `https://www.google.com/s2/favicons?domain=${encodeURIComponent(key)}&sz=64`,
    SERVICE_TIMEOUT_MS
  )
}

/** 同一 key 只抓一次；成功/失败都落盘。 */
function loadIcon(kind: IconKind, key: string): Promise<IconBytes | null> {
  const id = `${kind}:${key}`
  const existing = inflight.get(id)
  if (existing) return existing
  const task = withSlot(async () => {
    const icon = await fetchIcon(kind, key)
    try {
      if (icon) await saveHit(kind, key, icon)
      else await saveMiss(kind, key)
    } catch {
      /* 落盘失败不影响本次返回 */
    }
    return icon
  }).finally(() => {
    inflight.delete(id)
  })
  inflight.set(id, task)
  return task
}

function imageResponse(icon: IconBytes, maxAgeMs: number): Response {
  return new Response(icon.bytes, {
    headers: {
      'content-type': icon.mime,
      'cache-control': `public, max-age=${Math.max(0, Math.floor(maxAgeMs / 1000))}`
    }
  })
}

function notFound(maxAgeMs: number): Response {
  return new Response('Not Found', {
    status: 404,
    headers: { 'cache-control': `public, max-age=${Math.max(0, Math.floor(maxAgeMs / 1000))}` }
  })
}

async function resolveIcon(kind: IconKind, key: string): Promise<Response> {
  const hit = await readHit(kind, key)
  if (hit.status === 'fresh') return imageResponse(hit.icon, TTL_MS - hit.ageMs)
  if (hit.status === 'miss') return notFound(MISS_TTL_MS - hit.ageMs)
  // 过期且刚刷新失败过：先用旧图，等 miss 过期后再重试。
  if (hit.status === 'stale' && !hit.refresh) return imageResponse(hit.icon, 0)

  const icon = await loadIcon(kind, key)
  if (icon) return imageResponse(icon, TTL_MS)
  // 网络失败时继续用旧图（最多 30 天），避免图标突然消失。
  if (hit.status === 'stale' && hit.ageMs <= STALE_MAX_MS) return imageResponse(hit.icon, 0)
  return notFound(MISS_TTL_MS)
}

/** 启动后清理过期文件；顺带按数量上限淘汰最旧的一批。 */
async function cleanupCache(): Promise<void> {
  const root = cacheRoot()
  let names: string[]
  try {
    names = await fs.readdir(root)
  } catch {
    return
  }
  const now = Date.now()
  const files: Array<{ path: string; mtimeMs: number }> = []
  for (const name of names) {
    const path = join(root, name)
    try {
      const stat = await fs.stat(path)
      if (!stat.isFile()) continue
      if (now - stat.mtimeMs > MAX_AGE_MS) {
        await fs.rm(path, { force: true })
        continue
      }
      files.push({ path, mtimeMs: stat.mtimeMs })
    } catch {
      /* 忽略读不到的文件 */
    }
  }
  if (files.length <= MAX_ENTRIES) return
  files.sort((a, b) => a.mtimeMs - b.mtimeMs)
  for (const file of files.slice(0, files.length - MAX_ENTRIES)) {
    await fs.rm(file.path, { force: true }).catch(() => undefined)
  }
}

/** 注册 `astorm-icon://` 协议（scheme 权限在 app ready 前注册；此处逻辑在 ready 后调用）。 */
export function registerIconProtocol(): void {
  protocol.handle(SCHEME, async (request) => {
    let url: URL
    try {
      url = new URL(request.url)
    } catch {
      return notFound(0)
    }
    const kind = url.hostname as IconKind
    let key = ''
    try {
      key = decodeURIComponent(url.pathname.replace(/^\/+/, '')).trim()
    } catch {
      return notFound(0)
    }
    if (!key) return notFound(0)

    if (kind === 'favicon' || kind === 'mail') {
      const host = normalizeIconHost(key)
      if (!host) return notFound(0)
      return resolveIcon(kind, host)
    }
    if (kind === 'avatar') {
      if (!isHexHash(key.toLowerCase())) return notFound(0)
      return resolveIcon('avatar', key.toLowerCase())
    }
    return notFound(0)
  })

  void cleanupCache()
  // 长驻进程也要有上限：定时清理过期文件，避免只依赖下次启动。
  setInterval(() => void cleanupCache(), 6 * 60 * 60 * 1000).unref()
}
