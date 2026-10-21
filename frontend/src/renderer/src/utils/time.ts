import { i18n } from '@/i18n'

/**
 * 相对时间:刚刚 / n 分钟前 / n 小时前 / 昨天 / n 天前
 *
 * `now` 可显式传入：需要「时间自己走」的卡片（任务看板）传 `useNow()` 的共享 ticker，
 * 组件读这个 ref 就会在每次 tick 重渲染，空闲卡片的「5 分钟前」不会停在旧值上。
 */
export function relativeTime(ts: number, now: number = Date.now()): string {
  const t = i18n.global.t
  const diff = now - ts
  const minute = 60_000
  const hour = 3_600_000
  const day = 86_400_000
  if (diff < minute) return t('time.justNow')
  if (diff < hour) return t('time.minutesAgo', { n: Math.floor(diff / minute) })
  if (diff < day) return t('time.hoursAgo', { n: Math.floor(diff / hour) })
  if (diff < 2 * day) return t('time.yesterday')
  return t('time.daysAgo', { n: Math.floor(diff / day) })
}

/**
 * 日期时间格式化（助手消息底部）：`10月1日 18:54`（zh）/ `10/1 18:54`（en）。
 */
export function formatDateTime(ts: number): string {
  const t = i18n.global.t
  const d = new Date(ts)
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  return t('time.monthDayTime', { m: d.getMonth() + 1, d: d.getDate(), hh, mm })
}

/**
 * 时长格式化（子智能体行的耗时）：`N秒 / N分M秒 / N小时M分`。
 * 用于子会话「创建 → 最近更新」的跨度展示。
 */
export function formatDuration(ms: number): string {
  const t = i18n.global.t
  const total = Math.max(0, Math.floor(ms / 1000))
  if (total < 60) return t('time.durSec', { s: total })
  const m = Math.floor(total / 60)
  const s = total % 60
  if (m < 60) return s === 0 ? t('time.durMin', { m }) : t('time.durMinSec', { m, s })
  const h = Math.floor(m / 60)
  const rm = m % 60
  return rm === 0 ? t('time.durHour', { h }) : t('time.durHourMin', { h, m: rm })
}
