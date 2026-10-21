/**
 * 内置浏览器数据：从本机 Chrome 导入 Cookie / 清理缓存与站点数据。
 *
 * 内置浏览器是渲染层里的跨域 `<iframe>`，与宿主 UI 共用 `session.defaultSession`
 * （全仓库唯一的会话；没有 partition），所以 Cookie 只能由主进程写进这个会话。
 * Rust 后端负责「读 Chrome 的 SQLite + 解密」（`astorm_core::browser`），本模块负责
 * 「写进 cookie jar / 清理」，因此导入结果里的明文只在本进程内流转。
 *
 * 两个必须记住的约束（详见各函数注释）：
 * 1. 导入时强制 `SameSite=None + Secure`，否则跨域 iframe 里站点自身的 Lax/Strict
 *    会话 Cookie 根本不会被带上，导入等于白导；
 * 2. 清理时**必须按站点 origin 限定范围**——默认会话与宿主 UI 共用，无条件
 *    `clearStorageData()` 会连应用自己的 localStorage 一起抹掉。
 */
import { session } from 'electron'
import { napiCall } from './napi'

/** Rust 侧 `import_chrome_cookies` 的返回（napi object，camelCase）。 */
interface NapiChromeCookie {
  hostKey: string
  name: string
  value: string
  path: string
  secure: boolean
  httpOnly: boolean
  sameSite: string
  expires: number | null
}

interface NapiChromeCookieImport {
  profile: string
  profileDir: string
  total: number
  skipped: number
  failed: number
  cookies: NapiChromeCookie[]
}

/** 导入结果（供设置页展示；不包含任何 cookie 值）。 */
export interface ChromeCookieImportResult {
  /** 使用的 Chrome Profile 名。 */
  profile: string
  /** Profile 目录（便于用户核对来源）。 */
  profileDir: string
  /** Cookie 库里的总条数。 */
  total: number
  /** 因加密方式不支持而跳过的条数（Windows app-bound v20）。 */
  skipped: number
  /** 解密失败的条数。 */
  decryptFailed: number
  /** 实际写入会话成功的条数。 */
  imported: number
  /** 写入会话失败的条数（个别脏数据）。 */
  writeFailed: number
}

/** 清理结果统计。 */
export interface ClearBrowserDataResult {
  /** 清理了站点数据的站点（域名）数。 */
  sites: number
  /** 删除的 cookie 条数。 */
  cookies: number
}

/** 站点数据（不含 Cookie，Cookie 单独逐条删以便统计）：与 Electron 的 storages 联合类型一致。 */
const SITE_STORAGES = [
  'cachestorage',
  'filesystem',
  'indexdb',
  'localstorage',
  'serviceworkers',
  'shadercache'
] as const

/** 清理时两种协议都要覆盖：DOM 存储按 origin（含 scheme）分桶，站点可能只用 http。 */
const ORIGIN_SCHEMES = ['https', 'http'] as const

/** 逐条写入时的并发度：Chromium 的 cookie 写入是本地 mojo 调用，批量并发即可。 */
const WRITE_BATCH = 32

/**
 * 导入本机 Chrome「最近使用」Profile 的 Cookie 到内置浏览器会话。
 * 一次性、不同步、不修改 Chrome 数据（Rust 侧先复制数据库再只读打开）。
 */
export async function importChromeCookies(): Promise<ChromeCookieImportResult> {
  const result = (await napiCall('importChromeCookies')) as NapiChromeCookieImport
  const jar = session.defaultSession.cookies

  let imported = 0
  let writeFailed = 0
  for (let i = 0; i < result.cookies.length; i += WRITE_BATCH) {
    const batch = result.cookies.slice(i, i + WRITE_BATCH)
    const outcomes = await Promise.all(
      batch.map(async (cookie) => {
        try {
          await jar.set(toCookieDetails(cookie))
          return true
        } catch {
          // 单条脏数据(域名非法 / 值超长等)不该中断整次导入,计入统计即可
          return false
        }
      })
    )
    for (const ok of outcomes) {
      if (ok) imported += 1
      else writeFailed += 1
    }
  }

  return {
    profile: result.profile,
    profileDir: result.profileDir,
    total: result.total,
    skipped: result.skipped,
    decryptFailed: result.failed,
    imported,
    writeFailed
  }
}

