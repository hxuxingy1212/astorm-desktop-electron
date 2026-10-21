<script setup lang="ts">
/**
 * 会话信息入口（原顶栏 ⓘ 按钮 + 卡片迁移至此）：输入框工具行右侧的小按钮，
 * 点击在**输入框上方**弹出卡片，展示会话参数（模型/供应商/模式/权限/思考级别/工作区/
 * 轮次/用户提问次数/摘要次数/用量/缓存/技能/工具/更新时间）与上下文容量进度条。
 *
 * 数据来自 store.sessionInfo（后端 session_info 事件实时刷新）。上下文容量的分母是系统
 * 设置里的摘要阈值（与看板卡片同一口径），打开卡片时强制重拉一次以反映最新设置。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import { useContextCapacity, loadSummaryThreshold } from '@/composables/useContextCapacity'

const { t } = useI18n()
const sessions = useSessionsStore()

// 交互：悬浮即预览（移开消失）；点击固定（再点取消固定）。
// open = 固定中 或 正在悬浮；取消固定时把 hovered 也清零，鼠标仍在按钮上也立即隐藏。
const pinned = ref(false)
const hovered = ref(false)
const open = computed(() => pinned.value || hovered.value)
const btnRef = ref<HTMLElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)

function onTriggerClick(): void {
  if (pinned.value) {
    pinned.value = false
    hovered.value = false
  } else {
    pinned.value = true
    // 固定时强制重拉摘要阈值分母：系统设置里可能刚改过（缓存只在首次拉取）
    void loadSummaryThreshold(true)
  }
}

const activeSession = computed(() => sessions.sessions.find((s) => s.id === sessions.activeId))
const sessionInfo = computed(() => {
  const a = activeSession.value
  return a ? sessions.sessionInfo[a.id] : undefined
})

// Token 用量区块（参考图）：总量 = 自身 + 子 agent 聚合；明细 = 缓存命中率 / 未缓存输入 / 缓存读取 / 输出
type UsageLike =
  | { prompt?: number; completion?: number; total?: number; cache_hit?: number; cache_miss?: number; cacheHit?: number; cacheMiss?: number }
  | undefined
function cacheOf(u: UsageLike, which: 'hit' | 'miss'): number {
  if (!u) return 0
  const v = which === 'hit' ? (u.cache_hit ?? u.cacheHit) : (u.cache_miss ?? u.cacheMiss)
  return typeof v === 'number' ? v : 0
}
const ownUsage = computed(() => sessionInfo.value?.sessionUsage)
const subUsage = computed(() => sessionInfo.value?.subagentUsage)
const tokTotal = computed(() => (ownUsage.value?.total ?? 0) + (subUsage.value?.total ?? 0))
const tokCacheHit = computed(() => cacheOf(ownUsage.value, 'hit') + cacheOf(subUsage.value, 'hit'))
const tokCacheMiss = computed(() => cacheOf(ownUsage.value, 'miss') + cacheOf(subUsage.value, 'miss'))
const tokOutput = computed(() => (ownUsage.value?.completion ?? 0) + (subUsage.value?.completion ?? 0))
const cacheHitRate = computed(() => {
  const total = tokCacheHit.value + tokCacheMiss.value
  return total > 0 ? `${Math.round((tokCacheHit.value / total) * 100)}%` : '—'
})
/** token 数 → 千分位 + ` token`（完整单位，不缩写、不压缩） */
function formatTok(n: number): string {
  return `${Math.round(n).toLocaleString('en-US')} token`
}

// 会话统计：轮次（累计 LLM 调用）/ 用户提问次数 / 摘要次数
const roundNo = computed(() => sessionInfo.value?.currentRound ?? 0)
const questionTimes = computed(() => sessionInfo.value?.questionTimes ?? 0)
const summaryTimes = computed(() => sessionInfo.value?.summaryTimes ?? 0)

// 上下文容量：当前上下文真实 token / 摘要阈值（打开卡片时强制重拉分母）
const contextTokens = computed(() => sessionInfo.value?.contextTokens ?? 0)
const { pct: contextPct, text: contextCapacityText } = useContextCapacity(contextTokens)

// 触发按钮的进度环几何（48 网格；半径调小让圆更小巧、四周留白）
const RING_R = 14
const RING_STROKE = 7
const RING_C = 2 * Math.PI * RING_R

