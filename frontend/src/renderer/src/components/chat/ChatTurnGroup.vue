<script setup lang="ts">
/**
 * 一轮对话的「过程收纳块」:把该轮的 think / 工具 / 中间叙述收成一行表头，
 * 结尾那条正文(没有 tool_call 的助手消息)由 ChatPanel 留在块外正常显示。
 *
 * 开合策略（优先级：阻断性中断 > 用户手动 > 自动规则）：
 * - 流式中自动展开（能看见正在跑的步骤）
 * - **本轮没有块外助手正文**（异常收尾：以工具调用 / 中断结束，`expand-no-final`）→ 展开：
 *   收起后整轮只剩一行耗时，用户看不到任何结论
 * - **其余（有块外正文，无论正常完成 / 用户打断 / 出错）→ 收起**成「已处理 1m41s」
 * - **待审批 / 待答表单**（停在等用户决定）保持展开、且不给收起：
 *   它不是在「异常中断」，而是用户此刻正要依据这些步骤做判断
 * - 其余情况用户手动点过后以手动为准
 * - 折叠时不挂载体（v-if）：收起态不解析 markdown、不做 JSON.stringify（沿用既有约定）
 *
 * 进行 / 暂停的状态描述不在这里:它固定在对话框上方（components/chat/TurnStatusBar.vue），
 * 本块只负责表头的耗时。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import ChatStepList from './ChatStepList.vue'
import {
  elapsedSeconds,
  hasBlockingInterrupt,
  pendingDecision,
  resumableForm,
  type TurnStep
} from './turnGrouping'

const props = defineProps<{
  steps: TurnStep[]
  isDark: boolean
  /** 本轮是否已结束（结束 → 正常完成才自动收起 + 耗时定格） */
  ended: boolean
  /** 本轮结束原因（实时打点） */
  endReason?: 'done' | 'cancelled' | 'error'
  /**
   * 该会话最近一轮的结束原因（后端会话状态回填，冷启动恢复）；
   * 只由 ChatPanel 传给最新一轮——否则历史轮次会被误判成非正常结束而全部展开
   */
  sessionEnd?: 'done' | 'error' | 'cancelled' | 'interrupted' | 'ignored'
  /** 本轮起点（用户消息时间） */
  startedAt: number
  /** 本轮终点（实时打点或历史记录 metadata.ended_at） */
  endedAt?: number
  /**
   * 本轮**没有块外正文**（以工具调用 / 中断收尾，`turn.final === null`）时为 true，默认展开。
   *
   * 由 ChatPanel 对**每一轮**传入 `!turn.final`：这种收尾下整轮内容都在块内，收起后界面上只剩
   * 一行「已处理 Xs」，用户看不到任何结论（典型：提问表单被忽略、工具失败/被取消收尾）。
   */
  expandNoFinal?: boolean
  /** 进行中相位(仅最新一轮流式中传入):过程列表末尾追加流光状态行 */
  phase?: 'awaiting' | 'thinking' | 'tool' | 'summarizing'
}>()

const { t } = useI18n()

/** 需要用户介入的中断 → 强制展开（见文件头注释） */
const blocking = computed(() => hasBlockingInterrupt(props.steps))

const manual = ref<boolean | null>(null)
/**
 * 展开态优先级：**阻断性中断 > 用户手动 > 自动规则**。
 *
 * 自动规则：流式中展开；**没有块外正文（异常收尾）展开**；其余一律收起。
 * 待审批 / 待答表单这种「停下来等用户」过程正是用户判断的依据（要看清它准备执行什么），
 * 所以这类轮次**强制保持展开**，也不允许手动收起。
 */
const open = computed(
  () => blocking.value || (manual.value ?? (!props.ended || (props.expandNoFinal ?? false)))
)
function toggle(): void {
  // 等用户决定期间不响应收起（否则「点了没反应却改了手动态」，恢复后行为会怪）
  if (blocking.value) return
  manual.value = !open.value
}

// 阻断中断出现时清掉用户先前的手动收起（它已经压不过 blocking，这里只是复位状态）
watch(blocking, (need) => {
  if (need) manual.value = null
})

// ---------- 计时：仅流式中的那一块起 1s 心跳 ----------
const nowTs = ref(Date.now())
let timer: number | null = null
watch(
  () => props.ended,
  (ended) => {
    if (!ended && timer == null) {
      nowTs.value = Date.now()
      timer = window.setInterval(() => {
        nowTs.value = Date.now()
      }, 1000)
    } else if (ended && timer != null) {
      window.clearInterval(timer)
      timer = null
    }
  },
  { immediate: true }
)
onBeforeUnmount(() => {
  if (timer != null) window.clearInterval(timer)
})

/** 本轮已耗秒数:与对话框上方的状态行共用 elapsedSeconds,保证两处数字一致 */
const elapsedSec = computed(() =>
  elapsedSeconds(props.startedAt, props.ended ? (props.endedAt ?? Date.now()) : nowTs.value)
)

