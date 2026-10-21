import type { ChatMessage, ToolCallView } from '@/types'

/**
 * 「一轮 = 一条用户消息 + 其后所有助手消息」的切分(对话区收纳块的输入)。
 *
 * 为什么按数组相邻关系切:一次发送只有一条用户消息,而助手侧每个 LLM 轮次
 * 都是一条独立消息(见 stores/sessions.ts 的 rotateRound),消息上没有 turn/step id。
 *
 * **性能约束(务必遵守)**:本模块会被 ChatPanel 的渲染 computed 调用,而流式正文
 * 是逐 token 追加到 `msg.content` / `msg.reasoning` 的。因此这里只允许读**结构性字段**
 * (role / id / tools.length / streaming / createdAt / startedAt / completedAt);一旦读 content 或
 * reasoning,每帧都会让整个消息列表重渲染(commit 5031717 的渲染隔离会被改回去)。
 * 正文与思考的分流通过 `parts` 交给子组件,不在这里判断内容。
 */

/** 一条消息要渲染的部分(收纳块内外拆分用) */
export type MessagePart = 'reasoning' | 'tools' | 'content'

/** 稳定引用:避免每次渲染传新数组字面量导致子组件 props 抖动 */
export const PART_FULL: MessagePart[] = ['reasoning', 'tools', 'content']
export const PART_REASONING: MessagePart[] = ['reasoning']
export const PART_CONTENT: MessagePart[] = ['content']
/** 流式中的末条消息:块内收「过程」(思考 + 已发起的工具),块外只留正文 */
export const PART_REASONING_TOOLS: MessagePart[] = ['reasoning', 'tools']

/** 收纳块内的一条步骤(整条消息 + 该渲染哪些部分) */
export interface TurnStep {
  msg: ChatMessage
  parts: MessagePart[]
}

/** 一轮对话的渲染描述 */
export interface TurnItem {
  /** v-for key:该轮用户消息 id(无用户消息时为该条助手消息 id) */
  id: string
  /** 该轮的用户消息(空消息头部的助手消息为 null → 不分组) */
  user: ChatMessage | null
  /** 不参与分组的独立消息(user 之前的助手消息),原样渲染 */
  /** 收纳块内的步骤 */
  steps: TurnStep[]
  /** 块外单独渲染的助手消息(结尾那条正文) */
  final: ChatMessage | null
  /** 块外消息的渲染部分(恒为正文:think 已收进块内) */
  finalParts: MessagePart[]
  /** 该轮是否已结束(→ 收纳块自动收起、表头耗时定格) */
  ended: boolean
  /** 该轮结束原因(正常完成 / 用户打断 / 出错);历史轮次无此信息 */
  endReason?: ChatMessage['endReason']
  /**
   * 该轮**处理起点**（收纳块耗时的被减数）：本轮各步骤里最早的 `startedAt`；
   * 一个都没有（老记录 / 尚无助手消息）时退回 user 消息时间，无 user 则取首条助手消息时间。
   */
  startedAt: number
  /** 该轮结束时间(实时打点或历史记录 metadata.ended_at) */
  endedAt?: number
}

/** 该轮是否在等用户决定:审批卡 / 提问表单 / 写邮件确认 */
export function pendingDecision(steps: TurnStep[]): 'approval' | 'form' | 'mail' | null {
  for (const step of steps) {
    for (const tool of step.msg.tools ?? []) {
      if (tool.interrupt?.status !== 'pending') continue
      if (tool.interrupt?.interrupt_type === 'mail_compose') return 'mail'
      return tool.name === 'chat_with_user' ? 'form' : 'approval'
    }
  }
  return null
}

/**
 * 是否有**正在等用户决定**的中断态(待审批 / 待答表单)→ 收纳块必须展开，且表头不给收起。
 *
 * 只认 pending：**已定论的拒绝不是「等你决定」**——沙箱硬拒（如 Plan 模式拒写）、
 * 用户点过拒绝、审批已决，都属于已经发生的失败，由工具卡自己的显性化（`needsAttention`，
 * 见 toolState.ts）承担展示，**不该把收纳块永久锁在展开态**。
 *
 * 曾经这里还判「失败且带 denial」，后果是：一次 Plan 拒写就让该轮表头「已处理 Xs」
 * 点了没反应（`ChatTurnGroup.toggle` 对 blocking 直接 return）——子会话里很容易撞上。
 */
