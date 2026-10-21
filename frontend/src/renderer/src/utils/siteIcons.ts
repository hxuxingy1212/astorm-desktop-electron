/**
 * 站点 / 邮件图标 URL 的唯一入口。
 *
 * 桌面端（preload 注入 `iconProtocol: true`）统一走 `astorm-icon://`：由主进程按域名做
 * 7 天磁盘缓存、回退链和失败标记（见 `src/main/iconCache.ts`）。浏览器部署与预览站
 * 没有这个协议，回落到原来的直连候选链。
 *
 * 调用方只依赖这里返回的候选 URL 列表：桌面端列表通常只有一项（主进程内部已含回退），
 * Web 端则保留「站点直取 → Google / DuckDuckGo」的逐项回退。
 */

/** 当前环境是否支持主进程图标缓存协议（旧 preload / 预览桥为 undefined → false）。 */
export const hasIconProtocol = window.api?.iconProtocol === true

/** 从完整 URL 取 hostname；解析失败返回空串。 */
export function hostOfUrl(url: string): string {
  try {
    return new URL(url).hostname.toLowerCase().replace(/\.$/, '')
  } catch {
    return ''
  }
}

/** 归一化邮件域名（允许误传完整邮箱，取 @ 之后的部分）。 */
export function normalizeMailDomain(input: string): string {
  const raw = input.trim().toLowerCase()
  if (!raw) return ''
  const at = raw.lastIndexOf('@')
  const domain = (at >= 0 ? raw.slice(at + 1) : raw).replace(/\.$/, '')
  if (!domain || domain.startsWith('.') || domain.includes('..')) return ''
  if (domain.includes(':') || /^\d{1,3}(?:\.\d{1,3}){3}$/.test(domain)) return ''
  return /^[a-z0-9.-]+$/.test(domain) ? domain : ''
}

/** 邮件域名的回退链：整域 → 末两段（Web 端直连时使用；桌面端由主进程处理）。 */
export function mailDomainCandidates(domain: string): string[] {
  const d = normalizeMailDomain(domain)
  if (!d) return []
  const parts = d.split('.')
  return parts.length > 2 ? [d, parts.slice(-2).join('.')] : [d]
}

/** 站点图标的候选 URL（按优先级）。 */
export function siteIconUrls(url: string, size = 64): string[] {
  const host = hostOfUrl(url)
  if (!host) return []
  if (hasIconProtocol) {
    return [`astorm-icon://favicon/${encodeURIComponent(host)}?sz=${size}`]
  }
  return [
    `https://${host}/favicon.ico`,
    `https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=${size}`
  ]
}

/** 站点图标首选 URL（无候选时为空串）。 */
export function siteIconUrl(url: string, size = 64): string {
  return siteIconUrls(url, size)[0] ?? ''
}

/** 邮件发件人头像的候选 URL（Gravatar 由调用方另行追加）。 */
export function mailIconUrls(domain: string, size = 64): string[] {
  const candidates = mailDomainCandidates(domain)
  if (!candidates.length) return []
  if (hasIconProtocol) {
    return [`astorm-icon://mail/${encodeURIComponent(candidates[0])}?sz=${size}`]
  }
  const urls: string[] = []
  for (const d of candidates) urls.push(`https://${d}/favicon.ico`)
  for (const d of candidates) urls.push(`https://icons.duckduckgo.com/ip3/${d}.ico`)
  return urls
}

/** Gravatar 头像 URL（桌面端走主进程缓存，Web 端直连）。 */
export function gravatarIconUrl(hash: string, size = 64): string {
  const h = hash.trim().toLowerCase()
  if (!/^[a-f0-9]{64}$/.test(h)) return ''
  return hasIconProtocol
    ? `astorm-icon://avatar/${h}?sz=${size}`
    : `https://www.gravatar.com/avatar/${h}?d=404&s=${size}`
}
