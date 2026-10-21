<script setup lang="ts">
/**
 * 任务看板卡片：一张 = 一条会话。
 *
 * 卡片从上到下的顺序：**标题**（最多两行）→ **正文**（两行最后一次用户提问 + 三行
 * 最后一条**无工具调用**的助手正文；都取不到则回退状态文案）→ **上下文容量**（标签 + 数值 +
 * 细进度条）→ **底行**（工作区最后一级目录名 + 最后活动时间，钉在卡片底部）。
 *
 * **紧凑布局**：各区块高度随内容收紧（不再为等高预留最小高度），空区块直接不渲染，
 * 避免卡片内出现空白间隙。
 *
 * 状态点与状态文案全部来自 store 的统一判定链（`statusKeyOf` / `statusLabelKeyOf`），
 * 与左侧任务栏的行内状态点同源，不会两处说法不一致。
 *
 * 纯展示 + 事件：点击由父级决定去哪（当前是打开该会话的聊天页）；
 * 悬浮操作只留「启动」（待办列专用）—— 置顶 / 打开文件 / 删除走侧栏。
 *
 * **悬浮预览面板**（参考状态栏 Token 用量悬浮卡）：悬停卡片片刻后，在卡片**侧面**弹出
 * 加宽面板，只显示 3 行最后一次用户提问 + 6 行最后一次无工具调用的助手正文（其余信息不重复）。
 * Teleport 到 body + fixed 定位（列体 `overflow-y:auto` 会裁切），空间不足时翻到另一侧；
 * 光标可移入面板继续阅读（跨过缝隙有短暂延迟防误关），滚动/缩放窗口即隐藏。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import { relativeTime } from '@/utils/time'
import { useNow } from '@/composables/useNow'
import { useContextCapacity } from '@/composables/useContextCapacity'
import type { Session } from '@/types'
import { localizedSessionTitle } from '@/utils/sessionTitle'

const props = defineProps<{
  session: Session
}>()

const emit = defineEmits<{
  (e: 'open'): void
  (e: 'start'): void
}>()

const { t } = useI18n()
const sessions = useSessionsStore()

/** 状态点档位（与侧栏一致：running/form/interrupted/ignored/cancelled/error/done/null） */
const statusKey = computed(() => sessions.statusKeyOf(props.session.id))

/**
 * 卡片上**不出文字**的状态：已忽略 / 已取消 / 已完成。
 * 这三者列归属或状态点已经说清楚了（完成列 = 已完成，红点 = 取消，灰点 = 忽略），
 * 再写一行字是噪音；需要精确状态的还有卡片悬浮（title）与打开会话两条路。
 */
const QUIET_STATUS = new Set(['ignored', 'cancelled', 'done'])

/** 状态行文案（复用会话状态文案；无标记的新会话、以及上面三种静默状态都不显示） */
const statusLabel = computed(() => {
  const key = sessions.statusLabelKeyOf(props.session.id)
  if (!key || QUIET_STATUS.has(key)) return ''
  return t(`session.status.${key}`)
})

// 卡片/悬浮预览最多显示六行。先规范空白再截取，避免 line-clamp 隐藏的
// 数万字回复仍进入 DOM 并在每次列宽变化时参与文本排版；会话原文保留在 store。
const PREVIEW_TEXT_LIMIT = 2000
function previewText(content: string | null | undefined): string {
  const text = (content ?? '').replace(/\s+/g, ' ').trim()
  return text.length > PREVIEW_TEXT_LIMIT ? `${text.slice(0, PREVIEW_TEXT_LIMIT)}…` : text
}

/** 最后一次用户提问（正文区上块，最多两行）：从后往前找最后一条**有正文**的 user 消息 */
const lastQuestion = computed(() => {
  const list = sessions.messages[props.session.id] ?? []
  for (let i = list.length - 1; i >= 0; i -= 1) {
    const msg = list[i]
    if (msg.role !== 'user') continue
    const text = previewText(msg.content)
    if (text) return text
  }
  return ''
})

