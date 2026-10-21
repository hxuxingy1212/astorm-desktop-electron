/**
 * 会话类型(后端 `SessionRecord.sessionType` 是唯一真源):定时任务 / 普通会话 / 项目会话 / 子会话。
 */
export type SessionType = 'scheduled' | 'normal' | 'project' | 'child'

export interface Session {
  id: string
  title: string
  /** 中文标题（后端双标题生成；旧数据缺省，回退 title） */
  titleZh?: string
  /** English title（后端双标题生成；旧数据缺省，回退 title） */
  titleEn?: string
  createdAt: number
  /** 最近活动时间(会话每轮更新;重启回填自后端 updated_at;缺省回退 createdAt) */
  updatedAt?: number
  /** 侧栏分组:任务 / 项目(由 {@link SessionType} 推导;无类型的老记录才按工作区路径猜) */
  kind: 'task' | 'project'
  /**
   * 会话类型(后端权威):定时任务会话由后端创建/回填时标记;
   * 本地新建尚未拿到后端记录时缺省,发首条消息时按 `kind` 推导后声明给后端。
   */
  sessionType?: SessionType
  /**
   * 产生该会话的定时任务 id(定时任务会话才有值)。
   *
   * 定时任务会话**不进侧栏**,只在「定时任务」页的任务卡片下按时间列出 —— 靠它归集。
   */
  scheduledTaskId?: string | null
  /** 所属工作区(mock 数据) */
  workspace?: string
  /** 后端会话记录的模型名(列表回填;流式发送缺省时作 model 兜底) */
  model?: string
  /** Rust 后端 session_id(接入 napi 后惰性创建;缺省时走本地 Mock) */
  backendId?: string
  /**
   * 父会话的后端 id(子会话才有值;普通会话缺省)。
   *
   * 子会话由后端 `sub_session` 工具创建,左栏据此**缩进挂在父会话下**;
   * 父会话已删除(本地查不到)时该子会话当根行显示,不隐藏。
   */
  pid?: string | null
  /** 该会话使用的智能体 id(builtin-main / builtin-explore / 自定义 uuid) */
  agentType?: string | null
}

/** 提问表单字段(后端中断信封 `form.fields`；字段定义见 backend-contract §4.2) */
export interface UserFormField {
  name: string
  field_type: 'text' | 'number' | 'boolean' | 'single_choice' | 'multiple_choice'
  /** 向用户提出的问题原文(卡片标题；后端 schema 中必填) */
  description?: string
  required?: boolean
  options?: string[]
  /** boolean 字段「是」选项文案(两选项初始都不选中；缺省回退「是」) */
  true_label?: string | null
  /** boolean 字段「否」选项文案(用户未选则该键不出现在回答里；缺省回退「否」) */
  false_label?: string | null
}

/** 提问表单载荷(中断信封 `form`) */
export interface UserChatForm {
  message?: string
  tip?: string | null
  fields: UserFormField[]
}

/** 安全拦截分类（后端 DenialCategory，8 值；全链路统一，见 backend-contract §4.2） */
export type DenialCategory =
  | 'file_read'
  | 'file_write'
  | 'danger_command'
  | 'cwd_change'
  | 'network'
  | 'resource'
  | 'browser'
  | 'mail'

/** 分类来源 / 可信度：static_precheck|sandbox_preflight|sandbox_runtime 为确定，其余为推断 */
export type DenialSource =
  | 'static_precheck'
  | 'sandbox_preflight'
  | 'sandbox_runtime'
  | 'stderr_attribution'
  | 'exit_code'

/** 一次安全拦截的结构化描述（tool_result 的 denial / 消息 metadata.denial） */
export interface ToolCallDenial {
  category: DenialCategory
  /** 分类来源 / 可信度。审计记录（AuditLogEntry）历史上不含来源，故为可选：
   *  缺失时徽标只展示分类，不显示来源提示（不再由前端伪造一个来源）。 */
  source?: DenialSource
  detail: string
  targets?: string[]
  command?: string
}

/**
 * 一次 bash 执行的**沙箱事实**（后端 `ShellTool::result_meta` 从结果 JSON 提取，
 * tool_result 事件的 `sandbox` / 消息 `metadata.sandbox`；其他工具不产出）。
 * 供工具卡标注「无防护执行 / 被驱逐 / 输出截断」；全部正常时各字段不触发标注。
 */