export function hasBlockingInterrupt(steps: TurnStep[]): boolean {
  for (const step of steps) {
    for (const tool of step.msg.tools ?? []) {
      // `awaitingApproval` 是 store 的实时标志（中断事件置位、提交决议后清除）；
      // `interrupt.status === 'pending'` 覆盖实时事件与历史重建两条路径
      if (tool.awaitingApproval || tool.interrupt?.status === 'pending') return true
    }
  }
  return false
}

/**
 * 可**恢复**的表单中断终态：提问被忽略 / 被新消息取代 / 进程中断 / 落定失败。
 * `pending`（正在等用户填，卡片就在输入框位置）与 `resolved`（已作答）不在此列。
 */
const RESUMABLE_FORM_STATUS = new Set(['ignored', 'superseded', 'interrupted', 'failed'])

/**
 * 该轮里可「继续填写」的提问（`chat_with_user` 被放弃后仍可重新弹表单）。
 *
 * 恢复走 napi `reopenInterrupt`：后端用那条 tool 消息上的表单定义重新登记待决中断，
 * 表单卡照旧在输入框位置弹出（空白，不保留上次填写）。取**最后一条**——
 * 同一轮里被放弃的多次提问只恢复最近那次。
 *
 * 只读结构性字段（工具名 / 中断类型与状态 / 表单字段数），不读正文，遵守文件头的渲染隔离约定。
 */
export function resumableForm(steps: TurnStep[]): ToolCallView | null {
  let found: ToolCallView | null = null
  for (const step of steps) {
    for (const tool of step.msg.tools ?? []) {
      const interrupt = tool.interrupt
      if (!interrupt || interrupt.interrupt_type !== 'form') continue
      if (!RESUMABLE_FORM_STATUS.has(interrupt.status)) continue
      // 纯 tip 文本提问没有字段，重开也填不了 → 不给入口
      const fields = (interrupt.request as { form?: { fields?: unknown[] } } | undefined)?.form
        ?.fields
      if (!fields?.length) continue
      found = tool
    }
  }
  return found
}

/**
 * 该轮各步骤里**最早的处理起点**（`ChatMessage.startedAt`：后端 `metadata.started_at` 或
 * 实时占位打点）。缺 `startedAt` 的步骤（老记录 / 正文分片）跳过；一个都没有 → undefined。
 */
function earliestStepStart(steps: TurnStep[]): number | undefined {
  let min: number | undefined
  for (const step of steps) {
    const ts = step.msg.startedAt ?? 0
    if (ts > 0 && (min == null || ts < min)) min = ts
  }
  return min
}

/**
 * 把扁平消息列表切成轮次。
 *
 * 末条助手消息的分流规则(与产品约定一致):
 * - 流式中:正文留在块外(避免边读边跳),think 与**已发起的工具**收进块内
 *   (块内=过程、块外=输出;工具只有块内这一个落点,漏掉就整批看不见)
 * - 已结束且**没有 tool_call**:正文留在块外,think 收进块内
 * - 已结束但**有 tool_call**(取消/中断收尾):没有块外消息,整条留在块内
 *
 * 轮次起点（`startedAt`，收纳块耗时的被减数）取该轮最早的处理起点，而不是本轮 user 消息
 * 的 `createdAt`：看板「待办」是「仅创建」建出来的，指令可能隔几小时/几天才被「启动」，
 * 拿消息时间当起点会把等待时间算进耗时（显示成 41h）。缺 `startedAt`（老记录 / 正文分片）
 * 时才退回 user 消息时间 —— 实时发送的场景下两者本就同一时刻。
 */
