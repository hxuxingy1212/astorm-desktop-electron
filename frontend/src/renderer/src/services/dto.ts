/**
 * 后端 DTO 类型定义
 *
 * 对应 astorm-desktop 中 Rust 后端(serde 序列化)的 JSON 结构,
 * 供前端 store/组件共用(真实后端 napi 契约,无 mock 实现)。
 * 字段命名与真实后端 napi 输出一致(camelCase):
 * 技能/工具/定时任务的 DTO 输出均为 camelCase(sourcePath / parametersSchema / cronExpr)。
 */

import type { DenialCategory, DenialSource } from '@/types'

/** 技能摘要(list_skills 返回,不含 prompt) */
export interface SkillSummary {
  name: string
  description: string
  /** 逗号分隔的分类字符串,如 '研发效能,办公自动化' */
  category: string
  version: string
  /** user | project | global | builtin */
  source: string
  sourcePath: string
  enabled: boolean
  score?: number | null
  /** 卡片展示用(摘要可能携带) */
  tags?: string[]
}

/** 技能详情(get_skill 返回,编辑用) */
export interface SkillDoc {
  name: string
  description: string
  /** Markdown 指令内容 */
  prompt: string
  category: string
  tags: string[]
  version: string
  author: string
  source: string
  sourcePath: string
  enabled: boolean
  updatedAt: number
}

/** discover_skills 返回的盘上发现项 */
export interface DiscoveredSkill {
  name: string
  description: string
  path: string
  /** SKILL.md 解析失败时填充 */
  error?: string | null
}

/** create_skill 请求体 */
export interface CreateSkillRequest {
  name: string
  description: string
  prompt: string
  category: string[]
  tags: string[]
  version: string
  author: string
}

/** update_skill 请求体(全字段可选) */
export interface SkillUpdate {
  name?: string
  description?: string
  prompt?: string
  category?: string
  tags?: string[]
  version?: string
  author?: string
  enabled?: boolean
}

/** 智能体执行模式：只读(plan) / 读写(execute) / 自动(auto) / 继承(不干预) */
export type AgentExecMode = 'read_only' | 'read_write' | 'auto' | 'inherit'

/** 智能体权限模式：手动 / 自动 / 继承(不干预) */
export type AgentPermissionMode = 'manual' | 'auto' | 'inherit'

/** 智能体卡片(list_agents / get_agent / create_agent / update_agent 返回) */
export interface AgentCard {
  /** 智能体 id(自定义 = uuid;内置 = builtin-main / builtin-explore) */
  agentId: string
  name: string
  /** 描述(供模型判断该委派给谁) */
  description: string
  /** 额外的用户指引:非空则拼进系统提示词之后 */
  guidance: string
  /** 工具集(空 = 无工具可用) */
  toolIds: string[]
  /** 技能集(空 = 环境提示词不列技能;不影响 skills 工具加载) */
  skillIds: string[]
  /** 执行模式:决定会话模式(只读=plan,工具层硬拒写) */
  mode: AgentExecMode
  /** 权限模式:决定会话审批(手动弹卡 / 自动同意) */
  permission: AgentPermissionMode
  /** 是否内置(内置不可编辑/删除) */
  builtin: boolean
  createdAt: number
  updatedAt: number
}

/** create_agent 请求体 */
export interface CreateAgentRequest {
  name: string
  description?: string
  guidance?: string
  toolIds?: string[]
  skillIds?: string[]
  /** 缺省 inherit = 不干预 */
  mode?: AgentExecMode
  /** 缺省 inherit = 不干预 */
  permission?: AgentPermissionMode
}

/** update_agent 请求体(缺省字段保持原值;给出空数组 = 清空该集合) */
export interface AgentUpdate {
  name?: string
  description?: string
  /** 空串 = 清空指引 */
  guidance?: string
  toolIds?: string[]
  skillIds?: string[]
  mode?: AgentExecMode
  permission?: AgentPermissionMode
}

/** 工具来源:builtin | plugin | mcp(server) */export interface ToolSource {
  type: 'builtin' | 'plugin' | 'mcp'
  server?: string
}

