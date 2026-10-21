import { app, ipcMain, safeStorage } from 'electron'
import { readFileSync } from 'fs'
import { writeFileAtomicSync } from './atomicWrite'
import { join } from 'path'

/**
 * 连接方式（本机 / 云端）—— Electron 壳子启动时「去哪找后端」。
 *
 * - **本机**（缺省）：进程内 napi 后端（`astorm-backend.node`），数据落本机 `~/astorm`，
 *   即一直以来的桌面形态；
 * - **云端**：不跑本机后端，窗口直接加载云容器的 **Web UI**（服务端自带托管的前端，
 *   与浏览器访问同一套界面），凭证 = 容器启动时配置的访问令牌（`ASTORM_TOKEN`，
 *   Bearer）。壳子只做三件事：把端点/令牌记下来、把令牌预先写进远端页面的
 *   localStorage（免登录遮罩）、给窗口一个原生菜单（连接方式… / 刷新 / 开发者工具）。
 *
 * 配置落 `<userData>/connection.json`（userData 已重定向到 `<appData>/astorm/electron`，
 * 与用户数据目录无关——换数据目录不该丢「我用的是云端」这件事）；令牌经 safeStorage
 * 加密，不可用时降级明文并标注 storage（与 providers.json 同一套做法）。
 */

export type ConnectionMode = 'local' | 'cloud'

/** 渲染层读到的连接信息（不含令牌明文） */
export interface ConnectionInfo {
  /** `unset` = 尚未完成引导/选择连接方式 */
  mode: ConnectionMode | 'unset'
  endpoint: string
  hasToken: boolean
  /** 是否完成过引导（全局标记；决定启动直接进应用还是出欢迎页） */
  onboarded: boolean
}

interface StoredConnection {
  schemaVersion: number
  mode: ConnectionMode | 'unset'
  endpoint: string
  /** safeStorage → 加密后 hex；plaintext → 明文；null = 无令牌 */
  token: string | null
  storage: 'safeStorage' | 'plaintext'
  /** 完成过引导的标记（旧配置按 mode 推断） */
  onboarded: boolean
}

const DEFAULT_STORED: StoredConnection = {
  schemaVersion: 1,
  mode: 'unset',
  endpoint: '',
  token: null,
  storage: 'safeStorage',
  onboarded: false
}

let cache: StoredConnection | null = null

function filePath(): string {
  return join(app.getPath('userData'), 'connection.json')
}

/** 归一化：类型校验 + 丢弃多余字段（兼容手改文件 / 旧版无 onboarded 的配置） */
function normalize(raw: unknown): StoredConnection {
  const x = (raw ?? {}) as Partial<Record<keyof StoredConnection, unknown>>
  const mode = x.mode === 'local' || x.mode === 'cloud' ? x.mode : 'unset'
  return {
    schemaVersion: 1,
    mode,
    endpoint: typeof x.endpoint === 'string' ? x.endpoint : '',
    token: typeof x.token === 'string' && x.token ? x.token : null,
    storage: x.storage === 'plaintext' ? 'plaintext' : 'safeStorage',
    // 旧配置没有标记：已选过连接方式即视为完成过引导
    onboarded: typeof x.onboarded === 'boolean' ? x.onboarded : mode !== 'unset'
  }
}

function read(): StoredConnection {
  if (cache) return cache
  try {
    const raw = JSON.parse(
      readFileSync(filePath(), 'utf8').replace(/^\uFEFF/, '')
    ) as Partial<StoredConnection>
    cache = normalize(raw)
    // 旧配置补写 onboarded，之后启动直接按标记判断
    if (typeof raw.onboarded !== 'boolean' && cache.onboarded) persist(cache)
  } catch {
    cache = { ...DEFAULT_STORED }
  }
  return cache
}

function persist(next: StoredConnection): void {
  cache = next
  try {
  writeFileAtomicSync(filePath(), JSON.stringify(next, null, 2), 'utf8', 0o600)
  } catch (err) {
    console.warn('[connection] 写入失败:', (err as Error).message)
  }
}

