/**
 * 邮件正文 HTML 清洗：只保留安全的展示内容。
 *
 * 与 mail-proxy Web 端 app.js 的 sanitizeHtml 同策略：
 * - 移除 script / style / iframe / object / embed / link / meta / form / input / button
 * - 移除所有 on* 事件属性
 * - 移除 javascript: 的 href / src
 * - 图片：协议相对（//）补 https、加 referrerpolicy=no-referrer（避免图床防盗链 403）
 */

const DROP_TAGS = 'script,style,iframe,object,embed,link,meta,form,input,button,base'

export function sanitizeHtml(html: string): string {
  if (!html) return ''
  try {
    const doc = new DOMParser().parseFromString(html, 'text/html')
    doc.querySelectorAll(DROP_TAGS).forEach((el) => el.remove())
    // 图片源修正：协议相对 URL（//host/x.png）在 file:// 下无法解析，统一补 https
    doc.querySelectorAll('img').forEach((el) => {
      const src = (el.getAttribute('src') || '').trim()
      if (src.startsWith('//')) el.setAttribute('src', 'https:' + src)
      el.setAttribute('referrerpolicy', 'no-referrer')
      el.setAttribute('loading', 'lazy')
    })
    doc.querySelectorAll('*').forEach((el) => {
      for (const attr of Array.from(el.attributes)) {
        const name = attr.name.toLowerCase()
        if (name.startsWith('on')) {
          el.removeAttribute(attr.name)
          continue
        }
        if (name === 'href' || name === 'src') {
          const value = attr.value.trim().toLowerCase()
          if (value.startsWith('javascript:') || value.startsWith('data:text/html')) {
            el.removeAttribute(attr.name)
          }
        }
      }
    })
    return doc.body.innerHTML
  } catch {
    return ''
  }
}