/**
 * 最后一条**无工具调用**的助手正文（正文区下块，最多三行）：从后往前跳过带工具调用的
 * 消息（执行中/刚跑完工具的），只取纯文字回答。
 * 都取不到（历史会话：消息是内存态，本次运行没打开过、后端也没跑过）时返回空，
 * 与提问一起由模板回退显示状态文案 —— 卡片不留空洞。
 */
const lastAnswer = computed(() => {
  const list = sessions.messages[props.session.id] ?? []
  for (let i = list.length - 1; i >= 0; i -= 1) {
    const msg = list[i]
    if (msg.role !== 'assistant') continue
    if (msg.tools?.length) continue
    const text = previewText(msg.content)
    if (text) return text
  }
  return ''
})

/** 正文区是否有内容（决定回退状态文案 / 状态行是否出现） */
const hasBody = computed(() => !!lastQuestion.value || !!lastAnswer.value)

/**
 * 工作区只显示最后一个目录名（卡片窄，整条路径挤成省略号反而认不出来）：
 * 去掉结尾分隔符后取最后一段，兼容 Windows 反斜杠；完整路径走 title 悬浮。
 */
const workspaceName = computed(() => {
  const path = (props.session.workspace ?? '').replace(/[/\\]+$/, '')
  if (!path) return ''
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1) || path
})

/**
 * 能否「启动」：无状态标记（= 看板「待办」列）且已有后端会话。
 * 「待办」的判定与列归属同源（`statusKeyOf` 为空 ⇔ `boardColumnOf` 落 `todo`），
 * 所以按钮出现的位置恰好就是待办列，不用另外传列名进来。
 * 无 `backendId` 的本地草稿没有可执行的指令，不给按钮。
 */
const startable = computed(() => statusKey.value === null && !!props.session.backendId)

/** 共享的 60s ticker：读 `now.value` 让「5 分钟前」自己走（一列卡片共用一个定时器） */
const now = useNow()

/**
 * 上下文容量：当前上下文真实 token（后端每轮推的 `sessionInfo.contextTokens`）占摘要阈值的比例。
 * 与状态栏信息面板同一份实现（分母 = 系统设置里的摘要阈值，不是模型窗口）。
 * 这一块**常驻**：没跑过的卡片（0）也显示（`0/20万（0.0%）`+ 空条）——
 * 它原先是不显示的（免得待办列全是空条），但那样卡片会比别的矮一截，与「卡片等高」冲突。
 */
const contextTokens = computed(() => sessions.sessionInfo[props.session.id]?.contextTokens ?? 0)
const { pct: contextPct, text: contextText } = useContextCapacity(contextTokens)

/** 底行时间：显式传 now，空闲卡片的时间文案也会随时间刷新 */
const timeLabel = computed(() =>
  relativeTime(props.session.updatedAt ?? props.session.createdAt, now.value)
)

/* ---- 悬浮预览面板（侧面加宽，只放 3 行提问 + 6 行正文） ---- */

const cardRef = ref<HTMLElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)
const previewOpen = ref(false)
/** 面板翻到卡片左侧（右侧空间不够时）：箭头随之换边 */
const panelFlip = ref(false)
const panelStyle = ref<Record<string, string>>({})

let showTimer: ReturnType<typeof setTimeout> | undefined
let hideTimer: ReturnType<typeof setTimeout> | undefined

function clearTimers(): void {
  if (showTimer) clearTimeout(showTimer)
  if (hideTimer) clearTimeout(hideTimer)
}

/** 悬停片刻才弹（扫过卡片不闪）；没有可预览正文的卡片不出面板 */
function onCardEnter(): void {
  clearTimers()
  showTimer = setTimeout(() => {
    if (hasBody.value) previewOpen.value = true
  }, 150)
}

/** 离开卡片：延迟一小段再关 —— 留出跨过缝隙、移入面板的时间（进面板会取消本定时器） */
function onCardLeave(): void {
  clearTimers()
  hideTimer = setTimeout(() => {
    previewOpen.value = false
  }, 150)
}

function onPanelEnter(): void {
  if (hideTimer) clearTimeout(hideTimer)
}

function closePreview(): void {
  clearTimers()
  previewOpen.value = false
}

