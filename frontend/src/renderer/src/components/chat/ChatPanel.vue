<script setup lang="ts">
import {
  computed,
  nextTick,
  onActivated,
  onBeforeUnmount,
  onDeactivated,
  onMounted,
  provide,
  ref,
  watch
} from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import type { SessionSettingsView } from '@/stores/sessions'
import { useTabsStore } from '@/stores/tabs'
import type { ChatMode } from '@/stores/chatPrefs'
import type { ChipMentionRef, QuoteRefPayload } from '@/components/mentions/types'
import { useSettingsStore } from '@/stores/settings'
import { usePluginsStore } from '@/stores/plugins'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { FILE_REF_BASE } from '@/utils/fileRefs'
import { elementCardFromBlock, findElementBlocks } from '@/utils/elementBlock'
import { useElementPickStore } from '@/stores/elementPick'
import { useQuotePickStore } from '@/stores/quotePick'
import { resolveSystemDark } from '@/theme'
import type { ChatMessage, DeliverableRow, MessageAttachment, PickedElementData } from '@/types'
import type { PendingInterrupt } from '@/stores/sessions'
import ChatInput from './ChatInput.vue'
import PendingQueue from './PendingQueue.vue'
import { collectTurnSources } from './sources'
import type { MemorySource, WebSource } from '@/types'
import ChatMessageItem from './ChatMessageItem.vue'
import ChatTurnGroup from './ChatTurnGroup.vue'
import TurnArtifacts from './TurnArtifacts.vue'
import TurnStatusBar from './TurnStatusBar.vue'
import { groupTurns, resumableForm, type TurnItem, type TurnStep } from './turnGrouping'
import type { TurnArtifacts as TurnArtifactsData, UserChatForm } from '@/types'
import ChatFormBar from './ChatFormBar.vue'
import MailComposeBar from './MailComposeBar.vue'
import ChatFormResume from './ChatFormResume.vue'
import ChatMessageRail from './ChatMessageRail.vue'
import ApprovalBar from './ApprovalBar.vue'

const props = defineProps<{
  /**
   * 要渲染的会话 id。缺省 = 当前激活会话（主对话页）。
   * 子会话页签传入具体会话 id：渲染它的对话、不切换激活会话。
   */
  sessionId?: string
  /**
   * 内嵌形态（子会话页签）：**无输入框、无规划胶囊**，但审批卡与提问表单照常可用
   * （它们替代输入框的位置，见底部区域）。
   */
  embedded?: boolean
}>()

const { t } = useI18n()
const sessions = useSessionsStore()
const settings = useSettingsStore()
const tabsStore = useTabsStore()
const plugins = usePluginsStore()
const elementPick = useElementPickStore()
const quotePick = useQuotePickStore()

/** 本面板渲染的会话：主对话页 = 激活会话；子会话页签 = 指定会话（只读渲染） */
const sid = computed(() => props.sessionId ?? sessions.activeId)
const embedded = computed(() => !!props.embedded)

const messages = computed(() => sessions.messages[sid.value] ?? [])

/** 本面板会话的工作区（交付物相对路径解析 / 审查页签聚焦用） */
const sessionWorkspace = computed(
  () => sessions.sessions.find((s) => s.id === sid.value)?.workspace ?? null
)

/**
 * 该轮的交付物表行（`submit_deliverables` 工具逐轮落库，对话结束后拉取）。
 * 按当轮用户消息 join：历史消息有 seq 精确匹配，直发消息回退文本匹配；无用户消息的轮次无行。
 */
function deliverableRowOf(turn: TurnItem): DeliverableRow | null {
  if (!turn.user) return null
  return sessions.deliverableRowOf(sid.value, turn.user.seq, turn.user.content) ?? null
}

/**
 * 取该轮的交付物 / 变更清单：优先交付物表行（新数据源，空清单不产出空卡片），
 * 回退末尾助手消息挂载的旧 `metadata.artifacts`（老会话历史）。
 * 正常完成时挂在块外的 `final`；以工具收尾（取消 / 中断）时退回到最后一步。
 */
function artifactsOf(turn: TurnItem): TurnArtifactsData | null {
  const row = deliverableRowOf(turn)
  if (row && (row.deliverables?.length || row.changedFiles?.length)) {
    return { deliverables: row.deliverables, changes: row.changedFiles }
  }
  if (turn.final?.artifacts) return turn.final.artifacts
  for (let i = turn.steps.length - 1; i >= 0; i--) {
    const a = turn.steps[i].msg.artifacts
    if (a) return a
  }
  return null
}

/** 空产物（仅「相关资料」时也要渲染底部入口行） */
const EMPTY_ARTIFACTS: TurnArtifactsData = { deliverables: [], changes: [] }

/**
 * 该轮末尾是否还有需要展示的入口：产物 / 变更 / 相关资料 / 相关记忆。
 * 只有交付物记录但两个计数都是 0、也没有资料时，不渲染整块尾部 UI。
 */
function hasTurnArtifacts(turn: TurnItem): boolean {
  const a = artifactsOf(turn)
  return (
    (a?.deliverables?.length ?? 0) > 0 ||
    (a?.changes?.length ?? 0) > 0 ||
    turnWebSources(turn).length > 0 ||
    turnMemorySources(turn).length > 0
  )
}

/** 该轮是否**正常结束**（历史轮次无 endReason 视为正常；处理中 / 取消 / 出错不显示底部入口） */
function turnDone(turn: TurnItem): boolean {
  return turn.ended && (turn.endReason === undefined || turn.endReason === 'done')
}

/**
 * 本轮步数 = 本轮 LLM 调用次数（一个助手消息 = 一次 LLM 请求）。
 * 一次 LLM 响应里发起多个工具调用仍只算 1 步；不重复计 `final`——它只是末条助手消息
 * 的渲染引用，同一条消息已在 `steps` 里（此前 `steps.length + (final?1:0)` 会多算 1，
 * 正是「1 条回复显示 2 轮」的根因）。
 */
function turnSteps(turn: TurnItem): number {
  return turn.steps.length
}

