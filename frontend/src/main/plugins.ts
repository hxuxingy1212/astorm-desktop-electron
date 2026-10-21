/**
 * 插件系统主进程实现（系统目录 plugins/ + 自定义协议托管 + 注册表持久化）。
 *
 * - 插件 = 一个本地目录（plugin.json 清单 + 静态网站），仓库根 `plugins/<插件>/` 是插件包源码，
 *   安装时复制到 `<系统目录>/plugins/<id>/`；启用状态与配置记录在 plugins/plugins.json。
 * - `astorm-plugin://<id>/...` 自定义协议离线托管插件网站（每个插件独立 origin = 插件 id）；
 *   特殊 host `__sdk__` 返回内置 SDK（仓库根 plugins/sdk/astorm-sdk.js）。
 * - `secret` 类型配置经 safeStorage 加密落盘（解密仅发生在主进程 → 渲染层展示）。
 */
import { app, protocol, safeStorage } from 'electron'
import { pluginsDir } from './systemPaths'
import { promises as fs } from 'fs'
import { writeFileAtomic } from './atomicWrite'
import { join, normalize, resolve, sep } from 'path'

/* ---------- 类型 ---------- */

export interface PluginConfigField {
  key: string
  label: string
  /** text / textarea / password / secret(加密) / number / boolean / select */
  type: 'text' | 'textarea' | 'password' | 'secret' | 'number' | 'boolean' | 'select'
  required?: boolean
  default?: unknown
  placeholder?: string
  description?: string
  options?: Array<{ label: string; value: string }>
}

export interface PluginManifest {
  id: string
  name: string
  version: string
  description?: string
  /** 入口页面（相对插件根目录，如 ./index.html） */
  entry: string
  icon?: string
  /** 申请的能力：llm / chat / messages …；缺省按默认权限门控 */
  permissions?: string[]
  sdkVersion?: string
  configSchema?: PluginConfigField[]
}

/** 返回渲染层的插件视图（secret 已解密；启用 + 配置合一） */
export interface PluginView {
  manifest: PluginManifest
  enabled: boolean
  config: Record<string, unknown>
  /** 内部字段：协议入口 URL（渲染层据此加载 iframe） */
  entryUrl: string
}

/** 注册表记录：明文配置 + 加密 secret 分开存 */
interface PluginEntry {
  enabled: boolean
  config: Record<string, unknown>
  configSecret: Record<string, string>
}

interface PluginsFile {
  schemaVersion: number
  plugins: Record<string, PluginEntry>
}

const DEFAULT_FILE: PluginsFile = { schemaVersion: 1, plugins: {} }

const ID_RE = /^[a-z0-9][a-z0-9-_]{0,63}$/

/**
 * 仓库插件目录(独立于 frontend 的插件包仓库):dev 指向仓库根 `plugins/`,
 * 打包后随应用复制到 `process.resourcesPath/plugins`(见 electron-builder extraResources)。
 */
function repoPluginsDir(): string {
  return app.isPackaged ? join(process.resourcesPath, 'plugins') : join(app.getAppPath(), '..', 'plugins')
}

/* ---------- 路径与持久化 ---------- */

function pluginRoot(): string {
  return pluginsDir()
}

function pluginDir(id: string): string {
  return join(pluginRoot(), id)
}

function registryFile(): string {
  return join(pluginsDir(), 'plugins.json')
}

async function readRegistry(): Promise<PluginsFile> {
  try {
    const text = await fs.readFile(registryFile(), 'utf-8')
    const raw = JSON.parse(text) as { plugins?: unknown }
    const plugins: Record<string, PluginEntry> = {}
    for (const [id, entry] of Object.entries((raw.plugins ?? {}) as Record<string, unknown>)) {
      const e = (entry ?? {}) as { enabled?: unknown; config?: unknown; configSecret?: unknown }
      plugins[id] = {
        enabled: e.enabled !== false,
        config: typeof e.config === 'object' && e.config ? (e.config as Record<string, unknown>) : {},
        configSecret:
          typeof e.configSecret === 'object' && e.configSecret ? (e.configSecret as Record<string, string>) : {}
      }
    }
    return { schemaVersion: 1, plugins }
  } catch {
    return DEFAULT_FILE
  }
}