export function groupTurns(messages: ChatMessage[]): TurnItem[] {
  const turns: TurnItem[] = []
  /** 当前累积的轮次;遇到下一条 user 消息或列表结束即封口 */
  let acc: TurnItem | null = null

  const seal = (): void => {
    if (!acc) return
    const last = acc.steps.length ? acc.steps[acc.steps.length - 1].msg : null
    if (last) {
      acc.ended = !last.streaming
      // 轮次结束时间取该轮所有步骤里最晚的一个:历史记录里个别轮次可能缺时间
      // (被打断的那一轮 ended_at 写 0,由后端字段兜底),只认末条会漏掉耗时
      let end = 0
      for (const step of acc.steps) {
        const ts = step.msg.completedAt ?? 0
        if (ts > end) end = ts
      }
      acc.endedAt = end > 0 ? end : undefined
      // 起点同样来自该轮各步骤（见函数头注释）；流式中也在走这条路 —— 末尾那轮
      // 每次渲染都会封口一次，所以「处理中 Xs」的起点当场就是对的，不用等收尾
      acc.startedAt = earliestStepStart(acc.steps) ?? acc.startedAt
      acc.endReason = last.endReason
      const hasTools = !!last.tools?.length
      // 流式中恒不收纳末条正文;结束后仅当末条无工具调用时留正文在外
      if (!acc.ended || !hasTools) {
        acc.final = last
        // 流式中的末条:think 与**已发起的工具**一起留在块内(块内=过程),只把正文留块外。
        // 工具若不给块内就再没有渲染落点:块外只渲染正文,工具卡会整批不可见——
        // 长任务(下载/构建)一跑就是几百秒,用户全程看不到正在跑什么,只能干等。
        acc.steps[acc.steps.length - 1] = {
          msg: last,
          parts: !acc.ended && hasTools ? PART_REASONING_TOOLS : PART_REASONING
        }
      }
    }
    turns.push(acc)
    acc = null
  }

  for (const msg of messages) {
    if (msg.role === 'user') {
      seal()
      acc = {
        id: msg.id,
        user: msg,
        steps: [],
        final: null,
        finalParts: PART_CONTENT,
        ended: true,
        startedAt: msg.createdAt
      }
      continue
    }
    // 助手消息:无前导用户消息(摘要重灌后的历史头部等)→ 开一个「无用户气泡」的收纳块,
    // 后续消息照常累积,与正常轮次共用封口 / 末条分流规则——统一走新收纳 UI
    // (单条卡片的老渲染路径已删除)
    if (!acc) {
      acc = {
        id: msg.id,
        user: null,
        steps: [],
        final: null,
        finalParts: PART_CONTENT,
        ended: true,
        startedAt: msg.startedAt ?? msg.createdAt
      }
    }
    acc.steps.push({ msg, parts: PART_FULL })
    if (!acc.startedAt) acc.startedAt = msg.createdAt
  }
  seal()
  return turns
}

/** 耗时文案:`41s` / `1m41s` / `1h02m`(纯数字格式,不走 i18n) */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (h > 0) return `${h}h${String(m).padStart(2, '0')}m`
  if (m > 0) return `${m}m${s}s`
  return `${s}s`
}

/**
 * 该轮是否以「非正常结束」收尾：用户打断 / 出错 / 进程中断（应用被杀）。
 * 这类轮次收纳块**不自动收起**——过程是排查依据，收起来等于把线索藏掉；
 * 正常完成（done 或历史记录无结束原因）才自动收起。
 */
export function isAbnormalEnd(ended: boolean, endReason?: string): boolean {
  return (
    ended && (endReason === 'cancelled' || endReason === 'error' || endReason === 'interrupted')
  )
}

/**
 * 已耗秒数:收纳块表头与底部状态行共用同一公式(两处数字必须一致)。
 * 起点/终点缺失或倒挂(历史记录没时间)时返回 null,由调用方决定兜底文案。
 */
export function elapsedSeconds(startedAt: number, endTs: number): number | null {
  if (!(startedAt > 0) || endTs < startedAt) return null
  return Math.floor((endTs - startedAt) / 1000)
}