/**
 * 每轮对应的「第几次用户提问」序号（跨会话累计，1 起）。
 * 只给带用户消息的轮次编号（历史头部孤立的助手消息没有锚点）。
 */
const turnNumbers = computed(() => {
  const numbers = new Map<string, number>()
  let n = 0
  for (const turn of turns.value) {
    if (turn.user) numbers.set(turn.id, ++n)
  }
  return numbers
})

/**
 * 本轮检索结果（「相关资料」/「相关记忆」入口）。
 * 按 `turn.steps` 数组缓存拆分结果：一次扫描、稳定引用，避免模板里多次调用重复扫描。
 */
const sourceSplitCache = new WeakMap<TurnStep[], { web: WebSource[]; memory: MemorySource[] }>()
function sourceSplit(turn: TurnItem): { web: WebSource[]; memory: MemorySource[] } {
  const hit = sourceSplitCache.get(turn.steps)
  if (hit) return hit
  const all = collectTurnSources(turn.steps)
  const split = {
    web: all.filter((s): s is WebSource => s.kind === 'web'),
    memory: all.filter((s): s is MemorySource => s.kind === 'memory')
  }
  sourceSplitCache.set(turn.steps, split)
  return split
}
/** 本轮网页检索结果（「相关资料」入口） */
function turnWebSources(turn: TurnItem): WebSource[] {
  return sourceSplit(turn).web
}
/** 本轮记忆检索结果（「相关记忆」入口） */
function turnMemorySources(turn: TurnItem): MemorySource[] {
  return sourceSplit(turn).memory
}

/**
 * 一轮 = 一条用户消息 + 其后所有助手消息：中间过程(think/工具/中间叙述)收进 ChatTurnGroup，
 * 结尾那条正文留在块外。分组只读结构性字段，正文/思考的逐 token 追加不会重算这里
 * （渲染隔离约定见 components/chat/turnGrouping.ts 文件头注释）。
 */
const turns = computed(() => groupTurns(messages.value))

/** 打开会话时默认只渲染最近 N 轮用户对话（更早的经「加载更早」逐轮展开） */
const DEFAULT_VISIBLE_TURNS = 2
/** 当前窗口可见的用户轮次数（只收敛渲染范围；消息本身已全量在 store，流式/轨线仍按全量工作） */
const visibleTurnCount = ref(DEFAULT_VISIBLE_TURNS)

/** turns 里「带用户消息」的轮次总数（历史头部孤立的助手消息不计） */
const userTurnCount = computed(() => turns.value.reduce((n, turn) => n + (turn.user ? 1 : 0), 0))

/**
 * 渲染窗口起点：只显示最后 `visibleTurnCount` 个用户轮次（不足则全部）。
 * 取该区间的用户轮次下标 —— 从这一轮开始到列表末尾，避免把某一轮从中间切开。
 */
const windowStartIndex = computed(() => {
  const total = userTurnCount.value
  const show = Math.max(1, visibleTurnCount.value)
  if (total <= show) return 0
  const target = total - show
  let seen = 0
  for (let i = 0; i < turns.value.length; i++) {
    if (!turns.value[i].user) continue
    if (seen === target) return i
    seen++
  }
  return 0
})

/** 实际渲染的轮次（最后 N 轮用户对话 + 其间的助手消息） */
const visibleTurns = computed(() => turns.value.slice(windowStartIndex.value))
/** 顶部是否还有更早的轮次可加载 */
const hasEarlierTurns = computed(() => windowStartIndex.value > 0)

/**
 * 每轮里「可继续填写」的提问（被忽略 / 被取代 / 被中断的 chat_with_user）。
 *
 * 与 turns 同源同类：同下标一一对应，供模板在收纳块下方渲染恢复入口。
 * 判定只读结构性字段（见 resumableForm），逐 token 追加的正文不会让它重算。
 */
const turnResumables = computed(() => turns.value.map((turn) => resumableForm(turn.steps)))

/**
 * 左侧快捷轨的数据源：只保留「带用户消息的轮次」（历史头部孤立的助手消息没有锚点）。
 * 与可见窗口同源（隐藏的轮次没有 DOM 锚点，留在轨上会导致点不到）。
 * 只读结构性字段（`turn.user` 是否存在 / id），不读正文——逐 token 追加不会让它重算。
 */
const railTurns = computed(() => visibleTurns.value.filter((turn) => turn.user))

/** 当前视口所在的那条用户消息（轨线高亮；-1 = 还没滚过任何一条） */
const railActive = ref(-1)

/**
 * 当前会话的「上一轮结束原因」:由后端会话状态回填(taskFlags.end),
 * 冷启动按后端记录恢复 —— 用于让「工作已暂停，用户主动打断」在重启后仍能显示。
 * 实时轮次优先用自己的 endReason,这里只作历史轮次的兜底(见 TurnStatusBar)。
 */
const activeSessionEnd = computed(() => sessions.taskFlags[sid.value]?.end)

/** 最新一轮(状态行只服务它)。有步骤 → 显示;无步骤但在流式中(刚发出、等待首个回复)
 *  也显示——此时状态行承载「正在等待模型回复」的流光相位文案 */
const statusTurn = computed(() => {
  const turn = turns.value[turns.value.length - 1]
  if (!turn) return null
  if (turn.steps.length) return turn
  return !turn.ended && sessions.runPhase[sid.value] ? turn : null
})

/**
 * 当前会话工作区绝对目录(@ 提及的文件列表数据源):
 * 会话记录里可能是相对工作区(日期目录),按后端同口径解析为绝对路径(见 utils/workspace)
 */
const mentionDir = ref('')
watch(
  () => sessions.sessions.find((s) => s.id === sid.value)?.workspace,
  async (ws) => {
    mentionDir.value = await resolveSessionWorkspaceDir(ws)
  },
  { immediate: true }
)

/**
 * 正文里的**文件引用**（`` `auth.py:47` `` → 可点击 chip）按会话工作区解析相对路径
 * （见 utils/fileRefs 的 FILE_REF_BASE；注入 ref：工作区是异步解析出来的）。
 * 注入在本面板，故正文、过程步骤、思考卡片里的引用同一套解析。
 */