/**
 * 把后端返回的 cookie 转成 Electron `cookies.set` 的入参。
 *
 * 这里做三件"改写"（都是刻意的，见模块头注释）：
 * - `hostKey` 带前导 `.` 才是域 cookie，不带就只传 url 交给 Chromium 推导（保住 host-only 语义）；
 * - 强制 `sameSite: 'no_restriction'` + `secure: true`：跨域 iframe 只认这一组合；
 * - `expires` 为 null（会话 cookie）时不传 `expirationDate`。
 */
function toCookieDetails(cookie: NapiChromeCookie): Electron.CookiesSetDetails {
  const host = cookie.hostKey.replace(/^\./, '')
  const path = cookie.path.startsWith('/') ? cookie.path : '/'
  const details: Electron.CookiesSetDetails = {
    url: `https://${host}${path}`,
    name: cookie.name,
    value: cookie.value,
    path,
    secure: true,
    httpOnly: cookie.httpOnly,
    sameSite: 'no_restriction'
  }
  if (cookie.hostKey.startsWith('.')) details.domain = cookie.hostKey
  if (cookie.expires !== null) details.expirationDate = cookie.expires
  return details
}

/**
 * 清除 HTTP 缓存、Cache Storage、Service Worker 与 shader 缓存，**保留 Cookie 与站点数据**。
 */
export async function clearBrowserCache(): Promise<void> {
  const ses = session.defaultSession
  await ses.clearCache()
  await ses.clearStorageData({ storages: ['cachestorage', 'serviceworkers', 'shadercache'] })
}

/**
 * 清除全部浏览器数据：Cookie + 站点数据 + 缓存。
 *
 * 范围由**会话里已有 Cookie 的域名**反推，逐个 origin 清理，并排除宿主 UI 自身
 * （开发态 `ELECTRON_RENDERER_URL` 的 hostname）。不用 `clearStorageData()` 无参形式：
 * 那会连宿主 UI 自己的 localStorage（设置、页签布局）一起清掉——默认会话是共用的。
 *
 * 已知代价（可接受折中，设置页文案已说明需重新登录）：
 * - 没有 Cookie 的站点，其站点数据清不到；
 * - Cookie 的 host_key 不含端口，故非默认端口站点（如 `http://localhost:3000`）的
 *   localStorage 清不到——它们的 origin 带端口，反推不出来。
 * Cookie 本身按 name+domain+path 删除，与 scheme 无关，故 http 站点的 Cookie 也能删掉。
 */
export async function clearBrowserData(): Promise<ClearBrowserDataResult> {
  const ses = session.defaultSession
  const jar = ses.cookies
  const cookies = await jar.get({})
  const excluded = appRendererHosts()

  const hosts = new Set<string>()
  let removed = 0
  for (const cookie of cookies) {
    const host = (cookie.domain ?? '').replace(/^\./, '')
    if (!host || excluded.has(host)) continue
    hosts.add(host)
    try {
      await jar.remove(`https://${host}${cookie.path}`, cookie.name)
      removed += 1
    } catch {
      // 个别 cookie 删不掉(已过期/域名异常)不影响整体清理
    }
  }

  for (const host of hosts) {
    for (const scheme of ORIGIN_SCHEMES) {
      await ses.clearStorageData({ origin: `${scheme}://${host}`, storages: [...SITE_STORAGES] })
    }
  }

  await ses.clearCache()
  await ses.clearAuthCache()
  return { sites: hosts.size, cookies: removed }
}

/**
 * 宿主 UI 自身的 hostname（开发态是 vite dev server；打包后是 `file:`，不会有 Cookie）。
 * 只按 hostname 比对：Chrome 的 host_key 不含端口，dev server 的 `localhost:5173`
 * 落到 Cookie 里是 `localhost`，带端口的精确 origin 比对匹配不上。
 */
function appRendererHosts(): Set<string> {
  const devUrl = process.env['ELECTRON_RENDERER_URL']
  if (!devUrl) return new Set()
  try {
    return new Set([new URL(devUrl).hostname])
  } catch {
    return new Set()
  }
}