/** 紧凑中文耗时:77秒 / 1分17秒 */
function compactDuration(sec: number): string {
  const m = Math.floor(sec / 60)
  const s = sec % 60
  return m > 0 ? t('chat.turnDurMin', { m, s }) : t('chat.turnDurSec', { s })
}

/** 表头:进行中走秒,结束定格总耗时;时间缺失(历史记录无 metadata)时只显示「已完成」 */
const timerText = computed(() => {
  if (!props.ended) return t('chat.turnProcessing', { s: elapsedSec.value ?? 0 })
  if (elapsedSec.value == null) return t('chat.turnProcessedNoTime')
  return t('chat.turnProcessed', { duration: compactDuration(elapsedSec.value) })
})

// ---------- 过程过长:限高 + 滚动,并在上下边缘渐隐提示 ----------
const stepsBodyRef = ref<HTMLElement | null>(null)
const fadeTop = ref(false)
const fadeBottom = ref(false)
function updateFade(): void {
  const el = stepsBodyRef.value
  if (!el) return
  fadeTop.value = el.scrollTop > 1
  fadeBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight > 1
}

/**
 * 流式「吸底」自动滚动：会话执行中，过程列表跟随最新步骤滚动（步骤追加 / 相位行
 * 出现消失 / 展开内容变化都由 MutationObserver 捕捉）。
 * 用户上滚查看历史步骤时暂停吸底（回到底部自动恢复），执行结束不再抢滚动条。
 */
const STICK_THRESHOLD = 24
const stick = ref(true)
let mo: MutationObserver | null = null

function onStepsScroll(): void {
  const el = stepsBodyRef.value
  if (!el) return
  stick.value = el.scrollHeight - el.scrollTop - el.clientHeight <= STICK_THRESHOLD
  updateFade()
}

/** 滚到最新一条；force=展开瞬间等需要强制跟随的场合（不看 stick，但不打断用户） */
function scrollToLatest(force = false): void {
  const el = stepsBodyRef.value
  if (!el) return
  if (!force && (!stick.value || props.ended)) return
  el.scrollTop = el.scrollHeight
  updateFade()
}

function detachObserver(): void {
  mo?.disconnect()
  mo = null
}

/** 步骤载体随 open 挂卸(v-if)，观察器跟随挂卸 */
async function attachObserver(): Promise<void> {
  await nextTick()
  detachObserver()
  const el = stepsBodyRef.value
  if (!el || props.ended) return
  mo = new MutationObserver(() => scrollToLatest())
  mo.observe(el, { childList: true, subtree: true, characterData: true })
  scrollToLatest(true)
}

onMounted(() => {
  if (open.value) void attachObserver()
})
/** 展开时若仍在执行:直接跟到最新(用户主动展开本就为看当前进展);收起/结束即摘观察器 */
watch(open, (v) => {
  if (v) void attachObserver()
  else detachObserver()
})
watch(
  () => props.ended,
  (v) => {
    if (v) detachObserver()
    else if (open.value) void attachObserver()
  }
)
// 步骤增删 / 本轮展开变化后重算边缘渐隐
watch(
  () => [props.steps.length, open.value],
  async () => {
    await nextTick()
    updateFade()
  },
  { immediate: true }
)

/**
 * 表头追加的「中止原因」（跟在「已处理 Xs」后面）——**为什么不正常收尾**。
 *
 * 起因：被忽略的提问此前只在块外那行「继续填写表单」里看得出来，而那一行现在只留给最新
 * 一轮（用户已经往下走就不该再挂着入口）；历史轮次于是只剩一行耗时，看不出发生过什么。
 * 这里把原因钉在表头上，任何轮次（含历史）都能一眼看出停在哪：
 * 等待审批 / 等待回答（待决中断）> 提问被放弃（忽略 / 作废 / 未答）> 用户打断 / 出错 / 进程中断。
 *
 * 只读结构性字段（工具的中断状态与结束原因），不读正文，遵守 turnGrouping 的渲染隔离约定。
 */
const noteText = computed(() => {
  if (!props.ended) return ''
  const decision = pendingDecision(props.steps)
  if (decision === 'approval') return t('session.status.awaitingApproval')
  if (decision === 'form') return t('session.status.awaitingForm')
  if (decision === 'mail') return t('session.status.awaitingMail')
  const form = resumableForm(props.steps)
  if (form) {
    return form.interrupt?.status === 'ignored'
      ? t('chat.turnNoteIgnored')
      : t('chat.turnNoteFormUnfinished')
  }
  // 实时结束原因优先；历史轮次用后端会话状态兜底（与 abnormal 同源）
  const reason = props.endReason ?? props.sessionEnd
  if (reason === 'cancelled') return t('chat.turnNoteCancelled')
  if (reason === 'error') return t('chat.turnNoteError')
  if (reason === 'interrupted') return t('chat.turnNoteInterrupted')
  if (reason === 'ignored') return t('chat.turnNoteIgnored')
  return ''
})
</script>