provide(FILE_REF_BASE, mentionDir)

// markdown 暗色模式跟随应用主题
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

// ---------- 自动滚动 ----------
const messagesRef = ref<HTMLElement | null>(null)
const autoScroll = ref(true)
const showScrollBtn = ref(false)
/** 待执行的合帧滚动句柄(0 = 无) */
let scrollRaf = 0

/**
 * 依据当前滚动位置刷新「向下按钮 / 是否跟随底部」。
 * 不只在 scroll 事件里算：容器高度（输入框 ↔ 审批卡/表单卡切换）或内容高度变化时
 * 不会触发 scroll，若不在这些时机重算，`showScrollBtn` 会停留在旧值——
 * 典型表现：内容很短、根本没得滚，却仍显示「向下」。
 */
function refreshScrollState(): void {
  const el = messagesRef.value
  if (!el) return
  const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60
  showScrollBtn.value = !nearBottom
  autoScroll.value = nearBottom
}

/**
 * 合帧滚动到底部:流式期间每 token 一次「读 scrollHeight(强制重排)+ 写 scrollTop」
 * 是主线程的主要开销之一,这里同一帧内多次请求只执行一次。
 */
function scrollToBottom(): void {
  autoScroll.value = true
  if (scrollRaf) return
  scrollRaf = requestAnimationFrame(() => {
    scrollRaf = 0
    const el = messagesRef.value
    if (el) el.scrollTop = el.scrollHeight
    // 已到底 → 收掉「向下」按钮(此前只在 scroll 事件里更新，程序化滚动可能不触发）
    refreshScrollState()
  })
}

/**
 * 「加载更早」：把可见窗口多展开一轮用户对话，并补偿滚动位置。
 *
 * 新内容插在**顶部**，浏览器会保持 scrollTop 不变 → 视口内容整体下移（看着像跳了）。
 * 这里记住展开前后高度差，下一帧把 scrollTop 加上差值，视口停留在原内容上。
 * 置 `autoScroll = false`：展开更早内容与「跟随底部」相悖（下一次滚动事件会按实际位置刷新）。
 */
async function loadEarlierTurns(): Promise<void> {
  const expandingLocal = hasEarlierTurns.value
  if (!expandingLocal && !(hasSummary.value && archivedMore.value)) return
  const el = messagesRef.value
  const prevHeight = el?.scrollHeight ?? 0
  const prevTop = el?.scrollTop ?? 0
  autoScroll.value = false
  if (expandingLocal) {
    // 先展开本地窗口(已装载的更早轮次)
    visibleTurnCount.value += 1
  } else {
    // 本地已尽:从摘要备份段取更早一批原始消息(5 个用户轮次)前插,并把窗口同步扩到新内容
    if (archivedLoading.value) return
    archivedLoading.value = true
    try {
      const added = await sessions.loadEarlierArchived(sid.value)
      if (added > 0) visibleTurnCount.value += added
      else archivedMore.value = false
    } finally {
      archivedLoading.value = false
    }
  }
  nextTick(() => {
    const node = messagesRef.value
    if (node) node.scrollTop = prevTop + (node.scrollHeight - prevHeight)
    updateRailActive()
  })
}

function handleScroll(): void {
  const el = messagesRef.value
  if (!el) return
  savedScrollTop.value = el.scrollTop
  // 上滑看历史:出现向下按钮并暂停自动跟随;回到底部后恢复跟随
  refreshScrollState()
  updateRailActive()
  hideQuoteBtn()
}

/** 轨线高亮的判定余量：消息顶端进入容器顶部 40px 内仍算「当前这一条」 */
const RAIL_ACTIVE_OFFSET = 40

/**
 * 视口所在的那条用户消息（轨线高亮）：取「顶端已滚过容器顶部（留 40px 余量）的最后一条」。
 * 只读 DOM 锚点（每轮一个），值不变不写 ref——避免每次滚动都触发轨线重渲染。
 */
function updateRailActive(): void {
  const el = messagesRef.value
  if (!el) return
  const anchors = el.querySelectorAll<HTMLElement>('[data-rail-anchor]')
  if (!anchors.length) {
    if (railActive.value !== -1) railActive.value = -1
    return
  }
  const top = el.getBoundingClientRect().top
  const ids = railTurns.value.map((turn) => turn.user?.id)
  let next = -1
  anchors.forEach((node) => {
    const i = ids.indexOf(node.dataset.railAnchor)
    if (i < 0) return
    if (node.getBoundingClientRect().top - top <= RAIL_ACTIVE_OFFSET) next = Math.max(next, i)
  })
  if (next !== railActive.value) railActive.value = next
}

/**
 * 点轨线 → 平滑滚到那条用户消息（消息顶部贴齐容器顶部并留 12px 空隙）。
 * 置 `autoScroll = false`：流式自动跟随会把视口拽回底部，与「回看历史」相悖
 * （下一次 scroll 事件仍会按实际位置刷新 autoScroll / 「向下」按钮）。
 */
function onRailJump(id: string): void {
  const el = messagesRef.value
  const target = el?.querySelector<HTMLElement>(`[data-rail-anchor="${CSS.escape(id)}"]`)
  if (!el || !target) return
  autoScroll.value = false
  const top =
    target.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop - 12
  el.scrollTo({ top: Math.max(0, top), behavior: 'smooth' })
}

// KeepAlive 保活:设置往返时 DOM 不被重建,但 Chromium 重新挂载会复位 scrollTop;
// 离开时(onDeactivated)捕获位置,重新激活(返回聊天)后恢复
// KeepAlive 保活:设置往返时 DOM 不被重建,但 Chromium 摘除/再挂载子树会复位 scrollTop。
// 滚动事件持续记录位置(handleScroll);离场时再取一次,与已有值取较大者(摘除瞬间可能已被复位为 0,
// 不能覆盖已记录的真实位置);重新激活(返回聊天)后恢复。
const savedScrollTop = ref(0)
onActivated(() => {
  const target = savedScrollTop.value
  if (target <= 0) return
  nextTick(() => {
    if (messagesRef.value) messagesRef.value.scrollTop = target
  })
})
onDeactivated(() => {
  if (messagesRef.value) savedScrollTop.value = Math.max(savedScrollTop.value, messagesRef.value.scrollTop)
})