function onDocMouseDown(e: MouseEvent): void {
  const target = e.target as Node | null
  if (!target) return
  if (btnRef.value?.contains(target)) return
  if (panelRef.value?.contains(target)) return
  pinned.value = false
  hovered.value = false
}

/* 定位：Teleport 到 body 后 fixed 定位；空间足够时以按钮为**水平中心**，贴边时收敛到视口内 */
const panelStyle = ref<Record<string, string>>({})
function place(): void {
  const rect = btnRef.value?.getBoundingClientRect()
  const panel = panelRef.value
  if (!rect || !panel) return
  const MARGIN = 12
  const panelW = panel.offsetWidth || 300
  const btnCenter = rect.left + rect.width / 2
  const left = Math.min(
    Math.max(btnCenter - panelW / 2, MARGIN),
    Math.max(MARGIN, window.innerWidth - panelW - MARGIN)
  )
  panelStyle.value = {
    left: `${left}px`,
    right: 'auto',
    bottom: `${window.innerHeight - rect.top + 8}px`,
    '--info-arrow-left': `${btnCenter - left - 5}px`
  }
}

watch(open, (v) => {
  if (v) {
    document.addEventListener('mousedown', onDocMouseDown, true)
    void loadSummaryThreshold()
    nextTick(place)
  } else {
    document.removeEventListener('mousedown', onDocMouseDown, true)
  }
})
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocMouseDown, true))
</script>

<template>
  <span ref="btnRef" class="session-info-wrap">
    <button
      type="button"
      class="session-info-btn"
      :class="{ active: open }"
      :aria-label="t('sessionInfo.title')"
      @mouseenter="hovered = true"
      @mouseleave="hovered = false"
      @click="onTriggerClick"
    >
      <!-- 上下文容量进度环（浅灰轨道 + 较深弧段 = 已占用比例） -->
      <svg class="session-info-ring" viewBox="0 0 48 48" width="20" height="20" fill="none" aria-hidden="true">
        <circle class="ring-track" cx="24" cy="24" :r="RING_R" :stroke-width="RING_STROKE" />
        <circle
          class="ring-arc"
          cx="24"
          cy="24"
          :r="RING_R"
          :stroke-width="RING_STROKE"
          stroke-linecap="round"
          :stroke-dasharray="`${(contextPct / 100) * RING_C} ${RING_C}`"
          transform="rotate(-90 24 24)"
        />
      </svg>
      <!-- 上下文占用百分比（环旁数值） -->
      <span class="session-info-pct">{{ Math.round(contextPct) }}%</span>
    </button>

    <Teleport to="body">
      <transition name="info-fade">
        <div v-if="open && activeSession" ref="panelRef" class="session-info-panel" :style="panelStyle">
          <div class="session-info-scroll">
          <!-- Token 用量区块（参考图样式）：标题 + 总量 / 明细四行 -->
          <section class="usage-block">
            <div class="usage-head">
              <span class="usage-title">
                <icon-data :size="15" />
                {{ t('sessionInfo.usage') }}
              </span>
              <span class="usage-total">{{ formatTok(tokTotal) }}</span>
            </div>
            <div class="usage-divider" />
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.cacheHit') }}</span>
              <span class="usage-value">{{ cacheHitRate }}</span>
            </div>
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.uncachedInput') }}</span>
              <span class="usage-value">{{ formatTok(tokCacheMiss) }}</span>
            </div>
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.cacheRead') }}</span>
              <span class="usage-value">{{ formatTok(tokCacheHit) }}</span>
            </div>
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.output') }}</span>
              <span class="usage-value">{{ formatTok(tokOutput) }}</span>
            </div>
          </section>

          <div class="session-info-sep" />

          <!-- 会话统计：轮次（累计 LLM 调用）/ 用户提问次数 / 摘要次数 -->
          <div class="stats-block">
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.round') }}</span>
              <span class="usage-value">{{ roundNo }}</span>
            </div>
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.questionTimes') }}</span>
              <span class="usage-value">{{ questionTimes }}</span>
            </div>
            <div class="usage-row">
              <span class="usage-label">{{ t('sessionInfo.summaryTimes') }}</span>
              <span class="usage-value">{{ summaryTimes }}</span>
            </div>
          </div>

          <div class="session-info-sep" />

          <!-- 上下文容量：当前上下文真实 token / 摘要阈值（带进度条） -->
          <div class="info-item context-capacity">
            <div class="context-capacity-head">
              <span class="info-label">{{ t('sessionInfo.contextCapacity') }}</span>
              <span class="info-value">{{ contextCapacityText }}</span>
            </div>
            <div class="context-capacity-bar">
              <div class="context-capacity-fill" :style="{ width: `${contextPct}%` }" />
            </div>
          </div>
        </div>
        </div>
      </transition>
    </Teleport>
  </span>