/** 定位：卡片右侧优先、放不下翻左侧，上下收敛进视口；箭头 y 钉在卡片上部 */
function place(): void {
  const rect = cardRef.value?.getBoundingClientRect()
  const panel = panelRef.value
  if (!rect || !panel) return
  const GAP = 12
  const MARGIN = 12
  const w = panel.offsetWidth
  const h = panel.offsetHeight
  const flip =
    window.innerWidth - rect.right < w + GAP + MARGIN &&
    rect.left - GAP - w >= MARGIN
  panelFlip.value = flip
  let left = flip ? rect.left - GAP - w : rect.right + GAP
  left = Math.min(Math.max(left, MARGIN), Math.max(MARGIN, window.innerWidth - w - MARGIN))
  const top = Math.min(
    Math.max(rect.top + 8, MARGIN),
    Math.max(MARGIN, window.innerHeight - h - MARGIN)
  )
  const arrowTop = Math.min(Math.max(rect.top + 22 - top, 12), h - 12)
  panelStyle.value = {
    left: `${left}px`,
    top: `${top}px`,
    '--preview-arrow-top': `${arrowTop}px`
  }
}

watch(previewOpen, (v) => {
  if (v) {
    window.addEventListener('scroll', closePreview, true)
    window.addEventListener('resize', closePreview)
    void nextTick(place)
  } else {
    window.removeEventListener('scroll', closePreview, true)
    window.removeEventListener('resize', closePreview)
  }
})

onBeforeUnmount(() => {
  clearTimers()
  window.removeEventListener('scroll', closePreview, true)
  window.removeEventListener('resize', closePreview)
})
</script>

<template>
  <article
    ref="cardRef"
    class="board-card"
    role="button"
    tabindex="0"
    @click="closePreview(); emit('open')"
    @keydown.enter.prevent="closePreview(); emit('open')"
    @keydown.space.prevent="closePreview(); emit('open')"
    @mouseenter="onCardEnter"
    @mouseleave="onCardLeave"
  >
    <!-- 快捷操作：悬停/聚焦时浮现（不占常态版位）。只留「启动」（待办列专用）；
         置顶 / 打开文件 / 删除走侧栏，不在看板卡片上重复 -->
    <span v-if="startable" class="card-actions" @click.stop>
      <a-tooltip :content="t('board.start')">
        <button type="button" class="card-action" :aria-label="t('board.start')" @click.stop="emit('start')">
          <icon-play :size="13" />
        </button>
      </a-tooltip>
    </span>

    <div class="card-head">
      <span class="card-dot" :class="`dot-${statusKey ?? 'none'}`" />
      <span class="card-title" :title="localizedSessionTitle(session)">{{ localizedSessionTitle(session) || t('common.unnamed') }}</span>
    </div>

    <!-- 正文区：最后一次用户提问 + 最后无工具调用的助手正文；
         两者都取不到时回退一行状态文案，卡片不留空洞 -->
    <div v-if="lastQuestion" class="card-content card-question">{{ lastQuestion }}</div>
    <div v-if="lastAnswer" class="card-content">{{ lastAnswer }}</div>
    <div v-if="!hasBody" class="card-content" :class="`status-${statusKey}`">
      {{ statusLabel }}
    </div>

    <!-- 状态行：有正文且需要出声时才占一行（紧凑布局，空则完全省略） -->
    <div v-if="hasBody && statusLabel" class="card-status" :class="`status-${statusKey}`">
      {{ statusLabel }}
    </div>

    <!-- 上下文容量：标签 + 「2.2万/20万（10.8%）」+ 细进度条（参考状态栏信息面板）。
         **常驻**（没跑过的卡片显示 0 / 空条）：这一块在不显示时会把卡片压矮一小截，
         与「卡片等高」冲突；分母是摘要阈值，不是模型窗口。 -->
    <div class="card-context">
      <div class="ctx-head">
        <span class="ctx-label">{{ t('sessionInfo.contextCapacity') }}</span>
        <span class="ctx-value">{{ contextText }}</span>
      </div>
      <div class="ctx-bar">
        <span class="ctx-fill" :style="{ width: `${contextPct}%` }" />
      </div>
    </div>

    <!-- 工作区（只留最后一个目录名，完整路径走 title 悬浮）+ 最后活动时间 -->
    <div class="card-meta">
      <span v-if="workspaceName" class="card-workspace" :title="session.workspace">
        <!-- 13px 与卡片内其它图标（悬浮操作行）同一口径：手写图标是 48 网格、按 16px 设计，
             缩到 11px 时线宽不足 1px，文件夹只剩一个糊掉的小方框 -->
        <icon-folder :size="13" class="card-workspace-icon" />
        <span class="card-workspace-name">{{ workspaceName }}</span>
      </span>
      <span class="card-foot">{{ timeLabel }}</span>
    </div>

    <!-- 悬浮预览面板：Teleport 到 body（列体 overflow 会裁切），只放 3 行提问 + 6 行正文 -->
    <Teleport to="body">
      <transition name="preview-fade">
        <div
          v-if="previewOpen && hasBody"
          ref="panelRef"
          class="board-preview"
          :class="{ 'is-flipped': panelFlip }"
          :style="panelStyle"
          @mouseenter="onPanelEnter"
          @mouseleave="onCardLeave"
        >
          <div v-if="lastQuestion" class="preview-block preview-question">{{ lastQuestion }}</div>
          <div
            v-if="lastAnswer"
            class="preview-block preview-answer"
            :class="{ 'with-divider': !!lastQuestion }"
          >
            {{ lastAnswer }}
          </div>
        </div>
      </transition>
    </Teleport>
  </article>