// 新消息 / 流式追加时自动滚到底部(用户上翻时暂停)。
// 监听 store 的 streamTick(浅信号)而非对 messages 做 deep 监听——后者每个 delta 都要
// 深遍历整棵消息对象图(含每个工具的 params/result),是流式卡死的主因之一。
watch(
  () => sessions.streamTick[sid.value] ?? 0,
  () => {
    if (autoScroll.value) scrollToBottom()
  }
)

// 切换会话 → 回到该会话最新消息（旧行为由 messages 深监听承担；仍尊重「已上翻暂停跟随」）；
// 可见窗口复位为「最近 N 轮用户对话」，更早内容重新经「加载更早」展开。
/** 归档分页:本地窗口展开到尽头后,「加载更早」继续从摘要备份段按批次取更早内容 */
const archivedMore = ref(true)
const archivedLoading = ref(false)
/** 该会话是否发生过摘要(无摘要则没有归档可翻,按钮不显示) */
const hasSummary = computed(() => (sessions.sessionInfo[sid.value]?.summaryTimes ?? 0) > 0)
watch(sid, () => {
  visibleTurnCount.value = DEFAULT_VISIBLE_TURNS
  archivedMore.value = true
  archivedLoading.value = false
  if (autoScroll.value) scrollToBottom()
})

// 消息数量变化（首次灌入历史 / 新增一条用户消息）→ 刷新轨线高亮 + 滚动状态。
// 不依赖滚动事件：容器已在底部时不再触发 scroll，高亮/按钮会停在旧值。
watch(
  () => messages.value.length,
  () =>
    nextTick(() => {
      updateRailActive()
      refreshScrollState()
    })
)

// 滚动容器尺寸变化（输入框 ↔ 审批卡/表单卡切换、窗口缩放等）→ 重算滚动状态。
// 这类变化不触发 scroll 事件，若不重算，「向下」按钮会停留在旧值（短内容也显示）。
let scrollAreaObserver: ResizeObserver | null = null
/** 把消息区高度写进 CSS 变量：过程收纳块的展开限高按它的 2/5 取（见 ChatTurnGroup .steps-body） */
function publishChatAreaHeight(): void {
  const el = messagesRef.value
  if (el) el.style.setProperty('--chat-area-h', `${el.clientHeight}px`)
}
onMounted(() => {
  if (!messagesRef.value) return
  scrollAreaObserver = new ResizeObserver(() => {
    refreshScrollState()
    publishChatAreaHeight()
  })
  scrollAreaObserver.observe(messagesRef.value)
  publishChatAreaHeight()
})
onBeforeUnmount(() => {
  scrollAreaObserver?.disconnect()
  scrollAreaObserver = null
})


// ---------- 中断(安全审批 / 提问):同一机制、两种卡片,在底部替代发送框 ----------
/** 当前会话待决中断(审批卡优先于提问表单) */
const pendingInterrupt = computed<PendingInterrupt | null>(
  () => sessions.pendingInterrupts[sid.value] ?? null
)
/** 待审批中断(展示字段取信封 approval 段) */
const pendingApproval = computed<PendingInterrupt | null>(() =>
  pendingInterrupt.value?.interrupt_type === 'approval' ? pendingInterrupt.value : null
)
/** 待回答的提问表单(载荷取信封 form 段) */
const pendingForm = computed<UserChatForm | null>(() =>
  pendingInterrupt.value?.interrupt_type === 'form' ? (pendingInterrupt.value.request.form ?? null) : null
)
/**
 * 待确认发送的邮件草稿(只读预览)。
 *
 * 信封刻意只带 `attachment_meta`（附件名/大小），收件人/主题/正文/附件路径都在
 * 本次 tool 调用的参数里 → 按 `tool_call_id` 取回工具卡参数还原完整草稿。
 */
const pendingMailCompose = computed(() => {
  const pi = pendingInterrupt.value
  if (pi?.interrupt_type !== 'mail_compose') return null
  const params = (sessions.toolById(sid.value, pi.tool_call_id)?.params ?? {}) as Record<
    string,
    unknown
  >
  const s = (k: string): string => (typeof params[k] === 'string' ? (params[k] as string) : '')
  const attachments = Array.isArray(params.attachments)
    ? (params.attachments as unknown[]).filter((x): x is string => typeof x === 'string')
    : []
  return {
    to: s('to'),
    cc: s('cc'),
    bcc: s('bcc'),
    subject: s('subject'),
    body: s('body'),
    html: s('html'),
    attachments,
    attachment_meta: pi.request.mail_compose?.attachment_meta ?? []
  }
})

/** 提问表单提交:表单值交后端写入该次 tool 结果(决议即 tool 结果,不追加用户消息) */
function onChatFormSubmit(dataJson: string): void {
  void sessions.submitInterrupt({ answer: dataJson }, sid.value)
}

/** 忽略提问:落成决议(后端标记 ignored 且不续跑),状态不悬挂、重启不复现 */
function onChatFormIgnore(): void {
  void sessions.submitInterrupt({ ignored: true }, sid.value)
}

/** 确认发送邮件:后端在决议处理时执行 mail.send(answer 透传草稿,前端不可编辑) */
function onMailComposeSend(): void {
  const draft = pendingMailCompose.value
  if (!draft) return
  void sessions.submitInterrupt({ answer: JSON.stringify(draft) }, sid.value)
}

/** 取消发送邮件:落成决议(后端写「用户取消」并续跑,让模型知道) */
function onMailComposeCancel(): void {
  void sessions.submitInterrupt({ ignored: true }, sid.value)
}