/** 工具信息(list_tools / list_mcp_server_tools 返回) */
export interface ToolInfo {
  name: string
  description: string
  /** JSON Schema */
  parametersSchema: Record<string, unknown>
  source: ToolSource
  enabled?: boolean
  /** 是否「按需加载」动态工具(初始不下发 LLM,经 tool_load 加载后生效) */
  dynamic?: boolean
}

/** MCP 服务器摘要 */
export interface McpServerSummary {
  name: string
  transportType: 'http' | 'stdio'
  command: string
  args: string[]
  url: string
  enabled: boolean
  connected: boolean
  /** 远程服务器最近一次连接要求授权（OAuth / Bearer） */
  authRequired: boolean
}

/** MCP OAuth 授权启动结果 */
export interface McpOAuthStart {
  authorizationUrl: string
  redirectUri: string
}

/** MCP OAuth 授权状态（前端轮询） */
export interface McpOAuthStatus {
  status: 'none' | 'pending' | 'authorized' | 'failed'
  error: string | null
}

/** 后端真实支持的会话摘要配置(与 Rust AppConfig.summary 字段一致;DTO 输出 camelCase) */
export interface SummarySettings {
  token_threshold: number
  min_messages: number
  keep_token_threshold: number
  keep_tail_rounds: number
}

/** 后端真实支持的会话消息剪支配置(与 Rust AppConfig.compact 字段一致) */
export interface CompactSettings {
  compact_threshold: number
  compact_recent_rounds: number
  compact_byte_count: number
  compact_head_chars: number
  compact_tail_chars: number
}

/** 后端真实支持的目标循环配置(与 Rust AppConfig.goal 字段一致) */
export interface GoalSettings {
  max_tokens: number
  max_iterations: number
}

/** 网络搜索配置(与 Rust AppConfig.webSearch 字段一致);多引擎免 key 抓取,失败自动回退 */
export interface WebSearchSettings {
  /** 默认引擎:baidu | sogou | bing | duckduckgo | google */
  defaultEngine: string
  /** 单次请求超时(秒) */
  timeoutSec: number
}

/** 安全配置文件访问名单(条目为 glob,如 /Users/me/docs/**;与 Rust SecurityFileConfig 对齐) */
export interface SecurityFileSettings {
  readBlack: string[]
  readWhite: string[]
  writeBlack: string[]
  writeWhite: string[]
}

/** 安全配置网络名单(条目: 域名[:端口] 或 IP[/掩码];与 Rust SecurityNetConfig 对齐) */
export interface SecurityNetSettings {
  useWhitelist: boolean
  inBlack: string[]
  inWhite: string[]
  outBlack: string[]
  outWhite: string[]
}

/** 安全配置危险命令(条目为正则;与 Rust SecurityDangerConfig 对齐) */
export interface SecurityDangerSettings {
  useBuiltin: boolean
  /** 内置清单的用户编辑副本；null = 跟随代码内置 */
  builtinPatterns: string[] | null
  black: string[]
}

/** 安全配置资源限制(null = 不限;与 Rust SecurityResourceConfig 对齐) */
export interface SecurityResourceSettings {
  memoryMb: number | null
  diskMb: number | null
  cpuSeconds: number | null
  /** CPU 使用率上限（%）；macOS 采样节流 / Windows Job 硬上限；null = 不限 */
  cpuPercent: number | null
  gpu: boolean
}

/** 安全配置(与 Rust AppConfig.security 字段一致;camelCase) */
export interface SecuritySettings {
  enabled: boolean
  file: SecurityFileSettings
  net: SecurityNetSettings
  danger: SecurityDangerSettings
  resource: SecurityResourceSettings
  /** 本地环境页维护的被禁用工具 bin 名(沙箱自动注入据此过滤) */
  toolchainDisabled: string[]
  /** 额外可执行目录(非常规安装位置的工具链/编译器;exec 与只读一并放行) */
  execWhite: string[]
}

