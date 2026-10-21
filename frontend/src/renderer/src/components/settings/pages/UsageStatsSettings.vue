<script setup lang="ts">
/**
 * 使用统计(数据分析):token 用量聚合。
 *
 * - 指标卡 / 趋势 / 会话排行:按所选「时间范围(今天/昨天/7天/30天)」统计。
 * - 活跃热力图:固定回看约一年(GitHub 贡献图样式),铺满面板宽度。
 *
 * 数据来源优先走后端 `usage_stats`(逐轮落库的用量事件,按本地日期×模型聚合);
 * 后端暂无事件(旧数据 / 未运行)时回退到 `sessions.sessionInfo` 的会话累计量。
 * 会话排行基于每会话累计用量(`sessionUsage.total`)。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import SegmentedControl from '@/components/common/SegmentedControl.vue'
import { useSessionsStore } from '@/stores/sessions'
import { invoke } from '@/services/backend'
import { localizedSessionTitle } from '@/utils/sessionTitle'

const { t, locale } = useI18n()
const sessions = useSessionsStore()

const DAY_MS = 86_400_000
/** 热力图固定回看天数(≈52 周,铺满面板宽度) */
const HEAT_DAYS = 364
const PALETTE = ['#3b82f6', '#22c55e', '#a855f7', '#f97316', '#ef4444', '#06b6d4', '#eab308', '#ec4899']
const PIE_TOP = 8
const RANK_LIMIT = 10

type RangeKey = 'today' | 'yesterday' | 'd7' | 'd30'
interface RangeOpt {
  key: RangeKey
  labelKey: string
  /** 覆盖天数 */
  days: number
  /** 相对今天的偏移天数(0=今天结尾;1=昨天结尾) */
  offset: number
}
const RANGES: RangeOpt[] = [
  { key: 'today', labelKey: 'usage.rangeToday', days: 1, offset: 0 },
  { key: 'yesterday', labelKey: 'usage.rangeYesterday', days: 1, offset: 1 },
  { key: 'd7', labelKey: 'usage.range7', days: 7, offset: 0 },
  { key: 'd30', labelKey: 'usage.range30', days: 30, offset: 0 }
]
const rangeKey = ref<RangeKey>('d30')
const rangeOptions = computed<{ label: string; value: RangeKey }[]>(() =>
  RANGES.map((r) => ({ label: t(r.labelKey), value: r.key }))
)
const activeRange = computed(() => RANGES.find((r) => r.key === rangeKey.value) ?? RANGES[3])

/** 后端聚合行(逐轮用量事件;camelCase) */
interface UsageStatRowDto {
  day: string
  model: string
  prompt?: number
  completion?: number
  cacheHit?: number
  cacheMiss?: number
  total?: number
}
const usageRows = ref<UsageStatRowDto[]>([])
const backendActive = computed(() => usageRows.value.length > 0)

function dtoTotal(r: UsageStatRowDto): number {
  return r.total ?? (r.prompt ?? 0) + (r.completion ?? 0)
}

/** 拉取后端用量聚合(拉满热力图的 ~1 年);失败静默回退会话聚合 */
async function loadUsage(): Promise<void> {
  try {
    const sinceTs = Math.floor((startOfDay(Date.now()) - HEAT_DAYS * DAY_MS) / 1000)
    const raw = await invoke('usage_stats', { since_ts: sinceTs })
    const parsed = typeof raw === 'string' ? JSON.parse(raw) : raw
    usageRows.value = Array.isArray(parsed) ? (parsed as UsageStatRowDto[]) : []
  } catch {
    usageRows.value = []
  }
}

