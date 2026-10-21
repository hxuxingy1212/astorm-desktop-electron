import { app, BrowserWindow } from 'electron'
import { createRequire } from 'module'
import { join } from 'path'

/**
 * astorm-electron-backend(napi/Rust)主进程桥接。
 * - 加载编译产物 resources/astorm-backend.node(未提供则以 null 降级,Mock 兜底);
 * - start() 注入 AppConfig(供应商配置在 llm.providers,与契约 docs/backend-contract.md 一致);
 * - 后端子注册供应商无运行时 API,配置变更 = shutdown + start 重启;
 * - subscribe() 事件流中继到渲染进程(window webContents.send 'napi:event')。
 */

export interface NapiProviderModel {
  name: string
  order: number
}

/** 供应商配置快照(供后端配置组装;models 为 {id, name} 引用对,order 可选) */
export interface NapiProviderConfig {
  name: string
  baseUrl: string
  apiKey: string
  /** API 协议(chat_completions / responses / anthropic_messages) */
  apiFormat?: string
  models: Array<{ id: string; name: string }>
  order?: number
}

/** 会话摘要配置(camelCase,与后端 AppConfig.summary 对齐) */
export interface NapiSummaryConfig {
  tokenThreshold: number
  minMessages: number
  keepTokenThreshold: number
  keepTailRounds: number
}

/** 会话消息剪支配置(camelCase,与后端 AppConfig.compact 对齐) */
export interface NapiCompactConfig {
  compactThreshold: number
  compactRecentRounds: number
  compactByteCount: number
  compactHeadChars: number
  compactTailChars: number
}

/** 目标循环配置(camelCase,与后端 AppConfig.goal 对齐) */
export interface NapiGoalConfig {
  maxTokens: number
  maxIterations: number
}

/** 安全配置(camelCase,与后端 SecurityConfig 对齐;文件 glob / 网络规则 / 危险命令正则) */
export interface NapiSecurityConfig {
  enabled: boolean
  file: {
    readBlack: string[]
    readWhite: string[]
    writeBlack: string[]
    writeWhite: string[]
  }
  net: {
    useWhitelist: boolean
    inBlack: string[]
    inWhite: string[]
    outBlack: string[]
    outWhite: string[]
  }
  danger: { useBuiltin: boolean; builtinPatterns: string[] | null; black: string[] }
  resource: {
    memoryMb: number | null
    diskMb: number | null
    cpuSeconds: number | null
    cpuPercent: number | null
    gpu: boolean
  }
  /** 本地环境页维护的被禁用工具 bin 名（沙箱自动注入据此过滤） */
  toolchainDisabled: string[]
  /** 额外可执行目录：非常规安装位置的工具链/编译器（exec 与只读一并放行） */
  execWhite: string[]
}

/** 记忆分类(与 Rust MemoryCategory 对齐;type 键名与后端一致,不用 typeName) */
export interface NapiMemoryCategoryConfig {
  /** 分类 id = 提取阶段的 event_type 取值(小写) */
  id: string
  /** 目标目录(相对 wiki/;空串 = 直接放 wiki/ 根) */
  dir: string
  /** 写入 Concept frontmatter 的 OKF type 值 */
  type: string
  /** 语义说明(拼进提示词的取值集) */
  hint: string
}

/**
 * 长期记忆配置(camelCase,与 Rust MemoryConfig 对齐)。
 *
 * 分类体系是配置驱动的:OKF 的 `type` 是描述性而非枚举性的,新增分类只需改这里。
 * 预算字段只在设置页高级区暴露,缺省即后端默认值。
 */
export interface NapiMemoryConfig {
  enabled: boolean
  /** 会话结束后自动增量编译(默认关:避免静默消耗 API 额度) */
  autoCompile: boolean
  /** 编译所用供应商名(缺省 = 默认供应商) */
  provider?: string | null
  /** 编译所用模型(模型引用 id;缺省 = 默认模型) */
  model?: string | null
  categories: NapiMemoryCategoryConfig[]
  maxUnitsPerCompileCall: number
  rawCharBudget: number
  toolResultChars: number
  judgeCandidates: number
  /** 提取二遍自查(成本约 2×;默认关) */
  extractSelfCritique: boolean
  /** 第二遍精选(去噪 + 同主题合并;默认开) */
  curate: boolean
  curateUnitsPerCall: number
  minConfidence: number
  maxUnitsPerSession: number
  /** 合并候选的主题重叠度下限(Jaccard;0 = 不过滤) */
  candidateMinOverlap: number
  /** 单概念 tags 上限(0 = 不限) */
  maxTags: number
  /** 正文「相关」小节里的概念互链条数（0 = 不生成） */
  relatedConcepts: number
  /** 单概念追加段上限(0 = 不限) */
  maxAppendSections: number
  /** 会话中是否注入「与当前用户消息相关的记忆」(只影响模型看到什么,不改变编译产出) */
  injectMemories: boolean
  /** 每次注入的相关记忆条数上限(0 = 不注入) */
  injectedConcepts: number
}