/** 审计日志条目（后端 AuditLog JSON 行结构） */
export interface AuditLogEntry {
  session_id: string
  /** unix 毫秒 */
  time: number
  /** fileread / filewrite / bash / bash_exec / network / resource / browser */
  type: string
  detail: string
  /** agree / deny / auto / none */
  approval: string
  approved_at?: number | null
  /** 执行前输入的原始命令（bash 原文 / file_operation 等；后端未记录时缺省） */
  command?: string | null
  /** 安全拦截分类（拒绝读/写/危险命令/网络/资源/浏览器；非拦截场景缺省不写） */
  category?: DenialCategory | null
  /** 分类来源 / 可信度（静态预检 / 沙箱预检 / 沙箱运行期 / 退出码归因；无分类时缺省不写） */
  source?: DenialSource | null
}

/** 本地编程环境探测结果（后端 detectLocalEnv 返回项） */
export interface LocalEnv {
  /** 环境显示名（如 "Node.js"） */
  name: string
  /** 是否检测到（可执行文件存在） */
  available: boolean
  /** 版本号（如 "v22.12.0"）；未检测到为 null */
  version: string | null
  /** 版本号来源（当前统一 "system"） */
  version_src: string | null
}

/** 沙箱自动注入白名单报告（后端 toolchain_allowances 返回） */
export interface ToolchainAllowanceReport {
  /** 全部探测工具的注入状态（含未安装项） */
  tools: ToolchainAllowanceTool[]
  /** 当前生效的 exec 放行目录（过滤禁用 + Cellar 前缀合并后） */
  execDirs: string[]
  /** 当前生效的只读放行 globs */
  readGlobs: string[]
  /** 系统技能目录（固定只读注入；后端未初始化为 undefined/缺省） */
  skillsDir?: string | null
}

/** 单个工具的沙箱注入状态（「本地环境」卡片展开区 + 安全配置只读展示共用） */
export interface ToolchainAllowanceTool {
  /** 环境显示名（与 localenv 探测一致） */
  name: string
  /** 工具 bin 名（toolchainDisabled 列表的键） */
  bin: string
  /** 是否检测到（可执行文件存在） */
  available: boolean
  /** 是否已被用户禁用注入 */
  disabled: boolean
  /** PATH 命中的真实所在目录（canonicalize；未检测到为 null） */
  resolvedDir: string | null
  /** 附加注入目录（rustup 真实工具链等；无则空） */
  extraDirs: string[]
  /** 禁用该工具后其目录是否仍被其他启用工具共享留在白名单 */
  sharedDir: boolean
}

export interface AppSettings {
  locale: string
  summary: SummarySettings
  compact: CompactSettings
  /** 目标循环配置(/goal 外循环预算) */
  goal?: GoalSettings
  /** 网络搜索配置(默认引擎;多引擎免 key 抓取,失败自动回退) */
  webSearch?: WebSearchSettings
  /** 默认会话空间基目录(相对日期工作区归入其下;未设置回退后端默认) */
  workspace?: string
  /** 安全配置(旧配置文件缺失时后端回退默认值) */
  security?: SecuritySettings
  /** 长期记忆配置(缺省回退后端默认值) */
  memory?: MemorySettings
  /** 约定文件配置(AGENTS.md 注入开关;缺省回退后端默认值 = 均开启) */
  instructions?: InstructionSettings
}

/** 约定文件注入开关(与 Rust `InstructionConfig` 对齐) */
export interface InstructionSettings {
  /** 注入用户目录下的全局 AGENTS.md(对所有会话生效) */
  injectGlobalAgents: boolean
  /** 注入会话工作区根目录的 AGENTS.md(仅该会话生效,冲突时覆盖全局) */
  injectWorkspaceAgents: boolean
}

/** 记忆分类(与 Rust MemoryCategory 对齐;type 键名与后端一致) */
export interface MemoryCategorySettings {
  /** 分类 id = 提取阶段 LLM 输出的 event_type 取值(小写、无空格) */
  id: string
  /** 目标目录名(相对 wiki/;空串 = 直接放 wiki/ 根) */
  dir: string
  /** 写入 Concept frontmatter 的 OKF type 值 */
  type: string
  /** 语义说明(拼进提取/编译提示词的取值集) */
  hint: string
}

