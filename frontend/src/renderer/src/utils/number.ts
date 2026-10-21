import { i18n } from '@/i18n'

/**
 * 数量级压缩展示（token 数 / 字数量）：中文按「万」，其它语言按 K / M。
 *
 * 状态栏的「上下文容量 x/y」与任务看板卡片的进度药丸共用这一份 —— 两处各写一套会出现
 * 「12.4万」与「124K」并存的观感，而且中文环境的「万」直接搬到英文界面也不合适。
 */
export function formatCount(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0'
  const zh = String(i18n.global.locale.value).startsWith('zh')
  if (zh) {
    if (n < 10_000) return String(Math.round(n))
    const v = n / 10_000
    return `${Number.isInteger(v) ? v : v.toFixed(1)}万`
  }
  if (n < 1000) return String(Math.round(n))
  if (n < 1_000_000) {
    const v = n / 1000
    return `${v < 10 ? v.toFixed(1) : Math.round(v)}K`
  }
  return `${(n / 1_000_000).toFixed(1)}M`
}
