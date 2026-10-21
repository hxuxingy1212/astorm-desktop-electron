/**
 * 交付物 / 变更清单的机器可读标记块（HTML 注释）在前端的展示侧剥离。
 *
 * 后端在助手最终答复落盘前会剥离标记块（见 `agent::artifacts`），但**流式期间**正文是
 * 逐 token 到达、且原始标记会短暂出现在渲染内容里；另外 `artifacts` 事件到达前也需兜底。
 * 这里提供纯文本剥离：先做一次廉价的 `includes` 预检，命中才跑正则，避免每 token 都整篇正则。
 */
const MARKER_RE = /<!--\s*astorm:artifacts[\s\S]*?-->/gi

/** 剥离正文中的交付物标记块（只影响展示，不改 store 里的原文）。 */
export function stripArtifactMarkers(content: string): string {
  if (!content.includes('astorm:artifacts')) return content
  return content.replace(MARKER_RE, '').trimEnd()
}
