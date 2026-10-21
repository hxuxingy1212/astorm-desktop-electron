/**
 * 会话上下文容量（当前上下文 token / 摘要阈值）。
 *
 * 分母**不是**模型窗口，而是系统设置里的**摘要阈值** `summary.token_threshold`
 * （读满 100% 即触发摘要压缩）—— 与状态栏信息面板同一口径，别处别再换一个分母。
 *
 * 阈值是全局设置、全局一份，所以这里做模块级缓存 + 一次性加载：状态栏与看板卡片共用，
 * 不必各自 `get_settings` 一遍。拿不到（后端未就绪 / 纯浏览器调试）时退回默认 20 万。
 */
import { computed, ref } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import { invoke } from '@/services/backend'
import { formatCount } from '@/utils/number'

const DEFAULT_SUMMARY_THRESHOLD = 200000
const threshold = ref(DEFAULT_SUMMARY_THRESHOLD)
let loading: Promise<void> | null = null

/**
 * 加载摘要阈值。默认幂等（模块级缓存，多处调用只真拉一次）；
 * `force = true` 强制重拉 —— 状态栏在**打开信息卡时**用它，好让设置页刚改过的阈值立刻生效。
 */
export function loadSummaryThreshold(force = false): Promise<void> {
  if (force) loading = null
  if (!loading) {
    loading = (async () => {
      try {
        const s = (await invoke('get_settings')) as { summary?: { token_threshold?: number } }
        const t = s?.summary?.token_threshold
        if (typeof t === 'number' && t > 0) threshold.value = t
      } catch {
        /* 后端不可用：保持默认分母 */
      }
    })()
  }
  return loading
}

/** token 数 → 占用百分比 + 「2.2万/20万（10.8%）」文案 */
export function useContextCapacity(tokens: Ref<number> | ComputedRef<number>): {
  pct: ComputedRef<number>
  text: ComputedRef<string>
} {
  const pct = computed(() =>
    threshold.value > 0 ? Math.min(100, (tokens.value / threshold.value) * 100) : 0
  )
  const text = computed(() =>
    threshold.value > 0
      ? `${formatCount(tokens.value)}/${formatCount(threshold.value)}（${pct.value.toFixed(1)}%）`
      : '—'
  )
  return { pct, text }
}