export interface ToolCallSandboxFacts {
  /** 是否真跑在内核沙箱里；false = 无防护（总开关关闭 / 强制层降级 / 兜底重跑） */
  protected?: boolean
  /** 运行期被驱逐的原因（网络 / 危险命令 / 资源超限；已裁剪） */
  evicted?: string[]
  /** 因超时终止进程树 */
  timed_out?: boolean
  /** 输出被截断（沙箱侧或工具侧） */
  output_truncated?: boolean
}

/**
 * 写文件的增删统计 + diff（后端 `file_operation` 的 edit/write/append 结果元数据，
 * 或前端从入参派生）。`patch` 为限上下文 unified diff，供 `CodeDiffView` 渲染。
 */
export interface ToolFileDiff {
  path: string
  additions: number
  deletions: number
  patch?: string
  /** patch 新侧逻辑行数（CodeDiffView 尾部折叠条） */
  newLines?: number
  /** patch 因体积/行数超限被截断（此时前端只显示增删计数） */
  truncated?: boolean
}

/** 读文件的结果统计（后端 `file_operation` read 结果元数据）。 */
export interface ToolReadStats {
  lines: number
  chars: number
  truncated?: boolean
}

/** 网页搜索单条结果（后端 `websearch` 结果元数据 `web_results` 元素）。 */
export interface WebSearchResult {
  title: string
  url: string
  snippet: string
}

/** 记忆搜索单条结果（后端 `memory` search 结果元数据 `memory_results` 元素）。 */
export interface MemorySearchResult {
  concept_id: string
  title: string
  snippet: string
  observed_at?: string | null
}

/** 工具调用视图(供消息区渲染 tool 卡片) */
export interface ToolCallView {
  id: string
  name: string
  /** 参数(后端解析后的对象,展示时格式化) */
  params?: unknown
  status: 'pending' | 'completed' | 'error'
  result?: string
  duration?: number
  /** 写文件的结构化 diff（后端 metadata.file_diff；冷启动恢复） */
  fileDiff?: ToolFileDiff | null
  /** 读文件的行/字符统计（后端 metadata.read_stats；冷启动恢复） */
  readStats?: ToolReadStats | null
  /** 网页搜索结果（后端 websearch 结果元数据 web_results；冷启动恢复） */
  webResults?: WebSearchResult[] | null
  /** 记忆搜索结果（后端 memory search 结果元数据 memory_results；冷启动恢复） */
  memoryResults?: MemorySearchResult[] | null
  /** 等待用户决定(安全审批 / 提问):结果区显示为「等待…」,且不随流式收尾被标记失败 */
  awaitingApproval?: boolean
  /** 安全拦截分类(被拦/被拒时展示分类徽标;重启读历史仍恢复) */
  denial?: ToolCallDenial | null
  /** 沙箱运行事实(无防护 / 被驱逐 / 输出截断;仅 bash 产出,重启读历史仍恢复) */
  sandbox?: ToolCallSandboxFacts | null
  /** 中断标记(后端 metadata.interrupt;pending 时审批卡/提问表单可重新弹出) */
  interrupt?: {
    request_id: string
    interrupt_type: 'approval' | 'form' | 'mail_compose'
    status: string
    request?: unknown
  }
}

/**
 * 消息附件：按 `mime` 分两类（判定与渲染都以它为准）。
 * - **图片**（`image/*`）：`path` 发送前 = staging 副本、发送后 = 会话工作区 `.astorm/attachment/` 副本
 *   （都能经 `astorm-attachment://` 直读缩略图）；
 * - **其他文件**：`path` 始终是**用户原始路径**（既不 staging 也不复制进工作区），前端只按扩展名
 *   画文件图标、不取字节；后端把该路径写进用户消息文本，由 agent 按类型自行读取
 *   （PDF/Office 用 astorm-office 技能、图片用 read_images、文本用 file_operation）。
 */
export interface MessageAttachment {
  path: string
  /** 原始文件名(展示用) */
  name: string
  /** 字节数(可选;历史记录缺省时只显示文件名) */
  size?: number
  /** MIME(可选;非图片为按扩展名粗判,未知为 application/octet-stream) */
  mime?: string
}

/**
 * 一轮结束后展示的交付物（`submit_deliverables` 工具上报后由后端富集：
 * 仅保留磁盘真实存在的文件，防幻觉死卡片）。
 * `path` 相对会话工作区；`size` 为字节数（缺失时前端只显示名称 / 类型）。
 */
export interface ArtifactDeliverable {
  path: string
  name: string
  size?: number
}