function dayKey(ms: number): string {
  const d = new Date(ms)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function startOfDay(ms: number): number {
  const d = new Date(ms)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

interface UsageRow {
  id: string
  title: string
  model: string
  provider: string
  ts: number
  total: number
  prompt: number
  completion: number
  round: number
}

/** 会话记录 → 聚合行(用量字段缺失按 0) */
const rows = computed<UsageRow[]>(() =>
  sessions.sessions.map((s) => {
    const info = sessions.sessionInfo[s.id]
    const u = (info?.sessionUsage ?? {}) as Record<string, number | undefined>
    const prompt = u.prompt ?? 0
    const completion = u.completion ?? 0
    const total = u.total ?? prompt + completion
    return {
      id: s.id,
      title: localizedSessionTitle(s).trim() || t('common.unnamed'),
      model: (info?.model ?? s.model ?? '').trim() || t('usage.unknownModel'),
      provider: (info?.provider ?? '').trim(),
      ts: info?.createdAt ? info.createdAt * 1000 : s.createdAt || Date.now(),
      total,
      prompt,
      completion,
      round: info?.currentRound ?? 0
    }
  })
)

/* ---------- 所选范围(今天/昨天/7天/30天) ---------- */
const todayStart = computed(() => startOfDay(Date.now()))
const rangeStart = computed(() => todayStart.value - (activeRange.value.days - 1 + activeRange.value.offset) * DAY_MS)
const rangeEnd = computed(() => todayStart.value - activeRange.value.offset * DAY_MS + DAY_MS)
const rangeStartKey = computed(() => dayKey(rangeStart.value))
const rangeEndKey = computed(() => dayKey(rangeEnd.value - 1))

const rangeRows = computed(() =>
  rows.value.filter((r) => r.ts >= rangeStart.value && r.ts < rangeEnd.value)
)
const usageRangeRows = computed(() =>
  usageRows.value.filter((r) => r.day >= rangeStartKey.value && r.day <= rangeEndKey.value)
)

/** 总用量 / 会话数 / 轮次 / 活跃天数 */
const totals = computed(() => {
  let tokens = 0
  if (backendActive.value) {
    for (const r of usageRangeRows.value) tokens += dtoTotal(r)
  } else {
    for (const r of rangeRows.value) tokens += r.total
  }
  let rounds = 0
  for (const r of rangeRows.value) rounds += r.round
  const activeDays = new Set<string>()
  if (backendActive.value) {
    for (const r of usageRangeRows.value) activeDays.add(r.day)
  } else {
    for (const r of rangeRows.value) activeDays.add(dayKey(r.ts))
  }
  return { tokens, sessions: rangeRows.value.length, rounds, activeDays: activeDays.size }
})

/** 当前连续天数:从今天(或昨天)起连续有会话活动的天数 */
const streak = computed(() => {
  const active = new Set<string>()
  for (const s of sessions.sessions) {
    if (s.createdAt) active.add(dayKey(s.createdAt))
    if (s.updatedAt) active.add(dayKey(s.updatedAt))
  }
  const today = startOfDay(Date.now())
  let cursor = active.has(dayKey(today)) ? today : today - DAY_MS
  let count = 0
  while (active.has(dayKey(cursor))) {
    count += 1
    cursor -= DAY_MS
  }
  return count
})

/** 各模型用量(降序;后端优先,回退会话聚合) */
const modelTotals = computed(() => {
  const map = new Map<string, number>()
  if (backendActive.value) {
    for (const r of usageRangeRows.value) {
      const model = r.model || t('usage.unknownModel')
      map.set(model, (map.get(model) ?? 0) + dtoTotal(r))
    }
  } else {
    for (const r of rangeRows.value) map.set(r.model, (map.get(r.model) ?? 0) + r.total)
  }
  return [...map.entries()].map(([model, value]) => ({ model, value })).sort((a, b) => b.value - a.value)
})

const topModel = computed(() => {
  const first = modelTotals.value[0]
  if (!first || totals.value.tokens === 0) return null
  return { model: first.model, share: Math.round((first.value / totals.value.tokens) * 100) }
})

/* ---------- 活跃热力图(固定约一年,按周分列,铺满宽度) ---------- */
/** 每天 token 用量(全量,后端优先 / 回退会话) */
const heatMap = computed(() => {
  const map = new Map<string, number>()
  if (backendActive.value) {
    for (const r of usageRows.value) map.set(r.day, (map.get(r.day) ?? 0) + dtoTotal(r))
  } else {
    for (const r of rows.value) {
      const k = dayKey(r.ts)
      map.set(k, (map.get(k) ?? 0) + r.total)
    }
  }
  return map
})

const heatMax = computed(() => Math.max(1, ...heatMap.value.values()))

/** 热力图分列(每列 7 天);首尾补空格,保证每列都是 7 格、行列对齐 */
const heatWeeks = computed(() => {
  const end = startOfDay(Date.now())
  const lead = new Date(end - HEAT_DAYS * DAY_MS).getDay()
  const cells: Array<{ level: number; title: string }> = []
  for (let i = 0; i < lead; i++) cells.push({ level: 0, title: '' })
  for (let i = HEAT_DAYS; i >= 0; i--) {
    const key = dayKey(end - i * DAY_MS)
    const v = heatMap.value.get(key) ?? 0
    const level = v === 0 ? 0 : Math.min(4, Math.ceil((v / heatMax.value) * 4))
    cells.push({ level, title: `${key} · ${fmt(v)}` })
  }
  // 末尾补空格到 7 的整数倍:最后一周不足 7 天时补满,避免右缘阶梯错位
  while (cells.length % 7 !== 0) cells.push({ level: 0, title: '' })
  const weeks: Array<Array<{ level: number; title: string }>> = []
  for (let i = 0; i < cells.length; i += 7) weeks.push(cells.slice(i, i + 7))
  return weeks
})

/** 饼图圆弧悬浮:显示对应会话名 + token */
const hoveredArc = ref<{ label: string; value: number; pct: number } | null>(null)
function hoverArc(s: { label: string; value: number; pct: number }): void {
  hoveredArc.value = s
}
function clearArc(): void {
  hoveredArc.value = null
}

/** 热力图格悬浮提示(即时,跟随鼠标;原生 title 有约 1s 延迟) */
const heatRef = ref<HTMLElement | null>(null)
const heatTip = ref<{ text: string; x: number; y: number } | null>(null)
function showHeatTip(e: MouseEvent, title: string): void {
  if (!title) return
  const rect = heatRef.value?.getBoundingClientRect()
  if (!rect) return
  heatTip.value = { text: title, x: e.clientX - rect.left, y: e.clientY - rect.top }
}
function clearHeatTip(): void {
  heatTip.value = null
}

/* ---------- 会话排行(饼图 + 列表) ---------- */
const sessionRank = computed(() => {
  const list = rangeRows.value.filter((r) => r.total > 0).sort((a, b) => b.total - a.total)
  const total = totals.value.tokens || list.reduce((a, r) => a + r.total, 0)
  return list.slice(0, RANK_LIMIT).map((r, i) => ({
    ...r,
    rank: i + 1,
    sharePct: total > 0 ? (r.total / total) * 100 : 0,
    /** 与饼图同步的颜色(前 N 名用调色板,其余归入「其他」灰) */
    color: i < PIE_TOP ? PALETTE[i % PALETTE.length] : '#94a3b8'
  }))
})

/** 饼图:会话排行榜前 N 名 + 其他(供 SVG 圆弧渲染,含累计偏移) */
const sessionPie = computed(() => {
  const total = rangeRows.value.reduce((a, r) => a + r.total, 0)
  const base = sessionRank.value
    .slice(0, PIE_TOP)
    .map((r) => ({ label: r.title, value: r.total, color: r.color }))
  const topSum = base.reduce((a, s) => a + s.value, 0)
  const other = Math.max(0, total - topSum)
  const segs = other > 0 ? [...base, { label: t('usage.other'), value: other, color: '#94a3b8' }] : base
  let acc = 0
  const arcs = segs.map((s) => {
    const pct = total > 0 ? (s.value / total) * 100 : 0
    const arc = { ...s, pct, offset: acc }
    acc += pct
    return arc
  })
  return { total, arcs }
})

const nf = computed(() => new Intl.NumberFormat(locale.value, { notation: 'compact', maximumFractionDigits: 1 }))
function fmt(n: number): string {
  return nf.value.format(n)
}

onMounted(loadUsage)
watch(rangeKey, loadUsage)
</script>

<template>
  <div class="usage-page">
    <div class="usage-head">
      <h2 class="usage-title">{{ t('usage.title') }}</h2>
      <SegmentedControl v-model="rangeKey" :options="rangeOptions" />
    </div>

    <!-- 指标卡 -->
    <div class="usage-cards">
      <div class="usage-card">
        <div class="usage-card-label"><icon-bar-chart :size="14" /> {{ t('usage.tokens') }}</div>
        <div class="usage-card-value">{{ fmt(totals.tokens) }}</div>
      </div>
      <div class="usage-card">
        <div class="usage-card-label"><icon-message :size="14" /> {{ t('usage.sessions') }}</div>
        <div class="usage-card-value">{{ totals.sessions }}</div>
      </div>
      <div class="usage-card">
        <div class="usage-card-label"><icon-clipboard-list :size="14" /> {{ t('usage.rounds') }}</div>
        <div class="usage-card-value">{{ totals.rounds }}</div>
      </div>
      <div class="usage-card">
        <div class="usage-card-label"><icon-calendar :size="14" /> {{ t('usage.activeDays') }}</div>
        <div class="usage-card-value">{{ totals.activeDays }}</div>
      </div>
      <div class="usage-card">
        <div class="usage-card-label"><icon-calendar :size="14" /> {{ t('usage.streak') }}</div>
        <div class="usage-card-value">{{ streak }}</div>
      </div>
      <div class="usage-card">
        <div class="usage-card-label"><icon-thunderbolt :size="14" /> {{ t('usage.topModel') }}</div>
        <div class="usage-card-value usage-card-model">{{ topModel?.model ?? '—' }}</div>
        <div class="usage-card-sub">{{ topModel ? t('usage.share', { n: topModel.share }) : ' ' }}</div>
      </div>
    </div>

    <!-- 会话排行:饼图 + 列表 -->
    <div class="usage-panel usage-panel-rank">
      <div class="usage-panel-head">
        <span class="usage-panel-title">{{ t('usage.sessionRank') }}</span>
      </div>
      <div v-if="!sessionRank.length" class="usage-empty">{{ t('usage.empty') }}</div>
      <div v-else class="usage-rank-wrap">
        <div class="usage-pie-block">
          <div class="usage-pie">
            <svg class="usage-pie-svg" viewBox="0 0 42 42">
              <circle class="usage-pie-track" cx="21" cy="21" r="15.9155" />
              <g transform="rotate(-90 21 21)">
                <circle
                  v-for="s in sessionPie.arcs"
                  :key="s.label"
                  class="usage-pie-arc"
                  cx="21"
                  cy="21"
                  r="15.9155"
                  :stroke="s.color"
                  :stroke-dasharray="`${s.pct} ${100 - s.pct}`"
                  :stroke-dashoffset="-s.offset"
                  @mouseenter="hoverArc(s)"
                  @mouseleave="clearArc"
                />
              </g>
            </svg>
            <div class="usage-pie-tip" :class="{ show: !!hoveredArc }">
              <span class="usage-pie-tip-name">{{ hoveredArc?.label }}</span>
              <span class="usage-pie-tip-value">
                {{ hoveredArc ? `${fmt(hoveredArc.value)} · ${Math.round(hoveredArc.pct)}%` : '' }}
              </span>
            </div>
            <div class="usage-pie-hole">
              <span class="usage-pie-total">{{ fmt(sessionPie.total) }}</span>
              <span class="usage-pie-unit">tokens</span>
            </div>
          </div>
        </div>

        <div class="usage-rank">
          <div v-for="r in sessionRank" :key="r.id" class="usage-rank-row">
            <span class="usage-rank-no">{{ r.rank }}</span>
            <span class="usage-rank-swatch" :style="{ background: r.color }" />
            <span class="usage-rank-title" :title="r.title">{{ r.title }}</span>
            <span class="usage-rank-bar"><i :style="{ width: `${r.sharePct}%`, background: r.color }" /></span>
            <span class="usage-rank-value">{{ fmt(r.total) }}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 活跃热力图(仅所选范围) -->
    <div class="usage-panel usage-panel-heat">
      <div class="usage-panel-head">
        <span class="usage-panel-title">{{ t('usage.heatmap') }}</span>
        <span class="usage-legend-heat">
          {{ t('usage.less') }}
          <i class="heat-legend lv0" /><i class="heat-legend lv1" /><i class="heat-legend lv2" /><i class="heat-legend lv3" /><i class="heat-legend lv4" />
          {{ t('usage.more') }}
        </span>
      </div>
      <div ref="heatRef" class="usage-heat" @mouseleave="clearHeatTip">
        <div v-for="(week, wi) in heatWeeks" :key="wi" class="heat-week">
          <span
            v-for="(cell, ci) in week"
            :key="ci"
            class="heat-cell"
            :class="`lv${cell.level}`"
            @mouseenter="showHeatTip($event, cell.title)"
          />
        </div>
        <div v-if="heatTip" class="heat-tip" :style="{ left: `${heatTip.x}px`, top: `${heatTip.y}px` }">
          {{ heatTip.text }}
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.usage-page {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 100%;
  max-width: 1200px;
  min-width: 0;
  min-height: 100%;
  margin: 0 auto;
  box-sizing: border-box;
  padding: 20px 56px 16px;
}

.usage-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  min-width: 0;
}

.usage-title {
  margin: 0;
  font-size: var(--font-size-xl);
  font-weight: 700;
  color: var(--text-primary);
}

/* 指标卡 */
.usage-cards {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
  min-width: 0;
}

.usage-card {
  height: 80px;
  overflow: hidden;
  padding: 12px 16px;
  border-radius: 12px;
  background: var(--bg-1);
  border: 1px solid var(--border-light);
}

.usage-card-label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-code);
  color: var(--text-tertiary);
}