/**
 * 约定文件注入配置(camelCase,与 Rust InstructionConfig 对齐)。
 *
 * 两份 AGENTS.md 拼进同一条 system 消息(静态 → 全局 → 会话空间 → 智能体指引):
 * 全局 = `<用户目录>/AGENTS.md`,会话空间 = `<会话工作区>/AGENTS.md`。路径由后端自行推导。
 */
export interface NapiInstructionConfig {
  /** 注入用户目录下的全局 AGENTS.md(对所有会话生效) */
  injectGlobalAgents: boolean
  /** 注入会话工作区根目录的 AGENTS.md(仅该会话生效,冲突时覆盖全局) */
  injectWorkspaceAgents: boolean
}

/** 后端 AppConfig JSON(camelCase,见 astorm-core/src/app/config.rs) */
export interface NapiAppConfig {
  log: { level: string }
  workspace: string
  llm: {
    providers: NapiProviderConfig[]
    defaultProvider?: string
  }
  summary?: NapiSummaryConfig
  compact?: NapiCompactConfig
  goal?: NapiGoalConfig
  /** browser 工具(内置浏览器 AI 代理操控):agent-browser 可执行文件绝对路径 */
  browser?: { binary?: string }
  /**
   * office 工具 + 内置 office skill(astorm-office 套件,scripts/sync-office.mjs 同步):
   * binDir=五 CLI 目录(统一入口 + 四格式);skillDir=kit skill 树(启动时复制进用户技能目录注册为内置技能);
   * chromeBin/sofficePath 转发 CLI 环境变量;timeoutSec 单次调用超时。
   */
  office?: {
    binDir?: string
    skillDir?: string
    chromeBin?: string
    sofficePath?: string
    timeoutSec?: number
  }
  /** 网络搜索(多引擎免 key 抓取):默认引擎 + 超时;失败按内置顺序(国内优先)自动回退 */
  webSearch?: { defaultEngine?: string; timeoutSec?: number }
  /**
   * 页面渲染服务(websearch/webfetch 共用):复用 Electron 自带 Chromium 的常驻隐藏窗口。
   * bin=process.execPath;script=page-render.cjs;并发/排队/空闲回收/节流由后端治理。
   */
  webRender?: {
    bin?: string
    script?: string
    profileDir?: string
    concurrency?: number
    queueMax?: number
    idleSecs?: number
    minIntervalMs?: number
  }
  security?: NapiSecurityConfig
  /** 长期记忆(OKF v0.2 编译管道):开关 / 分类表 / 各类预算 */
  memory?: NapiMemoryConfig
  /** 约定文件(AGENTS.md 注入开关:全局 + 会话空间) */
  instructions?: NapiInstructionConfig
  /** 用户语言偏好(设置页语言;会话环境信息展示用) */
  locale?: string
}