/** 当前会话的错误提示(显示在聊天框上方,不占内容区):
 *  仅由后端顶层 error 事件写入(工具失败不写,错误已回给模型),随 done 自动取消 */
const currentError = computed<string | null>(() => sessions.sessionError[sid.value] ?? null)

/** 当前会话的瞬时提示(信息级,与错误条同一位置;目前用于「流中断,正在自动重试」) */
const currentNotice = computed<string | null>(() => sessions.sessionNotice[sid.value] ?? null)

function onSend(payload: {
  text: string
  attachments?: MessageAttachment[]
  mode?: ChatMode
  goal?: string
  goalLang?: string
  mentions?: ChipMentionRef[]
  elements?: string[]
  officeRefs?: string[]
  quotes?: QuoteRefPayload[]
}): void {
  // 会话设置(模式/权限/思考强度/模型/供应商)不随消息下发:它们是会话属性,
  // 由输入框经 sessions.setSessionSettings 立即改写;这里只发文本与附件。
  // 例外:mode(/plan)、goal(/goal)、mentions/elements(引用元数据)随本条消息下发。
  sessions.sendMessage(payload.text, {
    attachments: payload.attachments,
    mode: payload.mode,
    goal: payload.goal,
    goalLang: payload.goalLang,
    mentions: payload.mentions,
    elements: payload.elements,
    officeRefs: payload.officeRefs,
    quotes: payload.quotes
  })
}

/** 当前会话的设置(唯一读路径:本地草稿 → 后端会话记录 → 新建默认) */
const sessionSettings = computed(() => sessions.settingsOf(sid.value))

/** 输入框改设置:立即下发到该会话(不必发消息),后端下一个工具调用即生效 */
function onSettingsChange(patch: Partial<SessionSettingsView>): void {
  sessions.setSessionSettings(sid.value, patch)
}

function onStop(): void {
  sessions.stopGeneration()
}

// 当前会话是否正在生成:任务进行中发送按钮切换为红色暂停按钮
const isStreaming = computed(() => sessions.isStreamingSession(sid.value))

// 前端消息队列:会话忙时发送的消息在此排队(拖拽排序 / 立即 / 编辑 / 取消)
const pendingItems = computed(() => sessions.pendingQueues[sid.value] ?? [])

// 编辑队列消息:从队列移除,内容回显到发送框;再次发送时(会话仍忙)重新入队
const chatInputRef = ref<InstanceType<typeof ChatInput> | null>(null)
function onQueueEdit(payload: { id: string; text: string }): void {
  sessions.cancelPending(sid.value, payload.id)
  chatInputRef.value?.setDraft(payload.text)
}

// ---------- 撤回用户消息 ----------
/** 该用户消息是否可撤回(摘要边界之前的不可撤回;内嵌只读页签不提供) */
function canRetract(msg: ChatMessage): boolean {
  return !embedded.value && sessions.canRetract(sid.value, msg)
}

/** 撤回后回填发送框:提及 chip(setDraft)+ 附件(setAttachments)+ 元素拾取(降级回推) */
function applyRestoredDraft(restored: {
  content: string
  attachments?: MessageAttachment[]
}): void {
  chatInputRef.value?.setDraft(restoreElementPicks(restored.content))
  chatInputRef.value?.setAttachments(restored.attachments ?? [])
}

/** 点击某条用户消息的撤回:截断到它之前并回填 */
async function onUndoUser(msg: ChatMessage): Promise<void> {
  const list = sessions.messages[sid.value] ?? []
  const idx = list.findIndex((m) => m.id === msg.id)
  if (idx < 0) return
  const ordinal = list.slice(0, idx + 1).filter((m) => m.role === 'user').length - 1
  if (ordinal < 0) return
  const restored = await sessions.rewindToUserMessage(sid.value, ordinal)
  if (restored) applyRestoredDraft(restored)
}

/** 输入框控制命令:undo=撤回最后一条用户消息;fork=派生新会话;compact=生成摘要 */
async function onCommand(id: string): Promise<void> {
  if (id === 'undo') {
    const restored = await sessions.commandUndo(sid.value)
    if (restored) applyRestoredDraft(restored)
    return
  }
  if (id === 'fork') {
    await sessions.forkConversation(sid.value)
    return
  }
  if (id === 'compact') {
    await sessions.summarizeConversation(sid.value)
  }
}

/** 该助手消息在「无工具调用的助手消息」中的序号(0-based,排除摘要);找不到回退最后一个 */
function forkIndexOfFinal(msg: ChatMessage): number {
  const list = sessions.messages[sid.value] ?? []
  let k = -1
  for (const m of list) {
    if (m.role !== 'assistant' || m.summary || m.tools?.length) continue
    k++
    if (m.id === msg.id) return k
  }
  return k
}

/** 在某条最终助手消息处 fork(继承其之前的消息,保留该轮的中间工具步骤) */
async function onForkFinal(msg: ChatMessage): Promise<void> {
  await sessions.forkConversation(sid.value, forkIndexOfFinal(msg))
}

// 元素拾取回显:消息里只剩 ```element 围栏文本,按现有 i18n 反查字段 key 重建最小悬停卡片
function elementDataFromBlock(body: string): PickedElementData {
  return {
    url: '',
    tagName: '',
    id: '',
    classList: [],
    xpath: '',
    cssSelector: '',
    text: '',
    card: elementCardFromBlock(body, t)
  }
}

/** 从正文剥离 ```element 围栏 → 回推到 elementPick(降级 chip),返回剩余文本 */
function restoreElementPicks(content: string): string {
  const blocks = findElementBlocks(content)
  if (!blocks.length) return content
  let out = ''
  let last = 0
  for (const b of blocks) {
    out += content.slice(last, b.start)
    elementPick.push({ data: elementDataFromBlock(b.body), token: content.slice(b.start, b.end) })
    last = b.end
  }
  out += content.slice(last)
  return out.replace(/\n{3,}/g, '\n\n').trim()
}