async function writeRegistry(file: PluginsFile): Promise<void> {
  await writeFileAtomic(registryFile(), JSON.stringify(file, null, 2), 'utf-8', 0o600)
}

/* ---------- 清单读取与校验 ---------- */

function normalizeEntry(entry: string): string {
  let e = String(entry ?? '').replace(/^\.\//, '')
  if (!e || e.startsWith('/') || e.includes('..') || e.includes(':')) {
    throw new Error(`plugin.json 的 entry 非法: ${entry}`)
  }
  return e
}

async function loadManifest(dir: string): Promise<PluginManifest> {
  let raw: unknown
  try {
    const text = await fs.readFile(join(dir, 'plugin.json'), 'utf-8')
    raw = JSON.parse(text.replace(/^\uFEFF/, ''))
  } catch {
    throw new Error('缺少可用的 plugin.json（解析失败）')
  }
  const m = (raw ?? {}) as {
    id?: unknown
    name?: unknown
    version?: unknown
    entry?: unknown
    description?: unknown
    icon?: unknown
    permissions?: unknown
    sdkVersion?: unknown
    configSchema?: unknown
  }
  const id = String(m.id ?? '')
  const name = String(m.name ?? '')
  const version = String(m.version ?? '0.0.0')
  const entry = String(m.entry ?? '')
  if (!ID_RE.test(id)) throw new Error(`plugin.json 的 id 非法（需 ^[a-z0-9][a-z0-9-_]*$）: ${id}`)
  if (!name.trim()) throw new Error('plugin.json 缺少 name 字段')
  const configSchema: PluginConfigField[] = Array.isArray(m.configSchema)
    ? (m.configSchema as PluginConfigField[]).filter(
        (f) =>
          f &&
          typeof f.key === 'string' &&
          f.key &&
          typeof f.label === 'string' &&
          ['text', 'textarea', 'password', 'secret', 'number', 'boolean', 'select'].includes(f.type)
      )
    : []
  return {
    id,
    name,
    version,
    description: typeof m.description === 'string' ? m.description : '',
    entry: normalizeEntry(entry),
    icon: typeof m.icon === 'string' ? m.icon : '',
    permissions: Array.isArray(m.permissions) ? m.permissions.map(String) : [],
    sdkVersion: typeof m.sdkVersion === 'string' ? m.sdkVersion : '1',
    configSchema
  }
}

/* ---------- 配置：默认 + 落盘合并 + secret 加密 ---------- */

function encryptSecret(plain: string): string {
  if (!plain) return ''
  if (safeStorage.isEncryptionAvailable()) {
    return safeStorage.encryptString(plain).toString('hex')
  }
  // 降级：明文前缀标记（与 providers.ts 约定一致）
  return `plain:${plain}`
}

function decryptSecret(stored: string): string {
  if (!stored) return ''
  if (stored.startsWith('plain:')) return stored.slice('plain:'.length)
  try {
    return safeStorage.decryptString(Buffer.from(stored, 'hex'))
  } catch {
    return ''
  }
}

function coerceValue(f: PluginConfigField, value: unknown): unknown {
  switch (f.type) {
    case 'boolean':
      return value === true || value === 'true'
    case 'number': {
      const n = Number(value)
      return Number.isFinite(n) ? n : f.default ?? null
    }
    case 'select': {
      const opts = (f.options ?? []).map((o) => o.value)
      return typeof value === 'string' && (opts.length === 0 || opts.includes(value)) ? value : f.default ?? ''
    }
    default:
      return String(value ?? '')
  }
}

/** 由清单 + 注册表条目组装渲染层可见配置（默认值 → 明文配置 → 解密 secret）。 */
function mergeConfig(manifest: PluginManifest, entry: PluginEntry | undefined): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const f of manifest.configSchema ?? []) {
    if (f.default !== undefined) out[f.key] = f.default
  }
  for (const f of manifest.configSchema ?? []) {
    if (f.type === 'secret') {
      const hex = entry?.configSecret?.[f.key]
      if (hex) out[f.key] = decryptSecret(hex)
    } else if (entry && entry.config[f.key] !== undefined) {
      out[f.key] = entry.config[f.key]
    }
  }
  return out
}

