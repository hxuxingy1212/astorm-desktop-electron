// 内置浏览器页签的可嵌入性放行：iframe 架构下打不开「禁止被嵌入」的站点。
//
// 内置浏览器的页面是渲染层里的跨域 `<iframe>`（与宿主 UI 共用 defaultSession，
// 没有 partition）。github.com、google.com 等站点用两道声明禁止任何页面嵌入：
//   - 响应头 `X-Frame-Options: DENY|SAMEORIGIN`
//   - CSP 指令 `frame-ancestors 'none'|'self'`
// Chromium 对两者都强制，命中即报
//   `Framing 'https://github.com/' violates the following Content Security Policy
//    directive: "frame-ancestors 'none'". The request has been blocked.`
// 表现为页签一直空白。
//
// 折中：只对**子框架（subFrame）**响应剥离这两个「禁止被嵌入」声明——
// 顶层文档（应用自身 UI、外链打开）完全不受影响；语义上等价于「用户在自己的
// 浏览器里直接打开该站点」。唯一新增风险是站点无法再用 clickjacking 防护拒绝
// 本应用嵌入——本面板就是用户全权操作的自有浏览器，接受该权衡。
//
// 局限：仅处理 HTTP 响应头。<meta http-equiv="Content-Security-Policy"> 里写
// frame-ancestors 的站点（罕见）不受影响；站点用 JS frame-busting 的（top !== self
// 就跳走）也不受影响——若遇到，顶窗会被主窗口的 will-navigate 拦截并转外链打开。
import { session } from 'electron'

/** 从单条 CSP 策略串里剥掉 frame-ancestors 指令（分号分隔；其余指令原样保留） */
function stripFrameAncestors(csp: string): string {
  return csp
    .split(';')
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && !/^frame-ancestors\b/i.test(s))
    .join('; ')
}

/** 注册子框架响应头改写（app ready 后调用一次；作用于 defaultSession 的全部 subFrame 响应） */
export function allowSubframeEmbedding(): void {
  session.defaultSession.webRequest.onHeadersReceived((details, callback) => {
    const headers = details.responseHeaders
    if (details.resourceType !== 'subFrame' || !headers) {
      callback({ responseHeaders: headers })
      return
    }
    const out: Record<string, string[]> = {}
    for (const [key, value] of Object.entries(headers)) {
      const lower = key.toLowerCase()
      // X-Frame-Options：整条剥离
      if (lower === 'x-frame-options') continue
      // CSP：只剥 frame-ancestors 指令；剥空则整条丢弃（不发空头）
      if (lower === 'content-security-policy') {
        const cleaned = value.map(stripFrameAncestors).filter((s) => s.length > 0)
        if (cleaned.length > 0) out[key] = cleaned
        continue
      }
      out[key] = value
    }
    callback({ responseHeaders: out })
  })
}