</template>

<style scoped>
/* 默认底色与悬停反馈复用 styles/cards.css 的共享卡片配方。 */
.board-card {
  /* 各列独立滚动，屏外卡片无需随窗口/侧栏伸缩重新排版。
     auto 记住实测高度，首次进入视口前用估值占位；保留键盘查找与可达性。 */
  content-visibility: auto;
  contain-intrinsic-block-size: auto 200px;
  flex-shrink: 0;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 10px 12px;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

/* 键盘聚焦除共享底色反馈外，保留清晰的焦点轮廓。 */
.board-card:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 2px;
}

/* 快捷操作条：绝对定位浮在右上（常态 opacity 0 不占位、不抢点击），悬停或聚焦时浮现。
   背景用卡片底色盖住下方文字，避免按钮压着标题看不清 */
.card-actions {
  position: absolute;
  top: 5px;
  right: 7px;
  display: inline-flex;
  gap: 1px;
  padding: 1px;
  border-radius: 8px;
  background: var(--bg-base);
  opacity: 0;
  transition: opacity var(--transition-fast);
}

.board-card:hover .card-actions,
.board-card:focus-within .card-actions {
  opacity: 1;
}

.card-action {
  width: 22px;
  height: 22px;
  padding: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.card-action:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.card-head {
  display: flex;
  align-items: flex-start;
  gap: 7px;
}

/* 状态点：与侧栏任务行同义（8px，颜色即语义）。
   待办（无状态标记）用**中性灰** —— 与列头「待办」的灰点、侧栏 idle 同一档。
   这里刻意不用 --text-disabled：它是个偏蓝的灰（深色底 #4e5969），
   在卡片上会被读成「运行中」的蓝点，与真·运行中的主色点分不开。 */
.card-dot {
  width: 8px;
  height: 8px;
  flex: none;
  margin-top: 5px;
  border-radius: 50%;
  background: var(--text-tertiary);
}

.dot-running {
  background: var(--accent-text);
}

.dot-form {
  background: var(--warning);
}

.dot-error,
.dot-cancelled {
  background: var(--danger);
}

.dot-interrupted,
.dot-ignored {
  background: var(--text-tertiary);
}

.dot-done {
  background: var(--success);
}

/* 标题：最多两行（随内容收紧，不再预留第二行） */
.card-title {
  min-width: 0;
  font-size: var(--font-size-md);
  font-weight: 600;
  line-height: 1.45;
  color: var(--text-primary);
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  word-break: break-word;
}

/* 正文区:助手正文最多三行(随内容收紧) */
.card-content {
  font-size: var(--font-size-xs);
  line-height: 1.5;
  color: var(--text-secondary);
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 3;
  overflow: hidden;
  word-break: break-word;
}

/* 用户提问块:弱一档(tertiary)与助手正文区分,最多两行 */
.card-question {
  color: var(--text-tertiary);
  -webkit-line-clamp: 2;
}

/* 状态行:有正文预览且需要出声时才渲染(见模板 v-if) */
.card-status {
  font-size: var(--font-size-xs);
  line-height: 1.5;
  color: var(--text-secondary);
}

/* 运行中/等你处理加一点强调色，便于在一列里扫到「正在动」与「等你」的那些 */
.status-running {
  color: var(--accent-text);
}

.status-awaitingForm,
.status-awaitingApproval {
  color: var(--warning);
}

.status-error,
.status-cancelled {
  color: var(--danger);
}

/* 上下文容量：标签/数值左右分列 + 细进度条（与状态栏 `.context-capacity*` 同一套观感） */
.card-context {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.ctx-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.ctx-label {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

.ctx-value {
  font-size: var(--font-size-2xs);
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.ctx-bar {
  width: 100%;
  height: 6px;
  border-radius: 999px;
  background: var(--bg-3);
  overflow: hidden;
}

.ctx-fill {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: var(--accent-text);
  transition: width 0.2s ease;
}

/* 底行：工作区最后一级目录名（等宽，超长单行省略）+ 最后活动时间。
   `line-height` 写死在这里：没有工作区的卡片（新建会话）这一行只剩时间，
   不定行高的话它会比带工作区的那行矮一点，卡片就不等高了。
   `margin-top: auto` 兜底：万一将来某个区块的预留被去掉，底行仍钉在卡片底部 */
.card-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  line-height: 1.5;
  margin-top: auto;
}

.card-workspace {
  flex: 1;
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-family: monospace;
  font-size: var(--font-size-2xs);
  line-height: 1.5;
  color: var(--text-tertiary);
  white-space: nowrap;
}

.card-workspace-icon {
  flex: none;
}

.card-workspace-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.card-foot {
  flex: none;
  /* 没有工作区时（新建会话）靠 auto 外边距仍贴右，与其它卡片的时间对齐成一列 */
  margin-left: auto;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

/* ---- 悬浮预览面板：卡片侧面的加宽浮层（观感对齐状态栏 Token 用量悬浮卡） ---- */
.board-preview {
  position: fixed;
  z-index: 1100;
  /* 加宽：明显宽于单列卡片，让 3 行提问 / 6 行正文有可读的行宽 */
  width: min(420px, calc(100vw - 24px));
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 14px 16px;
  background: var(--hover-card-bg);
  border: none;
  border-radius: 10px;
  box-shadow: var(--hover-card-shadow);
}

/* 指向卡片的小箭头（45° 色块）：默认在面板左缘，翻面时换到右缘 */
.board-preview::after {
  content: '';
  position: absolute;
  top: var(--preview-arrow-top, 20px);
  left: -5px;
  width: 10px;
  height: 10px;
  background: var(--hover-card-bg);
  transform: rotate(45deg);
}

.board-preview.is-flipped::after {
  left: auto;
  right: -5px;
}

.preview-block {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-word;
  font-size: var(--font-size-sm);
  line-height: 1.55;
}

/* 用户提问：3 行，弱一档（tertiary）与正文区分 */
.preview-question {
  -webkit-line-clamp: 3;
  color: var(--text-tertiary);
}

/* 助手正文：6 行；两块都在时上方加分隔线（同 Token 用量面板的区块分隔） */
.preview-answer {
  -webkit-line-clamp: 6;
  color: var(--text-secondary);
}

.preview-answer.with-divider {
  border-top: 1px solid var(--border-base);
  padding-top: 8px;
}

.preview-fade-enter-active {
  transition:
    opacity 0.18s ease,
    transform 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}
.preview-fade-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
}
.preview-fade-enter-from,
.preview-fade-leave-to {
  opacity: 0;
  transform: translateY(4px);
}
</style>
