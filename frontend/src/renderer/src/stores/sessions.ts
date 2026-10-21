import { defineStore } from 'pinia'
import type {
  ArtifactChange,
  ChatMessage,
  DeliverableRow,
  DenialCategory,
  DenialSource,
  MessageAttachment,
  Session,
  SessionType,
  ToolCallDenial,
  ToolCallSandboxFacts,
  ToolCallView,
  ToolFileDiff,
  ToolReadStats,
  MemorySearchResult,
  TurnArtifacts,
  UserChatForm,
  WebSearchResult
} from '@/types'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import { uid, todayKey } from '@/utils'
import { stripArtifactMarkers } from '@/utils/artifacts'
import { i18n } from '@/i18n'
import type { StreamContext } from '@/services/agent'
import { invoke } from '@/services/backend'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { isWebShell } from '@/utils/platform'
import { deriveMentionRefs } from '@/components/mentions/derive'
import type { ChipMentionRef, QuoteRefPayload } from '@/components/mentions/types'
import { Message } from '@arco-design/web-vue'
import { useChatPrefsStore, type ChatMode } from '@/stores/chatPrefs'
import {
  EFFORT_LEVEL,
  MODE_FROM_BACKEND,
  MODE_TO_BACKEND,
  PERMISSION_FROM_BACKEND,
  PERMISSION_TO_BACKEND,
  effortOf,
  toBackendSettings,
  type SessionSettingsView
} from './sessionSettings'
import { useTabsStore } from '@/stores/tabs'
import { usePluginSessionStore } from '@/stores/pluginSession'

const SESSIONS_KEY = 'astorm:sessions:v2'
/** 已删除会话的 backendId 墓碑:防 bootstrapFromBackend 从后端列表回填 */
const DELETED_KEY = 'astorm:sessions:deleted:v1'
/** 前端消息队列(按会话)持久化键:会话忙时排队的待发消息,重启不丢失 */
const PENDING_KEY = 'astorm:sessions:pending:v1'

/**
 * 墓碑保留期：超过这个时间就不再拦回填。
 * 后端 uuid 不会复用，留着只是防「删除后运行循环 / 标题生成把记录写回」的反弹；
 * 无限增长会让 localStorage 里存一份越来越长的死名单（v1 是纯数组，没有时间戳，无从过期）。
 */
const TOMBSTONE_TTL_MS = 30 * 24 * 60 * 60 * 1000

/**
 * 读墓碑：backendId → 删除时刻(ms)。
 * 兼容 v1 的纯数组形态：老条目**按「现在」计时**（比直接丢弃安全 —— 丢弃会让当时后端删除
 * 失败的会话下次启动又冒出来），并立刻以新形态写回，完成一次性迁移。
 */