interface NapiBinding {
  start(config: string): boolean
  shutdown(): void
  /** napi-rs 默认把 Rust 蛇形函数名导出为 camelCase */
  isRunning(): boolean
  version(): string
  subscribe(): ReadableStream<unknown>
  createSession(request: unknown): string
  sendMessage(session_id: string, request: unknown): void
  /** 启动「仅创建」的会话:以已有历史(末尾那条指令)驱动一轮,不追加新的用户消息 */
  startSession(session_id: string): void
  /** 立即发送:进入打断消息队列 + 取消当前循环优先执行 */
  interruptMessage(session_id: string, request: unknown): void
  /** 提交中断决议(审批同意/拒绝、表单回答/忽略):就地把决议写回被中断的 tool 消息并续跑 */
  submitInterrupt(session_id: string, request: unknown): void
  /** 重开被忽略的提问:按工具调用 id 用原表单定义重新登记待决中断(前端表单卡重新弹出) */
  reopenInterrupt(session_id: string, tool_call_id: string): void
  cancelSession(session_id: string): void
  /** 撤回历史:保留前 keepUserMessages 条用户消息,删除其后全部(内存+磁盘),并中断当前循环 */
  rewindToUserMessage(session_id: string, keep_user_messages: number): void
  /** 基于已有会话 fork 出新会话(共用工作区),返回新会话 id;beforeFinal 见后端契约 */
  forkSession(session_id: string, before_final?: number): string
  /** 手动触发一次会话摘要(/compact,绕过阈值),返回是否完成摘要 */
  summarizeSession(session_id: string): Promise<boolean>
  /** 更新会话级设置(模式/审批/思考强度/模型/供应商):不必发消息,下一个工具调用即生效 */
  updateSessionSettings(session_id: string, request: unknown): void
  /** 设置/修改/删除会话目标(`/goal`):goal 为 null/空白 = 删除,不必发消息 */
  updateSessionGoal(session_id: string, goal: string | null, goal_lang?: string): void
  listSessions(): unknown[]
  listMessages(session_id: string): unknown[]
  /** 归档分页(「加载更早」):最新摘要备份段中 seq < beforeSeq 的消息,末尾 maxUserTurns 个用户轮次 */
  archivedMessagesBefore(session_id: string, before_seq: number, max_user_turns: number): unknown[]
  /** 读取会话系统提示词(按当前配置实时拼装,不落盘);供「会话轨迹」展示 */
  getSystemPrompt(session_id: string): string
  deleteSession(session_id: string): void
  /** 热更新 LLM 配置(不 shutdown,运行中会话下一轮生效) */
  updateConfig?: (config: string) => void
  /** 单次 LLM 补全(插件 SDK `llm.chat` 通路):启动即返回,事件经全局总线 type:"llm" + callId 回传 */
  askLlm(request: unknown, callId: string): void
  // ---- 长期记忆(OKF v0.2):进度与结果经全局总线 type:"memory" 回传 ----
  memoryRoot(): string
  memoryStatus(): unknown
  /** 触发编译(立即返回;force=全量,dryRun=只跑确定性阶段) */
  memoryCompile(force?: boolean, dryRun?: boolean): void
  /** 取消运行中的编译;返回是否确有任务被取消 */
  memoryCancel(): boolean
  memoryReindex(): number
  memoryListConcepts(): unknown[]
  memoryShowConcept(conceptId: string): unknown
  memorySearch(query: string, limit?: number): unknown[]
  memoryValidate(): unknown
  memoryReadLog(limit?: number): unknown[]
  memoryListRawSources(): unknown[]
  memoryReadRawSource(stem: string): string
}

let binding: NapiBinding | null = null
let subscriberCancel: (() => void) | null = null

function resolveNodePath(): string {
  const file = 'astorm-backend.node'
  return app.isPackaged
    ? join(process.resourcesPath, file)
    : join(__dirname, '../../resources', file)
}

/** 加载原生绑定;失败返回 false(Mock 兜底),不抛错。幂等:已加载直接返回 true */
export function loadNapiBackend(): boolean {
  if (binding) return true
  try {
    const req = createRequire(__filename)
    const nodePath = resolveNodePath()
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    binding = req(nodePath) as NapiBinding
    console.log(`[napi] backend loaded: version=${binding.version?.()} path=${nodePath}`)
    return true
  } catch (err) {
    binding = null
    console.warn('[napi] backend unavailable, falling back to mock:', (err as Error)?.message)
    return false
  }
}

export function isNapiAvailable(): boolean {
  return binding !== null
}

/** 订阅后端全局事件流并中继到所有渲染窗口;重启(start)后需重新订阅 */
function ensureSubscription(): void {
  subscriberCancel?.()
  subscriberCancel = null
  if (!binding) return
  try {
    const stream = binding.subscribe()
    const reader = stream.getReader()
    subscriberCancel = (): void => {
      void reader.cancel().catch(() => {})
    }
    void (async () => {
      try {
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          relayEvent(value)
        }
      } catch (err) {
        console.warn('[napi] event stream closed:', (err as Error)?.message)
      } finally {
        subscriberCancel = null
      }
    })()
  } catch (err) {
    console.warn('[napi] subscribe failed:', (err as Error)?.message)
  }
}

function relayEvent(chunk: unknown): void {
  for (const win of BrowserWindow.getAllWindows()) {
    win.webContents.send('napi:event', chunk)
  }
}

/** 停止并重新注入配置(shutdown 会中断会话,后端无运行时 register,故全量重启) */
export function startNapiBackend(config: NapiAppConfig): boolean {
  if (!binding) return false
  try {
    if (binding.isRunning()) binding.shutdown()
    const started = binding.start(JSON.stringify(config))
    ensureSubscription()
    return started
  } catch (err) {
    console.error('[napi] start failed:', (err as Error)?.message)
    return false
  }
}