// ---------- 对话正文选区 → 「添加到当前任务」(引用) ----------
interface QuoteBtnState {
  x: number
  y: number
  text: string
  messageId: string
  seq?: number
  role: 'assistant' | 'user'
}
const quoteBtn = ref<QuoteBtnState | null>(null)
const quoteBtnStyle = computed(() =>
  quoteBtn.value ? { left: `${quoteBtn.value.x}px`, top: `${quoteBtn.value.y}px` } : {}
)

function hideQuoteBtn(): void {
  quoteBtn.value = null
}

/** 在消息正文里松开鼠标:若形成选区则在其上方浮出「添加到当前任务」 */
function onMessagesMouseUp(): void {
  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || !sel.rangeCount) {
    hideQuoteBtn()
    return
  }
  const text = sel.toString().replace(/\s+/g, ' ').trim()
  const listEl = messagesRef.value
  const anchor = sel.anchorNode
  const anchorEl =
    anchor?.nodeType === Node.ELEMENT_NODE ? (anchor as Element) : anchor?.parentElement
  if (!text || !listEl || !anchorEl || !listEl.contains(anchorEl)) {
    hideQuoteBtn()
    return
  }
  const row = anchorEl.closest('.msg-row[data-message-id]') as HTMLElement | null
  if (!row) {
    hideQuoteBtn()
    return
  }
  const rect = sel.getRangeAt(sel.rangeCount - 1).getBoundingClientRect()
  quoteBtn.value = {
    x: Math.min(Math.max(8, rect.right - 60), window.innerWidth - 160),
    y: Math.max(8, rect.top - 38),
    text,
    messageId: row.dataset.messageId ?? '',
    seq: row.dataset.messageSeq ? Number(row.dataset.messageSeq) : undefined,
    role: row.dataset.messageRole === 'user' ? 'user' : 'assistant'
  }
}

/** 把当前选区加入引用(对话框上方出现「N 条对话引用」胶囊 + 编辑器内联 chip) */
function addQuote(): void {
  const q = quoteBtn.value
  if (!q) return
  quotePick.push({ text: q.text, messageId: q.messageId, seq: q.seq, role: q.role })
  hideQuoteBtn()
  window.getSelection()?.removeAllRanges()
}

/** 在别处按下鼠标即隐藏浮动按钮(mousedown 会清选区,故先收起) */
function onDocMouseDown(e: MouseEvent): void {
  const target = e.target as Element | null
  if (target?.closest('.quote-add-btn')) return
  hideQuoteBtn()
}

onMounted(() => document.addEventListener('mousedown', onDocMouseDown, true))
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocMouseDown, true))

// 插件 SDK chat.appendInput:把文本填入发送框(复用队列编辑的回显路径,不自动发送)
watch(
  () => plugins.injectedInput,
  (injected) => {
    if (embedded.value) return
    if (injected?.text) {
      nextTick(() => chatInputRef.value?.setDraft(injected.text))
    }
  }
)

// ---------- 相关入口(计划 / 子会话)已迁到对话框工具行的 TaskPlanButtons ----------
/** 当前会话（子会话按它的 backendId 归集） */
const activeSession = computed(() => sessions.sessions.find((s) => s.id === sid.value))

// 子会话创建后，把对应的「未开始」页签原地升级为实时子会话页签
// （只依赖子会话集合的签名，避免随消息逐 token 触发）
watch(
  () => sessions.childSessionsOf(activeSession.value?.backendId).map((s) => s.id).join(','),
  () => {
    const backendId = activeSession.value?.backendId
    if (!backendId) return
    tabsStore.upgradePendingTabs(
      backendId,
      sessions.childSessionsOf(backendId).map((s) => ({ id: s.id, title: localizedSessionTitle(s) }))
    )
  }
)

/**
 * 内嵌形态（子会话页签）：把**指定会话**的历史灌进 store 的 per-session 消息表。
 *
 * `loadHistory` 本身就按参数会话读取（不依赖 activeId），已加载过/正在流式时会自行跳过；
 * 需要重复时（切页签回来）也没关系——它只在未加载时真正拉取。
 */
function ensureSessionHistory(): void {
  if (!props.sessionId) return
  void sessions.loadHistory(props.sessionId)
}
onMounted(ensureSessionHistory)
watch(() => props.sessionId, ensureSessionHistory)
</script>