function loadDeletedIds(): Record<string, number> {
  try {
    const raw = localStorage.getItem(DELETED_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    const now = Date.now()
    const out: Record<string, number> = {}
    if (Array.isArray(parsed)) {
      for (const id of parsed) if (typeof id === 'string' && id) out[id] = now
      saveDeletedIds(out)
      return out
    }
    if (parsed && typeof parsed === 'object') {
      let pruned = false
      const entries = Object.entries(parsed as Record<string, unknown>)
      for (const [id, at] of entries) {
        const ts = typeof at === 'number' ? at : 0
        if (now - ts <= TOMBSTONE_TTL_MS) out[id] = ts
        else pruned = true
      }
      // 剪掉的别留在 localStorage 里（否则这份死名单只会越来越长）
      if (pruned) saveDeletedIds(out)
    }
    return out
  } catch {
    return {}
  }
}

function saveDeletedIds(ids: Record<string, number>): void {
  try {
    localStorage.setItem(DELETED_KEY, JSON.stringify(ids))
  } catch {
    /* 忽略持久化失败 */
  }
}

/** 读取持久化的前端消息队列(按会话;字段校验,损坏项丢弃) */
function loadPendingQueues(): Record<string, PendingQueueItem[]> {
  try {
    const raw = localStorage.getItem(PENDING_KEY)
    const parsed = raw ? (JSON.parse(raw) as unknown) : null
    if (!parsed || typeof parsed !== 'object') return {}
    const out: Record<string, PendingQueueItem[]> = {}
    for (const [sid, items] of Object.entries(parsed as Record<string, unknown>)) {
      if (!Array.isArray(items)) continue
      const valid = items
        .filter(
          (x): x is PendingQueueItem =>
            !!x &&
            typeof (x as PendingQueueItem).id === 'string' &&
            typeof (x as PendingQueueItem).text === 'string' &&
            typeof (x as PendingQueueItem).priority === 'number'
        )
        .map((x) => {
          // opts 归一化:早期持久化项 / 脏数据可能缺失 opts(发送时无 model → 后端报 Missing field);
          // 补成空对象,发送时 withModelFallback 再回退会话模型
          const opts = (x as PendingQueueItem).opts
          return { ...x, opts: opts && typeof opts === 'object' ? opts : {} }
        })
      if (valid.length) out[sid] = valid
    }
    return out
  } catch {
    return {}
  }
}

function savePendingQueues(queues: Record<string, PendingQueueItem[]>): void {
  try {
    localStorage.setItem(PENDING_KEY, JSON.stringify(queues))
  } catch {
    /* 忽略持久化失败 */
  }
}

/**
 * 工作区为绝对路径(用户选择了目录)→ 会话归类「项目」;
 * 默认日期工作区(yyyy-MM-dd)或未配置 → 「任务」。
 * **仅**用于没有后端 `sessionType` 的老记录兜底;有类型时一律以 {@link deriveKind} 为准。
 */
function isProjectWorkspace(workspace: unknown): boolean {
  if (typeof workspace !== 'string') return false
  const ws = workspace.trim()
  if (!ws) return false
  // Windows 盘符开头的绝对路径 / POSIX 根路径
  return /^[a-zA-Z]:[\\/]/.test(ws) || /^\//.test(ws)
}

/**
 * 会话类型 → 侧栏分组(`kind`)。
 *
 * 后端 `sessionType` 是唯一真源:只有「项目会话」进「项目」分组,定时任务 / 普通 / 子会话
 * 都归「任务」(子会话另由调用方按父会话分组覆盖)。缺类型的老记录仍按工作区路径猜,避免
 * 升级瞬间把所有历史会话都挪进「任务」。
 */
function deriveKind(sessionType: unknown, workspace: unknown): Session['kind'] {
  if (typeof sessionType === 'string' && sessionType) {
    return sessionType === 'project' ? 'project' : 'task'
  }
  return isProjectWorkspace(workspace) ? 'project' : 'task'
}

/** 后端事件/记录里的会话类型(`sessionType`) → 前端枚举(非法值当缺省)。 */
function readSessionType(value: unknown): SessionType | undefined {
  return value === 'scheduled' || value === 'normal' || value === 'project' || value === 'child'
    ? value
    : undefined
}

/**
 * 审批信封的 approval 段 → 工具卡分类（用于显示分类徽标）。
 * 新记录带 `category`（6 值）；已落盘的旧待决记录只有 `kind`
 * （`file_read` / `file_write` / `network`，取值与分类一致）→ 直接兜底复用。
 */
function denialFromApproval(a?: InterruptRequest['approval']): ToolCallDenial | null {
  const category = (a?.category ?? a?.kind) as DenialCategory | undefined
  if (!category) return null
  return {
    category,
    source: (a?.source as DenialSource) ?? 'static_precheck',
    detail: a?.reason ?? '',
    targets: a?.targets ?? [],
    command: a?.command
  }
}

/**
 * 中断信封 → 待决中断(仅接受结构化字段,不从工具结果文本里刮取)。
 * 信封结构由后端 `agent::interrupt` 统一约定,审批与提问同构,仅 `interrupt_type` 与专属载荷不同。
 */
function interruptFromPayload(payload: unknown): PendingInterrupt | null {
  if (!payload || typeof payload !== 'object') return null
  const obj = payload as Record<string, unknown>
  const rid = obj.request_id
  const type = obj.interrupt_type
  if (typeof rid !== 'string' || !rid) return null
  if (type !== 'approval' && type !== 'form' && type !== 'mail_compose') return null
  return {
    request: obj as unknown as InterruptRequest,
    request_id: rid,
    interrupt_type: type,
    tool_call_id: typeof obj.tool_call_id === 'string' ? obj.tool_call_id : undefined
  }
}

/** 工具结果的 metadata.interrupt(后端持久化的中断标记;冷启动据此重绘审批卡/提问表单) */
interface InterruptMarker {
  request_id?: string
  interrupt_type?: InterruptType
  status?: string
  /** 完整中断信封(重建卡片与提问表单所需) */
  request?: InterruptRequest
}

/** 按 tool_call id 在全部消息里定位工具卡:重放结果到达时最新消息已不是发起调用的那条助手消息 */
function findToolById(list: ChatMessage[], id: string | undefined): ToolCallView | undefined {
  if (!id) return undefined
  for (let i = list.length - 1; i >= 0; i--) {
    const tool = list[i].tools?.find((t) => t.id === id)
    if (tool) return tool
  }
  return undefined
}

/** 最后一条仍在流式的助手消息(收尾打点用;列表尾部可能是新插入的用户消息/占位) */
function findLastStreamingAssistant(list: ChatMessage[] | undefined): ChatMessage | undefined {
  if (!list?.length) return undefined
  for (let i = list.length - 1; i >= 0; i--) {
    const msg = list[i]
    if (msg.role === 'assistant' && msg.streaming) return msg
  }
  return undefined
}

/** 后端消息 metadata 里的 unix 毫秒时间戳（缺失 / 为 0 / 非数字 → undefined） */
function asRecordTime(value?: unknown): number | undefined {
  return typeof value === 'number' && value > 0 ? value : undefined
}

/**
 * 助手记录的「结束时间」:优先 ended_at,为 0(被打断的轮次 stream 未走完)时退回 started_at。
 * 两者都是 unix 毫秒;都缺失则返回 undefined(界面显示「已处理」不带数字)。
 */
function pickRecordTime(endedAt?: unknown, startedAt?: unknown): number | undefined {
  return asRecordTime(endedAt) ?? asRecordTime(startedAt)
}

/** 文本行数（与后端 `agent::artifacts::text_lines` 同口径：末尾换行不额外算一行） */
function countTextLines(content: string): number {
  if (!content) return 0
  const parts = content.replace(/\r\n/g, '\n').split('\n')
  if (parts.length && parts[parts.length - 1] === '') parts.pop()
  return parts.length
}

/** 把相对工作区路径拼成绝对路径（正/反斜杠都归一为 `/`，后端可读） */
function joinWorkspacePath(dir: string, rel: string): string {
  const base = dir.replace(/[\\/]+$/, '')
  return `${base}/${rel.replace(/^[\\/]+/, '')}`
}

/** 后端事件订阅句柄(模块级,响应式状态不需要) */
let chatEventOff: (() => void) | null = null

/** 丢帧补拉的防抖句柄与节流时间(模块级:与 store 实例无关的一次性兜底) */
let catchUpTimer: ReturnType<typeof setTimeout> | null = null
let catchUpAt = 0
/** 两次丢帧补拉之间的最小间隔:积压时后端可能连推多条告警帧,不要每条都全量重拉 */
const CATCH_UP_MIN_INTERVAL_MS = 5000

/**
 * 各会话当前已见轮次(来自事件的 round 字段)。
 * 轮次变大且 >1 时,新一轮的 think/content/tools 应拆到独立的助手消息。
 */
const roundBySession: Record<string, number> = {}

/**
 * 「会话正在干活」的轮次级事件:收到任一即说明新一轮已在跑。
 * 用于撤掉上一轮遗留的错误条 / 重试提示——用户发「继续」后旧错误条不再代表当前状态,
 * 继续挂着会与正在流式的这一轮自相矛盾(实测反馈)。
 * 不含 done/error/session_info:前者是终态、后者在后端每轮落库时都会推(会在错误条刚
 * 挂上时又把它抹掉)。
 */
const TURN_ACTIVITY_EVENTS = new Set([
  'user_message',
  'goal_continue',
  'text',
  'reasoning',
  'tool_call',
  'tool_result',
  'interrupt',
  'stream_reset'
])

/** 会话列表持久化的 trailing 节流窗口(ms):每 token 一次 JSON.stringify + localStorage 会卡死主线程 */
const PERSIST_THROTTLE_MS = 1000

/**
 * 流式文本 delta 的**合帧缓冲**(模块级,非响应式)。
 *
 * 后端每 token 一条事件(一帧内可达几十条),逐条同步写 store 会各自触发一次依赖通知与渲染,
 * 大上下文下 markdown 重解析叠加整表重渲染直接把主线程压死。这里把 `text`/`reasoning`
 * 累积进缓冲,用一次 `requestAnimationFrame` 合并写回(**一帧最多一次**);
 * 非文本事件(工具/中断/终态)到达前先 flush,保证「正文先于工具卡/终态落位」的时序不变。
 */
interface StreamDeltaBuf {
  sessionId: string
  /** 目标消息对象引用:期间即使轮次切换导致列表 push 新占位,也不会把正文写到错的消息上 */
  msg: ChatMessage
  text: string
  reasoning: string
}
const streamDeltaBuf = new Map<string, StreamDeltaBuf>()
/** 待执行的合帧句柄(0 = 无) */
let streamDeltaRaf = 0
/** persist 的 trailing 节流句柄 */
let persistTimer: ReturnType<typeof setTimeout> | null = null

/** 后端消息记录(chat:list-messages 返回;napi #{napi(object)} + serde camelCase 序列化) */
interface BackendMessageRecord {
  id?: string
  role?: string
  content?: string
  toolCalls?: unknown
  seq?: number
  createdAt?: number
  reasoningContent?: string | null
  toolCallId?: string | null
  metadata?: {
    started_at?: number
    ended_at?: number
    /** 工具结果状态(后端显式给出:ok / error / pending;缺省按结果文风推断) */
    status?: string
    /** 安全拦截分类(后端持久化;冷启动恢复分类徽标) */
    denial?: ToolCallDenial | null
    /** 沙箱运行事实(后端持久化;冷启动恢复无防护/被驱逐/输出截断标注) */
    sandbox?: ToolCallSandboxFacts | null
    /** 写文件结构化 diff(后端 execute_with_meta;冷启动恢复 +N -N / 展开 diff) */
    file_diff?: ToolFileDiff | null
    /** 读文件行/字符统计(后端 execute_with_meta;冷启动恢复 N行M字符) */
    read_stats?: ToolReadStats | null
    /** 网页搜索结果(后端 websearch 结果元数据;冷启动恢复「相关资料」) */
    web_results?: WebSearchResult[] | null
    /** 记忆搜索结果(后端 memory search 结果元数据;冷启动恢复「相关记忆」) */
    memory_results?: MemorySearchResult[] | null
    /** 中断标记(后端持久化;冷启动据此重绘审批卡 / 提问表单 / 恢复工具卡状态) */
    interrupt?: InterruptMarker | null
    /** 用户消息图片附件(后端复制进会话工作区后的路径;重载后显示缩略图) */
    attachments?: { path: string; name?: string; size?: number; mime?: string }[] | null
    /** 助手最终答复的交付物 / 变更清单(后端落盘前提取;重载后恢复消息下方的卡片) */
    artifacts?: TurnArtifacts | null
    /** 会话摘要消息标记(后端压缩历史时写入;用于撤回的摘要边界判定) */
    summary?: boolean
  } | null
}

/** 中断类型:安全审批 / 向用户提问 / 写邮件确认 */
export type InterruptType = 'approval' | 'form' | 'mail_compose'

/** 中断信封(工具结果 = interrupt 事件 payload = metadata.interrupt.request 的同构结构) */
export interface InterruptRequest {
  interrupt: true
  interrupt_type: InterruptType
  request_id: string
  /** 被中断的工具调用 id(前端把卡片关联到对应工具卡) */
  tool_call_id?: string
  /** 给模型的说明(中断期间模型看不到) */
  message?: string
  /**
   * 审批专属载荷。
   * `category`/`source`/`targets` 为分类契约(6 值 + 可信度);`kind` 为旧字段
   * (已落盘的旧待决中断记录仍带),读取按 `category ?? kind` 兜底。
   */
  approval?: {
    category?: DenialCategory
    kind?: string
    source?: DenialSource
    targets?: string[]
    command?: string
    reason?: string
  }
  /** 提问专属载荷 */
  form?: UserChatForm
  /**
   * 写邮件确认专属载荷。**刻意极小**：只带附件元数据（文件名 + 大小，前端算不出）。
   * 收件人/主题/正文/附件路径都在本次 tool 调用的参数里，前端按 `tool_call_id` 取回。
   */
  mail_compose?: {
    attachment_meta?: { path: string; name: string; size: number }[]
  }
}

/** 待决中断(前端卡片状态;来自 interrupt 事件或历史 metadata.interrupt) */
export interface PendingInterrupt {
  request: InterruptRequest
  request_id: string
  interrupt_type: InterruptType
  tool_call_id?: string
}

/** 中断决议(与 napi `SubmitInterruptRequest` 对应,按类型取用) */
export type InterruptDecision =
  /** 审批:`once` 缺省(一次性)/ `session` 记入会话内存白名单,本会话内同目标不再弹卡 */
  | { decision: 'allow'; scope?: 'once' | 'session' }
  | { decision: 'deny' }
  | { answer: string }
  | { ignored: true }

/** 会话运行状态(后端 SessionStatus;会话级唯一真相源,侧栏状态图标据此渲染) */
export type SessionStatus =
  | 'idle'
  | 'running'
  | 'awaiting_approval'
  | 'awaiting_form'
  | 'awaiting_mail_compose'
  | 'done'
  | 'error'
  | 'cancelled'
  | 'interrupted'
  /** 用户忽略了提问(侧栏灰点;可经「继续填写表单」恢复) */
  | 'ignored'

/** 会话状态点/看板列的**判定结果**（见 `statusKeyOf`）：null = 无标记（新会话/未跑过）。 */
export type SessionStatusKey =
  | 'running'
  | 'form'
  | 'interrupted'
  | 'ignored'
  | 'cancelled'
  | 'error'
  | 'done'
  | null

/**
 * 细粒度状态 key（= `session.status.*` 的 i18n key；null = 无标记）。
 * 比 [`SessionStatusKey`] 多区分「等待你的回答 / 等待你的审批」（两者共用同一个橙色状态点）。
 */
export type SessionStatusLabelKey =
  | 'running'
  | 'awaitingForm'
  | 'awaitingApproval'
  | 'awaitingMail'
  | 'interrupted'
  | 'ignored'
  | 'cancelled'
  | 'error'
  | 'done'
  | null

/** 任务看板列（见 `boardColumnOf`）。 */
export type BoardColumnKey = 'todo' | 'pending' | 'running' | 'done'

/** 状态判定所需的最小 state 视图（纯函数入参；含 `sessions` 以聚合子会话状态）。 */
interface StatusStateView {
  sessions: Session[]
  streamingIds: string[]
  taskFlags: Record<
    string,
    { formWaiting?: boolean; awaitingApproval?: boolean; mailComposeWaiting?: boolean; end?: string }
  >
  sessionInfo: Record<string, SessionInfo | undefined>
}

/**
 * **单个会话自身**的状态判定（纯函数内核；优先级：流式/后端运行中 > 等你处理(待审批·待答表单)
 * > 进程中断 > 已忽略提问 > 用户打断·执行失败 > 正常结束 > 无标记）。
 *
 * 「等你处理」必须先于终态：「中断待答」的会话同时带着 done 事件写下的 `end`
 * （见 stores 的 interrupt 处理），先判终态会把它误显示成「已完成」。
 * 未打开过的会话没有 `taskFlags`，退化为看后端状态。
 */
function ownStatusLabelKey(state: StatusStateView, id: string | undefined): SessionStatusLabelKey {
  if (!id) return null
  if (state.streamingIds.includes(id) || state.sessionInfo[id]?.status === 'running') return 'running'
  const flag = state.taskFlags[id]
  if (flag?.formWaiting) return 'awaitingForm'
  if (flag?.awaitingApproval) return 'awaitingApproval'
  if (flag?.mailComposeWaiting) return 'awaitingMail'
  if (flag?.end === 'interrupted') return 'interrupted'
  if (flag?.end === 'ignored') return 'ignored'
  if (flag?.end === 'cancelled') return 'cancelled'
  if (flag?.end === 'error') return 'error'
  if (flag?.end === 'done') return 'done'
  switch (state.sessionInfo[id]?.status) {
    case 'awaiting_form':
      return 'awaitingForm'
    case 'awaiting_approval':
      return 'awaitingApproval'
    case 'awaiting_mail_compose':
      return 'awaitingMail'
    case 'done':
      return 'done'
    case 'error':
      return 'error'
    case 'cancelled':
      return 'cancelled'
    case 'interrupted':
      return 'interrupted'
    case 'ignored':
      return 'ignored'
    default:
      return null
  }
}

/**
 * 状态严重度序号（越大越「糟」）。主任务聚合子任务时取最大值。
 *
 * 排序意图：**需要用户介入的排最前**——待审批 > 待答表单 > 执行失败 > 进程中断 > 用户打断 >
 * 运行中 > 已完成 > 已忽略（`ignored` 是用户主动放弃，不算需要介入，**不盖过 done**）> 无标记。
 */
const STATUS_SEVERITY: Record<Exclude<SessionStatusLabelKey, null>, number> = {
  awaitingApproval: 8,
  awaitingForm: 7,
  awaitingMail: 7,
  error: 6,
  interrupted: 5,
  cancelled: 4,
  running: 3,
  done: 2,
  ignored: 1
}

function severityOf(key: SessionStatusLabelKey): number {
  return key ? STATUS_SEVERITY[key] : 0
}

/**
 * 会话状态（对外唯一入口）：**父任务聚合直接子会话的状态，取最糟的一个**。
 *
 * 子会话（后端 `sub_session` 创建，深度=1）不进侧栏/看板，入口收在规划胶囊里；
 * 但父任务派发子任务后，子任务可能停在「待审批 / 待答表单 / 执行失败 / 中断」而父任务
 * 仍在运行或已结束——只看父自身状态会让用户看不出需要介入。这里把 `pid === 父.backendId`
 * 的直接子会话自身状态并入，按 [`STATUS_SEVERITY`] 取最差者，父行状态点随之同步。
 */
function sessionStatusLabelKey(
  state: StatusStateView,
  id: string | undefined
): SessionStatusLabelKey {
  const own = ownStatusLabelKey(state, id)
  if (!id) return own
  const backendId = state.sessions.find((s) => s.id === id)?.backendId
  if (!backendId) return own
  let worst = own
  let worstSeverity = severityOf(own)
  for (const child of state.sessions) {
    if (!child.pid || child.pid !== backendId) continue
    const childKey = ownStatusLabelKey(state, child.id)
    const severity = severityOf(childKey)
    if (severity > worstSeverity) {
      worst = childKey
      worstSeverity = severity
    }
  }
  return worst
}

/** 会话信息(后端 SessionRecord camelCase;经 session_info 事件实时推送 / 列表记录回填) */
export interface SessionInfo {  id?: string
  /** 父会话 id（子会话才有值；父会话行悬浮列表据它归集） */
  pid?: string | null
  /** 会话类型：scheduled | normal | project | child（后端唯一真源） */
  sessionType?: string
  /** 产生该会话的定时任务 id（定时任务会话才有值；任务卡片下按它归集执行记录） */
  scheduledTaskId?: string | null
  /** 该会话使用的智能体 id */
  agentType?: string | null
  title?: string | null
  /** 中文标题（后端双标题生成） */
  titleZh?: string | null
  /** English title（后端双标题生成） */
  titleEn?: string | null
  model?: string | null
  provider?: string | null
  workspace?: string | null
  sessionMode?: string
  reasoningEffort?: string
  permissionMode?: string
  skillList?: string[]
  toolList?: string[]
  mcpList?: string[]
  /** 经 tool_load 加载的延迟工具（LLM 实际可用集 = toolList ∪ loadedToolList；后端唯一真源） */
  loadedToolList?: string[]
  sessionUsage?: { prompt?: number; completion?: number; total?: number; cache_hit?: number; cache_miss?: number }
  /** 子会话聚合用量（父会话：所有子 agent 的 token 之和；展示总量 = sessionUsage + subagentUsage） */
  subagentUsage?: { prompt?: number; completion?: number; total?: number; cache_hit?: number; cache_miss?: number }
  /** 当前上下文占用的真实 token（上一轮请求 prompt_tokens；会话信息「上下文容量」进度条） */
  contextTokens?: number
  summaryTimes?: number
  /** 用户提问次数（累计追加的用户消息条数；撤回历史时回退） */
  questionTimes?: number
  currentRound?: number
  createdAt?: number
  updatedAt?: number
  /** 会话运行状态（后端唯一真相源；冷启动恢复侧栏状态图标） */
  status?: SessionStatus
  /** 状态最后变更时间（unix 秒） */
  statusUpdatedAt?: number
  /** 目标循环状态（`/goal`；null = 无目标） */
  goal?: {
    text?: string
    lang?: string
    tokensUsed?: number
    iterations?: number
    /** active = 仍在推进；completed = 已达成；stopped = 预算/判定失败等结束 */
    status?: 'active' | 'completed' | 'stopped'
  } | null
}

/** 发送选项:会话参数 + 前端对话框设置(经映射传给后端) */
export type ChatSendOptions = Omit<StreamContext, 'mode'> & {
  workspace?: string
  /** 图片附件(staging 副本;toBackendRequest 只取路径下发,气泡即时显示缩略图用完整描述) */
  attachments?: MessageAttachment[]
  /** 显式会话模式(仅 `/plan` 用):随本条消息原子下发,后端在跑循环前写入会话记录 */
  mode?: ChatMode
  /** 目标文本(仅 `/goal` 用):随本条消息原子设置会话目标循环 */
  goal?: string
  /** 目标语言(界面语言;判定提示词/续跑指令语言) */
  goalLang?: string
  /** 文件/智能体引用:正文只保留「原文」,这里下发结构化标识(后端生成/重建引用说明) */
  mentions?: ChipMentionRef[]
  /** 浏览器拾取元素的围栏原文(后端按元数据落盘,不渲染) */
  elements?: string[]
  /** office 预览拾取的 ```office-ref 围栏原文(后端按元数据落盘,不渲染) */
  officeRefs?: string[]
  /** 对话正文引用(完整文本 + 来源;后端生成/重建引用说明) */
  quotes?: QuoteRefPayload[]
}

/* ---------- 会话级设置(模式 / 审批 / 思考强度 / 模型 / 供应商)----------
 *
 * 这五项都是**会话的持久化属性**:唯一真相是后端会话记录(`SessionRecord`,
 * 经 `session_info` 事件与冷启动列表回填到 `sessionInfo`)。前端只留一份
 * 「刚改完、等后端回执」的乐观值(`settingsDraft`),回执到达即清 —— 不再有第二份真相。
 * 改动经 `chat:update-session-settings` 异步下发,**不必发消息**,也不需要打断当前循环:
 * 后端写的是内存活记录,工具策略每个工具调用、LLM 每轮都读它。
 * 前后端枚举换算集中在 stores/sessionSettings.ts(纯映射,配断言)。
 */

export type { SessionSettingsView } from './sessionSettings'

/** 会话忙时排队的待发消息(前端消息队列):priority 越高越先自动发送,拖拽调序为同级内的次序 */
export interface PendingQueueItem {
  id: string
  text: string
  priority: number
  opts: ChatSendOptions
  createdAt: number
}

/** 后端事件负载(react-loop 协议) */
interface BackendEvent {
  type?: string
  session_id?: string
  content?: string
  title?: string
  title_zh?: string
  title_en?: string
  message?: string
  tool_calls?: Array<{ id?: string; name?: string; params?: unknown }>
  id?: string
  name?: string
  result?: string
  duration_ms?: number
  round?: number
  /** 工具结果显式状态(引擎路径总是给出;审批重放等业务侧补发亦同) */
  status?: 'ok' | 'error' | 'pending'
  /** tool_result 事件:安全拦截分类(拒绝读/写/危险命令/网络/资源/浏览器 + 来源可信度) */
  denial?: ToolCallDenial | null
  /** tool_result 事件:沙箱运行事实(无防护 / 被驱逐 / 输出截断;仅 bash 产出) */
  sandbox?: ToolCallSandboxFacts | null
  /** tool_result 事件:写文件的结构化 diff(edit/write/append) */
  file_diff?: ToolFileDiff | null
  /** tool_result 事件:读文件的行/字符统计 */
  read_stats?: ToolReadStats | null
  /** tool_result 事件:网页搜索结果元数据 */
  web_results?: WebSearchResult[] | null
  /** tool_result 事件:记忆搜索结果元数据 */
  memory_results?: MemorySearchResult[] | null
  /** session_stats 事件:该轮 token 用量(挂到本轮结尾的无工具助手消息) */
  prompt_tokens?: number
  completion_tokens?: number
  total_tokens?: number
  /** interrupt 事件:发起中断的工具名 */
  tool_name?: string
  /** interrupt 事件:中断信封(审批请求 / 提问表单) */
  payload?: InterruptRequest | null
  /** plan_updated 事件:task_list 的变更动作(create/update_detail/update_nodes/delete) */
  action?: string
  /** plan_updated 事件:变更后的计划 id(删除时为被删计划) */
  task_id?: string
}

/** 前端参数 → 后端枚举映射(前端 plan|build|auto / ask|allow / thinking 0-10)。
 *  注意:后端 napi DTO 为 camelCase(serde rename_all),多词字段必须用
 *  reasoningEffort(snake 会静默丢弃,思考强度丢失);git 分支不进请求体。 */
function toBackendRequest(
  message: string,
  opts: ChatSendOptions,
  settings?: SessionSettingsView,
  sessionType?: SessionType,
  /** 创建后是否立即执行：`false` = 仅创建（后端落 idle + 首条消息进历史，不起循环） */
  autoRun?: boolean
): Record<string, unknown> {
  const req: Record<string, unknown> = {
    model: opts.model,
    workspace: opts.workspace,
    // 附件只下发路径列表：图片由后端复制进会话工作区并上送，其他文件后端只记录路径
    attachments: opts.attachments?.length ? opts.attachments.map((a) => a.path) : undefined,
    message
  }
  // 结构化引用:正文只保留「原文」(@文件→路径、$智能体→名称、元素→编号标签);
  // 机器可读标识由输入框 chip 收集后经 opts.mentions 下发,$技能等仍从文本派生补齐。
  // 后端据此生成「引用说明」,作为消息元数据落盘(不渲染),喂模型与冷启动重建时再变成正常 user 消息。
  const derived = deriveMentionRefs(message)
  const seenTokens = new Set<string>()
  const mentions = [...(opts.mentions ?? []), ...derived].filter((m) =>
    seenTokens.has(m.token) ? false : (seenTokens.add(m.token), true)
  )
  if (mentions.length) req.mentions = mentions
  if (opts.elements?.length) req.elements = opts.elements
  if (opts.officeRefs?.length) req.officeRefs = opts.officeRefs
  if (opts.quotes?.length) req.quotes = opts.quotes
  // 缺省不下发：后端 `None` = 执行，保持既有语义
  if (autoRun === false) req.autoRun = false
  // 会话级设置**只在创建会话时随首条消息落库**;之后由 update_session_settings 单独改写。
  // 发消息不再携带(后端「缺省 = 保持」):否则队列项/插件路径里捕获的旧值会把会话悄悄改回去。
  if (settings) {
    req.provider = settings.provider || undefined
    req.mode = MODE_TO_BACKEND[settings.mode]
    req.permission = PERMISSION_TO_BACKEND[settings.permission]
    req.reasoningEffort = effortOf(settings.thinkingLevel)
  }
  // 显式模式(仅 `/plan`):随本条消息原子下发,后端 send_message 先写会话记录再跑循环(无竞态)
  if (opts.mode) req.mode = MODE_TO_BACKEND[opts.mode]
  // 目标(仅 `/goal`):随本条消息原子设置目标循环
  if (opts.goal) {
    req.goal = opts.goal
    req.goalLang = opts.goalLang
  }
  // 会话类型(同样只在创建时声明):后端按它标记来源,侧栏分组与类型徽标据此渲染
  if (sessionType) req.sessionType = sessionType
  return req
}

/**
 * 前端会话 → 后端会话类型:已有后端类型直接沿用;本地新建(尚无后端记录)按 `kind` 推导
 * ——选了目录 = 项目会话,否则普通会话(定时任务/子会话只可能由后端创建,不会走这里)。
 */
function backendSessionTypeOf(session: Session): SessionType {
  return session.sessionType ?? (session.kind === 'project' ? 'project' : 'normal')
}

/**
 * 后端记录是否已经体现草稿里的**每一项**。
 *
 * 只在确认到自己的改动时才清草稿:会话每一轮都会广播 `session_info`,
 * 若一有推送就清,用户刚拨的那一档会被无关推送瞬间打回旧值(界面闪回)。
 */
function draftConfirmedBy(draft: Partial<SessionSettingsView>, info: SessionInfo): boolean {
  if (draft.mode && MODE_FROM_BACKEND[info.sessionMode ?? ''] !== draft.mode) return false
  if (draft.permission && PERMISSION_FROM_BACKEND[info.permissionMode ?? ''] !== draft.permission) {
    return false
  }
  if (draft.thinkingLevel !== undefined && effortOf(draft.thinkingLevel) !== (info.reasoningEffort ?? '')) {
    return false
  }
  if (draft.provider !== undefined && (info.provider ?? '') !== draft.provider) return false
  if (draft.model !== undefined && (info.model ?? '') !== draft.model) return false
  return true
}

function loadSessions(): Session[] {
  try {
    const raw = localStorage.getItem(SESSIONS_KEY)
    if (!raw) return []
    return JSON.parse(raw) as Session[]
  } catch {
    return []
  }
}

/* ---------- 置顶状态:独立于会话列表持久化(后端会话合并不会覆盖) ---------- */

const PINS_KEY = 'astorm:sessions:pins:v1'
const PROJECT_PINS_KEY = 'astorm:projects:pins:v1'

/** 会话置顶时间戳表:sessionId → pinnedAt(epoch ms);仅存被置顶的会话 */
function loadPins(): Record<string, number> {
  try {
    const raw = JSON.parse(localStorage.getItem(PINS_KEY) ?? '{}') as Record<string, unknown>
    const out: Record<string, number> = {}
    for (const [id, v] of Object.entries(raw)) {
      if (typeof v === 'number' && Number.isFinite(v)) out[id] = v
    }
    return out
  } catch {
    return {}
  }
}

/** 项目(工作区目录)置顶时间戳表:workspace 路径 → pinnedAt */
function loadProjectPins(): Record<string, number> {
  try {
    const raw = JSON.parse(localStorage.getItem(PROJECT_PINS_KEY) ?? '{}') as Record<string, unknown>
    const out: Record<string, number> = {}
    for (const [path, v] of Object.entries(raw)) {
      if (typeof v === 'number' && Number.isFinite(v)) out[path] = v
    }
    return out
  } catch {
    return {}
  }
}

/**
 * 持久化记录 → 渲染消息（`loadHistory` 与「加载更早」的归档前插共用同一映射）。
 * `records` 按 seq 升序消费；返回渲染列表与摘要边界 seq（该 seq 及之前的用户消息不可撤回）。
 * `onInterrupt` 用于恢复工具卡的中断标记（存储侧注入回调，避免本函数依赖 store 实例）。
 */
function buildHistoryMessages(
  records: BackendMessageRecord[],
  onInterrupt?: (tool: ToolCallView, marker: InterruptMarker | null | undefined) => void
): { msgs: ChatMessage[]; boundarySeq: number } {
  const msgs: ChatMessage[] = []
  // 摘要边界 seq:该 seq 及之前的用户消息不可撤回(缺省无摘要 → -Infinity,全部可撤回)
  let boundarySeq = Number.NEGATIVE_INFINITY
  for (const r of records.sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))) {
          const role = r.role
          if (role === 'system') continue
          if (r.metadata?.summary && typeof r.seq === 'number') {
            boundarySeq = Math.max(boundarySeq, r.seq)
          }
          const createdAt = (r.createdAt ?? 0) * 1000
          if (role === 'user') {
            const userMsg: ChatMessage = {
              id: r.id || uid(),
              role: 'user',
              content: r.content ?? '',
              createdAt,
              seq: r.seq,
              // 附件描述(后端落库在 metadata.attachments)→ 重载会话后仍显示缩略图 / 文件胶囊
              attachments: r.metadata?.attachments?.length
                ? r.metadata.attachments.map((a) => ({
                    path: a.path,
                    name: a.name || a.path.split(/[\\/]/).pop() || '附件',
                    size: a.size,
                    mime: a.mime
                  }))
                : undefined
            }
            msgs.push(userMsg)
          } else if (role === 'assistant') {
            const tools: ToolCallView[] = Array.isArray(r.toolCalls)
              ? r.toolCalls.map((tc) => {
                  const fc = (tc as { function?: { name?: string; arguments?: string } })?.function
                  let params: unknown
                  try {
                    params = JSON.parse(fc?.arguments || '{}')
                  } catch {
                    params = fc?.arguments
                  }
                  return {
                    id: (tc as { id?: string })?.id ?? '',
                    name: fc?.name ?? 'unknown',
                    params,
                    // 有结果才置完成:tool 记录到达时被覆盖(修复原先硬编码 completed 的假完成态)
                    status: 'pending' as const
                  }
                })
              : []
            const assistantMsg: ChatMessage = {
              id: r.id || uid(),
              role: 'assistant',
              // 后端已在落盘前剥离交付物标记块；这里再兜底一次（老记录 / 异常路径）
              content: stripArtifactMarkers(r.content ?? ''),
              reasoning: r.reasoningContent || undefined,
              createdAt,
              summary: r.metadata?.summary === true ? true : undefined,
              tools: tools.length ? tools : undefined,
              // 交付物 / 变更清单（后端 metadata.artifacts）→ 重载会话后仍在消息下方展示
              artifacts: r.metadata?.artifacts ?? undefined,
              // 后端 assistant 记录带 metadata.ended_at(unix 毫秒,见 react-loop/engine.rs):
              // 回填后对话区收纳块的历史耗时与实时口径一致。
              // 注意**被打断的那一轮 ended_at 会写 0**(stream 未走完),此时退回该轮的
              // started_at —— 仍是真实时间戳,耗时略小于实际但不会是 0 / 负数 / 缺失
              completedAt: pickRecordTime(r.metadata?.ended_at, r.metadata?.started_at),
              // 本轮处理起点同样以后端为准:收纳块耗时算「模型真的在跑的那段」,
              // 而不是「从写下这条指令算起」——看板待办被「启动」时两者能差几十小时
              startedAt: asRecordTime(r.metadata?.started_at)
            }
            msgs.push(assistantMsg)
          } else if (role === 'tool') {
            // 工具结果按 toolCallId 折叠进对应工具卡片;metadata.interrupt 恢复中断态
            // (pending → 重新弹审批卡;replaying/interrupted/denied → 失败态);
            // metadata.{status,denial} 恢复状态与安全拦截分类(重启后仍可见分类徽标)
            const tool = findToolById(msgs, r.toolCallId ?? undefined)
            if (tool) {
              tool.result = r.content ?? ''
              tool.denial = (r.metadata?.denial as ToolCallDenial | undefined) ?? null
              // 沙箱运行事实（无防护/被驱逐/输出截断）与分类徽标同源恢复
              tool.sandbox = (r.metadata?.sandbox as ToolCallSandboxFacts | undefined) ?? null
              // 冷启动恢复文件级结构化元数据（与实时事件同口径）
              tool.fileDiff = (r.metadata?.file_diff as ToolFileDiff | undefined) ?? null
              tool.readStats = (r.metadata?.read_stats as ToolReadStats | undefined) ?? null
              tool.webResults = (r.metadata?.web_results as WebSearchResult[] | undefined) ?? null
              tool.memoryResults = (r.metadata?.memory_results as MemorySearchResult[] | undefined) ?? null
              if (r.metadata?.status === 'error') tool.status = 'error'
              else if (r.metadata?.status === 'ok') tool.status = 'completed'
              if (r.metadata?.started_at && r.metadata?.ended_at) {
                tool.duration = r.metadata.ended_at - r.metadata.started_at
              }
              onInterrupt?.(tool, r.metadata?.interrupt)
            }
          }
  }
  return { msgs, boundarySeq }
}