/** 一轮结束后展示的变更文件（交付物同源；增删计数来自后端 git 差异摘要）。 */
export interface ArtifactChange {
  path: string
  name: string
  /** 目录部分（含尾部分隔符，与审查页签同口径） */
  dir?: string
  /** 状态字 A/M/D/R/C；未跟踪为 ? */
  status?: string
  additions: number
  deletions: number
}

/** 一轮的交付物 / 变更汇总（对话结束后展示；交付物与变更各上限展示 6，超出可「查看全部」）。 */
export interface TurnArtifacts {
  deliverables: ArtifactDeliverable[]
  changes: ArtifactChange[]
}

/**
 * 交付物表一行（后端 `session_deliverables` 表，`submit_deliverables` 工具逐轮落库，
 * 经 `list_session_deliverables` 拉取；前端在对话结束后按会话拉取渲染）。
 */
export interface DeliverableRow {
  sessionId: string
  /** 轮次 = 后端 question_times（本会话第几条用户消息，1 起） */
  round: number
  /** 会话标题快照 */
  sessionTitle: string
  /** 当轮用户消息 jsonl seq（与历史消息精确 join；直发消息无 seq 时回退文本匹配） */
  userMsgSeq: number
  /** 当轮用户消息文本快照 */
  userMessage: string
  deliverables: ArtifactDeliverable[]
  changedFiles: ArtifactChange[]
  /** 本轮真实有用的记忆 concept_id */
  memoryIds: string[]
  createdAt: number
  updatedAt: number
}

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  createdAt: number
  /** 旧会话的交付物 / 变更清单（历史消息 metadata.artifacts 只读兜底；新数据走交付物表） */
  artifacts?: TurnArtifacts
  /** 用户消息携带的图片附件(输入框「+」/ 粘贴截图 / 拖拽) */
  attachments?: MessageAttachment[]
  /** 是否正在流式输出 */
  streaming?: boolean
  /** 思考链(后端 reasoning 事件累积) */
  reasoning?: string
  reasoningStreaming?: boolean
  /** 工具调用(后端 tool_call / tool_result 事件) */
  tools?: ToolCallView[]
  /** 本轮用量（后端 `session_stats` 事件；挂在该轮结尾的无工具助手消息上） */
  usage?: { prompt?: number; completion?: number; total?: number }
  /** 后端致命错误(渲染为红色横幅,非正常回复) */
  error?: string
  /**
   * 本轮**处理起点**(ms):本轮第一个模型调用开始的时间,即收纳块耗时的被减数。
   * 实时流式由前端创建占位气泡时打点;历史会话从后端记录 `metadata.started_at` 回填
   * (消息元数据契约见 react-loop/engine.rs)。
   *
   * 为什么不直接用「本轮 user 消息的 createdAt」:看板「待办」是「仅创建」建出来的,
   * 指令可能隔几小时/几天才被「启动」——拿它当起点会把等待时间也算进耗时(显示成 41h)。
   */
  startedAt?: number
  /**
   * 本轮结束时间(ms):流式终态(done/取消/错误)由前端打点,
   * 历史会话从后端记录 metadata.ended_at 回填。对话区收纳块的耗时 = 该值 − 本轮处理起点
   */
  completedAt?: number
  /**
   * 本轮结束原因(done/取消/错误):只在前端打点(后端历史不区分),
   * 用于对话区的「工作已暂停，用户主动打断」状态行;历史轮次无此字段按正常完成处理
   */
  endReason?: 'done' | 'cancelled' | 'error'
  /**
   * 后端持久化序号(仅历史回载的 user 消息有;前端即时发送的消息无)。
   * 撤回据此判断是否可撤回(是否在会话摘要边界之后);缺省视为可撤回。
   */
  seq?: number
  /** 会话摘要消息标记(后端压缩历史写入;fork 计数需排除,避免误当「无工具助手消息」) */
  summary?: boolean
}

/** 本地文件树节点(后端 list_directory 镜像;children 递归整树) */
export interface FileNode {
  name: string
  /** 节点类型:"folder" | "file" */
  type: 'folder' | 'file'
  children?: FileNode[]
}