</template>

<style scoped>
/* 触发按钮包裹层：relative 定位锚点，卡片向右对齐按钮 */
.session-info-wrap {
  position: relative;
  flex-shrink: 0;
  display: inline-flex;
}

/* 小按钮：与底部工具行其它控件同高（28px）、方形圆角 hover 底（非圆形）；环 + 百分比 */
.session-info-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  height: 28px;
  padding: 0 6px;
  border: none;
  border-radius: var(--radius-sm);
  background-color: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  line-height: 0;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}
.session-info-btn:hover,
.session-info-btn.active {
  background-color: var(--bg-3);
  color: var(--text-primary);
}

/* 上下文占用百分比：环旁数值 */
.session-info-pct {
  font-size: var(--font-size-xs);
  line-height: 1;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

/* 对话框宽度不足时只显示图标（隐藏百分比）；容器为 .chat-section（全局声明） */
@container (max-width: 720px) {
  .session-info-pct {
    display: none;
  }
}

/* 进度环：浅灰轨道 + 较深弧段（弧长 = 上下文占用比例），随占用变化平滑过渡 */
.session-info-ring {
  display: block;
}
.ring-track {
  stroke: color-mix(in srgb, var(--text-tertiary) 30%, transparent);
}
.ring-arc {
  stroke: var(--text-tertiary);
  transition: stroke-dasharray 0.3s ease;
}

/* 卡片：Teleport 到 body 后 fixed 定位（输入栏 overflow:hidden 不会裁切），
   位置由组件按按钮 rect 计算（在按钮上方、右缘对齐） */
.session-info-panel {
  position: fixed;
  z-index: 1100;
  width: min(300px, calc(100vw - 180px));
  background: var(--panel-bg);
  /* 无外边框（按需求去掉；靠背景与投影区分层次） */
  border: none;
  border-radius: 10px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.14);
}
.session-info-panel::after {
  content: '';
  position: absolute;
  bottom: -6px;
  left: var(--info-arrow-left, 14px);
  width: 10px;
  height: 10px;
  background: var(--panel-bg);
  border: none;
  transform: rotate(45deg);
}

.session-info-scroll {
  max-height: 45vh;
  overflow-y: auto;
  padding: 14px 18px 16px;
}

/* Token 用量区块（参考图：标题+总量 / 分隔线 / 明细四行） */
.usage-block {
  padding: 2px 0;
}
.usage-head {
  display: flex;
  align-items: center;
  gap: 6px;
}
.usage-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}
.usage-title svg {
  color: var(--text-secondary);
}
.usage-total {
  margin-left: auto;
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
  font-variant-numeric: tabular-nums;
}
.usage-divider {
  height: 1px;
  margin: 10px 0 2px;
  background: var(--border-base);
}
.usage-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 5px 0;
  font-size: var(--font-size-sm);
}
.usage-label {
  color: var(--text-tertiary);
}
.usage-value {
  color: var(--text-primary);
  font-variant-numeric: tabular-nums;
}

/* 用量区块与下方上下文容量之间的分隔线 */
.session-info-sep {
  height: 1px;
  margin: 12px 0;
  background: var(--border-base);
}

.info-item {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
}
.info-label {
  flex-shrink: 0;
  min-width: 64px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.info-value {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
}

/* 上下文容量：整行纵向排布（标题行 + 进度条） */
.context-capacity {
  flex-direction: column;
  align-items: stretch;
  gap: 6px;
}
.context-capacity-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  min-width: 0;
}
.context-capacity-bar {
  width: 100%;
  height: 8px;
  border-radius: 999px;
  background: var(--bg-3);
  overflow: hidden;
}
.context-capacity-fill {
  height: 100%;
  border-radius: 999px;
  background: var(--accent-text);
  transition: width 0.2s ease;
}

.info-fade-enter-active {
  transition:
    opacity 0.18s ease,
    transform 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}
.info-fade-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
}
.info-fade-enter-from,
.info-fade-leave-to {
  opacity: 0;
  transform: translateY(6px);
}
</style>