/** 校验并按 schema 归一化渲染层提交的配置：secret 加密进 configSecret。 */
function coerceConfig(
  manifest: PluginManifest,
  input: Record<string, unknown>
): { config: Record<string, unknown>; configSecret: Record<string, string> } {
  const config: Record<string, unknown> = {}
  const configSecret: Record<string, string> = {}
  for (const f of manifest.configSchema ?? []) {
    const value = input[f.key] ?? f.default
    if (f.type === 'secret') {
      configSecret[f.key] = encryptSecret(String(value ?? ''))
    } else {
      config[f.key] = coerceValue(f, value)
    }
  }
  return { config, configSecret }
}

/* ---------- 插件生命周期（install / remove / enable / config） ---------- */

function pluginView(manifest: PluginManifest, entry: PluginEntry | undefined): PluginView {
  return {
    manifest,
    enabled: entry?.enabled !== false,
    config: mergeConfig(manifest, entry),
    entryUrl: `astorm-plugin://${manifest.id}/${manifest.entry}`
  }
}

/** 扫描已安装插件（userData/plugins 下含 plugin.json 的目录，按名称排序）。 */
export async function scanPlugins(): Promise<PluginView[]> {
  const file = await readRegistry()
  const out: PluginView[] = []
  let names: string[] = []
  try {
    names = await fs.readdir(pluginRoot())
  } catch {
    names = []
  }
  for (const name of names) {
    if (!ID_RE.test(name)) continue
    try {
      const manifest = await loadManifest(pluginDir(name))
      if (manifest.id !== name) {
        console.warn(`[plugins] 目录 ${name} 的 plugin.json id 不匹配，跳过`)
        continue
      }
      out.push(pluginView(manifest, file.plugins[name]))
    } catch (err) {
      console.warn(`[plugins] 清单解析失败（${name}）:`, (err as Error).message)
    }
  }
  // 注册表中有记录但目录缺失（残留）→ 一并返回，供前端引导重新安装
  for (const id of Object.keys(file.plugins)) {
    if (!out.some((v) => v.manifest.id === id)) {
      out.push({
        manifest: { id, name: id, version: '', entry: '', permissions: [], configSchema: [] },
        enabled: false,
        config: {},
        entryUrl: ''
      })
    }
  }
  out.sort((a, b) => a.manifest.name.localeCompare(b.manifest.name))
  return out
}

/** 从本地目录安装：校验清单 → 复制到 userData/plugins/<id> → 注册（默认启用）。 */
export async function installPlugin(dir: string): Promise<PluginView> {
  const manifest = await loadManifest(dir)
  const dest = pluginDir(manifest.id)
  const file = await readRegistry()
  if (file.plugins[manifest.id] || (await exists(dest))) {
    throw new Error(`插件已存在：${manifest.id}（请先卸载再安装）`)
  }
  await fs.mkdir(pluginRoot(), { recursive: true })
  await fs.cp(dir, dest, { recursive: true })
  file.plugins[manifest.id] = { enabled: true, config: {}, configSecret: {} }
  await writeRegistry(file)
  return pluginView(manifest, file.plugins[manifest.id])
}

export async function removePlugin(id: string): Promise<{ ok: boolean }> {
  const file = await readRegistry()
  delete file.plugins[id]
  await writeRegistry(file)
  try {
    await fs.rm(pluginDir(id), { recursive: true, force: true })
  } catch {
    /* 目录清理失败不阻断注册表删除 */
  }
  return { ok: true }
}

export async function setPluginEnabled(id: string, enabled: boolean): Promise<PluginView> {
  const manifest = await loadManifest(pluginDir(id))
  const file = await readRegistry()
  const entry = file.plugins[id] ?? { enabled: true, config: {}, configSecret: {} }
  entry.enabled = enabled
  file.plugins[id] = entry
  await writeRegistry(file)
  return pluginView(manifest, entry)
}

export async function savePluginConfig(id: string, config: Record<string, unknown>): Promise<PluginView> {
  const manifest = await loadManifest(pluginDir(id))
  const file = await readRegistry()
  const entry = file.plugins[id] ?? { enabled: true, config: {}, configSecret: {} }
  const coerced = coerceConfig(manifest, config ?? {})
  entry.config = coerced.config
  entry.configSecret = coerced.configSecret
  file.plugins[id] = entry
  await writeRegistry(file)
  return pluginView(manifest, entry)
}