/** 文件预览页签内容状态(懒加载;content 未读时为 null) */
export interface FileTabState {
  filePath: string
  fileName: string
  loading: boolean
  content: string | null
  /** 读取失败原因(渲染为错误提示) */
  error?: string
  /** 是否二进制/不可预览(读取返回空但非文本,提示外部打开) */
  binary?: boolean
  /**
   * 打开来源。`deliverable` = 对话里的交付物（Agent 产出的成品），这类预览默认按
   * **宽档**打开（见 `panels/index.ts` 的 `widthModeOfTab`）——成品多为文档/表格/网页，
   * 窄档看不下；同一条路径先去重，重复打开仍复用已存在的页签与宽度。
   */
  origin?: 'deliverable'
  /**
   * 定位行（1 起）：正文里的**文件引用**（`` `auth.py:47` ``）带行号打开时给出，
   * 面板滚到该行并高亮；普通打开（文件树 / 交付物卡片）不带行号。
   */
  targetLine?: number
  /**
   * 定位请求序号：每点一次引用自增一次——重复点同一引用的**同一行**也要重新滚回去，
   * 而切页签 / 会话换挡不重复滚动（面板只认没处理过的序号，见 FilePreviewPanel）。
   */
  revealSeq?: number
}

/**
 * 内置浏览器页签的视口 / 缩放:
 * **运行期状态**,随会话内存快照保留(切会话/切页签不丢),但不落磁盘,应用重启即回默认。
 */
export interface WebViewportState {
  /** 铺满面板 / 按自定义尺寸居中 */
  mode: 'adaptive' | 'custom'
  /** 自定义视口宽高(仅 custom 模式生效) */
  w: number
  h: number
  /** 预设索引(-1 = 自定义尺寸) */
  presetIndex: number
  /** 缩放百分比(精确值;四角拖拽可产生任意整数) */
  zoomPercent: number
}

/** 内置浏览器页签内容状态(单网址页签;title/favicon 随加载更新,viewport 随会话内存快照) */
export interface WebTabState {
  url: string
  title: string
  /** 站点图标(abs/favicon URL;加载失败回退图标) */
  favicon?: string
  /** 视口 / 缩放(运行期;缺省 = 尚未设置,按面板默认值渲染) */
  viewport?: WebViewportState
}

/** 元素组件信息(内置浏览器拾取:Vue2/Vue3/React dev 构建下可探测到) */
export interface PickedComponent {
  name: string
  framework: 'vue2' | 'vue3' | 'react'
  /** 源码文件(dev 构建才带;生产站点通常缺失) */
  file?: string
  line?: number
  column?: number
}

/** 悬停卡片的一行(key = inspect.* 的字段名;值与页面内卡片同源,直接展示) */
export interface PickedElementCardRow {
  key: string
  value: string
  /** 渲染色块用的颜色值(rgb/rgba) */
  swatch?: string
  /** 该行前插入分隔线 */
  sep?: boolean
  /** 值用高亮色(源码路径) */
  path?: boolean
}

/** 悬停卡片模型:与页面内高亮卡片同一份数据(见 main/element-picker.ts cardModel) */
export interface PickedElementCard {
  /** 元素名(如 div.panel) */
  name: string
  /** 尺寸文本(如 1280×63) */
  size: string
  rows: PickedElementCardRow[]
}

/** 内置浏览器「选择元素」拾取结果(主进程注入脚本采集,见 src/main/element-picker.ts) */
export interface PickedElementData {
  /** 元素所在页面 URL */
  url: string
  /** 标签名(小写,如 button) */
  tagName: string
  id: string
  /** class 列表(空格顺序原样) */
  classList: string[]
  /** 绝对 XPath(如 /html/body/div[1]/form/button[1]) */
  xpath: string
  /** 最短可定位 CSS 选择器(#id 优先,否则 tag.class:nth-of-type) */
  cssSelector: string
  /** 元素文本(折叠空白,截断 120 字) */
  text: string
  /** 命中的最近组件(无框架页面为 undefined) */
  component?: PickedComponent
  /** 悬停卡片数据(发送框附件胶囊悬停时展示) */
  card: PickedElementCard
}

/**
 * 页签:五种形态
 * - 插件页签(pluginId):插件网站,一个插件最多一个页签
 * - 工具页签(toolId):固定类型面板,一个工具最多一个页签
 * - 文件页签(文件预览):每个文件一个页签,标题 = 文件名,内容区预览文件
 * - 网页页签(内置浏览器):**多页签**,按显式键去重(见 `stores/tabs.ts` 的 `tabKey`):
 *   有 URL 的按 URL(`web:<url>`,同 URL 复用、不同 URL 各自成页);空白页签(无 URL/about:blank)
 *   键带唯一令牌,每次手动新建都是新页签。标题 = 站点 title、图标 = 站点 favicon、视口/缩放 = viewport
 * - 子会话页签(session):后端 `sub_session` 创建的子会话,内容 = 它的实时对话
 *   (与对话页同款渲染,但无输入框、无规划胶囊;审批与表单照常)。按会话 id 去重
 */
