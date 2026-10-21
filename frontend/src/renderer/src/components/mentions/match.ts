/**
 * 提及匹配:大小写不敏感的子串 / 前缀 / 子序列匹配,带命中区间(供高亮)。
 * 无外部依赖。排序在匹配分基础上叠加使用频率,并支持文件夹优先。
 */

/** 未命中哨兵(仅用于过滤判断,非评分) */
export const NO_MATCH = -1

export interface TextMatch {
  score: number
  /** UTF-16 下标区间 [start, end);同一文本内不重叠且有序 */
  ranges: [number, number][]
}

/** 单段文本匹配:全等 > 前缀 > 子串 > 子序列;未命中返回 null */
export function matchText(rawText: string, rawQuery: string): TextMatch | null {
  const text = rawText.toLowerCase()
  const query = rawQuery.toLowerCase()
  if (!query) return { score: 0, ranges: [] }
  if (!text) return null
  if (text === query) return { score: 10000, ranges: [[0, text.length]] }

  const idx = text.indexOf(query)
  if (idx >= 0) {
    const score = (idx === 0 ? 5000 : 2000) - idx * 2 - Math.min(text.length - query.length, 500)
    return { score, ranges: [[idx, idx + query.length]] }
  }

  // 子序列:逐字符命中,跨度越大扣分越多
  const ranges: [number, number][] = []
  let cursor = 0
  let gaps = 0
  let last = -2
  for (const ch of query) {
    const found = text.indexOf(ch, cursor)
    if (found < 0) return null
    gaps += found === last + 1 ? 0 : 1
    last = found
    const tail = ranges[ranges.length - 1]
    if (tail && tail[1] === found) tail[1] = found + 1
    else ranges.push([found, found + 1])
    cursor = found + 1
  }
  const score = 1000 - gaps * 5 - Math.min(text.length - query.length, 500)
  return { score, ranges }
}

/** 多字段取最高匹配分;query 非空且全不命中返回 NO_MATCH */
export function scoreTexts(texts: (string | undefined | null)[], query: string): number {
  if (!query) return 0
  let best = NO_MATCH
  for (const t of texts) {
    if (!t) continue
    const m = matchText(t, query)
    if (m && m.score > best) best = m.score
  }
  return best
}

export interface HighlightSegment {
  text: string
  hit: boolean
}

/** 按 query 把文本切成命中/未命中片段,供模板渲染 <mark> */
export function highlightSegments(text: string, query: string): HighlightSegment[] {
  const m = matchText(text, query)
  if (!m || !m.ranges.length) return text ? [{ text, hit: false }] : []
  const out: HighlightSegment[] = []
  let pos = 0
  for (const [s, e] of m.ranges) {
    if (s > pos) out.push({ text: text.slice(pos, s), hit: false })
    out.push({ text: text.slice(s, e), hit: true })
    pos = e
  }
  if (pos < text.length) out.push({ text: text.slice(pos), hit: false })
  return out
}

export interface RankOptions<T> {
  query: string
  /** 参与匹配的字段(顺序无关,取最高分) */
  textsOf: (item: T) => (string | undefined | null)[]
  /** 使用频率(命中 token 次数),叠加为排序加权 */
  usageOf?: (item: T) => number
  /** 同分时文件夹优先(用于 @ 文件列表) */
  foldersFirst?: boolean
  isFolder?: (item: T) => boolean
}

/** 过滤(未命中丢弃)并按匹配分 + 使用频率排序;同分保持原始顺序 */
export function rankItems<T>(items: T[], opts: RankOptions<T>): T[] {
  const { query } = opts
  const scored: { item: T; score: number }[] = []
  for (const item of items) {
    const score = scoreTexts(opts.textsOf(item), query)
    if (query && score === NO_MATCH) continue
    scored.push({ item, score })
  }
  scored.sort((a, b) => {
    const ra = a.score + (opts.usageOf?.(a.item) ?? 0) * 50
    const rb = b.score + (opts.usageOf?.(b.item) ?? 0) * 50
    if (rb !== ra) return rb - ra
    if (opts.foldersFirst) {
      const fa = opts.isFolder?.(a.item) ? 0 : 1
      const fb = opts.isFolder?.(b.item) ? 0 : 1
      if (fa !== fb) return fa - fb
    }
    return 0
  })
  return scored.map((s) => s.item)
}