<template>
  <div class="turn-group" :class="{ open, blocking }">
    <!-- 轮标题:已完成，用时 X（右侧箭头）；流式中显示转圈 -->
    <button type="button" class="turn-header" @click="toggle">
      <!-- 品牌标:以 alpha 蒙版渲染,固定纯色 -->
      <span class="turn-logo" aria-hidden="true" />
      <span class="turn-title">{{ timerText }}</span>
      <!-- 旋转图标:排在 logo 与文字之后(进行中提示) -->
      <span v-if="!ended" class="turn-status">
        <icon-loading :size="16" class="turn-spin" />
      </span>
      <!-- 中止原因（等待审批 / 提问被忽略 / 用户打断 / 出错…） -->
      <span v-if="noteText" class="turn-note">· {{ noteText }}</span>
      <span class="turn-arrow">
        <icon-chevron-up v-if="open" :size="15" />
        <icon-chevron-down v-else :size="15" />
      </span>
    </button>
    <!-- 折叠时不挂载步骤（v-if）：收起态零 markdown / JSON 开销。
         过程步骤直接挂在轮容器下（外层已有「已完成,用时 X」一层折叠,不再套第二层摘要行） -->
    <div
      v-if="open"
      ref="stepsBodyRef"
      class="turn-body steps-body"
      :class="{ 'fade-top': fadeTop, 'fade-bottom': fadeBottom }"
      @scroll="onStepsScroll"
    >
      <ChatStepList :steps="steps" :phase="ended ? undefined : phase" :is-dark="isDark" />
    </div>
  </div>
</template>

<style scoped>
.turn-group {
  display: flex;
  flex-direction: column;
}

/* 表头：与 Think / 工具组同款的无边框透明按钮，不引入新的胶囊样式。
   整行铺满（分割线要横贯消息列），但箭头紧跟文字、不推到行尾；字号比正文小一档 */
.turn-header {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  padding: 5px 0 7px;
  border: none;
  /* 收纳头与下方内容（收起的正文 / 展开的步骤）之间画一条横贯分隔线 */
  border-bottom: 1px solid var(--border-base);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-title);
  font-weight: 500;
  text-align: left;
  cursor: pointer;
  user-select: none;
}

.turn-header:hover {
  color: var(--text-primary);
}

.turn-arrow {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  color: var(--text-tertiary);
}

.turn-status {
  display: inline-flex;
  align-items: center;
}

.turn-spin {
  color: var(--text-tertiary);
  animation: turn-spin 1s linear infinite;
}

@keyframes turn-spin {
  to {
    transform: rotate(360deg);
  }
}

.turn-ok {
  color: var(--success);
}

.turn-title {
  line-height: 1;
  font-variant-numeric: tabular-nums;
}

/* 轮尾品牌标:logo 的 alpha 作蒙版,固定纯色(不随标题 currentColor/悬停变化);
   24px 允许超过 15px 文字行高,轮头整行随标撑高(星形尖角细,盒给足视觉才立得住) */
.turn-logo {
  width: 24px;
  height: 24px;
  flex: none;
  background-color: #000;
  -webkit-mask: url('@/assets/logo-mark.png') center / contain no-repeat;
  mask: url('@/assets/logo-mark.png') center / contain no-repeat;
}

/* 暗色主题下纯黑不可见,取纯白(仍不参与悬停变化) */
html[data-theme='dark'] .turn-logo {
  background-color: #fff;
}

/* 中止原因：与耗时同基线、更轻的字重与颜色，读起来是「一句补充说明」而不是第二个标题 */
.turn-note {
  line-height: 1;
  font-size: var(--font-size-xs);
  font-weight: 400;
  color: var(--text-tertiary);
}

/* 步骤体：与消息列表同间距；左侧缩进（2px）表示层级，不加竖线避免噪声 */
.turn-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 8px 0 2px;
  padding-left: 2px;
}

/* 过程过长时限高 + 滚动查看;上下边缘渐隐提示还有内容 */
.steps-body {
  /* 展开限高 = 对话内容区高度的 2/5（--chat-area-h 由 ChatPanel 随消息区尺寸发布） */
  max-height: calc(var(--chat-area-h, 640px) * 2 / 5);
  overflow-y: auto;
  overflow-x: hidden;
  scrollbar-gutter: stable;
}
.steps-body.fade-top {
  mask-image: linear-gradient(to bottom, transparent 0, #000 10px);
}
.steps-body.fade-bottom {
  mask-image: linear-gradient(to bottom, #000 calc(100% - 10px), transparent 100%);
}
.steps-body.fade-top.fade-bottom {
  mask-image: linear-gradient(
    to bottom,
    transparent 0,
    #000 10px,
    #000 calc(100% - 10px),
    transparent 100%
  );
}
</style>
