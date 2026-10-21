/**
 * 知识库文件协议(astorm-kb://doc/<relpath>):从知识库根目录真实读文件。
 *
 * 用于 markdown 等文本预览中相对路径的图片/资源——查看器只拿到文件字节(Blob),
 * 相对路径没有基准 URL,故渲染层把相对引用改写成 astorm-kb:// 绝对地址,
 * 此处按相对路径从知识库根读取并回正确的 Content-Type。
 */
import { protocol } from 'electron'
import { promises as fs } from 'fs'
import { isAbsolute, join, normalize, sep } from 'path'
import { isNapiLoaded, napiCall } from './napi'

const MIME: Record<string, string> = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp',
  svg: 'image/svg+xml', bmp: 'image/bmp', ico: 'image/x-icon', avif: 'image/avif',
  mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg', m4a: 'audio/mp4',
  mp4: 'video/mp4', webm: 'video/webm', mkv: 'video/x-matroska', mov: 'video/quicktime',
  woff: 'font/woff', woff2: 'font/woff2', ttf: 'font/ttf', otf: 'font/otf',
  pdf: 'application/pdf',
  md: 'text/markdown; charset=utf-8', mdx: 'text/markdown; charset=utf-8', txt: 'text/plain; charset=utf-8',
  html: 'text/html; charset=utf-8', htm: 'text/html; charset=utf-8', css: 'text/css; charset=utf-8',
  js: 'text/javascript; charset=utf-8', mjs: 'text/javascript; charset=utf-8', json: 'application/json',
  wasm: 'application/wasm'
}

/** 当前知识库根目录;后端未加载时为空(协议返回 404) */
let kbRoot = ''
export function resolveKbRoot(): string {
  if (isNapiLoaded()) {
    try {
      const root = napiCall('getKnowledgeRoot') as string
      if (typeof root === 'string' && root) kbRoot = root
    } catch {
      /* 后端不可用则保持空 */
    }
  }
  return kbRoot
}

/** 注册 astorm-kb 协议(scheme 权限在 ready 前注册,此处处理逻辑在 ready 后调用)。 */
export function registerKbProtocol(): void {
  protocol.handle('astorm-kb', async (request) => {
    const root = resolveKbRoot()
    if (!root) return new Response('knowledge base unavailable', { status: 404 })

    const url = new URL(request.url)
    // 形如 astorm-kb://doc/<seg1>/<seg2>/x.png —— pathname 为各段 encodeURIComponent 后的相对路径
    const segs = url.pathname
      .split('/')
      .filter(Boolean)
      .map((s) => { try { return decodeURIComponent(s) } catch { return s } })
    if (!segs.length) return new Response('Not Found', { status: 404 })

    const target = normalize(join(root, ...segs))
    // 越界防护:必须在知识库根目录之内
    if (isAbsolute(target) && !(target === root || target.startsWith(root + sep))) {
      return new Response('Forbidden', { status: 403 })
    }
    try {
      const buf = await fs.readFile(target)
      const ext = (target.split('.').pop() ?? '').toLowerCase()
      return new Response(buf, {
        headers: { 'content-type': MIME[ext] ?? 'application/octet-stream', 'cache-control': 'no-cache' }
      })
    } catch {
      return new Response('Not Found', { status: 404 })
    }
  })
}