/** 长期记忆配置(与 Rust MemoryConfig 字段一致;camelCase) */
export interface MemorySettings {
  enabled: boolean
  /** 会话结束后自动增量编译(默认关:避免静默消耗 API 额度) */
  autoCompile: boolean
  /** 编译所用供应商名(null = 默认供应商) */
  provider: string | null
  /** 编译所用模型(模型引用 id;null = 默认模型) */
  model: string | null
  categories: MemoryCategorySettings[]
  maxUnitsPerCompileCall: number
  rawCharBudget: number
  toolResultChars: number
  judgeCandidates: number
  extractSelfCritique: boolean
  curate: boolean
  curateUnitsPerCall: number
  minConfidence: number
  maxUnitsPerSession: number
  candidateMinOverlap: number
  maxTags: number
  maxAppendSections: number
  /** 正文「相关」小节里的概念互链条数(0 = 不生成) */
  relatedConcepts: number
  /** 会话中是否注入「与当前用户消息相关的记忆」(只影响模型看到什么,不改变编译产出) */
  injectMemories: boolean
  /** 每次注入的相关记忆条数上限(0 = 不注入) */
  injectedConcepts: number
}

/* ---------- 记忆库视图(memory_* 命令的返回形状;camelCase 直出) ---------- */

/** 单个分类的统计 */
export interface MemoryCategoryStat {
  id: string
  dir: string
  type: string
  hint: string
  count: number
}

/** 记忆库状态(记忆页顶部状态卡) */
export interface MemoryStatus {
  root: string
  enabled: boolean
  autoCompile: boolean
  running: boolean
  conceptTotal: number
  /** 待编译会话数(源指纹比对,不花 token) */
  pending: number
  /** 上次编译成功时间(RFC3339) */
  lastSync: string | null
  categories: MemoryCategoryStat[]
  /** 已落盘概念的解析错误(损坏文件) */
  parseErrors: string[]
}

/** 概念列表项 */
export interface MemoryConcept {
  conceptId: string
  /** 分类目录名(空 = wiki 根) */
  dir: string
  /** 分类 id(目录未配置分类时为空串) */
  categoryId: string
  type: string
  title: string
  description: string
  tags: string[]
  observedAt: string | null
  /** 最后写入时间(unix 秒) */
  updatedAt: number
  /**
   * 检索相关度(仅检索结果有值)。
   *
   * 注意:napi 对 `Option<f64>` 的 `None` 会**整个省略该字段**(不是 null),
   * 所以这里必须是可选,判断一律用 `!= null`。
   */
  score?: number | null
  /** 命中字段(仅检索结果有值) */
  matched: string[]
}

/** 概念的来源引用 */
export interface MemorySourceRef {
  /** 相对 Bundle 的资源路径(如 raw/sessions/2026-09-25-a1f9.md) */
  resource: string
  title: string | null
  lastModified: string | null
}

/** 概念详情(含 Markdown 原文) */
export interface MemoryConceptDetail {
  conceptId: string
  dir: string
  categoryId: string
  type: string
  title: string
  description: string
  tags: string[]
  status: string | null
  staleAfter: string | null
  observedAt: string | null
  /** 支撑锚点(如 m108)：权威依据集,前端据此渲染可点的「依据」并跳回原文对应行 */
  fragIds: string[]
  sources: MemorySourceRef[]
  generatedBy: string | null
  generatedAt: string | null
  model: string | null
  updatedAt: number
  /** 完整 Markdown 原文(含 frontmatter;预览区渲染正文) */
  markdown: string
}

/**
 * 变更历史条目(wiki/log.md)。
 *
 * 无 `reason`:log.md 行格式是 `- <时间> <动作> <概念ID> ← <来源>`,出于体积考虑不落变更原因。
 */
export interface MemoryLogEntry {
  at: string
  action: string
  conceptId: string
  resource: string
}

/** 源会话(raw/sessions/*.md) */
export interface MemoryRawSource {
  stem: string
  resource: string
  size: number
  updatedAt: number
}

/** 单条校验问题 */
export interface MemoryIssue {
  level: 'error' | 'warning'
  conceptId: string
  message: string
}

/** OKF 合规校验报告 */
export interface MemoryValidateReport {
  checked: number
  errors: MemoryIssue[]
  warnings: MemoryIssue[]
}