export interface PanelTab {
  id: string
  /** 插件页签(渲染 PluginSitePanel;一个插件最多一个页签) */
  pluginId?: string
  toolId?: string
  /** 页签自定义标题(如终端 = 会话目录名);缺省用注册标题 */
  label?: string
  file?: FileTabState
  web?: WebTabState
  /** 子会话页签(渲染 SessionPanel;按会话 id 去重) */
  session?: SessionTabState
  /** 「相关资料」页签(渲染 SourceResultsPanel):本轮**网页**检索结果卡片 */
  sources?: { items: SourceItem[] }
  /** 「相关记忆」页签(渲染 MemoryResultsPanel):本轮**记忆**检索结果卡片 */
  memoryResults?: { items: MemorySource[] }
  /** 记忆概念页签(渲染 MemoryConceptPanel):记忆文档 markdown 预览 */
  memory?: { conceptId: string; label?: string }
  /** 记忆原文页签(渲染 MemoryRawPanel):raw 会话底稿预览 */
  memoryRaw?: { stem: string; label?: string }
}

/** 网页检索结果卡片(对话区「相关资料」) */
export interface WebSource {
  kind: 'web'
  title: string
  url: string
  snippet: string
  /** 来源域名(展示用) */
  source: string
}

/** 记忆检索结果卡片 */
export interface MemorySource {
  kind: 'memory'
  conceptId: string
  title: string
  snippet: string
  /** 观察时间（后端 `（观察于 …）`；缺失为空） */
  observedAt?: string
}

export type SourceItem = WebSource | MemorySource

/** 子会话页签状态:只存会话标识,标题实时取会话列表里的当前标题 */
export interface SessionTabState {
  /** 本地会话 id(= 子会话自身列表条目的 id,与 backendId 一致);
   *  「未开始」的委派用合成 id `pending:<toolCallId>` */
  sessionId: string
  /** 兜底标题(会话被删/列表未就绪时显示) */
  label?: string
  /** 「未开始」的子智能体委派:页签里只展示任务文本(用户消息),子会话创建后自动升级 */
  pendingTask?: string
  /** 发起该委派的工具调用 id */
  pendingToolCallId?: string
  /** 关联父会话后端 id + 该调用在 sub_session 调用序列中的下标(升级时按顺序对齐子会话) */
  pendingParentId?: string
  pendingCallIndex?: number
}

/* ── 页签 ↔ 后端同步契约(后端 `tabs` 工具;字段 snake_case,与 Rust `tool::tabs` 一致) ── */

/** 单个页签的上报形态(供后端 `list` 展示) */
export interface TabInfoPayload {
  id: string
  kind: 'browser' | 'file' | 'tool' | 'plugin' | 'session'
  tool_id?: string
  plugin_id?: string
  label?: string
  url?: string
  /** 站点标题(页面可控内容) */
  title?: string
  path?: string
  name?: string
  /** 缩放百分比 */
  viewport?: { mode: 'adaptive' | 'custom'; w: number; h: number; zoom_percent: number }
  active: boolean
}

/** 一条页签命令的异步回执(随下次上报带回,后端 `list` 会显示失败告警) */
export interface TabCommandResult {
  request_id: string
  ok: boolean
  message?: string
}

/** 上报给后端的整份页签快照 */
export interface TabsStatePayload {
  tabs: TabInfoPayload[]
  /** 当前激活页签序号(1 基;0 = 无) */
  active_index: number
  collapsed: boolean
  last_command?: TabCommandResult
}

/** 后端下发的一条页签命令(`tabs_command` 事件的 payload) */
export interface TabsCommand {
  type?: string
  session_id?: string
  timestamp?: number
  request_id?: string
  action: 'open' | 'switch' | 'remove'
  kind?: 'browser' | 'file'
  url?: string
  path?: string
  width?: number
  height?: number
  /** 页面缩放百分比(50 = 50%) */
  zoom?: number
  /** open(browser) 专用:true = 总是新建一个浏览器页签(多页签),缺省复用现有浏览器页签 */
  new_tab?: boolean
  /** 1 基序号,与 `list` 输出一致 */
  index?: number
  id?: string
}