function encryptToken(plain: string): { token: string | null; storage: StoredConnection['storage'] } {
  if (!plain) return { token: null, storage: 'safeStorage' }
  if (safeStorage.isEncryptionAvailable()) {
    return { token: safeStorage.encryptString(plain).toString('hex'), storage: 'safeStorage' }
  }
  return { token: plain, storage: 'plaintext' }
}

function decryptToken(s: StoredConnection): string {
  if (!s.token) return ''
  if (s.storage !== 'safeStorage') return s.token
  try {
    return safeStorage.decryptString(Buffer.from(s.token, 'hex'))
  } catch {
    return ''
  }
}

/**
 * 端点归一化：补协议、去尾斜杠、剥掉用户常整段粘来的 `/api/v1`（端点 = 站点根）。
 * 返回空串表示输入不足以构成端点。
 */
export function normalizeEndpoint(input: string): string {
  let url = (input ?? '').trim()
  if (!url) return ''
  if (!/^https?:\/\//i.test(url)) url = `http://${url}`
  url = url.replace(/\/+$/, '')
  url = url.replace(/\/api\/v1$/i, '')
  try {
    const u = new URL(url)
    if (!u.hostname) return ''
    return `${u.protocol}//${u.host}${u.pathname.replace(/\/+$/, '')}`
  } catch {
    return ''
  }
}

/** 当前连接信息（渲染层用；不含令牌明文） */
export function connectionInfo(): ConnectionInfo {
  const s = read()
  return {
    mode: s.mode,
    endpoint: s.endpoint,
    hasToken: !!s.token,
    onboarded: s.onboarded
  }
}

/** 云端窗口加载地址（端点根 + 尾斜杠，避免被服务端当相对路径解析） */
export function cloudUrl(): string {
  const s = read()
  return s.endpoint ? `${s.endpoint}/` : ''
}

/** 云端令牌明文（仅 cloud preload 取用，用于免登录遮罩） */
export function connectionToken(): string {
  return decryptToken(read())
}

/** 落盘连接配置（mode/endpoint/token/onboarded 任一可省，省 = 不改） */
export function saveConnection(patch: {
  mode?: ConnectionMode
  endpoint?: string
  token?: string
  onboarded?: boolean
}): ConnectionInfo {
  const cur = read()
  const endpoint = patch.endpoint === undefined ? cur.endpoint : normalizeEndpoint(patch.endpoint)
  const enc = patch.token === undefined ? null : encryptToken(patch.token.trim())
  persist({
    schemaVersion: 1,
    mode: patch.mode ?? cur.mode,
    endpoint,
    token: enc ? enc.token : cur.token,
    storage: enc ? enc.storage : cur.storage,
    onboarded: patch.onboarded ?? cur.onboarded
  })
  return connectionInfo()
}

/** 测试失败的分类：渲染层按 code 本地化（message 只是兜底） */
export type ConnectionTestErrorCode =
  | 'endpointInvalid'
  | 'unauthorized'
  | 'notAstorm'
  | 'timeout'
  | 'unreachable'
  | 'http'

/** 测试结果（渲染层展示用） */
export interface ConnectionTestResult {
  ok: boolean
  /** 失败分类（ok=false 时非空） */
  code?: ConnectionTestErrorCode
  /** 兜底文案（主进程侧中文；渲染层优先按 code 本地化） */
  message?: string
  /** http 类失败的响应码 */
  status?: number
  /** unreachable 的原始错误信息（拼进本地化文案） */
  detail?: string
  /** 成功时：服务端默认模型 / 数据目录（渲染层本地化拼装） */
  model?: string
  dataDir?: string
}

/**
 * 探测端点：`GET <endpoint>/api/v1/status`（Bearer 令牌）。
 * 401 → 令牌不对；连不上 → 网络/地址问题。文案不在这里本地化——渲染层拿 code 自己拼，
 * 免得英文界面里蹦出中文（主进程没有 i18n 实例）。
 */
export async function testConnection(
  endpointInput: string,
  tokenInput: string
): Promise<ConnectionTestResult> {
  const endpoint = normalizeEndpoint(endpointInput)
  if (!endpoint) return { ok: false, code: 'endpointInvalid', message: '端点地址无效' }
  const token = (tokenInput ?? '').trim()
  try {
    const resp = await fetch(`${endpoint}/api/v1/status`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      signal: AbortSignal.timeout(8000)
    })
    if (resp.status === 401 || resp.status === 403) {
      return { ok: false, code: 'unauthorized', status: resp.status, message: '访问令牌无效（401）' }
    }
    if (!resp.ok) {
      return { ok: false, code: 'http', status: resp.status, message: `服务端返回 ${resp.status}` }
    }
    const body = (await resp.json()) as { code?: number; data?: Record<string, unknown> }
    if (body?.code !== 0) {
      return { ok: false, code: 'notAstorm', message: '端点不是 astorm 服务端（响应格式不符）' }
    }
    const data = body.data ?? {}
    const llm = (data.llm ?? {}) as Record<string, unknown>
    return {
      ok: true,
      model: typeof llm.defaultModel === 'string' ? llm.defaultModel : '',
      dataDir: typeof data.dataDir === 'string' ? data.dataDir : ''
    }
  } catch (err) {
    const msg = (err as Error)?.message ?? String(err)
    const timeout = /abort|timeout/i.test(msg)
    return {
      ok: false,
      code: timeout ? 'timeout' : 'unreachable',
      detail: msg,
      message: timeout ? '连接超时（8s）' : `无法连接：${msg}`
    }
  }
}