.usage-card-value {
  margin-top: 4px;
  font-size: var(--font-size-xl);
  font-weight: 700;
  line-height: 1.1;
  color: var(--text-primary);
}

.usage-card-model {
  font-size: var(--font-size-lg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.usage-card-sub {
  margin-top: 4px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 面板卡片 */
.usage-panel {
  padding: 14px 18px 16px;
  border-radius: 14px;
  background: var(--bg-1);
  border: 1px solid var(--border-light);
  min-width: 0;
}

.usage-panel-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}

/* 会话排行面板:固定高度(切换范围不位移),内容在面板内垂直居中 */
.usage-panel-rank {
  height: 300px;
  display: flex;
  flex-direction: column;
}

.usage-panel-rank .usage-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}

/* 热力图面板:占满剩余高度铺到底部,网格垂直居中 */
.usage-panel-heat {
  flex: 1;
  min-height: 130px;
  display: flex;
  flex-direction: column;
}

.usage-panel-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}

.usage-legend-heat {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 热力图:按周等分铺满宽度(每列 7 天,单元格正方形随宽度自适应) */
.usage-heat {
  position: relative;
  flex: 1;
  align-items: center;
  display: flex;
  gap: 3px;
  width: 100%;
}

/* 即时悬浮提示(跟随鼠标) */
.heat-tip {
  position: absolute;
  transform: translate(-50%, -130%);
  padding: 4px 8px;
  border-radius: 6px;
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  box-shadow: var(--shadow-md);
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  white-space: nowrap;
  pointer-events: none;
  z-index: 3;
}

.heat-week {
  flex: 1 1 0;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.heat-cell {
  width: 100%;
  aspect-ratio: 1 / 1;
  border-radius: 3px;
  background: var(--bg-3);
}

/* 图例色块:固定尺寸 */
.heat-legend {
  width: 14px;
  height: 14px;
  border-radius: 3px;
  background: var(--bg-3);
}

.lv0 {
  background: var(--bg-3);
}

.lv1 {
  background: color-mix(in srgb, #3b82f6 25%, transparent);
}

.lv2 {
  background: color-mix(in srgb, #3b82f6 45%, transparent);
}

.lv3 {
  background: color-mix(in srgb, #3b82f6 70%, transparent);
}

.lv4 {
  background: #3b82f6;
}

/* 会话排行:左饼图 + 右列表(在面板剩余高度里垂直居中,避免底部单侧留白) */
.usage-rank-wrap {
  flex: 1;
  display: flex;
  gap: 20px;
  align-items: center;
  min-width: 0;
}

.usage-pie-block {
  display: flex;
  flex-direction: column;
  align-items: center;
  flex-shrink: 0;
}

.usage-pie {
  position: relative;
  width: 150px;
  height: 150px;
  flex-shrink: 0;
}

.usage-pie-svg {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
}

.usage-pie-track {
  fill: none;
  stroke: var(--bg-2);
  stroke-width: 6;
}

/* 圆弧:hover 高亮;原生 <title> 悬浮浮层显示会话信息 */
.usage-pie-arc {
  fill: none;
  stroke-width: 6;
  stroke-linecap: butt;
  pointer-events: stroke;
  transition: opacity 0.15s ease;
}

.usage-pie-arc:hover {
  opacity: 0.75;
}

.usage-pie-hole {
  position: absolute;
  inset: 0;
  margin: auto;
  width: 92px;
  height: 92px;
  border-radius: 50%;
  background: var(--bg-1);
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1px;
  pointer-events: none;
}

.usage-pie-total {
  font-size: var(--font-size-lg);
  font-weight: 700;
  color: var(--text-primary);
}

.usage-pie-unit {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

/* 圆弧悬浮提示:会话名 + token */
.usage-pie-tip {
  position: absolute;
  left: 50%;
  bottom: calc(100% + 8px);
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-width: 260px;
  padding: 6px 10px;
  border-radius: 8px;
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  box-shadow: var(--shadow-md);
  font-size: var(--font-size-xs);
  white-space: nowrap;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s ease;
  z-index: 2;
}

.usage-pie-tip.show {
  opacity: 1;
}

.usage-pie-tip-name {
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
}

.usage-pie-tip-value {
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}

.usage-rank {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}

.usage-rank-row {
  display: grid;
  grid-template-columns: 24px 12px minmax(120px, 1.4fr) minmax(80px, 1.2fr) 72px;
  align-items: center;
  gap: 10px;
  height: 22px;
  padding: 0 6px;
  border-radius: 6px;
  font-size: var(--font-size-code);
}

.usage-rank-row:hover {
  background: var(--bg-2);
}

.usage-rank-no {
  text-align: center;
  font-weight: 600;
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}

/* 与饼图同步的颜色方块 */
.usage-rank-swatch {
  width: 10px;
  height: 10px;
  border-radius: 3px;
  justify-self: center;
}

.usage-rank-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
}

.usage-rank-bar {
  height: 5px;
  border-radius: 3px;
  background: var(--bg-3);
  overflow: hidden;
}

.usage-rank-bar i {
  display: block;
  height: 100%;
  border-radius: 3px;
}

.usage-rank-value {
  text-align: right;
  font-variant-numeric: tabular-nums;
  color: var(--text-secondary);
}

.usage-empty {
  padding: 40px 0;
  text-align: center;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}
</style>