/** 编译报告(事件 bus 的 report 字段;与 Rust CompileReport 一致) */
export interface MemoryCompileReport {
  totalSessions: number
  processedSessions: number
  skippedSessions: number
  failedSessions: { sessionId: string; stage: string; message: string }[]
  extractedUnits: number
  compiledConcepts: number
  created: string[]
  updated: string[]
  skipped: string[]
  warnings: string[]
  calls: number
  promptTokens: number
  completionTokens: number
  cancelled: boolean
}

/** 记忆编译进度事件(type === 'memory';session_id 为空) */
export interface MemoryEvent {
  kind: 'started' | 'stage' | 'finished' | 'failed' | 'cancelled'
  stage: 'parse' | 'extract' | 'compile' | 'merge' | 'write' | null
  current: number
  total: number
  message: string
  /** finished / cancelled 时携带完整结果 */
  report: MemoryCompileReport | null
}

/** 定时任务(与源码 Rust ScheduledTask 结构一致;enabled 为 0/1 整数;DTO 输出 camelCase) */
export interface ScheduledTask {
  id?: string
  name: string
  description: string
  /** 5 段 cron 表达式 */
  cronExpr: string
  /** 运行方式：new_session（每次新建会话）| continuous_session（持续复用同一会话） */
  mode: string
  instruction: string
  /** 会话目录（触发时创建的会话用它作 workspace） */
  workspacePath: string
  /** 会话执行模式：plan | execute | auto（缺省 = 全局默认；智能体卡片可覆盖） */
  sessionMode?: string | null
  /** 会话审批模式：manual | auto */
  permissionMode?: string | null
  /** 执行智能体 id（缺省 = 内置默认主智能体） */
  agentType?: string | null
  // 模型 / 供应商 / 思考强度不是任务字段：触发时一律用设置里的默认模型与默认强度
  /** 最近一次触发的会话 id（持续复用模式的复用目标） */
  sessionId: string
  /** 0 | 1 */
  enabled: number
  /** unix 秒 */
  createdAt: number
}

/** add/update_scheduled_task 请求体(前端 camelCase,与源码一致) */
export interface ScheduledTaskRequest {
  id?: string
  name: string
  description: string
  cronExpr: string
  mode: string
  instruction: string
  workspacePath: string
  sessionMode?: string
  permissionMode?: string
  agentType?: string
}

/* ---------- 插件系统（plugin.json 清单 / 主进程 plugins.ts 视图） ---------- */

/** 插件配置字段（configSchema 条目，驱动设置页动态表单） */
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

/** 插件清单（plugin.json） */
export interface PluginManifest {
  id: string
  name: string
  version: string
  description?: string
  entry: string
  icon?: string
  permissions?: string[]
  sdkVersion?: string
  configSchema?: PluginConfigField[]
}

/** 已安装插件视图（plugin_scan 返回；secret 配置已解密） */
export interface PluginView {
  manifest: PluginManifest
  enabled: boolean
  config: Record<string, unknown>
  entryUrl: string
}
/* ---------- 浏览器数据（本机 Chrome Cookie 导入 / 清理） ---------- */

/**
 * 导入本机 Chrome Cookie 的结果（`import_chrome_cookies` 返回）。
 * 注意：不包含任何 cookie 值，只有统计数字。
 */
export interface ChromeCookieImportResult {
  /** 使用的 Chrome Profile 名（如 `Default`）。 */
  profile: string
  /** Profile 目录（便于用户核对来源）。 */
  profileDir: string
  /** Cookie 库里的总条数（含解不开的）。 */
  total: number
  /** 因加密方式不支持而跳过的条数（Windows Chrome 127+ 的 app-bound 加密）。 */
  skipped: number
  /** 解密失败的条数。 */
  decryptFailed: number
  /** 成功写入内置浏览器会话的条数。 */
  imported: number
  /** 写入会话失败的条数（个别脏数据）。 */
  writeFailed: number
}

/** 清除全部浏览器数据的结果（`clear_browser_data` 返回）。 */
export interface ClearBrowserDataResult {
  /** 清理了站点数据的站点（域名）数。 */
  sites: number
  /** 删除的 cookie 条数。 */
  cookies: number
}
