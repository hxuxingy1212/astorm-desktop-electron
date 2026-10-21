/**
 * markdown 相对图片内联为 data: URL。
 * 文件预览(markdown 渲染)经 DOMPurify/渲染器消毒时自定义协议会被剥掉,而 data:image 默认放行;
 * 故把相对图片逐一经 kb_read_buffer 读成 base64 内联,保证消毒后仍可显示。
 * relDir 为 markdown 文件所在的知识库相对目录(空 = 知识库根层)。
 */
import { invoke } from '@/services/backend'

const KB_IMG_MIME: Record<string, string> = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  webp: 'image/webp', svg: 'image/svg+xml', bmp: 'image/bmp', ico: 'image/x-icon', avif: 'image/avif'
}

function bytesToBase64(bytes: Uint8Array): string {
  let bin = ''
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  return btoa(bin)
}

/** markdown 相对图片内联为 data: URL(缺失的图片保留原路径)。 */
export async function inlineMarkdownImages(text: string, relDir: string): Promise<string> {
  const schemeRe = /^[a-zA-Z][a-zA-Z0-9+.-]*:/
  const collectRel = (raw: string): string => {
    const t = raw.trim()
    if (schemeRe.test(t) || t.startsWith('/') || t.startsWith('#')) return ''
    const segs = relDir ? relDir.split('/') : []
    for (const part of t.split('/')) {
      if (!part || part === '.') continue
      if (part === '..') segs.pop()
      else segs.push(part)
    }
    return segs.join('/')
  }
  const images: Array<{ marker: string; rel: string }> = []
  const markerOf = (rel: string): string => {
    const marker = `\u0000KBIMG${images.length}\u0000`
    images.push({ marker, rel })
    return marker
  }
  // 图片引用先替换为哨兵(marker),异步读回后统一回填
  let replaced = text
    .replace(/!\[[^\]]*\]\(([^)]+)\)/g, (whole, innerRaw: string) => {
      const inner = innerRaw.trim()
      const sp = inner.search(/\s/)
      const path = sp === -1 ? inner : inner.slice(0, sp)
      const rest = sp === -1 ? '' : inner.slice(sp)
      return whole.replace(innerRaw, markerOf(collectRel(path)) + rest)
    })
    .replace(/<img([^>]*?)\bsrc="([^"]+)"/gi, (whole, attrs: string, src: string) => {
      return `<img${attrs} src="${markerOf(collectRel(src))}"`
    })
  const loaded = await Promise.all(
    images.map(async ({ rel }) => {
      if (!rel) return ''
      const ext = (rel.split('.').pop() ?? '').toLowerCase()
      try {
        const buf = (await invoke('kb_read_buffer', { path: rel })) as unknown
        const bytes = buf instanceof Uint8Array ? buf : Array.isArray(buf) ? Uint8Array.from(buf as number[]) : null
        if (!bytes) return ''
        return `data:${KB_IMG_MIME[ext] ?? 'application/octet-stream'};base64,${bytesToBase64(bytes)}`
      } catch {
        return ''
      }
    })
  )
  images.forEach((img, idx) => {
    const replacement = loaded[idx] || img.rel
    replaced = replaced.split(img.marker).join(replacement)
  })
  return replaced
}
/**
 * 渲染时把相对图片解析为 data URL(源文本保持原文):知识库相对目录 → kb_read_buffer。
 * 相对引用(相对当前 markdown 所在的知识库目录)才处理;绝对/协议/锚点原样返回。
 */
export async function resolveKbImageUrl(relDir: string, imgUrl: string): Promise<string> {
  if (!imgUrl) return imgUrl
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(imgUrl) || imgUrl.startsWith('/') || imgUrl.startsWith('#')) {
    return imgUrl
  }
  const segs = relDir ? relDir.split('/') : []
  for (const part of imgUrl.split('/')) {
    if (!part || part === '.') continue
    if (part === '..') segs.pop()
    else segs.push(part)
  }
  const rel = segs.join('/')
  if (!rel) return imgUrl
  const ext = (rel.split('.').pop() ?? '').toLowerCase()
  try {
    const buf = (await invoke('kb_read_buffer', { path: rel })) as unknown
    const bytes = buf instanceof Uint8Array ? buf : Array.isArray(buf) ? Uint8Array.from(buf as number[]) : null
    if (!bytes) return imgUrl
    return `data:${KB_IMG_MIME[ext] ?? 'application/octet-stream'};base64,${bytesToBase64(bytes)}`
  } catch {
    return imgUrl
  }
}

/** 已是 data:/协议/绝对路径/锚点的图片无需内联 */
function isResolvableRelative(src: string): boolean {
  return !!src && !src.startsWith('data:') && !/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(src) && !src.startsWith('/') && !src.startsWith('#')
}

/**
 * 把**已渲染**容器里的相对 `<img src>` 就地升级为 data URL（知识库相对图片内联，源文本不变）。
 *
 * 为什么用观察器而不是渲染后跑一次：markdown 渲染器的节点渲染器是异步组件，图片元素可能在
 * 挂载后若干帧才出现；切换文档时容器本身不重建，只靠一次 nextTick 会漏掉新图片。
 * 只在真的改写 src 时才会再次触发观察（改完已是 data:，直接跳过），不会自激。
 *
 * @returns 停止观察（调用方在容器卸载 / 换目录时调用）
 */
export function upgradeRelativeImages(host: HTMLElement, relDir: string): () => void {
  /** 正在读取的相对路径：避免同一路径并发重复读盘 */
  const pending = new Set<string>()
  let stopped = false

  async function pass(): Promise<void> {
    const imgs = Array.from(host.querySelectorAll('img[src]')) as HTMLImageElement[]
    for (const img of imgs) {
      const src = img.getAttribute('src') ?? ''
      if (!isResolvableRelative(src) || pending.has(src)) continue
      pending.add(src)
      const data = await resolveKbImageUrl(relDir, src)
      pending.delete(src)
      if (stopped) return
      if (data && data !== src) img.setAttribute('src', data)
    }
  }

  const observer = new MutationObserver(() => void pass())
  observer.observe(host, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] })
  void pass()
  return () => {
    stopped = true
    observer.disconnect()
  }
}