export const useSessionsStore = defineStore('sessions', {
  state: () => ({
    sessions: [] as Session[],
    activeId: '',
    /** 每个会话的消息列表(内存态,不持久化) */
    messages: {} as Record<string, ChatMessage[]>,
    /** 正在流式输出的会话 id 列表(多会话可并发;会话列表显示脉动绿点) */
    streamingIds: [] as string[],
    /** 已删除会话的 backendId 墓碑(backendId → 删除时刻 ms;防后端列表回填;TTL 见 loadDeletedIds) */
    deletedBackendIds: loadDeletedIds(),
    /** 会话忙时的待发消息队列(按会话;done 后自动发送最高优先级的一条;localStorage 持久化) */
    pendingQueues: loadPendingQueues(),
    /** 自动派发暂停(按会话):立即发送 / 取消后置起,期间 done 不自动取队列发送;
     *  用户再次主动发送消息时恢复 */
    drainPaused: {} as Record<string, boolean>,
    /** 会话历史是否已从后端加载(内存态,防重复回灌) */
    historyLoaded: {} as Record<string, boolean>,
    /** 待重载会话消息(摘要压缩历史后置起;流式中推迟到本轮收尾再重载) */
    pendingHistoryReload: {} as Record<string, boolean>,
    /** 会话摘要边界 seq(按本地会话 id):该 seq 及之前的用户消息不可撤回(-Infinity/缺省 = 无摘要) */
    rewindBoundary: {} as Record<string, number>,
    /**
     * 流式推进信号(按会话自增):正文/思考合帧落库与轮次级事件后 +1。
     * 面板据此自动滚动——替代对 messages 的 deep 监听(那会把整棵消息对象图每帧遍历一遍)。
     */
    streamTick: {} as Record<string, number>,
    /**
     * 流式**相位**(key=本地会话 id):驱动过程末尾的状态行流光文案——
     * awaiting=已发出、等模型首个输出;thinking=思考链流式;tool=工具已调用、等结果。
     * 仅终态无关的进行中信号;正文流式 / 中断等待 / 收尾都会清空(undefined)。
     */
    runPhase: {} as Record<string, 'awaiting' | 'thinking' | 'tool' | 'summarizing' | undefined>,
    /** 会话信息(key=本地会话 id;后端每次持久化推送 session_info 事件实时更新) */
    sessionInfo: {} as Record<string, SessionInfo>,
    /**
     * 会话设置的**乐观值**(key=本地会话 id):用户刚改完、后端回执(session_info)未到的这段时间用它。
     * 回执到达即清 —— 真相在后端会话记录里,这里只是「发出去还没被确认」的过渡态。
     * 还没有 backendId 的新会话也落在这里:首次 create_session 会把整套设置带上。
     */
    settingsDraft: {} as Record<string, Partial<SessionSettingsView>>,
    /** 会话状态位(侧栏任务树图标):formWaiting=待答表单;end=done 正常结束 / error 异常 */
    taskFlags: {} as Record<
      string,
      {
        formWaiting?: boolean
        /** 待审批（安全拦截）：侧栏同样显示「等你处理」图标 */
        awaitingApproval?: boolean
        /** 待确认发送邮件（mail 工具）：侧栏同样显示「等你处理」图标 */
        mailComposeWaiting?: boolean
        /** 终态（互斥）：ignored = 用户忽略了提问（灰点，可经「继续填写表单」恢复） */
        end?: 'done' | 'error' | 'cancelled' | 'interrupted' | 'ignored'
      }
    >,
    /** 待审批(会话 key=本地会话 id):工具被安全策略拦截等待用户同意/拒绝 */
    /** 各会话当前待决中断(审批卡 / 提问表单;权威来源是后端 metadata.interrupt 与 interrupt 事件) */
    pendingInterrupts: {} as Record<string, PendingInterrupt | null>,
    /** 会话最新异常提示(聊天框上方错误条):仅顶层 error 事件写入(工具失败不写,见 tool_result 分支),done 正常事件清除 */
    sessionError: {} as Record<string, string>,
    /** 会话瞬时提示(信息级,同一位置显示):目前只有「流中断,正在自动重试」 */
    sessionNotice: {} as Record<string, string>,
    /** 任务计划变更版本号(会话 key=本地会话 id):后端 task_list 变更事件自增,驱动规划胶囊即时刷新 */
    planVersion: {} as Record<string, number>,
    /**
     * 交付物表行(会话 key=本地会话 id,按轮次升序):`submit_deliverables` 工具逐轮落库,
     * 对话结束后经 `session_deliverables` 命令拉取;ChatPanel 按轮次 join 到对话轮次渲染。
     */
    deliverablesBySession: {} as Record<string, DeliverableRow[]>,
    /** 会话置顶:sessionId → pinnedAt(独立持久化;置顶组按时间倒序) */
    pins: loadPins(),
    /** 项目(工作区目录)置顶:workspace 路径 → pinnedAt */
    pinnedProjects: loadProjectPins()
  }),
  getters: {
    tasks: (state) => state.sessions.filter((s) => s.kind === 'task'),
    projects: (state) => state.sessions.filter((s) => s.kind === 'project'),
    currentMessages: (state): ChatMessage[] => state.messages[state.activeId] ?? [],
    isStreamingSession: (state) => (id: string): boolean => state.streamingIds.includes(id),
    /** 按 tool_call id 取工具卡（写邮件确认卡据此从原 tool 调用参数还原草稿） */
    toolById:
      (state) =>
      (sessionId: string, toolCallId: string | undefined): ToolCallView | undefined =>
        findToolById(state.messages[sessionId] ?? [], toolCallId),
    /**
     * 会话状态的**细粒度 key**（= `session.status.*` 的 i18n key；null = 无标记）。
     * 悬停说明 / 看板卡片的状态行用它（能区分「等待你的回答」与「等待你的审批」）。
     */
    statusLabelKeyOf:
      (state) =>
      (id: string | undefined): SessionStatusLabelKey =>
        sessionStatusLabelKey(state, id),
    /**
     * 会话状态点/看板列的**统一判定链**（侧栏任务行 / 子会话列表 / 任务看板共用，避免多处
     * 各写一遍后语义漂移）。优先级：流式 > 等你处理(待审批·待答表单) > 进程中断 > 已忽略提问 >
     * 用户打断·执行失败 > 正常结束 > 无标记。
     *
     * 「运行中」同时看进程内流式状态（0 延迟）与后端状态（后台跑着的定时任务会话也算）；
     * 「等你处理」必须先于终态判定：中断待答的会话同时带着 done 事件写下的 `end`，
     * 先判终态会把它误显示成「已完成」。
     */
    statusKeyOf:
      (state) =>
      (id: string | undefined): SessionStatusKey => {
        const key = sessionStatusLabelKey(state, id)
        if (key === 'awaitingForm' || key === 'awaitingApproval' || key === 'awaitingMail') return 'form'
        return key
      },
    /**
     * 任务看板的列归属（四列：todo 待办 / pending 待处理 / running 进行中 / done 完成）。
     *
     * 语义（2024 调整）：
     * - **待办**只放「新建、从未执行」的会话（无状态标记 / 后端 `idle`）—— 看板「新建任务」
     *   的「仅创建」就落在这里；
     * - **待处理**收所有「停在那里等你处置」的：待审批 / 待填表 / 进程中断 / 已忽略提问 /
     *   执行失败 / 用户取消。卡片上靠状态点颜色与状态文案区分（见 statusKeyOf），
     *   前端筛选、状态点、列归属仍是同一条判定链；
     * - 具体用了哪条分支不在这里判断，交给 `statusKeyOf`（它把 awaitingForm/awaitingApproval
     *   折叠成 `form`，其余原样返回）。
     */
    boardColumnOf:
      (state) =>
      (id: string): BoardColumnKey => {
        const key = useSessionsStore().statusKeyOf(id)
        if (key === 'running') return 'running'
        if (key === 'done') return 'done'
        // 无标记 = 新会话 / 从未执行（后端 idle）→ 待办
        if (!key) return 'todo'
        // form / interrupted / ignored / error / cancelled → 待处理
        return 'pending'
      },
    /**
     * 会话设置(唯一读路径):本地草稿(刚改、未回执) → 后端会话记录 → 新建会话默认(chatPrefs)。
     *
     * 思考强度特例:后端只存五档,若后端档位与本地数值同档则保留本地数值,
     * 否则滑杆会在回执到达时自己跳档(用户拨 5 → 后端存 medium → 显示 6)。
     */
    settingsOf:
      (state) =>
      (id: string): SessionSettingsView => {
        const draft = state.settingsDraft[id] ?? {}
        const info = state.sessionInfo[id] ?? {}
        const session = state.sessions.find((s) => s.id === id)
        // 兜底:还没有任何会话记录时(新建会话页 / 后端未就绪)沿用输入框草稿
        const prefs = useChatPrefsStore()
        const localLevel = draft.thinkingLevel
        const backendEffort = info.reasoningEffort
        const thinkingLevel =
          localLevel !== undefined && (!backendEffort || effortOf(localLevel) === backendEffort)
            ? localLevel
            : ((backendEffort ? EFFORT_LEVEL[backendEffort] : undefined) ??
              localLevel ??
              prefs.thinkingLevel)
        return {
          mode: draft.mode ?? MODE_FROM_BACKEND[info.sessionMode ?? ''] ?? prefs.mode,
          permission:
            draft.permission ?? PERMISSION_FROM_BACKEND[info.permissionMode ?? ''] ?? prefs.permission,
          thinkingLevel,
          provider: draft.provider ?? info.provider ?? prefs.selectedPath[0] ?? '',
          model: draft.model ?? session?.model ?? info.model ?? prefs.selectedPath[1] ?? ''
        }
      }
  },
  actions: {
    init(): void {
      if (this.sessions.length) return
      const saved = loadSessions()
      if (saved.length) {
        this.sessions = saved
      }
      // 无本地会话时保持空(不填充 Mock 演示种子,避免「空内容却列出任务/项目」;
      // 真实会话由后端 bootstrap 提供,空态引导用户新建即可)
      this.activeId = this.sessions[0]?.id ?? ''
    },
    /**
     * 由事件**登记一个后端已存在、前端尚未持有的会话**（子会话 / 定时任务会话）。
     *
     * 为什么需要：会话列表的唯一真源是本地 `sessions`，只在启动时整体拉取一次；
     * 后端 `sub_session` 工具新建子会话时只会广播事件。若此处不登记，那条
     * `session_info`（以及紧随的全部流式事件）会被 `initChatStream` 直接丢弃——
     * 表现为「子会话在后端跑着，但左栏没有、也无法打开或审批」。
     *
     * 只认 `session_info`（payload 是完整 SessionRecord，含 pid / agentType / 时间戳）；
     * 其余事件类型对未知 id 无从补全记录，仍忽略。
     *
     * 不抢焦点（不改 `activeId`）：子会话出现不应该把用户正在看的会话切走。
     */
    registerSessionFromEvent(ev: BackendEvent): Session | null {
      if (ev.type !== 'session_info') return null
      const id = typeof ev.session_id === 'string' ? ev.session_id : ''
      if (!id) return null
      // 用户已删除（墓碑，TTL 内）→ 不复活
      if (id in this.deletedBackendIds) return null
      const info = ev as unknown as SessionInfo
      const created = typeof info.createdAt === 'number' ? info.createdAt * 1000 : Date.now()
      const updated = typeof info.updatedAt === 'number' ? info.updatedAt * 1000 : created
      const pid = typeof info.pid === 'string' && info.pid ? info.pid : null
      const sessionType = readSessionType(info.sessionType)
      // 子会话跟随父会话的分类（父查不到时才按自身类型/工作区兜底），保证与重启回填同规则
      const parent = pid ? this.sessions.find((x) => x.backendId === pid) : undefined
      const session: Session = {
        id,
        title: typeof info.title === 'string' && info.title ? info.title : i18n.global.t('common.unnamed'),
        titleZh: typeof info.titleZh === 'string' ? info.titleZh : undefined,
        titleEn: typeof info.titleEn === 'string' ? info.titleEn : undefined,
        createdAt: created,
        updatedAt: updated,
        kind: parent?.kind ?? deriveKind(sessionType, info.workspace),
        sessionType,
        scheduledTaskId: typeof info.scheduledTaskId === 'string' ? info.scheduledTaskId : null,
        workspace: typeof info.workspace === 'string' ? info.workspace : undefined,
        model: typeof info.model === 'string' ? info.model : undefined,
        backendId: id,
        pid,
        agentType: typeof info.agentType === 'string' ? info.agentType : null
      }
      this.sessions.unshift(session)
      this.sessionInfo[id] = info
      this.persist()
      return session
    },
    /**
     * 某定时任务产生的会话（按 `scheduledTaskId` 归集），按创建时间**倒序**（最新在前）。
     *
     * 定时任务会话不在侧栏显示，只在「定时任务」页的任务卡片下按时间列出执行记录；
     * 数据来源就是本 store（启动全量拉取 + `session_info` 事件实时登记），无需额外接口。
     */
    sessionsOfTask(taskId?: string | null): Session[] {
      if (!taskId) return []
      return this.sessions
        .filter((s) => s.scheduledTaskId === taskId)
        .sort((a, b) => b.createdAt - a.createdAt)
    },

    /**
     * 某会话（按**后端 id**）派生出的子会话：`pid` 指向它，按创建时间升序。
     *
     * 子会话由后端 `sub_session` 工具创建（一次委派可并发多个），不在左栏列表里，
     * 只在对话区右上角规划胶囊的展开面板中列出（见 `components/chat/ChildSessionsList.vue`）。
     */
    childSessionsOf(parentBackendId?: string | null): Session[] {
      if (!parentBackendId) return []
      return this.sessions
        .filter((s) => !!s.pid && s.pid === parentBackendId)
        .sort((a, b) => a.createdAt - b.createdAt)
    },
    /**
     * 有 Rust 后端时以真实会话列表替换 mock 种子(sidebar 数据真实化)。
     * 后端无消息历史读取接口,会话打开后从空开始,新消息流式填充;
     * 拉取为空/失败时保留 mock 种子。
     *
     * `opts.keepActive`：保留当前激活会话（任务看板的「刷新」用）——默认行为会把 activeId
     * 重置成列表首条（启动时无所谓，运行中再调就会把聊天页/侧栏选中项挪到别的会话上）。
     */
    /**
     * 丢帧后的兜底补拉。后端事件总线积压（lag）时会推一条 `{type:'error', session_id:null}`
     * 告警帧，那一窗的事件已经丢了 —— 会话状态、新建会话都可能对不上。
     * 节流成「每 5s 至多一次」全量重拉：积压时后端可能连推多条告警帧，逐条重拉会自伤。
     */
    scheduleCatchUp(): void {
      if (catchUpTimer) return
      const wait = Math.max(0, CATCH_UP_MIN_INTERVAL_MS - (Date.now() - catchUpAt))
      catchUpTimer = setTimeout(() => {
        catchUpTimer = null
        catchUpAt = Date.now()
        console.warn('[sessions] 事件总线积压丢帧，补拉一次会话列表兜底')
        void this.bootstrapFromBackend({ keepActive: true })
      }, wait)
    },
    async bootstrapFromBackend(opts?: { keepActive?: boolean }): Promise<void> {
      const chat = window.api.chat
      if (!chat?.listSessions) return
      try {
        const records = (await chat.listSessions()) as Array<{
          id?: unknown
          title?: unknown
          titleZh?: unknown
          titleEn?: unknown
          workspace?: unknown
          model?: unknown
          createdAt?: unknown
          updatedAt?: unknown
          pid?: unknown
          agentType?: unknown
          sessionType?: unknown
          scheduledTaskId?: unknown
        }>
        // 后端无会话时保留本地;有则过滤掉已删除(墓碑)记录后替换
        if (!Array.isArray(records) || !records.length) return
        const loaded: Session[] = records
          .map((r): Session | null => {
            const id = typeof r.id === 'string' && r.id ? r.id : ''
            if (!id || id in this.deletedBackendIds) return null
            // 后端时间戳为 unix 秒,前端展示用毫秒(×1000)
            const created = typeof r.createdAt === 'number' ? r.createdAt * 1000 : Date.now()
            const sessionType = readSessionType(r.sessionType)
            return {
              id,
              title: typeof r.title === 'string' && r.title ? r.title : '未命名',
              titleZh: typeof r.titleZh === 'string' ? r.titleZh : undefined,
              titleEn: typeof r.titleEn === 'string' ? r.titleEn : undefined,
              createdAt: created,
              // 会话更新时间(每轮活动后端 updated_at 都会刷新)→ 侧栏按此展示,重启不回退
              updatedAt: typeof r.updatedAt === 'number' ? r.updatedAt * 1000 : created,
              kind: deriveKind(sessionType, r.workspace),
              sessionType,
              scheduledTaskId: typeof r.scheduledTaskId === 'string' ? r.scheduledTaskId : null,
              workspace: typeof r.workspace === 'string' ? r.workspace : undefined,
              model: typeof r.model === 'string' ? r.model : undefined,
              backendId: id,
              pid: typeof r.pid === 'string' && r.pid ? r.pid : null,
              agentType: typeof r.agentType === 'string' ? r.agentType : null
            }
          })
          .filter((s): s is Session => s !== null)
        if (!loaded.length) return
        // 子会话分类**跟随父会话**（与实时登记 registerSessionFromEvent 同规则）：
        // 子会话工作区是绝对的 `父工作区/.astorm/subagent_<id>`，按自身类型/工作区判会归入「项目」，
        // 与父会话分居两个分组——这里按 pid 找到父记录后用父的分类覆盖。
        const kindById = new Map(loaded.map((s) => [s.id, s.kind]))
        for (const child of loaded) {
          if (child.pid && kindById.has(child.pid)) child.kind = kindById.get(child.pid)!
        }
        // 富会话信息回填:列表记录含完整 SessionRecord(camelCase),会话信息卡片初始即展示
        for (const s of loaded) {
          const rec = records.find((r) => r.id === s.id)
          if (rec) this.sessionInfo[s.id] = rec as unknown as SessionInfo
          // 后端记录已到 → 丢弃同名乐观值(避免草稿盖住权威设置)
          delete this.settingsDraft[s.id]
          // 冷启动恢复侧栏状态图标：唯一真相源是后端会话状态（进程内已重置为「无」）
          this.applyBackendStatus(s.id, (rec as unknown as SessionInfo | undefined)?.status)
        }
        this.sessions = loaded
        // 只清理「后端已经不存在」的会话的内存态，**不要**整体清空 `messages`：
        // 清空会让看板卡片的正文预览当场退回状态文案，更糟的是 `historyLoaded` 没跟着复位，
        // `loadHistory` 见标记为真直接 return —— 打开过的会话聊天页会变成空白。
        const alive = new Set(loaded.map((s) => s.id))
        for (const id of Object.keys(this.messages)) if (!alive.has(id)) delete this.messages[id]
        for (const id of Object.keys(this.historyLoaded)) if (!alive.has(id)) delete this.historyLoaded[id]
        if (!opts?.keepActive) this.activeId = loaded[0]?.id ?? ''
      } catch (err) {
        console.error('[sessions] backend list failed, keep local:', err)
      }
    },
    /**
     * 「仅创建」：建一条**后端已登记**的会话（`autoRun: false` → 后端落 `idle`、生成标题、
     * 把首条消息写进历史，但不起 ReAct 循环），看板「新建任务」弹窗的次要按钮走这里。
     *
     * 与本地草稿（`createSession`）的区别：这里立刻拿到 `backendId`，因此不会被随后的
     * `bootstrapFromBackend` 当成"列表里没有的本地会话"丢掉；后端随后推 `title` 事件，
     * 卡片标题会从下面这个截断占位替换成 LLM 生成的标题。
     *
     * **不动 `activeId`**：调用方（看板）创建完仍留在原地，退出看板时不该莫名跳到这条新任务。
     * 失败（无后端 / IPC 报错）时回滚本地会话并提示，不留一条永远没后端记录的幽灵卡片。
     */
    async createParkedSession(payload: {
      /** 首条消息（指令）：后端用它生成标题，并写进会话历史 */
      text: string
      kind: Session['kind']
      workspace?: string
      attachments?: MessageAttachment[]
      settings: SessionSettingsView
    }): Promise<string | null> {
      const t = i18n.global.t
      const localId = uid()
      // 占位标题：后端标题回来前先显示指令开头（30 字与后端标题截断长度一致）
      const localTitle = payload.text.trim().slice(0, 30) || t('sidebar.newTaskTitle')
      const session: Session = {
        id: localId,
        title: localTitle,
        createdAt: Date.now(),
        updatedAt: Date.now(),
        kind: payload.kind,
        workspace: payload.workspace || todayKey(),
        sessionType: payload.kind === 'project' ? 'project' : 'normal'
      }
      this.sessions.unshift(session)
      this.persist()
      try {
        const chat = window.api?.chat
        if (!chat?.createSession) throw new Error('后端未就绪')
        const backendId = (await chat.createSession(
          toBackendRequest(
            payload.text,
            {
              workspace: payload.workspace,
              attachments: payload.attachments,
              model: payload.settings.model,
              provider: payload.settings.provider
            },
            payload.settings,
            session.sessionType,
            false
          )
        )) as string
        session.backendId = backendId
        session.updatedAt = Date.now()
        this.sessionInfo[localId] = { status: 'idle' } as unknown as SessionInfo
        this.persist()
        return localId
      } catch (err) {
        // 回滚：本地列表里不留一条没有后端记录的会话
        const idx = this.sessions.findIndex((s) => s.id === localId)
        if (idx >= 0) this.sessions.splice(idx, 1)
        this.persist()
        Message.error(
          t('board.newTaskCreateFailed', { reason: err instanceof Error ? err.message : String(err) })
        )
        return null
      }
    },
    /**
     * 「启动」：把「仅创建」（`createParkedSession`）落下、尚未执行的指令跑起来。
     *
     * 与 `sendMessage` 的关键区别：**不追加新的用户消息** —— 指令在创建时就已由后端写进
     * 会话历史（`autoRun: false` 那条路径），这里只请后端以已有历史驱动一轮循环
     * （napi `startSession`）。所以历史里不会留下第二条同样的指令，模型也不会看到两遍。
     *
     * 乐观 UI：先在本地挂一个流式助手气泡（后端创建的会话没人建占位，同 `rotateRound`），
     * 卡片与侧栏立刻进「进行中」；后端拒绝（历史为空 / 末尾不是用户消息 / 已在运行）
     * 时把气泡标失败并提示。
     * **不动 `activeId`**：在看板上点启动，视图留在看板（与 `createParkedSession` 一致）。
     */
    async startParkedSession(sessionId: string): Promise<void> {
      const t = i18n.global.t
      const session = this.sessions.find((s) => s.id === sessionId)
      const chat = window.api?.chat
      const fail = (reason: string): void => {
        Message.error(t('board.startFailed', { reason }))
        this.failAssistant(sessionId, reason)
      }
      // 已在启动/运行中：忽略重复点击（乐观置位后再点一次会撞后端「会话正在运行」）
      if (this.streamingIds.includes(sessionId)) return
      if (!session?.backendId || !chat?.startSession) {
        fail('后端未就绪')
        return
      }
      // 重新发起 → 上一轮的失败条/提示立即失效（与 sendNow 同款）
      this.clearTurnAlerts(sessionId)
      const list = this.messages[sessionId] ?? (this.messages[sessionId] = [])
      const assistant: ChatMessage = {
        id: uid(),
        role: 'assistant',
        content: '',
        createdAt: Date.now(),
        // 处理起点 = 此刻（点「启动」的时间），不是那条指令当初被创建的时间；
        // 否则收纳块会从「建待办」开始算，显示成几十小时
        startedAt: Date.now(),
        streaming: true
      }
      list.push(assistant)
      this.startStreaming(sessionId)
      roundBySession[sessionId] = 0
      this.bumpStreamTick(sessionId)
      try {
        await chat.startSession(session.backendId)
      } catch (err) {
        // failAssistant 内含 stopStreaming + 收尾，失败气泡就地落在刚挂的那条上
        fail(err instanceof Error ? err.message : String(err))
      }
    },
    createSession(kind: Session['kind'] = 'task', extra: Partial<Session> = {}): string {
      const t = i18n.global.t
      const count = this.sessions.filter((s) => s.kind === kind).length + 1
      const title = `${kind === 'project' ? t('sidebar.newProjectTitle') : t('sidebar.newTaskTitle')} ${count}`
      // 未指定目录时工作区默认按当天日期分组(与后端一致)
      const session: Session = {
        id: uid(),
        title,
        createdAt: Date.now(),
        updatedAt: Date.now(),
        kind,
        ...extra,
        workspace: extra.workspace ?? todayKey()
      }
      this.sessions.unshift(session)
      this.activeId = session.id
      this.persist()
      return session.id
    },
    /**
     * 删除会话:先通知后端彻底删除(sqlite 元数据 + jsonl 会话目录,重启不回填),
     * 再从本地列表与消息内存移除;后端不可用(如 Mock 模式)时以墓碑标记防回填。
     * 返回是否删除成功(找不到返回 false)。
     */
    async deleteSession(id: string): Promise<boolean> {
      const idx = this.sessions.findIndex((s) => s.id === id)
      if (idx === -1) return false
      const s = this.sessions[idx]
      // 后端真实删除:无论成败都继续本地清理,失败仅告警(开发 Mock 模式无后端)
      if (s.backendId) {
        const chat = window.api.chat
        if (chat?.deleteSession) {
          try {
            await chat.deleteSession(s.backendId)
          } catch (err) {
            console.error('[sessions] backend delete failed, keep tombstone:', err)
          }
        }
      }
      this.sessions.splice(idx, 1)
      delete this.messages[id]
      delete this.historyLoaded[id]
      delete this.settingsDraft[id]
      // 状态标志一并清理（侧栏图标 / 错误条 / 待决卡 / 流式转圈残留）
      delete this.taskFlags[id]
      delete this.sessionError[id]
      delete this.pendingInterrupts[id]
      delete this.deliverablesBySession[id]
      const streamIdx = this.streamingIds.indexOf(id)
      if (streamIdx >= 0) this.streamingIds.splice(streamIdx, 1)
      // 一并清理该会话的消息队列(持久化同步)
      if (this.pendingQueues[id]) {
        delete this.pendingQueues[id]
        savePendingQueues(this.pendingQueues)
      }
      if (s.backendId) {
        this.deletedBackendIds[s.backendId] = Date.now()
        saveDeletedIds(this.deletedBackendIds)
      }
      if (this.activeId === id) {
        this.activeId = this.sessions[0]?.id ?? ''
      }
      // 清理该会话遗留的页签快照/持久化与插件会话状态(避免孤儿数据)
      useTabsStore().clearSession(id)
      usePluginSessionStore().clearSession(id)
      // 清理置顶:会话置顶移除;若该工作区已无其他会话,项目置顶一并移除
      delete this.pins[id]
      if (s.workspace && !this.sessions.some((x) => x.workspace === s.workspace)) {
        delete this.pinnedProjects[s.workspace]
      }
      try {
        localStorage.setItem(PINS_KEY, JSON.stringify(this.pins))
        localStorage.setItem(PROJECT_PINS_KEY, JSON.stringify(this.pinnedProjects))
      } catch {
        /* 忽略 */
      }
      this.persist()
      return true
    },
    setActive(id: string): void {
      this.activeId = id
      // 打开会话时回灌历史对话记录(幂等;流式/已有消息时跳过)
      void this.loadHistory(id)
    },
    /** 会话置顶/取消置顶(置顶组按 pinnedAt 倒序;独立持久化) */
    togglePinSession(id: string): void {
      if (!id) return
      if (this.pins[id]) delete this.pins[id]
      else this.pins[id] = Date.now()
      try {
        localStorage.setItem(PINS_KEY, JSON.stringify(this.pins))
      } catch {
        /* 忽略 */
      }
    },
    /** 项目(工作区目录)置顶/取消置顶 */
    togglePinProject(workspace: string): void {
      if (!workspace) return
      if (this.pinnedProjects[workspace]) delete this.pinnedProjects[workspace]
      else this.pinnedProjects[workspace] = Date.now()
      try {
        localStorage.setItem(PROJECT_PINS_KEY, JSON.stringify(this.pinnedProjects))
      } catch {
        /* 忽略 */
      }
    },
    /**
     * 从后端读取会话历史(jsonl)并重建消息列表。
     * 参考 astorm-desktop:assistant 消息独立保存 reasoning_content / tool_calls,
     * 工具结果经 tool_call_id 关联折叠进对应工具卡片;system 消息跳过。
     *
     * 两个注意点（后端创建的会话——定时任务 / 子会话——全靠这里补齐历史）：
     * - **不再因为「本地已有消息」就跳过**：后台运行的会话只会经事件累积本轮内容，
     *   若因此不读 jsonl，打开它就只能看到「后面追加的那一段」（历史全丢）；
     * - **正在输出时推迟**：立刻重建会打断正在渲染的这条（本轮消息也尚未全部落盘），
     *   记入 `pendingHistoryReload`，由 `finalizeStream` / `failAssistant` 在本轮收尾时补一次。
     */
    async loadHistory(sessionId: string): Promise<void> {
      const chat = window.api.chat
      const session = this.sessions.find((x) => x.id === sessionId)
      if (!chat?.listMessages || !session?.backendId) return
      if (this.historyLoaded[sessionId]) return
      // 「正在跑」有两种情形，都推迟到本轮收尾再重建：
      // 1) 本客户端正在流式（`streamingIds`）；
      // 2) **后端创建且正在跑的会话**（定时任务 / 子会话）——它不在 streamingIds 里，
      //    但本地已由轮次事件累积出一个 streaming 气泡，存在即说明本轮还没结束。
      // 立刻重建会把这条正在渲染的气泡丢掉（本轮消息也尚未落盘）。
      const inFlight = (this.messages[sessionId] ?? []).some((m) => m.streaming)
      if (this.streamingIds.includes(sessionId) || inFlight) {
        this.pendingHistoryReload[sessionId] = true
        return
      }
      try {
        const records = (await chat.listMessages(session.backendId)) as BackendMessageRecord[]
        // 记录 → 渲染消息（与「加载更早」的归档前插共用同一映射）
        const { msgs, boundarySeq } = buildHistoryMessages(records, (tool, marker) =>
          this.applyInterruptMarker(sessionId, tool, marker)
        )
        // 请求期间本会话又开跑了（立即发送 / 定时任务触发）→ 不覆盖本地气泡，改为收尾时补
        if (
          this.streamingIds.includes(sessionId) ||
          (this.messages[sessionId] ?? []).some((m) => m.streaming)
        ) {
          this.pendingHistoryReload[sessionId] = true
          return
        }
        this.messages[sessionId] = msgs
        this.rewindBoundary[sessionId] = boundarySeq
        this.historyLoaded[sessionId] = true
        // 历史装载完成 → 面板滚到底部（原先靠 messages 深监听，现已改为浅信号）
        this.bumpStreamTick(sessionId)
        // 交付物 / 变更清单里的增删统计是**生成当轮**按当时后端 + 当时 git 状态固化的。
        // 旧构建留下的「全 0」统计（如非 Git 工作区被记为 0/0）在这里按**当前**情况重算：
        // 仓库内取 git 摘要，否则直接数文件行数——保证历史消息也能显示 +行数 / 新增。
        void this.refreshArtifactStats(sessionId)
        // 交付物表行（submit_deliverables 逐轮落库）→ 历史轮次卡片按表数据渲染
        void this.fetchDeliverables(sessionId)
      } catch (err) {
        console.error('[sessions] load history failed:', err)
      }
    },
    /**
     * 「加载更早」—从**摘要备份段**按 seq 游标取更早一批原始消息（默认 5 个用户轮次）并前插。
     *
     * 游标 = 当前消息列表的最小 seq：归档记录 seq 全部小于活动日志/回填段，前插后顺序天然正确、
     * 且不会与已显示内容重叠。返回本次新增的用户轮次数（0 = 已到归档起点，按钮应收起）。
     */
    async loadEarlierArchived(sessionId: string, chunkTurns = 5): Promise<number> {
      const chat = window.api.chat
      const session = this.sessions.find((x) => x.id === sessionId)
      if (!chat?.archivedBefore || !session?.backendId) return 0
      const list = this.messages[sessionId] ?? []
      let minSeq = Number.POSITIVE_INFINITY
      for (const m of list) {
        if (typeof m.seq === 'number' && m.seq < minSeq) minSeq = m.seq
      }
      if (!Number.isFinite(minSeq)) return 0
      try {
        const records = (await chat.archivedBefore(
          session.backendId,
          minSeq,
          chunkTurns
        )) as BackendMessageRecord[]
        if (!records.length) return 0
        const { msgs } = buildHistoryMessages(records)
        // 请求期间会话开跑/切换 → 放弃前插（用户已不在这个视图里翻旧账）
        if (!this.messages[sessionId]) return 0
        this.messages[sessionId] = [...msgs, ...this.messages[sessionId]]
        this.bumpStreamTick(sessionId)
        return records.filter((r) => r.role === 'user').length
      } catch (err) {
        console.error('[sessions] load earlier archived failed:', err)
        return 0
      }
    },
    /**
     * 重算历史消息里「全 0」的变更统计：仓库内取当前 git 摘要，非仓库直接数文件行数。
     *
     * 为什么前端也做一遍：`metadata.artifacts` 是**生成当轮**固化的快照——旧构建在非 Git
     * 工作区把新文件写成 `status:"M", 0/0`，重编后端后旧消息仍是旧值。这里只对**统计缺失**
     * （`additions`/`deletions` 均为 0）的条目补值，不覆盖已有真实统计；补完即稳定，不重复刷。
     */
    async refreshArtifactStats(sessionId: string): Promise<void> {
      const list = this.messages[sessionId]
      if (!list?.length) return
      const targets: ArtifactChange[] = []
      for (const m of list) {
        if (m.role !== 'assistant') continue
        for (const c of m.artifacts?.changes ?? []) {
          if (!c.additions && !c.deletions) targets.push(c)
        }
      }
      if (!targets.length) return
      const session = this.sessions.find((s) => s.id === sessionId)
      const dir = await resolveSessionWorkspaceDir(session?.workspace)
      if (!dir) return
      // 仓库内 → 拉当前差异摘要（含未跟踪）；非仓库 / 失败 → 留空，逐文件数行数兜底
      let summary: Array<{ path: string; status: string; additions: number; deletions: number }> = []
      try {
        if (await window.api.git?.isRepo?.(dir)) {
          summary = (await invoke('git_diff_summary', {
            dir,
            staged: false
          })) as typeof summary
        }
      } catch {
        summary = []
      }
      const norm = (p: string): string => p.replace(/\\/g, '/').replace(/^\.\//, '')
      for (const c of targets) {
        const rel = norm(c.path)
        const hit =
          summary.find((e) => norm(e.path) === rel) ??
          summary.find((e) => norm(e.path).endsWith(`/${rel}`))
        if (hit) {
          c.status = hit.status
          c.additions = hit.additions
          c.deletions = hit.deletions
          continue
        }
        // 非仓库 / 摘要里没有：文件行数当新增行数（新增文件也要显示 +N）。单文件失败不影响其余
        try {
          const content = (await invoke('read_file_content', {
            path: joinWorkspacePath(dir, rel)
          })) as string
          c.status = '?'
          c.additions = countTextLines(content ?? '')
          c.deletions = 0
        } catch {
          /* 文件缺失 / 不可读：保留原值 */
        }
      }
      this.bumpStreamTick(sessionId)
    },
    /**
     * 会话摘要压缩历史后请求重载消息：后端把「系统提示词 + 摘要 + 保留消息」写回活动日志，
     * 旧对话不再属于会话历史（前端也不回读）——已渲染列表随之过期。
     *
     * 流式中（摘要发生在 loop_start）推迟到本轮收尾再重载，避免打断正在输出的内容。
     */
    scheduleHistoryReload(sessionId: string): void {
      if (!sessionId) return
      if (this.streamingIds.includes(sessionId)) {
        this.pendingHistoryReload[sessionId] = true
        return
      }
      void this.reloadHistory(sessionId)
    },
    /** 丢弃本地消息列表并从后端重读（loadHistory 的「已加载」守卫先清掉） */
    async reloadHistory(sessionId: string): Promise<void> {
      if (!sessionId || this.streamingIds.includes(sessionId)) return
      delete this.pendingHistoryReload[sessionId]
      delete this.historyLoaded[sessionId]
      this.messages[sessionId] = []
      if (sessionId === this.activeId) await this.loadHistory(sessionId)
    },
    /**
     * 改写会话级设置(模式 / 审批 / 思考强度 / 模型 / 供应商)。
     *
     * **不必发消息**,后端立刻写内存活记录 → 下一个工具调用 / 下一轮 LLM 立即是新值
     * (不打断当前循环;已在等待用户决定的审批卡保持不变)。
     * 本地先写乐观值(界面与下一条消息立刻是新的),后端回执(session_info)到达即清草稿;
     * 后端失败则回滚乐观值并提示——不把界面留在假值上。
     */
    setSessionSettings(id: string, patch: Partial<SessionSettingsView>): void {
      const session = this.sessions.find((s) => s.id === id)
      if (!session) return
      const hadDraft = Object.prototype.hasOwnProperty.call(this.settingsDraft, id)
      const prev = this.settingsDraft[id]
      this.settingsDraft[id] = { ...(this.settingsDraft[id] ?? {}), ...patch }
      // 回复头部的模型名与会话记录同源(沿用 sendMessage 里记录会话模型的行为)
      if (patch.model) session.model = patch.model
      // 还没有后端会话:设置留在草稿里,首次 create_session 会带上整套
      if (!session.backendId) return
      const request = toBackendSettings(patch)
      if (!Object.keys(request).length) return
      const chat = window.api.chat
      if (!chat?.updateSessionSettings) return
      void chat.updateSessionSettings(session.backendId, request).catch((err) => {
        if (hadDraft && prev) this.settingsDraft[id] = prev
        else delete this.settingsDraft[id]
        const detail = err instanceof Error ? err.message : String(err)
        Message.error(i18n.global.t('chat.settingsUpdateFailed', { reason: detail }))
      })
    },
    /**
     * 设置 / 修改 / 删除会话目标（`/goal`，不必发消息）。
     *
     * 后端立刻写内存活记录并广播 `session_info`；成功后本地先同步目标文本，
     * 避免等事件回执才刷新卡片。`text` 为空 = 删除目标。
     */
    async updateSessionGoal(id: string, text: string | null): Promise<void> {
      const session = this.sessions.find((s) => s.id === id)
      if (!session?.backendId) throw new Error('会话尚未创建')
      const chat = window.api?.chat
      if (!chat?.updateSessionGoal) throw new Error('后端不可用')
      const goal = (text ?? '').trim()
      await chat.updateSessionGoal(session.backendId, goal || null, i18n.global.locale.value)
      const info = this.sessionInfo[id]
      if (info) {
        if (goal) {
          info.goal = {
            text: goal,
            lang: i18n.global.locale.value,
            tokensUsed: 0,
            iterations: 0,
            status: 'active'
          }
        } else {
          info.goal = null
        }
      }
    },
    /** 发送请求的模型 / 供应商:**会话设置优先**,调用方显式传入次之,最后会话记录。
     *  会话设置优先是有意的——设置已是会话属性,若让调用方(插件面板等)手里那份
     *  可能过期的值优先,后端 `sync_mode_permission` 会把会话的模型改回去。
     *  末尾兜底会话记录:旧持久化队列项可能不带 model,而后端 napi 要求该字段非空。 */
    resolveSendOptions(session: Session, opts: ChatSendOptions): ChatSendOptions {
      const settings = this.settingsOf(session.id)
      return {
        ...opts,
        model: settings.model || opts.model || session.model || '',
        provider: settings.provider || opts.provider || undefined
      }
    },
    /**
     * 发送消息:会话空闲直接发送;会话正在生成(任务进行中)则进入**前端消息队列**
     * (发送框上方列表,含优先级),收到后端 done 事件后自动发送优先级最高的一条。
     */
    sendMessage(text: string, opts: ChatSendOptions = {}): void {
      const sessionId = this.activeId
      // 允许「只有图片没有文字」的消息(附件在 opts.attachments)
      if (!sessionId || (!text.trim() && !opts.attachments?.length)) return
      // 普通消息到达:清掉本地待决中断卡,并把被取代的工具卡标为失败
      // (后端同时会把该 tool 消息置 superseded 并补发 error 结果的 tool_result,此处为即时反馈)
      const pending = this.pendingInterrupts[sessionId]
      if (pending) {
        const tool = findToolById(this.messages[sessionId] ?? [], pending.tool_call_id)
        if (tool) {
          tool.awaitingApproval = false
          tool.status = 'error'
        }
        this.pendingInterrupts[sessionId] = null
        this.setTaskForm(sessionId, false)
      }
      // 记录会话模型(发送框所选):纯文本回复头部展示模型名,流式兜底也用到
      if (opts.model) {
        const s = this.sessions.find((x) => x.id === sessionId)
        if (s) s.model = opts.model
      }
      // 用户主动发送:恢复队列自动派发(立即 / 取消曾置起 drainPaused)
      this.drainPaused[sessionId] = false
      if (this.isStreamingSession(sessionId)) {
        this.enqueuePending(text.trim(), opts)
        return
      }
      this.sendNow(text.trim(), opts)
    },
    /** 会话忙时排队(priority=列表序,即后台属性:发送顺序 + 拖拽排序决定,优先级越高越先自动发送) */
    enqueuePending(text: string, opts: ChatSendOptions = {}, priority = 5): void {
      const sessionId = this.activeId
      if (!sessionId) return
      const list = this.pendingQueues[sessionId] ?? (this.pendingQueues[sessionId] = [])
      list.push({ id: uid(), text, priority, opts, createdAt: Date.now() })
      savePendingQueues(this.pendingQueues)
    },
    /** 移动排队消息次序(拖拽排序):列表序即优先级,移动后同步后台优先级属性(持久化) */
    movePending(sessionId: string, from: number, to: number): void {
      const list = this.pendingQueues[sessionId]
      if (!list || from < 0 || from >= list.length || to < 0 || to >= list.length || from === to) return
      const [moved] = list.splice(from, 1)
      list.splice(to, 0, moved)
      list.forEach((x, i) => {
        x.priority = list.length - i
      })
      savePendingQueues(this.pendingQueues)
    },
    /** 取消排队:从消息队列移除,不发送(持久化) */
    cancelPending(sessionId: string, itemId: string): void {
      const list = this.pendingQueues[sessionId]
      if (!list) return
      this.pendingQueues[sessionId] = list.filter((x) => x.id !== itemId)
      savePendingQueues(this.pendingQueues)
    },
    /** 立即发送:不等当前任务处理完,打入后端打断消息队列(取消当前循环后优先执行;持久化)。
     *  为一次性操作:暂停自动派发,避免打破当前任务后的 done 又连发下一条 */
    sendImmediate(sessionId: string, itemId: string): void {
      const list = this.pendingQueues[sessionId] ?? []
      const idx = list.findIndex((x) => x.id === itemId)
      if (idx < 0) return
      const [item] = list.splice(idx, 1)
      savePendingQueues(this.pendingQueues)
      this.drainPaused[sessionId] = true
      this.sendNow(item.text, item.opts, true)
    },
    /** done 收尾后自动发送:取列表第一项(列表序即优先级:先发/拖到最前 = 最高优先级;持久化) */
    dispatchNextPending(sessionId: string): void {
      const list = this.pendingQueues[sessionId]
      if (!list || list.length === 0) return
      const [item] = list.splice(0, 1)
      savePendingQueues(this.pendingQueues)
      this.sendNow(item.text, item.opts)
    },
    /**
     * 真正发送:追加用户消息 + 流式助手气泡。
     * interrupt=true 表示「立即发送」→ 走后端打断消息队列(若会话正忙且已有后端会话)。
     */
    sendNow(text: string, opts: ChatSendOptions = {}, interrupt = false): void {
      const sessionId = this.activeId
      if (!sessionId || (!text.trim() && !opts.attachments?.length)) return
      const session = this.sessions.find((s) => s.id === sessionId)
      if (!session) return
      const list = this.messages[sessionId] ?? (this.messages[sessionId] = [])
      // 用户重新发起 → 上一轮的失败条/提示立即失效(不等首个 token,避免"点了没反应"的错觉)
      this.clearTurnAlerts(sessionId)
      list.push({
        id: uid(),
        role: 'user',
        content: text.trim(),
        createdAt: Date.now(),
        // 附件描述保留在本地气泡里(发送时后端会复制进会话工作区,此处路径仅用于即时缩略图)
        attachments: opts.attachments?.length ? [...opts.attachments] : undefined
      })
      // 关掉旧占位的流式标记:等待提示/工具动画只挂最新一条
      for (const m of list) if (m.streaming) m.streaming = false
      const assistant: ChatMessage = {
        id: uid(),
        role: 'assistant',
        content: '',
        createdAt: Date.now(),
        // 本轮处理起点（与上面那条 user 消息同一时刻发出，耗时不受影响）
        startedAt: Date.now(),
        streaming: true
      }
      list.push(assistant)
      this.startStreaming(sessionId)
      roundBySession[sessionId] = 0
      // 新消息已入列 → 通知面板滚到底部(面板监听 streamTick,不再深监听 messages)
      this.bumpStreamTick(sessionId)

      if (!window.api.chat) {
        // 无真实后端:不做 mock,直接提示失败(桌面应用必须 Rust napi 后端)
        this.failAssistant(sessionId, '后端不可用（window.api.chat 缺失）')
        return
      }
      if (interrupt && session.backendId) {
        // 立即发送:不等当前任务,打入后端打断消息队列(后端自动取消当前循环并优先执行)。
        // 不带会话设置:后端缺省 = 保持(打断不该顺带改模式)
        const req = toBackendRequest(text.trim(), this.resolveSendOptions(session, opts))
        void window.api.chat
          .interruptMessage(session.backendId, req)
          .catch((err) => this.failAssistant(sessionId, err instanceof Error ? err.message : String(err)))
        return
      }
      void this.sendBackend(session, assistant, text, opts)
    },
    /**
     * 真实后端路径:首次惰性建会话,再入队消息。
     * 失败**不做 mock 降级**:把错误写入助手气泡,界面即可见失败原因。
     */
    async sendBackend(session: Session, assistant: ChatMessage, text: string, opts: ChatSendOptions): Promise<void> {
      const chat = window.api.chat
      if (!chat) {
        this.failAssistant(session.id, '后端不可用（window.api.chat 缺失）')
        return
      }
      try {
        if (!session.backendId) {
          // 后端 create_session 会把首条消息自动入队并驱动循环,
          // 首条消息只调用 create_session,不重复 sendMessage(避免同一条消息跑两遍)。
          // 会话级设置随创建落库(settingsOf 读草稿:新建页选的就是它),之后的改动走 updateSessionSettings。
          session.backendId = await chat.createSession(
            toBackendRequest(
              text,
              this.resolveSendOptions(session, opts),
              this.settingsOf(session.id),
              backendSessionTypeOf(session)
            )
          )
          // 绑定后去重合并：事件流（WS）可能先于 createSession 返回到达，
          // registerSessionFromEvent 已按该 backendId 就地登记过一条——不合并会出现
          // 两条同 backendId 的会话，事件只路由到第一条，本地草稿永远「处理中」。
          const dup = this.sessions.find(
            (x) => x.id !== session.id && x.backendId === session.backendId
          )
          if (dup) {
            // 采纳事件登记条目已积累的消息（若有），保留当前草稿的身份与绑定
            if (!this.messages[session.id]?.length && this.messages[dup.id]?.length) {
              this.messages[session.id] = this.messages[dup.id]
            }
            const dupIdx = this.sessions.indexOf(dup)
            if (dupIdx >= 0) this.sessions.splice(dupIdx, 1)
            delete this.messages[dup.id]
            delete this.historyLoaded[dup.id]
            delete this.settingsDraft[dup.id]
            delete this.taskFlags[dup.id]
            delete this.sessionError[dup.id]
            delete this.pendingInterrupts[dup.id]
            delete this.deliverablesBySession[dup.id]
            const si = this.streamingIds.indexOf(dup.id)
            if (si >= 0) this.streamingIds.splice(si, 1)
            if (this.activeId === dup.id) this.activeId = session.id
            this.persist()
          }

        } else {
          await chat.sendMessage(
            session.backendId,
            toBackendRequest(text.trim(), this.resolveSendOptions(session, opts))
          )
        }
        session.updatedAt = Date.now()
      } catch (err) {
        const detail = err instanceof Error ? err.message : String(err)
        // 后端重启后内存会话注册表为空:老会话 send 报「会话不存在」→ 重建后端会话重试一次
        if (session.backendId && detail.includes('会话不存在')) {
          console.warn('[chat] backend session missing, recreate:', detail)
          session.backendId = ''
          return this.sendBackend(session, assistant, text, opts)
        }
        console.error('[chat] backend send failed:', detail)
        this.failAssistant(session.id, detail || '后端调用失败')
      }
    },
    /** 助手气泡标记失败(终止流式并把原因写入 error,界面以红色横幅展示) */
    failAssistant(sessionId: string, reason: string): void {
      const list = this.messages[sessionId]
      const last = list?.length ? list[list.length - 1] : undefined
      if (last?.role === 'assistant') {
        last.streaming = false
        last.reasoningStreaming = false
        if (!last.error) last.error = reason
      }
      // 失败也是本轮终点:耗时定格(与 finalizeStream 同口径),并标记错误结束
      const streamingMsg = findLastStreamingAssistant(list)
      if (streamingMsg) {
        if (streamingMsg.completedAt == null) streamingMsg.completedAt = Date.now()
        streamingMsg.endReason = 'error'
      }
      // 本轮以失败收尾 → 待审批态作废(避免收纳块被永久锁在展开且点不动)
      this.markPendingToolsFailed(sessionId, true)
      this.stopStreaming(sessionId)
      // 失败也是本轮终点：本轮期间跳过的历史装载（流式中打开 / 摘要压缩）在这里补上
      this.drainPendingHistoryReload(sessionId)
    },
    /**
     * 本轮收尾后补做「期间被推迟的历史装载」。
     *
     * 两个来源共用这个通道：流式中打开会话（`loadHistory` 推迟）、摘要压缩历史
     * （`scheduleHistoryReload` 推迟）。收尾时流式标记已清除，此刻重载不会打断渲染。
     */
    drainPendingHistoryReload(sessionId: string): void {
      if (this.pendingHistoryReload[sessionId]) void this.reloadHistory(sessionId)
    },
    /**
     * 事件轮次推进:第 2+ 轮开始时,收尾上一轮消息并开启新的独立助手消息,
     * 使每轮的 think / content / tools 各自成块(round 由后端事件携带)。
     */
    rotateRound(sessionId: string, list: ChatMessage[], round: number): void {
      if (typeof round !== 'number' || round < 1) return
      const prev = roundBySession[sessionId] ?? 0
      if (round <= prev) return
      roundBySession[sessionId] = round
      if (prev === 0) {
        // 首轮复用发送时创建的占位消息。后端创建的会话（定时任务 / 子会话）没人建占位:
        // 末尾不是本轮那个流式气泡就补一个,否则本轮正文会落到历史气泡上、工具卡直接丢弃。
        const tail = list.length ? list[list.length - 1] : undefined
        if (tail?.role === 'assistant' && tail.streaming) return
        list.push({
          id: uid(),
          role: 'assistant',
          content: '',
          createdAt: Date.now(),
          // 后端创建的会话（定时任务 / 子会话）没人打点，这里就是它本轮的起点
          startedAt: Date.now(),
          streaming: true
        })
        this.bumpStreamTick(sessionId)
        return
      }
      // 关掉全部旧流式占位(不只最后一条,避免历史消息残留 streaming 标记)
      for (const m of list) {
        if (m.streaming) {
          m.streaming = false
          m.reasoningStreaming = false
        }
      }
      list.push({
        id: uid(),
        role: 'assistant',
        content: '',
        createdAt: Date.now(),
        // 第 2+ 轮：整轮起点取本轮各步骤里最早的一个，所以这里晚于首轮无妨
        startedAt: Date.now(),
        streaming: true
      })
      // 新一轮独立气泡出现 → 面板跟随到底部
      this.bumpStreamTick(sessionId)
    },
    /**
     * 订阅后端全局事件流(须在 create_session 前完成,无回放):
     * text → 正文;reasoning → 思考链;tool_call/tool_result → 工具卡片;
     * title → 更新标题;done/error → 收尾。
     * 事件携带 round,轮次变化时把后续内容拆到新的助手消息。
     */
    initChatStream(): void {
      const chat = window.api.chat
      if (!chat?.onEvent || chatEventOff) return
      chatEventOff = chat.onEvent((chunk) => {
        const ev = chunk as BackendEvent
        // 事件总线积压丢帧的**全局告警帧**（session_id 为空，见 astorm-napi subscribe.rs 的
        // lag 降级）：它不带会话 id，落到下面会被当「未知会话」直接丢弃，但这一窗事件确已丢，
        // 状态可能长期不回正 → 防抖全量补拉一次（唯一需要主动拉的后端状态）。
        if (!ev.session_id && ev.type === 'error') {
          this.scheduleCatchUp()
          return
        }
        // 本地未登记的会话（后端子会话 / 定时任务等由后端创建的会话）：首条 session_info
        // 带完整记录 → 就地登记，后续事件（正文/工具/中断/终态）才有人接。查不到则忽略。
        const s =
          this.sessions.find((x) => x.backendId === ev.session_id) ??
          this.registerSessionFromEvent(ev)
        if (!s) return
        // 非文本事件前先把缓冲的正文/思考落位:保证「正文 → 工具卡 → 终态」的先后顺序
        if (ev.type !== 'text' && ev.type !== 'reasoning') this.flushStreamDeltas()
        // 会话活动 → 更新时间刷新(镜像后端每轮更新的 updated_at,侧栏即时可见)。
        // 只在轮次级事件更新:正文/思考每 token 写一次会让会话列表深监听每 token 触发一次
        // （→ JSON.stringify + localStorage），是流式卡死的主因之一。
        if (ev.type === 'tool_call' || ev.type === 'tool_result' || ev.type === 'title') {
          s.updatedAt = Date.now()
        }
        // 会话重新开始干活(新一轮的轮次级事件) → 上一轮的失败条 / 重试提示不再代表
        // 当前状态,立即撤掉。否则「上一轮报错后用户发『继续』」会一直挂着旧错误,
        // 与正在流式的这一轮自相矛盾。
        if (TURN_ACTIVITY_EVENTS.has(ev.type ?? '')) this.clearTurnAlerts(s.id)
        const list = this.messages[s.id] ?? (this.messages[s.id] = [])
        // 只有**开启新一轮 LLM 的内容事件**才做轮次切分。控制事件（tool_result / interrupt 等，
        // 比如用户忽略提问后决议写回的那条 tool_result）仍带着后端的累计 round，而前端已在
        // done 时把计数器归零——若也走 rotateRound，就会在末尾凭空补一条空的流式助手气泡：
        // 它没有工具、永远等不到终态事件，界面表现为「本轮明明已结束/已忽略，上方却一直
        // 显示『正在处理 Ns』」（收纳块 ended 恒为 false）。
        if (ev.type === 'text' || ev.type === 'reasoning' || ev.type === 'tool_call') {
          this.rotateRound(s.id, list, ev.round ?? 0)
        }
        // 相位清空表:终态 / 等待人(审批、填表、邮件) / 被打断——都不该再显示「正在…」流光
        if (
          ev.type === 'done' ||
          ev.type === 'error' ||
          ev.type === 'cancelled' ||
          ev.type === 'interrupted' ||
          ev.type === 'ignored' ||
          ev.type === 'awaiting_approval' ||
          ev.type === 'awaiting_form' ||
          ev.type === 'awaiting_mail_compose'
        ) {
          this.setRunPhase(s.id, undefined)
        }
        const last = list.length ? list[list.length - 1] : undefined
        switch (ev.type) {
          case 'goal_continue': {
            // 目标循环注入的续跑用户消息:即时补一条用户气泡 + 新的流式助手占位,
            // 并把轮次计数器归零,让下一轮事件落到新占位上(与用户主动发送同款)。
            const content = ev.content ?? ''
            if (content) {
              for (const m of list) {
                if (m.streaming) {
                  m.streaming = false
                  m.reasoningStreaming = false
                }
              }
              list.push({ id: uid(), role: 'user', content, createdAt: Date.now() })
              list.push({
                id: uid(),
                role: 'assistant',
                content: '',
                createdAt: Date.now(),
                startedAt: Date.now(),
                streaming: true
              })
              roundBySession[s.id] = 0
              this.startStreaming(s.id)
              this.bumpStreamTick(s.id)
            }
            break
          }
          case 'text':
            // 只入缓冲,由 rAF 合帧写回(一帧一次),避免每 token 一次渲染 + markdown 重解析
            if (last?.role === 'assistant') this.bufferStreamDelta(s.id, last, ev.content ?? '', '')
            // 正文已开始流式:内容本身就是反馈,状态行让位(不再显示「正在…」)
            this.setRunPhase(s.id, undefined)
            break
          case 'reasoning':
            if (last?.role === 'assistant') this.bufferStreamDelta(s.id, last, '', ev.content ?? '')
            this.setRunPhase(s.id, 'thinking')
            break
          case 'tool_call':
            // 工具已下发:进入「等待工具执行结果」
            this.setRunPhase(s.id, 'tool')
            if (last?.role === 'assistant' && Array.isArray(ev.tool_calls)) {
              last.tools ??= []
              for (const tc of ev.tool_calls) {
                last.tools.push({
                  id: tc.id ?? '',
                  name: tc.name ?? 'unknown',
                  params: tc.params,
                  status: 'pending'
                })
              }
              this.bumpStreamTick(s.id)
            }
            break
          case 'tool_result':
            // 结果已回:模型将带着结果继续推理 → 回到「等待模型回复」
            this.setRunPhase(s.id, 'awaiting')
            // 按 tool_call id 跨消息定位:自动重放的结果到达时,最新消息已是新插入的用户消息
            {
              const tool = findToolById(list, ev.id)
              if (tool) {
                // 后端自本次起显式给出 status(ok/error);缺省(旧后端)按结果文风兜底推断
                tool.awaitingApproval = false
                tool.status =
                  ev.status === 'error'
                    ? 'error'
                    : ev.status === 'ok' || !/^工具 .*执行失败/.test(ev.result ?? '')
                      ? 'completed'
                      : 'error'
                tool.result = ev.result ?? ''
                // 安全拦截分类(拒绝读/写/危险命令/网络/资源/浏览器 + 来源可信度)
                tool.denial = ev.denial ?? null
                // 沙箱运行事实(无防护 / 被驱逐 / 输出截断；后端仅在 bash 结果上给出)
                tool.sandbox = ev.sandbox ?? null
                // 文件级结构化元数据(后端 file_operation 的 execute_with_meta):
                // 写类 diff / 读类行字符统计;过程行据此显示 +N -N / N行M字符
                tool.fileDiff = ev.file_diff ?? null
                tool.readStats = ev.read_stats ?? null
                // 网页 / 记忆检索结果元数据（「相关资料」「相关记忆」直接读它）
                tool.webResults = ev.web_results ?? null
                tool.memoryResults = ev.memory_results ?? null
                tool.duration = typeof ev.duration_ms === 'number' ? ev.duration_ms : undefined
                // 工具失败一律不写顶部错误条：错误原文已随 tool 结果回给模型，由它判断改道
                // （安全拦截/沙箱拒绝的分类与明细留在工具卡上——denial-badge + 明细，
                //  needsAttention 会自动展开；纯执行失败见 ChatPanel.isQuietFailure）。
                // 顶部错误条只留给非工具的顶层 error 事件（LLM 服务失败 / 超时等）。
                // 待决中断已落定(决议改写后的结果到达)→ 清掉工具卡上的待决标记
                if (tool.interrupt?.status === 'pending') {
                  tool.interrupt = { ...tool.interrupt, status: ev.status === 'error' ? 'failed' : 'resolved' }
                }
                this.bumpStreamTick(s.id)
              }
            }
            break
          case 'interrupt':
            // 等待用户决定(审批 / 填表):状态行让位给卡片
            this.setRunPhase(s.id, undefined)
            // 中断(安全审批 / 提问):卡片完全由结构化 payload 驱动(不再从工具结果文本刮取);
            // 顺带把被中断工具卡置为「等待决定」,使其不随本轮 done 收尾被标记失败
            {
              const pending = interruptFromPayload(ev.payload)
              if (pending) {
                this.pendingInterrupts[s.id] = pending
                if (pending.interrupt_type === 'form') this.setTaskForm(s.id, true)
                if (pending.interrupt_type === 'mail_compose') this.setTaskMailCompose(s.id, true)
                const tool = findToolById(list, pending.tool_call_id)
                if (tool) {
                  tool.awaitingApproval = true
                  tool.status = 'pending'
                  tool.interrupt = {
                    request_id: pending.request_id,
                    interrupt_type: pending.interrupt_type,
                    status: 'pending',
                    request: pending.request
                  }
                }
                this.bumpStreamTick(s.id)
              }
            }
            break
          case 'title':
            if (ev.title) s.title = ev.title
            if (ev.title_zh) s.titleZh = ev.title_zh
            if (ev.title_en) s.titleEn = ev.title_en
            this.flushPersist()
            break
          case 'plan_updated':
            // 后端 task_list 创建/更新/删除后的即时通知:自增版本号,
            // ChatPanel 据此立即重读计划大纲（不必等「消息变化 + 防抖」的兜底刷新）
            this.planVersion[s.id] = (this.planVersion[s.id] ?? 0) + 1
            break
          case 'session_info':
            {
              // 会话每次持久化后推送完整记录 → 刷新会话信息卡片(轮次 / 用量 / 更新时间等)
              const info = ev as unknown as SessionInfo
              const prevSummaryTimes = this.sessionInfo[s.id]?.summaryTimes ?? 0
              this.sessionInfo[s.id] = info
              // 后端已确认本次设置改写 → 清掉乐观值(只认「草稿每一项都已落进记录」的推送,
              // 否则每轮的 session_info 会把用户刚改的值瞬间打回旧值)
              const draft = this.settingsDraft[s.id]
              if (draft && draftConfirmedBy(draft, info)) delete this.settingsDraft[s.id]
              // 摘要次数变大 = 后端已把历史压缩重写为「系统提示词 + 摘要 + 保留消息」，
              // 本地已渲染的消息列表随之过期（旧对话不再属于会话历史）→ 重载。
              if ((info.summaryTimes ?? 0) > prevSummaryTimes && this.historyLoaded[s.id]) {
                this.scheduleHistoryReload(s.id)
              }
              // 后端权威工作区回填本地列表：本地新建会话的 workspace 是 todayKey 日期占位
              // （createSession），后端实际按创建时刻命名（yyyy_MM_dd_HH_mm_ss）。不回填会让
              // 侧栏分组 / 文件树用占位值解析出不存在的目录（读目录 ENOENT）——卡片显示什么，
              // 列表就用什么（与 StatusBar 目录胶囊同源）。
              // 标题同样以后端记录为准（摘要后替换标题 / 列表回填）；
              // 双标题分别保留，界面按当前语言取用（utils/sessionTitle）。
              if (typeof info.title === 'string' && info.title) s.title = info.title
              if (typeof info.titleZh === 'string' && info.titleZh) s.titleZh = info.titleZh
              if (typeof info.titleEn === 'string' && info.titleEn) s.titleEn = info.titleEn
              const ws = typeof info.workspace === 'string' ? info.workspace.trim() : ''
              if (ws && ws !== s.workspace) s.workspace = ws
              // 会话类型同样以后端为准：类型变化 → 重算侧栏分组（后端回填定时任务会话 /
              // 老记录缺省类型被补上时，会话当场从「项目」挪回「任务」）
              const st = readSessionType(info.sessionType)
              if (st && st !== s.sessionType) {
                s.sessionType = st
                const parent = s.pid ? this.sessions.find((x) => x.backendId === s.pid) : undefined
                s.kind = parent?.kind ?? deriveKind(st, s.workspace)
              }
              // 归属任务 id 同样以后端为准（后端回填老定时会话时同步）
              const taskId = typeof info.scheduledTaskId === 'string' ? info.scheduledTaskId : null
              if (taskId && taskId !== s.scheduledTaskId) s.scheduledTaskId = taskId
              // 活动时间回填（后端每轮落库都会推）：侧栏排序 / 看板列内排序按它排，
              // 只回填不回退 —— 后端 unix 秒、本地毫秒，本地打点可能比落库值新。
              if (typeof info.updatedAt === 'number' && info.updatedAt > 0) {
                s.updatedAt = Math.max(s.updatedAt ?? 0, info.updatedAt * 1000)
              }
              // 运行状态同样以后端为准（多轮 drain / 中断 / 终态都有推送）：
              // 冷启动与「漏掉终态事件」两种情况都由它兜底
              this.applyBackendStatus(s.id, info.status)
            }
            break
          case 'summarizing':
            // 自动摘要(上下文压缩)开始:过程末尾状态行显示流光「正在摘要压缩...」
            this.setRunPhase(s.id, 'summarizing')
            break
          case 'summary_done':
            // 摘要完成:追问模型即将开始 → 状态落在「等待模型回复」
            this.setRunPhase(s.id, 'awaiting')
            break
          case 'stream_reset':
            // 流中断自动重试:重新进入等待模型回复
            this.setRunPhase(s.id, 'awaiting')
            // 后端本轮流式输出作废重来(流中途传输中断 → provider 自动重开重发)。
            // 必须丢弃本轮此前渲染的半截正文/思考/工具卡,否则两次尝试的内容会拼在一起;
            // 缓冲里的增量已由上面的 flushStreamDeltas() 落位,这里清掉即可。
            // 同时清掉上一轮遗留的错误条 + 挂一条信息级提示(用户知道"内容为什么消失")。
            if (last?.role === 'assistant') {
              last.content = ''
              last.reasoning = ''
              last.tools = []
              last.streaming = true
              last.reasoningStreaming = false
            }
            delete this.sessionError[s.id]
            this.sessionNotice[s.id] = i18n.global.t('chat.streamRetrying')
            this.bumpStreamTick(s.id)
            break
          case 'session_stats': {
            // 本轮 token 用量（双方）；挂到本轮结尾的无工具助手消息上，供底部「用量」展示
            const target = [...list].reverse().find((m) => m.role === 'assistant')
            if (target) {
              target.usage = {
                prompt: ev.prompt_tokens ?? 0,
                completion: ev.completion_tokens ?? 0,
                total: ev.total_tokens ?? 0
              }
              this.bumpStreamTick(s.id)
            }
            break
          }
          case 'done':
            roundBySession[s.id] = 0
            this.finalizeStream(s.id)
            // 任务正常结束:清除错误条显示(异常提示不再悬挂) + 侧栏绿标
            delete this.sessionError[s.id]
            delete this.sessionNotice[s.id]
            this.taskFlags[s.id] = { ...(this.taskFlags[s.id] ?? {}), end: 'done' }
            // 对话收尾 → 从交付物表拉取本轮及此前各轮的上报记录(工具已落库,这里只读)
            void this.fetchDeliverables(s.id)
            // 待决中断(待审批 / 待填表)不是「任务完成」:后端因中断退出循环同样会发 done
            // (状态本身由后端保持 awaiting_*,侧栏橙点靠 formWaiting / awaitingApproval,
            // 优先级高于 end)。这类会话不自动派发排队消息——
            // 派发等于用新消息顶掉待决中断(后端收到普通消息即 supersede 掉待审命令 / 待答表单)
            const waiting = !!this.pendingInterrupts[s.id]
            // 任务完成:自动发送消息队列中优先级最高的一条(若还有排队消息)。
            // 立即发送 / 用户取消后暂停派发(drainPaused),避免打断消息完成或取消后的
            // done 又连发下一条(立即只发点击的那条;取消后不再自动发送)
            if (!waiting && !this.drainPaused[s.id]) this.dispatchNextPending(s.id)
            this.bumpStreamTick(s.id)
            this.flushPersist()
            break
          case 'cancelled':
            // 用户取消(暂停):收尾保留半截内容;不派发排队消息、
            // 不显错误条(侧栏打红点表示「没正常退出」,与执行失败同色、文案区分)
            roundBySession[s.id] = 0
            this.finalizeStream(s.id, 'cancelled')
            delete this.sessionNotice[s.id]
            this.taskFlags[s.id] = { ...(this.taskFlags[s.id] ?? {}), end: 'cancelled' }
            // 取消前工具可能已完成上报(如已落库后被暂停)→ 同步拉一次,不多不少
            void this.fetchDeliverables(s.id)
            this.bumpStreamTick(s.id)
            this.flushPersist()
            break
          case 'error':
            console.error('[chat] backend error:', ev.message)
            this.failAssistant(s.id, ev.message || '后端错误')
            this.sessionError[s.id] = ev.message || '后端错误'
            delete this.sessionNotice[s.id]
            this.taskFlags[s.id] = { ...(this.taskFlags[s.id] ?? {}), end: 'error' }
            this.bumpStreamTick(s.id)
            this.flushPersist()
            break
        }
      })
    },
    /** 更新会话流式相位(状态行流光文案的数据源;undefined = 不显示状态) */
    setRunPhase(
      sessionId: string,
      phase: 'awaiting' | 'thinking' | 'tool' | 'summarizing' | undefined
    ): void {
      if (!sessionId) return
      this.runPhase[sessionId] = phase
    },
    /** 标记会话进入流式态(多会话可并发,各自进列表) */
    startStreaming(sessionId: string): void {
      if (!this.streamingIds.includes(sessionId)) this.streamingIds.push(sessionId)
      // 入流即「等待模型回复」(发送 / 目标续跑 / 决议续跑共用此入口)
      this.setRunPhase(sessionId, 'awaiting')
    },
    /** 会话流式收尾(从列表移除;幂等) */
    stopStreaming(sessionId: string): void {
      const i = this.streamingIds.indexOf(sessionId)
      if (i >= 0) this.streamingIds.splice(i, 1)
    },
    /** 会话表单待答状态(侧栏任务树图标):提交/忽略表单后由 ChatPanel 调用清除 */
    setTaskForm(sessionId: string, waiting: boolean): void {
      if (!sessionId) return
      this.taskFlags[sessionId] = { ...(this.taskFlags[sessionId] ?? {}), formWaiting: waiting }
    },
    /** 会话待确认发送邮件状态(侧栏任务树图标):发送/取消后由 ChatPanel 调用清除 */
    setTaskMailCompose(sessionId: string, waiting: boolean): void {
      if (!sessionId) return
      this.taskFlags[sessionId] = { ...(this.taskFlags[sessionId] ?? {}), mailComposeWaiting: waiting }
    },
    /**
     * 提交中断决议(审批同意/拒绝、表单回答/忽略):napi `submitInterrupt`。
     *
     * 与「发送用户消息」并列的第二条会话输入——决议不追加用户消息,而是就地把用户决定
     * 写回被中断的 tool 消息(决议即 tool 结果),后端随后续跑。
     * 入队前乐观收起本地卡片;后端拒绝(请求已失效/已被取代)时提示并保持工具卡为失败态。
     */
    async submitInterrupt(decision: InterruptDecision, targetSessionId?: string): Promise<void> {
      // 目标会话缺省 = 当前激活会话;子会话页签里的审批/表单传各自会话 id
      // （否则会把决议提交到主会话，后端按 request_id 找不到而失败）
      const sessionId = targetSessionId ?? this.activeId
      const session = this.sessions.find((x) => x.id === sessionId)
      const pending = this.pendingInterrupts[sessionId]
      if (!session?.backendId || !pending) return
      // 乐观收起卡片:工具卡转入「执行中」(待后端 tool_result 事件落定)
      this.pendingInterrupts[sessionId] = null
      const tool = findToolById(this.messages[sessionId] ?? [], pending.tool_call_id)
      if (tool) tool.awaitingApproval = false
      // 忽略提问:工具卡标记就地转 ignored（后端落盘 marker 同值）→ 实时视图与重读历史一致，
      // 且「继续填写表单」入口立刻出现（见 ChatFormResume / turnGrouping.resumableForm）。
      // 此后到达的 tool_result 只在 status 仍为 pending 时改写标记，不会覆盖它。
      if ('ignored' in decision && tool?.interrupt?.status === 'pending') {
        tool.interrupt = { ...tool.interrupt, status: 'ignored' }
      }
      this.setTaskForm(sessionId, false)
      this.setTaskMailCompose(sessionId, false)
      try {
        // 入参 camelCase:后端 napi DTO(SubmitInterruptRequest)字段为 requestId / interruptType
        // ——snake 会直接被 napi 判为缺字段(Missing field `requestId`)而整条决议提交失败。
        // 审批范围走 allowScope(后端 `allow_scope`);`once` 即缺省,不发字段。
        const payload =
          'decision' in decision
            ? decision.decision === 'allow'
              ? { decision: 'allow' as const, ...(decision.scope === 'session' ? { allowScope: 'session' } : {}) }
              : { decision: 'deny' as const }
            : decision
        await window.api.chat?.submitInterrupt(session.backendId, {
          requestId: pending.request_id,
          interruptType: pending.interrupt_type,
          ...payload
        })
        // 决议已交回,循环续跑 → 状态行回到「等待模型回复」
        this.setRunPhase(sessionId, 'awaiting')
      } catch (err) {
        const reason = err instanceof Error ? err.message : String(err)
        console.error('[sessions] submit interrupt failed:', reason)
        Message.warning(reason || i18n.global.t('approval.stale'))
      }
    },
    /**
     * 重新填写一条被忽略（或被新消息取代 / 被中断）的提问：napi `reopenInterrupt`。
     *
     * 后端按原 tool 消息上的 `metadata.interrupt.request.form` 重新登记待决中断（换新 request_id），
     * 并发 `interrupt` 事件 → 表单卡照旧在输入框位置弹出（新实例，天然空白，不保留上次填写）。
     * **本方法不改视图**：待决卡片由那条事件落定（避免出现「卡在 pending 却没有 request_id」的中间态）。
     */
    async reopenForm(toolCallId: string, targetSessionId?: string): Promise<void> {
      const sessionId = targetSessionId ?? this.activeId
      const session = this.sessions.find((x) => x.id === sessionId)
      if (!session?.backendId || !toolCallId) return
      if (!window.api.chat?.reopenInterrupt) return
      try {
        await window.api.chat.reopenInterrupt(session.backendId, toolCallId)
      } catch (err) {
        // 运行中 / 已有其他待决中断 / 提问不可恢复：后端拒绝的原因直接给用户看
        const reason = err instanceof Error ? err.message : String(err)
        console.error('[sessions] reopen form failed:', reason)
        Message.warning(reason || i18n.global.t('chat.resumeFormFailed'))
      }
    },
    /**
     * 后端持久化的中断标记(metadata.interrupt) → 工具卡状态,并重建待决卡片(冷启动 / 历史回读)。
     * 状态机见后端 `agent::interrupt`：approval = pending/replaying/approved/failed/denied/
     * superseded/interrupted；form = pending/resolved/ignored/superseded。
     * 无标记时按结果文风推断(兼容历史数据)。
     */
    applyInterruptMarker(sessionId: string, tool: ToolCallView, marker: InterruptMarker | null | undefined): void {
      const status = marker?.status
      if (!status) {
        tool.awaitingApproval = false
        // 历史数据可能只有文本：有分类元数据则按状态，否则按结果文风推断
        if (tool.denial) tool.status = 'error'
        else tool.status = /^工具 .*执行失败/.test(tool.result ?? '') ? 'error' : 'completed'
        return
      }
      tool.interrupt = {
        request_id: marker.request_id ?? '',
        interrupt_type: marker.interrupt_type ?? 'approval',
        status,
        request: marker.request
      }
      tool.awaitingApproval = status === 'pending'
      switch (status) {
        case 'pending': {
          // 待用户决定:重建卡片(审批卡 / 提问表单),冷启动后仍可继续处理;
          // 同时用信封里的分类填充徽标(旧记录只有 approval.kind,按 category ?? kind 兜底)
          tool.status = 'pending'
          tool.denial = tool.denial ?? denialFromApproval(marker.request?.approval)
          const pending = marker.request ? interruptFromPayload(marker.request) : null
          if (pending) {
            this.pendingInterrupts[sessionId] = pending
            if (pending.interrupt_type === 'form') this.setTaskForm(sessionId, true)
            if (pending.interrupt_type === 'mail_compose') this.setTaskMailCompose(sessionId, true)
          }
          break
        }
        case 'replaying':
          // 已同意、正在重放(进程若在此退出 → 下次读历史为 interrupted)
          tool.status = 'pending'
          break
        case 'approved':
        case 'resolved':
        case 'ignored':
          // 终态且无异常：重放成功 / 用户已回答 / 用户主动忽略（均正常展示结果）
          tool.status = 'completed'
          break
        case 'denied':
          // 用户拒绝：保留原拦截分类（徽标解释「为什么被拦」）
          tool.denial = tool.denial ?? denialFromApproval(marker.request?.approval)
          tool.status = 'error'
          break
        default:
          // failed / superseded / interrupted：未执行或结果未知的失败态
          tool.status = 'error'
      }
    },
    /**
     * 后端会话状态 → 侧栏状态标志（冷启动 / session_info 回填）。
     *
     * 只做「后端 → 前端视图字段」的映射，不反向写后端：
     * - `running`：不落标志（进程内运行中由 `streamingIds` 驱动，0 延迟；冷启动时后端会把
     *   陈旧的 running 报为 interrupted，故这里不会出现「假运行中」）
     * - `awaiting_form` / `awaiting_approval`：侧栏显示「等你处理」橙点
     * - `done` / `error` / `cancelled` / `interrupted`：终态点（终态互斥，先清旧终态）
     * - `idle` / 未知：清空标志（无图标）
     */
    applyBackendStatus(sessionId: string, status: SessionStatus | undefined): void {
      if (!status) return
      const prev = this.taskFlags[sessionId] ?? {}
      const next = { ...prev }
      switch (status) {
        case 'running':
          // 后端确认正在跑（新一轮 / 重试 / 队列派发）→ 上一轮的失败条失效
          this.clearTurnAlerts(sessionId)
          return
        case 'awaiting_form':
          next.formWaiting = true
          next.awaitingApproval = false
          next.mailComposeWaiting = false
          delete next.end
          break
        case 'awaiting_approval':
          next.awaitingApproval = true
          next.formWaiting = false
          next.mailComposeWaiting = false
          delete next.end
          break
        case 'awaiting_mail_compose':
          next.mailComposeWaiting = true
          next.formWaiting = false
          next.awaitingApproval = false
          delete next.end
          break
        case 'done':
        case 'error':
        case 'cancelled':
        case 'interrupted':
        case 'ignored':
          // ignored = 用户忽略了提问：本轮已停（不是完成、也不是失败），侧栏灰点 + 可继续填写
          next.end = status
          next.formWaiting = false
          next.awaitingApproval = false
          next.mailComposeWaiting = false
          break
        default:
          // idle / 未知：无图标
          next.formWaiting = false
          next.awaitingApproval = false
          next.mailComposeWaiting = false
          delete next.end
      }
      this.taskFlags[sessionId] = next
    },
    /**
     * 会话重新开始干活 → 撤掉上一轮的失败条与瞬时提示。
     *
     * 触发点：新一轮的轮次级事件（`TURN_ACTIVITY_EVENTS`）、后端状态回到 `running`、
     * 用户发出新消息。旧实现只在 `done`/新 `error` 时清，于是「上一轮报错 → 用户发
     * 『继续』」这条最常见的恢复路径下，错误条会一直挂到本轮结束，与正在流式的内容矛盾。
     */
    clearTurnAlerts(sessionId: string): void {
      if (this.sessionError[sessionId]) delete this.sessionError[sessionId]
      if (this.sessionNotice[sessionId]) delete this.sessionNotice[sessionId]
    },
    /**
     * 流中止后收尾:把没有结果的 pending 工具标记失败,停止转圈(取消 / 错误 / 正常收尾共用)。
     *
     * 等待用户审批的工具(`awaitingApproval`)默认不标记失败——它本就该保持待审批态
     * (会话停在「等审批」时对话区收纳块要一直展开、且不允许收起)。
     * 但**用户取消 / 打断**时该待审批态已作废:`clearApproval=true` 把它一并清掉，
     * 否则收纳块会被 `hasBlockingInterrupt` 永久锁在展开且点不动(表现为「点了收不起来」)。
     */
    markPendingToolsFailed(sessionId: string, clearApproval = false): void {
      const list = this.messages[sessionId]
      if (!list) return
      for (const msg of list) {
        if (!Array.isArray(msg.tools)) continue
        for (const tool of msg.tools) {
          if (clearApproval && tool.awaitingApproval) {
            tool.awaitingApproval = false
            if (tool.status === 'pending') tool.status = 'error'
            continue
          }
          if (tool.status === 'pending' && !tool.awaitingApproval) tool.status = 'error'
        }
      }
    },
    /**
     * 拉取会话交付物表（`submit_deliverables` 工具逐轮落库，按轮次升序）。
     *
     * 调用点：对话结束（done / cancelled）、历史装载完成。拉取是**只读**的——
     * 落库真相在后端 sqlite `session_deliverables` 表，这里只做缓存刷新；
     * 后端不可用 / 会话未登记时静默清空，旧会话的卡片显示回退消息 metadata.artifacts。
     */
    async fetchDeliverables(sessionId: string): Promise<void> {
      const s = this.sessions.find((x) => x.id === sessionId)
      if (!s?.backendId) {
        delete this.deliverablesBySession[sessionId]
        return
      }
      try {
        const raw = await invoke('session_deliverables', { session_id: s.backendId })
        const rows = typeof raw === 'string' ? (JSON.parse(raw) as DeliverableRow[]) : []
        this.deliverablesBySession[sessionId] = Array.isArray(rows) ? rows : []
      } catch (err) {
        console.warn('[sessions] fetch deliverables failed:', err)
      }
      this.bumpStreamTick(sessionId)
    },
    /** 某轮对话的交付物行：按当轮用户消息的 seq（历史消息）或文本（直发消息）匹配。 */
    deliverableRowOf(sessionId: string, userSeq: number | undefined, userText: string | undefined): DeliverableRow | undefined {
      const rows = this.deliverablesBySession[sessionId]
      if (!rows?.length) return undefined
      if (userSeq !== undefined && userSeq > 0) {
        const bySeq = rows.find((r) => r.userMsgSeq === userSeq)
        if (bySeq) return bySeq
      }
      // 直发消息在前端无 seq：回退按用户消息文本匹配（后端快照即原文）
      if (userText) return rows.find((r) => r.userMessage === userText)
      return undefined
    },
    /** 流式收尾:终止助手消息、pending 工具与侧栏脉动点。
     *  reason 标记本轮是正常完成还是被用户打断(对话区据此显示「工作已暂停…」状态行) */
    finalizeStream(sessionId: string, reason: 'done' | 'cancelled' = 'done'): void {
      const list = this.messages[sessionId]
      const last = list?.length ? list[list.length - 1] : undefined
      if (last?.role === 'assistant') {
        last.streaming = false
        last.reasoningStreaming = false
      }
      // 本轮结束时间(对话区收纳块耗时)与结束原因:认「最后一条还在流式的助手消息」,
      // 不盲取 list 末尾——立即发送/打断时下一条用户消息与占位可能已经入列
      const streamingMsg = findLastStreamingAssistant(list)
      if (streamingMsg) {
        if (streamingMsg.completedAt == null) streamingMsg.completedAt = Date.now()
        streamingMsg.endReason = reason
      }
      // 用户取消 / 打断:待审批态作废(清掉 awaitingApproval),否则收纳块点不动收不起来
      this.markPendingToolsFailed(sessionId, reason === 'cancelled')
      this.stopStreaming(sessionId)
      // 本轮期间被推迟的历史装载（流式中打开 / 摘要压缩过历史）→ 收尾后重载一次
      this.drainPendingHistoryReload(sessionId)
    },
    stopGeneration(): void {
      const sessionId = this.activeId
      // 取消后暂停队列自动派发:取消触发的 done/cancelled 不再自动取队列发送
      this.drainPaused[sessionId] = true
      // 立即收尾 pending 工具(转圈停止);用户主动停止 → 待审批态一并作废(clearApproval)
      this.markPendingToolsFailed(sessionId, true)
      const s = this.sessions.find((x) => x.id === sessionId)
      if (!window.api.chat) return
      if (!s?.backendId) {
        // 后端会话尚未创建(首条消息 createSession 未返回):本地直接收尾,避免永转圈
        this.finalizeStream(sessionId, 'cancelled')
        return
      }
      window.api.chat.cancel(s.backendId).catch((err) => {
        // 停止失败必须可见(binding 缺失/会话不存在等),否则用户以为已暂停
        console.error('[chat] cancel failed:', err)
        this.failAssistant(sessionId, `停止失败: ${err instanceof Error ? err.message : String(err)}`)
      })
    },

    /**
     * 该用户消息是否可撤回:历史回载的消息需在会话摘要边界**之后**;
     * 前端即时发送的消息(无 `seq`)恒可撤回(必然在边界之后)。摘要前的对话已压缩丢弃,
     * 撤回会跨摘要(后端也会拒绝),故置灰。
     */
    canRetract(sessionId: string, msg: ChatMessage): boolean {
      if (msg.role !== 'user') return false
      const boundary = this.rewindBoundary[sessionId] ?? Number.NEGATIVE_INFINITY
      return msg.seq === undefined || msg.seq > boundary
    },

    /**
     * 撤回会话历史到指定用户消息**之前**(该消息及其后全部轮次删除),并立即中断当前循环。
     *
     * `userIndex` = 该用户消息在会话用户消息中的序号(0-based),对齐后端 `keep_user_messages`。
     * 成功返回被撤回消息的 `{ content, attachments }`(供输入框回填);失败返回 null 并提示。
     */
    async rewindToUserMessage(
      sessionId: string,
      userIndex: number
    ): Promise<{ content: string; attachments?: MessageAttachment[] } | null> {
      const session = this.sessions.find((x) => x.id === sessionId)
      const list = this.messages[sessionId] ?? []
      let seen = -1
      let msgIndex = -1
      for (let i = 0; i < list.length; i++) {
        if (list[i].role !== 'user') continue
        seen++
        if (seen === userIndex) {
          msgIndex = i
          break
        }
      }
      if (!session || msgIndex < 0) return null
      const target = list[msgIndex]
      const captured = {
        content: target.content,
        attachments: target.attachments?.length ? [...target.attachments] : undefined
      }

      const streaming = this.streamingIds.includes(sessionId) || list.some((m) => m.streaming)
      // 撤回后暂停队列自动派发:待发队列保留,但不再自动发送
      const prevDrain = this.drainPaused[sessionId]
      this.drainPaused[sessionId] = true
      // 立即收尾 pending 工具(后端截断会作废这些中断台账)
      this.markPendingToolsFailed(sessionId)
      // 流式:先登记「收尾后重载」,避免 cancelled 事件早于 await 续体到达时漏掉重载
      if (streaming) this.pendingHistoryReload[sessionId] = true

      // 后端截断(运行中 = 立即中断循环;跨摘要等错误在此抛出)
      if (session.backendId && window.api.chat?.rewindToUserMessage) {
        try {
          await window.api.chat.rewindToUserMessage(session.backendId, userIndex)
        } catch (err) {
          // 失败回滚派发暂停,避免队列被永久搁置
          this.drainPaused[sessionId] = prevDrain
          if (streaming) this.pendingHistoryReload[sessionId] = false
          Message.error(err instanceof Error ? err.message : String(err))
          return null
        }
      }

      if (!streaming) {
        // 空闲:本地截断到目标之前,再从后端对齐一次
        this.messages[sessionId] = list.slice(0, msgIndex)
        if (session.backendId) void this.reloadHistory(sessionId)
      }
      // 流式:本地先不动(避免取消瞬间的 delta 写回残余消息);后端已截断,
      // 由 cancelled 收尾触发重载拿回干净历史。
      return captured
    },

    /** `/undo` 命令:撤回**最后一条**用户消息(与消息上的撤回按钮同路径)。 */
    async commandUndo(
      sessionId: string
    ): Promise<{ content: string; attachments?: MessageAttachment[] } | null> {
      const list = this.messages[sessionId] ?? []
      let lastIdx = -1
      for (let i = list.length - 1; i >= 0; i--) {
        if (list[i].role === 'user') {
          lastIdx = i
          break
        }
      }
      if (lastIdx < 0) {
        Message.warning(i18n.global.t('chat.undoEmpty'))
        return null
      }
      if (!this.canRetract(sessionId, list[lastIdx])) {
        Message.warning(i18n.global.t('chat.rewindDisabled'))
        return null
      }
      const ordinal = list.slice(0, lastIdx + 1).filter((m) => m.role === 'user').length - 1
      return this.rewindToUserMessage(sessionId, ordinal)
    },

    /**
     * 基于会话 fork 出新会话(共用工作区):调后端拿到新会话 id,按 backendId 去重后建本地会话
     * 并切换过去。`beforeFinal` = 在「无工具调用的助手消息」中 fork 在其之前的下标(0-based);
     * 缺省 = 最后一条(供 `/fork`)。
     */
    async forkConversation(sourceLocalId: string, beforeFinal?: number): Promise<void> {
      const src = this.sessions.find((s) => s.id === sourceLocalId)
      if (!src?.backendId || !window.api.chat?.forkSession) {
        Message.warning(i18n.global.t('chat.forkFailed'))
        return
      }
      try {
        const newBackendId = await window.api.chat.forkSession(src.backendId, beforeFinal)
        if (!newBackendId) {
          Message.error(i18n.global.t('chat.forkFailed'))
          return
        }
        // 后端同时广播 session_info;若事件先生成已加入,则复用,避免重复
        let target = this.sessions.find((s) => s.backendId === newBackendId)
        if (!target) {
          target = {
            id: newBackendId,
            title: `fork from ${localizedSessionTitle(src) || i18n.global.t('common.unnamed')}`,
            createdAt: Date.now(),
            updatedAt: Date.now(),
            kind: src.kind,
            sessionType: src.sessionType,
            workspace: src.workspace,
            model: src.model,
            backendId: newBackendId,
            pid: null,
            agentType: src.agentType ?? null
          }
          this.sessions.unshift(target)
          this.persist()
        }
        this.setActive(target.id)
        Message.success(i18n.global.t('chat.forked'))
      } catch (err) {
        Message.error(err instanceof Error ? err.message : String(err))
      }
    },

    /**
     * 手动摘要(`/compact`):调后端强制摘要当前会话(绕过阈值);完成后重载历史
     * (界面只剩「摘要 + 保留尾部」)。后端在生成中会拒绝(抛错)。
     */
    async summarizeConversation(sessionId: string): Promise<void> {
      const session = this.sessions.find((s) => s.id === sessionId)
      if (!session?.backendId || !window.api.chat?.summarizeSession) {
        Message.warning(i18n.global.t('chat.summarizeFailed'))
        return
      }
      const loading = Message.loading({ content: i18n.global.t('chat.summarizing'), duration: 0 })
      try {
        const done = await window.api.chat.summarizeSession(session.backendId)
        loading.close()
        if (done) {
          Message.success(i18n.global.t('chat.summarized'))
          await this.reloadHistory(sessionId)
        } else {
          Message.info(i18n.global.t('chat.nothingToSummarize'))
        }
      } catch (err) {
        loading.close()
        // 生成中 / 上下文过短等由后端拒绝：以警告提示（非故障）
        Message.warning(err instanceof Error ? err.message : i18n.global.t('chat.summarizeFailed'))
      }
    },

    /**
     * 用系统文件管理器打开会话目录(与后端 resolve_workspace 同口径):
     * 绝对路径工作区直接用;相对工作区(日期等)归入 `<用户目录>/workspace/` 下解析
     * (见 utils/workspace.resolveSessionWorkspaceDir)。
     */
    async openSessionFolder(session: Session): Promise<void> {
      const t = i18n.global.t
      const dir = await resolveSessionWorkspaceDir(session.workspace)
      if (!dir) {
        Message.info(t('chatMore.openDirNoPath'))
        return
      }
      // 浏览器部署没有系统文件管理器：改为提示路径（可复制），不报错
      if (isWebShell) {
        Message.info(`${t('chatMore.openDirPath')}: ${dir}`)
        return
      }
      const err = await window.api.fs.openFolder(dir)
      if (err) Message.error(`${t('chatMore.openDirFailed')}: ${err}`)
    },
    /**
     * 缓冲一条流式文本 delta(合帧):一帧最多写一次 store。
     * 缓冲持有目标消息对象引用,轮次切换/新占位不影响落点。
     */
    bufferStreamDelta(sessionId: string, msg: ChatMessage, text: string, reasoning: string): void {
      const buf = streamDeltaBuf.get(msg.id) ?? { sessionId, msg, text: '', reasoning: '' }
      if (text) buf.text += text
      if (reasoning) buf.reasoning += reasoning
      streamDeltaBuf.set(msg.id, buf)
      if (!streamDeltaRaf) {
        streamDeltaRaf = requestAnimationFrame(() => this.flushStreamDeltas())
      }
    },
    /** 把缓冲的流式 delta 一次性写回(合帧终点;非文本事件前也会同步调用以保证顺序) */
    flushStreamDeltas(): void {
      if (streamDeltaRaf) {
        cancelAnimationFrame(streamDeltaRaf)
        streamDeltaRaf = 0
      }
      if (!streamDeltaBuf.size) return
      const touched = new Set<string>()
      for (const buf of streamDeltaBuf.values()) {
        if (buf.text) buf.msg.content += buf.text
        if (buf.reasoning) {
          buf.msg.reasoning = (buf.msg.reasoning ?? '') + buf.reasoning
          buf.msg.reasoningStreaming = true
        }
        touched.add(buf.sessionId)
      }
      streamDeltaBuf.clear()
      for (const sid of touched) this.bumpStreamTick(sid)
    },
    /** 流式推进信号:面板据此自动滚动(浅监听,不做深遍历) */
    bumpStreamTick(sessionId: string): void {
      this.streamTick[sessionId] = (this.streamTick[sessionId] ?? 0) + 1
    },
    /** 会话列表持久化(节流):流式期间每 token 一次 JSON.stringify + localStorage 是卡死主因之一 */
    persistThrottled(): void {
      if (persistTimer != null) return
      persistTimer = setTimeout(() => {
        persistTimer = null
        this.persist()
      }, PERSIST_THROTTLE_MS)
    },
    /** 立即落盘(终态 / 标题变更等需要马上可见的场合),并取消待执行的节流 */
    flushPersist(): void {
      if (persistTimer != null) {
        clearTimeout(persistTimer)
        persistTimer = null
      }
      this.persist()
    },
    persist(): void {
      localStorage.setItem(SESSIONS_KEY, JSON.stringify(this.sessions))
    }
  }
})