/** 热更新 LLM 配置:运行中直接 updateConfig(不中断会话),未运行则走 start。 */
export function syncNapiConfig(config: NapiAppConfig): void {
  if (!binding) return
  if (binding.isRunning() && binding.updateConfig) {
    try {
      binding.updateConfig(JSON.stringify(config))
      return
    } catch (err) {
      console.error('[napi] updateConfig failed, fallback to restart:', (err as Error)?.message)
    }
  }
  startNapiBackend(config)
}

// ---------- 会话命令(供 chat IPC 转发) ----------
// 会话命令在 binding 缺失时抛 BACKEND_UNAVAILABLE(不静默):renderer 把错误写入
// 助手气泡/停止反馈,避免「发送看似成功、暂停却无效」的 mock 闭环
function requireBinding(): NapiBinding {
  if (!binding) {
    throw new Error('BACKEND_UNAVAILABLE: backend not loaded')
  }
  return binding
}
export function napiCreateSession(request: unknown): string {
  return requireBinding().createSession(request)
}
export function napiSendMessage(sessionId: string, request: unknown): void {
  requireBinding().sendMessage(sessionId, request)
}
export function napiStartSession(sessionId: string): void {
  requireBinding().startSession(sessionId)
}
export function napiInterruptMessage(sessionId: string, request: unknown): void {
  requireBinding().interruptMessage(sessionId, request)
}
export function napiSubmitInterrupt(sessionId: string, request: unknown): void {
  requireBinding().submitInterrupt(sessionId, request)
}
export function napiReopenInterrupt(sessionId: string, toolCallId: string): void {
  requireBinding().reopenInterrupt(sessionId, toolCallId)
}
export function napiCancelSession(sessionId: string): void {
  requireBinding().cancelSession(sessionId)
}
export function napiRewindToUserMessage(sessionId: string, keepUserMessages: number): void {
  requireBinding().rewindToUserMessage(sessionId, keepUserMessages)
}
export function napiForkSession(sessionId: string, beforeFinal?: number): string {
  return requireBinding().forkSession(sessionId, beforeFinal)
}
export function napiSummarizeSession(sessionId: string): Promise<boolean> {
  return requireBinding().summarizeSession(sessionId)
}
export function napiUpdateSessionSettings(sessionId: string, request: unknown): void {
  requireBinding().updateSessionSettings(sessionId, request)
}
export function napiUpdateSessionGoal(
  sessionId: string,
  goal: string | null,
  goalLang?: string
): void {
  requireBinding().updateSessionGoal(sessionId, goal, goalLang)
}
export function napiListSessions(): unknown[] {
  return binding?.listSessions() ?? []
}
export function napiListMessages(sessionId: string): unknown[] {
  return binding?.listMessages(sessionId) ?? []
}
export function napiArchivedMessagesBefore(
  sessionId: string,
  beforeSeq: number,
  maxUserTurns: number
): unknown[] {
  return binding?.archivedMessagesBefore(sessionId, beforeSeq, maxUserTurns) ?? []
}
export function napiGetSystemPrompt(sessionId: string): string {
  try {
    return binding?.getSystemPrompt(sessionId) ?? ''
  } catch {
    return ''
  }
}
export function napiDeleteSession(sessionId: string): void {
  binding?.deleteSession(sessionId)
}

/** 后端是否已加载（未编译 .node 的开发环境为 false，前端据此降级 mock）。 */
export function isNapiLoaded(): boolean {
  return binding !== null
}

/** 应用退出前停止后端：回收 PTY/MCP 子进程、取消会话并刷新日志。 */
export function shutdownNapiBackend(): void {
  if (!binding) return
  try {
    if (binding.isRunning()) binding.shutdown()
  } catch (err) {
    console.error('[napi] shutdown failed:', (err as Error)?.message)
  }
}

/**
 * 通用后端命令调用（设置页：技能 / 工具 / 定时任务等 napi 接口）。
 * napi-rs 把 Rust 蛇形函数名导出为 camelCase；`name` 传 binding 上的方法名。
 * 后端未加载时抛带 `BACKEND_UNAVAILABLE` 前缀的错误供前端降级。
 */
export function napiCall(name: string, ...args: unknown[]): unknown {
  if (!binding) {
    throw new Error(`BACKEND_UNAVAILABLE: backend not loaded (${name})`)
  }
  const fn = (binding as unknown as Record<string, (...a: unknown[]) => unknown>)[name]
  if (typeof fn !== 'function') {
    throw new Error(`BACKEND_UNAVAILABLE: missing binding function ${name}`)
  }
  return fn.apply(binding, args)
}