/* ---------- 自定义协议：astorm-plugin:// ---------- */

const MIME: Record<string, string> = {
  html: 'text/html; charset=utf-8',
  htm: 'text/html; charset=utf-8',
  js: 'text/javascript; charset=utf-8',
  mjs: 'text/javascript; charset=utf-8',
  css: 'text/css; charset=utf-8',
  json: 'application/json; charset=utf-8',
  svg: 'image/svg+xml',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  ico: 'image/x-icon',
  txt: 'text/plain; charset=utf-8',
  md: 'text/markdown; charset=utf-8',
  wasm: 'application/wasm'
}

async function exists(p: string): Promise<boolean> {
  try {
    await fs.access(p)
    return true
  } catch {
    return false
  }
}

/** 静态托管插件网站：站点根 = 插件目录自身（plugin.json 与入口同层）；路径经规范化防目录穿越。 */
async function servePluginSite(url: URL): Promise<Response> {
  const id = url.hostname
  if (!ID_RE.test(id)) return new Response('Bad Request', { status: 400 })
  const root = pluginDir(id)
  let pathname = decodeURIComponent(url.pathname)
  if (!pathname || pathname === '/') {
    // 裸 host：回落到清单入口页
    try {
      const manifest = await loadManifest(root)
      pathname = `/${manifest.entry}`
    } catch {
      return new Response('Not Found', { status: 404 })
    }
  }
  const rel = pathname.replace(/^\/+/, '')
  const target = resolve(root, rel)
  const normalizedRoot = normalize(root)
  if (target !== normalizedRoot && !target.startsWith(normalizedRoot + sep)) {
    return new Response('Forbidden', { status: 403 })
  }
  try {
    const buf = await fs.readFile(target)
    const ext = (target.split('.').pop() ?? '').toLowerCase()
    return new Response(buf, {
      headers: { 'content-type': MIME[ext] ?? 'application/octet-stream', 'cache-control': 'no-cache' }
    })
  } catch {
    return new Response('Not Found', { status: 404 })
  }
}

/** 注册 astorm-plugin 协议处理（须在 app ready 后调用；scheme 权限在 ready 前注册）。 */
export function registerPluginProtocol(): void {
  protocol.handle('astorm-plugin', async (request) => {
    const url = new URL(request.url)
    // 诊断日志：插件页面/资源加载失败排查用（dev 终端可见）
    console.log(`[plugin-srv] ${request.method} ${url.href}`)
    if (url.hostname === '__sdk__') {
      try {
        const sdkPath = join(repoPluginsDir(), 'sdk', 'astorm-sdk.js')
        const buf = await fs.readFile(sdkPath)
        return new Response(buf, {
          headers: { 'content-type': 'text/javascript; charset=utf-8', 'cache-control': 'no-cache' }
        })
      } catch {
        console.warn('[plugin-srv] sdk not found')
        return new Response('SDK not found', { status: 404 })
      }
    }
    return servePluginSite(url)
  })
}

/** 插件在 userData/plugins/<id> 下的目录（供打开文件管理器定位）。 */
export function pluginDirFor(id: string): string {
  return pluginDir(id)
}

/** 首次运行内置插件:仓库 plugins/<src> 复制到 userData/plugins/<id> 并注册启用。 */
export async function ensureDemoPlugin(): Promise<void> {
  for (const builtin of [
    { id: 'demo', src: 'demo-plugin' }
  ]) {
    await installBuiltin(builtin.id, builtin.src)
  }
}

/** 内置插件安装单元(幂等):userData 已存在则跳过;dev 迭代更新需手动删除副本后重启。 */
async function installBuiltin(id: string, srcDir: string): Promise<void> {
  const dest = pluginDir(id)
  if (await exists(dest)) return
  const src = join(repoPluginsDir(), srcDir)
  try {
    if (!(await exists(join(src, 'plugin.json')))) return
    await fs.mkdir(pluginRoot(), { recursive: true })
    await fs.cp(src, dest, { recursive: true })
    const file = await readRegistry()
    file.plugins[id] = { enabled: true, config: {}, configSecret: {} }
    await writeRegistry(file)
    console.log(`[plugins] builtin plugin installed: ${id}`)
  } catch (err) {
    console.warn(`[plugins] builtin plugin ${id} install failed:`, (err as Error).message)
  }
}