<template>
  <section class="flex h-full min-w-0 flex-col chat-section">
    <!-- 消息区(滚动容器):轨线是它的兄弟节点(见文件末尾的 ChatMessageRail),
         因为滚动容器里的绝对定位元素会跟着滚动内容一起走 -->
    <div
      ref="messagesRef"
      class="chat-messages"
      @scroll="handleScroll"
      @mouseup="onMessagesMouseUp"
    >
      <!-- 空态 -->
      <div v-if="!messages.length" class="empty-wrap">
        <span
          class="flex h-14 w-14 items-center justify-center rounded-2xl bg-[var(--color-primary-light-1)] text-2xl text-[var(--color-primary-6)]"
        >
          <icon-message />
        </span>
        <div class="text-base font-medium text-[var(--text-primary)]">{{ t('chat.emptyTitle') }}</div>
        <div class="max-w-xs text-sm text-[var(--text-tertiary)]">{{ t('chat.emptyDesc') }}</div>
      </div>

      <!-- 消息列表(与输入框共享宽度体系):按「轮」渲染,中间过程收进可折叠的收纳块 -->
      <div v-else class="msg-container chat-column">
        <!-- 顶部居中「加载更早」:初次只渲染最近 N 轮用户对话,点击逐轮向前展开 -->
        <button
          v-if="hasEarlierTurns || (hasSummary && archivedMore)"
          type="button"
          class="load-earlier-btn"
          :disabled="archivedLoading"
          @click="loadEarlierTurns"
        >
          <icon-loading v-if="archivedLoading" :size="12" class="load-earlier-spin" />
          {{ t('chat.loadEarlier') }}
        </button>
        <template v-for="(turn, ti) in visibleTurns" :key="turn.id">
          <ChatMessageItem
            v-if="turn.user"
            :msg="turn.user"
            :is-dark="isDark"
            :can-undo="canRetract(turn.user)"
            @undo="onUndoUser(turn.user)"
          />
          <ChatTurnGroup
            v-if="turn.steps.length"
            :steps="turn.steps"
            :is-dark="isDark"
            :ended="turn.ended"
            :end-reason="turn.endReason"
            :session-end="ti === visibleTurns.length - 1 ? activeSessionEnd : undefined"
            :started-at="turn.startedAt"
            :ended-at="turn.endedAt"
            :expand-no-final="!turn.final"
            :phase="ti === visibleTurns.length - 1 ? sessions.runPhase[sid] : undefined"
          />
          <!-- 块外正文(结尾那条没有 tool_call 的助手消息) -->
          <ChatMessageItem
            v-if="turn.final"
            :msg="turn.final"
            :is-dark="isDark"
            :parts="turn.finalParts"
            :can-fork="!embedded"
            :turn-no="turnNumbers.get(turn.id) ?? 0"
            :turn-steps="turnSteps(turn)"
            @fork="onForkFinal(turn.final)"
          />
          <!-- 本轮交付物 / 变更清单 / 相关资料 / 相关记忆（**正常结束后**才显示，作为该轮最后一行） -->
          <TurnArtifacts
            v-if="turnDone(turn) && hasTurnArtifacts(turn)"
            :artifacts="artifactsOf(turn) ?? EMPTY_ARTIFACTS"
            :sources="turnWebSources(turn)"
            :memory="turnMemorySources(turn)"
            :workspace="sessionWorkspace"
          />
          <!-- 被放弃的提问(忽略 / 被取代 / 被中断):该轮末尾单独一行给「继续填写表单」入口。
               放在收纳块外,故不受「已处理 Xs」折叠影响;点击后表单卡弹回输入框位置。

               **只给最新一轮**：用户已经往下走了(发了下一条消息、开了新的一轮)时，
               上一轮那个被忽略的提问就不该再挂着一个「继续填写表单」——它已经过期，
               点它会把表单弹回输入框、打断当前这轮。 -->
          <ChatFormResume
            v-if="turnResumables[windowStartIndex + ti] && ti === visibleTurns.length - 1"
            :tool="turnResumables[windowStartIndex + ti]!"
            :session-id="sid"
          />
        </template>
      </div>
    </div>
    <!-- 用户消息快捷轨:落在消息区左侧留白里(窄对话区由容器查询隐藏),点击滚到那条 -->
    <ChatMessageRail :turns="railTurns" :active-index="railActive" @jump="onRailJump" />
    <!-- 状态行 + 「向下」按钮:同一行(状态文字靠左,胶囊居中);
         两者都没有时整行不占高度,不做无谓留白 -->
    <div class="chat-column status-row">
      <span class="status-row-text">
        <TurnStatusBar
          v-if="statusTurn"
          :steps="statusTurn.steps"
          :ended="statusTurn.ended"
          :started-at="statusTurn.startedAt"
          :ended-at="statusTurn.endedAt"
          :phase="statusTurn.steps.length ? undefined : sessions.runPhase[sid]"
        />
      </span>
      <button v-show="showScrollBtn" class="scroll-bottom-btn" @click="scrollToBottom">
        <icon-down :size="14" />
        <span class="scroll-bottom-label">{{ t('chat.down') }}</span>
      </button>
    </div>

    <!-- 错误提示:显示在聊天框上方(macOS 材质卡片),不占内容区 -->
    <div v-if="currentError" class="chat-column chat-error-wrap">
      <div class="chat-error-bar" role="alert">
        <span class="chat-error-icon"><icon-exclamation-circle :size="12" /></span>
        <span class="chat-error-text">{{ currentError }}</span>
      </div>
    </div>

    <!-- 瞬时提示(信息级):如「生成中断,正在自动重试」——用户知道内容为何消失 -->
    <div v-else-if="currentNotice" class="chat-column chat-error-wrap">
      <div class="chat-error-bar chat-notice-bar" role="status">
        <span class="chat-notice-icon"><icon-loading :size="12" /></span>
        <span class="chat-error-text">{{ currentNotice }}</span>
      </div>
    </div>

    <!-- 底部:有待答表单时表单替代发送框,忽略/提交后恢复发送框 -->
    <div class="chat-column chat-input-wrap">
      <!-- 待审批卡:显示在输入框上方(有审批时压过表单/输入) -->
      <ApprovalBar v-if="pendingApproval" :session-id="sid" :interrupt="pendingApproval" />
      <ChatFormBar
        v-else-if="pendingForm"
        :form="pendingForm"
        @submit="onChatFormSubmit"
        @ignore="onChatFormIgnore"
      />
      <!-- 写邮件确认卡(mail send):只读预览草稿,发送/取消替代输入框 -->
      <MailComposeBar
        v-else-if="pendingMailCompose"
        :draft="pendingMailCompose"
        @send="onMailComposeSend"
        @cancel="onMailComposeCancel"
      />
      <!-- 内嵌形态（子会话页签）：无输入框——但要留审批/表单（上面几条分支照常生效） -->
      <ChatInput
        v-else-if="!pendingApproval && !pendingForm && !pendingMailCompose && !embedded"
        ref="chatInputRef"
        no-padding
        no-inner-border
        :streaming="isStreaming"
        :settings="sessionSettings"
        :workspace-dir="mentionDir"
        @send="onSend"
        @stop="onStop"
        @command="onCommand"
        @update:settings="onSettingsChange"
      >
        <template #top>
          <PendingQueue v-if="pendingItems.length" :items="pendingItems" @edit="onQueueEdit" />
        </template>
      </ChatInput>
    </div>

    <!-- 对话正文选区悬浮按钮:添加到当前任务(引用) -->
    <button
      v-if="quoteBtn"
      type="button"
      class="quote-add-btn"
      :style="quoteBtnStyle"
      @mousedown.prevent
      @click="addQuote"
    >
      {{ t('chat.addQuote') }}
    </button>
  </section>
</template>