/**
 * 注册连接方式相关 IPC。
 *
 * @param navigate   主进程的窗口导航（切模式 / 回欢迎页由它落地）：
 *   `navigate({ mode: 'welcome' | 'local' | 'cloud' })`
 * @param isWelcome  当前这次加载是不是「欢迎页」（主进程按加载目标给，preload 同步取用）
 */
export function registerConnectionIpc(
  navigate: (target: 'welcome' | 'local' | 'cloud') => void,
  isWelcome: () => boolean
): void {
  // 渲染层启动期同步读（preload 在页面脚本前取值）
  ipcMain.on('shellConnection:get-sync', (event) => {
    event.returnValue = { ...connectionInfo(), welcome: isWelcome() }
  })
  // cloud preload 取令牌明文（免登录遮罩）
  ipcMain.on('shellConnection:token-sync', (event) => {
    event.returnValue = connectionToken()
  })
  ipcMain.handle('shellConnection:test', (_e, endpoint: string, token: string) =>
    testConnection(endpoint, token)
  )
  ipcMain.handle(
    'shellConnection:save',
    (_e, patch: { mode: ConnectionMode; endpoint?: string; token?: string }) => {
      if (patch?.mode !== 'local' && patch?.mode !== 'cloud') {
        throw new Error('连接方式非法')
      }
      if (patch.mode === 'cloud') {
        const endpoint = normalizeEndpoint(patch.endpoint ?? '')
        if (!endpoint) throw new Error('端点地址无效')
        // 能走到 save 只会是引导页「完成/跳过」，顺手记下引导完成标记
        saveConnection({ mode: 'cloud', endpoint, token: patch.token ?? '', onboarded: true })
        navigate('cloud')
        return connectionInfo()
      }
      saveConnection({ mode: 'local', onboarded: true })
      navigate('local')
      return connectionInfo()
    }
  )
  ipcMain.handle('shellConnection:open-welcome', () => {
    navigate('welcome')
  })
  // 已配置过时从引导页「返回」：按当前配置回到应用（不重新落盘）
  ipcMain.handle('shellConnection:exit-welcome', () => {
    const c = connectionInfo()
    if (c.mode === 'unset') {
      navigate('welcome')
      return
    }
    navigate(c.mode === 'cloud' && c.endpoint ? 'cloud' : 'local')
  })
}
