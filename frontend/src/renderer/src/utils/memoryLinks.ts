/**
 * 记忆 Bundle 内部链接解析：正文「相关」小节里的概念互链与原文链接都是**相对路径**
 * （`../entities/x.md` / `../../raw/sessions/y.md`），这里按当前概念的目录折算出落点。
 */

/** 内部链接目标：概念文档或 raw 原始会话。 */
export type MemoryLinkTarget =
  | { kind: 'concept'; id: string }
  | { kind: 'raw'; stem: string }
  | null

/**
 * 把正文里的相对链接解析成站内目标。
 *
 * 概念文件在 Bundle 里的位置是 `wiki/<dir>/<slug>.md`，所以从它出发按段折算出
 * href 的落点：落在 `wiki/` 下 ⇒ 概念（id = 去 `.md`）；落在 `raw/sessions/` 下 ⇒ 原文；
 * 其他（外链 / 锚点 / 越出 Bundle）⇒ null，交给浏览器默认行为。
 */
export function resolveMemoryLink(href: string, dir: string): MemoryLinkTarget {
  const path = href.split('#')[0].split('?')[0]
  if (!path || !path.toLowerCase().endsWith('.md')) return null
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(path) || path.startsWith('/')) return null
  const parts: string[] = ['wiki']
  for (const seg of dir.split('/')) if (seg) parts.push(seg)
  for (const seg of path.split('/')) {
    if (seg === '' || seg === '.') continue
    if (seg === '..') parts.pop()
    else parts.push(seg)
  }
  const file = parts.pop() ?? ''
  const slug = file.replace(/\.md$/i, '')
  if (parts[0] === 'wiki' && parts.length >= 1) {
    // wiki 根下的散装概念（`wiki/x.md`）与分类目录下的概念都要能跳
    return { kind: 'concept', id: [...parts.slice(1), slug].filter(Boolean).join('/') }
  }
  if (parts[0] === 'raw' && parts[1] === 'sessions') return { kind: 'raw', stem: slug }
  return null
}

/** 来源 `resource`（`raw/sessions/<stem>.md`）→ stem */
export function rawStemOf(resource: string): string {
  const name = resource.split('/').pop() ?? ''
  return name.replace(/\.md$/i, '')
}