<style scoped>
/* 对话区定位上下文(规划胶囊绝对定位的包含块) */
.chat-section {
  position: relative;
}

/* 正文选区悬浮「添加到当前任务」:fixed 定位到选区上方(视口坐标,不随滚动偏移) */
.quote-add-btn {
  position: fixed;
  z-index: 120;
  height: 30px;
  padding: 0 12px;
  border: 0.5px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-base);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  white-space: nowrap;
  cursor: pointer;
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.08),
    0 6px 16px rgba(0, 0, 0, 0.12);
  transition: background-color 0.15s ease;
}

.quote-add-btn:hover {
  background: var(--bg-2);
}

/* 输入区外层:与消息列同宽(chat-column),底部留白替代原输入栏 padding-bottom */
.chat-input-wrap {
  padding-bottom: 12px;
}

/* 状态行与「向下」胶囊共用一行(固定在对话框上方,脱离消息流):
   三列网格 —— 左右各一份等宽 1fr,胶囊落在中间的 auto 列,因此**真正居中**
   且不随左侧状态文字长短漂移(状态文字固定左对齐在第一列)。
   minmax(0,1fr) 不设下限:窄对话区 / 英文长文案时文字在左列内换行,
   而不是把左列撑宽、把胶囊顶偏(不裁字,不用省略号)。
   纵向 **align-items: end**(共底边):胶囊比文字高,居中对齐会让文字随胶囊出现/消失
   上下浮动(有按钮上浮、没按钮下沉)。改成两者都贴住行底(= 贴着输入框),
   状态文字的位置就与胶囊是否显示无关了。
   与输入框的间距(8px)由本行统一给:状态行自身不再带下边距,
   避免「文字与胶囊同一行」时文字被自己的 padding 顶高半个身位。 */
.status-row {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
  align-items: end;
  padding-bottom: 8px;
}

.status-row-text {
  grid-column: 1;
  /* 允许左列被压到内容宽度以下(否则 1fr 会被文案最小宽度顶成不对称) */
  min-width: 0;
}

/* 消息区滚动容器 */
.chat-messages {
  flex: 1;
  overflow-y: scroll;
  display: flex;
  flex-direction: column;
  /* 顶部与顶栏的间距收紧到 5px;底部仍留 20px 呼吸 */
  padding: 5px 0 20px;
}

/* 空态 */
.empty-wrap {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  text-align: center;
}

/* 消息列:宽度/边距由共享的 .chat-column 提供 */
.msg-container {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-bottom: 16px;
}

/* 顶部居中「加载更早」:浅灰圆角胶囊,只在还有更早轮次时出现 */
.load-earlier-btn {
  align-self: center;
  margin-bottom: 4px;
  padding: 6px 18px;
  border: none;
  border-radius: 999px;
  background: var(--bg-2);
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition:
    background 0.15s ease,
    color 0.15s ease;
}

.load-earlier-btn:disabled {
  cursor: default;
  opacity: 0.7;
}
.load-earlier-spin {
  margin-right: 4px;
  vertical-align: -2px;
  animation: load-earlier-spin 0.9s linear infinite;
}
@keyframes load-earlier-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
.load-earlier-btn:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}














/* 「向下」胶囊:脱离文档流(不占状态行高度),绝对定位浮在消息区之上、可与其重叠;
   相对 .status-row 定位,底边贴状态行顶部再上移 6px,水平居中 */
.scroll-bottom-btn {
  position: absolute;
  left: 50%;
  bottom: 100%;
  transform: translateX(-50%);
  margin-bottom: 6px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  padding: 5px 14px;
  border: none;
  border-radius: 20px;
  background: var(--bg-2);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  font-weight: 600;
  cursor: pointer;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.12);
  z-index: 10;
  transition:
    opacity 0.2s,
    background 0.15s;
  user-select: none;
}

.scroll-bottom-btn:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}

/* ---------- 聊天框上方错误提示条(致命错误,不占内容区)
   macOS 风格:半透明材质 + 大圆角 + 发丝描边 + 柔和投影(与审批卡 / 提问表单同一配方),
   危险色只作用于圆角图标块与描边,正文保持中性可读 */

.chat-error-wrap {
  padding-bottom: 10px;
}

.chat-error-bar {
  display: flex;
  align-items: flex-start;
  gap: 9px;
  padding: 9px 12px;
  border: 0.5px solid color-mix(in srgb, var(--danger) 32%, var(--border-base));
  border-radius: 14px;
  background-color: color-mix(in srgb, var(--bg-2) 88%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  box-shadow:
    0 4px 16px rgba(0, 0, 0, 0.1),
    0 1px 2px rgba(0, 0, 0, 0.06);
  animation: chat-error-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}

@keyframes chat-error-in {
  from {
    opacity: 0;
    transform: translateY(5px) scale(0.99);
  }
}

@media (prefers-reduced-motion: reduce) {
  .chat-error-bar {
    animation: none;
  }
}

/* 圆角方块图标(SF Symbols 风格着色块,与审批卡 ApprovalBar 一致) */
.chat-error-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex: none;
  margin-top: 1px;
  border-radius: 6px;
  color: var(--danger);
  background-color: color-mix(in srgb, var(--danger) 16%, transparent);
}

.chat-error-text {
  min-width: 0;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
  line-height: 1.5;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  word-break: break-word;
}

/* 信息级瞬时提示(如「生成中断,正在自动重试」):同一位置、同一版式,只换主色 */
.chat-notice-bar {
  border-color: color-mix(in srgb, var(--color-primary-6) 30%, var(--border-base));
}

.chat-notice-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex: none;
  margin-top: 1px;
  border-radius: 6px;
  color: var(--color-primary-6);
  background-color: color-mix(in srgb, var(--color-primary-6) 16%, transparent);
  animation: chat-notice-spin 1.1s linear infinite;
}

@keyframes chat-notice-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .chat-notice-icon {
    animation: none;
  }
}



















/* 纯执行失败(静默态):中性灰,与成功/失败的红绿语义区分开——
   这类异常已回给模型改道,不需要用户介入 */







</style>
