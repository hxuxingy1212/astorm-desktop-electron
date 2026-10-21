/**
 * YAML frontmatter 的切分与解析（文件预览 / 记忆页共用的「元数据块」数据源）。
 *
 * 为什么需要它：CommonMark 里 `---` 会把**上一段**升级成 setext 二级标题，带 frontmatter 的
 * Markdown（如 SKILL.md）直接交给 markdown 渲染器，元数据会变成一整块巨型粗体标题。
 * 这里把元数据切出来交给 `MarkdownFrontmatterHeader` 呈现（与记忆页观感一致），正文再照常渲染。
 *
 * 解析的是 frontmatter 里实际会用到的一小段 YAML 子集：标量 / 行内数组（`[a, b]`）/
 * 块序列（`- a`）/ 序列里的小映射（`- resource: x`）/ 一层嵌套映射 / 块标量（`|` `>`）。
 *
 * **失败一律降级**：切不出闭合、形状不像 YAML、或语义上解析不了，都返回 null ——
 * 调用方按「没有 frontmatter」处理（原文一字不动地照常渲染，不丢内容、不报错）。
 *
 * 字段名与记忆页（OKF 概念文档）对齐：snake_case 与 camelCase 都接受。
 */

/** 来源引用（memory 概念文档的 `sources` 条目） */
export interface FrontmatterSourceRef {
  /** 资源路径（`resource` / `path` / `file`） */
  resource: string
  title: string | null
  lastModified: string | null
}

/** 未映射字段的保底展示（标量 → 细行；标量数组 → chips） */
export interface FrontmatterExtra {
  key: string
  /** 标量值（数组/映射为串行化文本，仅用于展示） */
  value: string
  /** 标量数组：逐项 chips（非数组为空） */
  items: string[]
}

/** 元数据块的数据模型（标题行 + 徽标 + chips + 细行） */
export interface FrontmatterMeta {
  /** 标题行：`title` / `name`（缺省则不渲染标题行） */
  title: string
  /** 徽标：`type` */
  type: string
  /** 徽标：`status` */
  status: string
  /** 描述行：`description` */
  description: string
  /** chips：`tags` */
  tags: string[]
  /** 徽标（观察于）：`observed_at` */
  observedAt: string
  /** 徽标（更新于）：`updated_at` */
  updatedAt: string
  /** 细行：`sources` */
  sources: FrontmatterSourceRef[]
  /** 细行（依据）：`frag_ids` */
  fragIds: string[]
  /** 细行（生成者）：`generated.by` / `generated_by` */
  generatedBy: string
  /** 细行（生成时间）：`generated.at` / `generated_at` */
  generatedAt: string
  /** 细行（模型）：`generated.model` / `model` */
  model: string
  /** 其余未映射字段（保底展示，避免信息静默丢失） */
  extras: FrontmatterExtra[]
  /** 原始 YAML 文本 */
  raw: string
}

/** frontmatter 切分结果 */
export interface FrontmatterSplit {
  /** `---` 与 `---` 之间的 YAML 原文（不含首尾围栏行） */
  yaml: string
  /** 正文（闭合围栏之后的内容） */
  body: string
  /** 闭合围栏的行号（0 基，含） */
  endLine: number
}

export interface ParsedFrontmatter {
  meta: FrontmatterMeta
  body: string
}

/* ---------- 切分 ---------- */

type YamlNode = string | YamlNode[] | { [key: string]: YamlNode }

interface YamlLine {
  indent: number
  text: string
  /** `- ` 序列项 */
  isSeq: boolean
}

/** `key:` / `key: value`（值允许为空；键不含冒号与空白，值必须与冒号间有空白，避免把 URL 当键值） */
const KEY_VALUE = /^([^\s:]+)\s*:(?:\s+([\s\S]*))?$/
/** 块标量标记：`|` `>`（可带缩进指示符） */
const BLOCK_SCALAR = /^[|>][+-]?\d?$/
/** 文档首行的 frontmatter 起始围栏 */
const OPEN_FENCE = /^---[ \t]*$/
/** 闭合围栏（`---`；`...` 是 YAML 文档结束标记，一并接受） */
const CLOSE_FENCE = /^(---|\.\.\.)[ \t]*$/

/**
 * 切出 frontmatter（不含围栏行）与正文。
 * 无起始围栏 / 无闭合围栏 / 区段不像 YAML 映射 → null（调用方按普通 markdown 处理）。
 */
export function splitFrontmatter(md: string): FrontmatterSplit | null {
  const src = md.startsWith('\uFEFF') ? md.slice(1) : md
  const lines = src.split('\n')
  if (!OPEN_FENCE.test(stripCr(lines[0] ?? ''))) return null

  let end = -1
  for (let i = 1; i < lines.length; i++) {
    if (CLOSE_FENCE.test(stripCr(lines[i]))) {
      end = i
      break
    }
  }
  if (end === -1) return null

  const yaml = lines.slice(1, end).map(stripCr).join('\n')
  // 形状校验：至少要有一行顶层 `key:`，否则（含空 frontmatter `---\n---`）当普通正文
  if (!yaml.split('\n').some((l) => l.trim() !== '' && !isComment(l) && KEY_VALUE.test(l.trim()))) {
    return null
  }

  // 正文：吃掉闭合围栏后紧随的一个空行（与记忆页 stripFrontmatter 的观感一致）
  let start = end + 1
  if (start < lines.length && stripCr(lines[start]).trim() === '') start += 1
  return { yaml, body: lines.slice(start).map(stripCr).join('\n'), endLine: end }
}

/**
 * 解析 frontmatter 为元数据模型。
 * 无 frontmatter / 形状不符 / YAML 解析失败 / 解析结果为空（无任何可展示字段）→ null。
 */
export function parseFrontmatter(md: string): ParsedFrontmatter | null {
  const split = splitFrontmatter(md)
  if (!split) return null
  const record = parseYamlSubset(split.yaml)
  if (!record) return null
  const meta = toMeta(record, split.yaml)
  if (!hasContent(meta)) return null
  return { meta, body: split.body }
}

/** 只取正文（无 frontmatter 或解析失败时原样返回，绝不丢内容） */
export function stripFrontmatter(md: string): string {
  const split = splitFrontmatter(md)
  return split ? split.body : md
}

/* ---------- YAML 子集解析 ---------- */

function stripCr(line: string): string {
  return line.endsWith('\r') ? line.slice(0, -1) : line
}

function isComment(line: string): boolean {
  return line.trimStart().startsWith('#')
}

/** 预处理：去空行 / 整行注释，记录缩进与序列标记（行内注释留到取值时按引号感知剥离） */
function toLines(yaml: string): YamlLine[] {
  const out: YamlLine[] = []
  for (const raw of yaml.split('\n')) {
    const line = stripCr(raw).replace(/\s+$/, '')
    if (line.trim() === '' || isComment(line)) continue
    const indent = line.length - line.trimStart().length
    const text = line.trim()
    out.push({ indent, text, isSeq: text === '-' || text.startsWith('- ') })
  }
  return out
}

/** 解析失败返回 null（调用方降级为普通正文渲染） */
function parseYamlSubset(yaml: string): { [key: string]: YamlNode } | null {
  const lines = toLines(yaml)
  if (lines.length === 0) return null
  if (lines[0].indent !== 0 || lines[0].isSeq) return null
  return parseMapBlock(lines, 0)
}

/** 一个映射块：首行缩进为 `indent`，更深缩进的行属于上一个键的值 */
function parseMapBlock(lines: YamlLine[], indent: number): { [key: string]: YamlNode } | null {
  const map: { [key: string]: YamlNode } = {}
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (line.indent !== indent) return null
    if (line.isSeq) return null
    const kv = splitKeyValue(line.text)
    if (!kv) return null
    i++

    if (kv.value !== '') {
      if (BLOCK_SCALAR.test(kv.value)) {
        const buf: string[] = []
        while (i < lines.length && lines[i].indent > indent) {
          buf.push(lines[i].text)
          i++
        }
        map[kv.key] = buf.join('\n')
      } else {
        map[kv.key] = parseScalar(kv.value)
      }
      continue
    }

    // 空值：无更深缩进 → 空标量；否则是「更深缩进的块」（序列或嵌套映射）
    if (i >= lines.length || lines[i].indent <= indent) {
      map[kv.key] = ''
      continue
    }
    const blockStart = i
    while (i < lines.length && lines[i].indent > indent) i++
    const block = lines.slice(blockStart, i)
    const child = lines[blockStart].isSeq
      ? parseSeqBlock(block, block[0].indent)
      : parseMapBlock(block, block[0].indent)
    if (child === null) return null
    map[kv.key] = child
  }
  return map
}

/** 一个序列块：所有项缩进为 `indent`；项的续行（更深缩进）并入该项 */
function parseSeqBlock(lines: YamlLine[], indent: number): YamlNode[] | null {
  const arr: YamlNode[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (line.indent !== indent || !line.isSeq) return null
    const inner = line.text === '-' ? '' : line.text.slice(1).trim()
    i++
    const cont: YamlLine[] = []
    while (i < lines.length && lines[i].indent > indent) {
      cont.push(lines[i])
      i++
    }

    if (inner === '') {
      if (cont.length === 0) {
        arr.push('')
        continue
      }
      const child = cont[0].isSeq ? parseSeqBlock(cont, cont[0].indent) : parseMapBlock(cont, cont[0].indent)
      if (child === null) return null
      arr.push(child)
      continue
    }

    const kv = splitKeyValue(inner)
    if (kv) {
      // `- key: value` + 后续更深缩进的键值 → 一个映射项。
      // 映射的基准缩进 = `-` 之后的第一个键所在列（YAML 的序列项映射就是按这个列对齐的）
      const afterDash = line.text.slice(1)
      const keyIndent = indent + 1 + (afterDash.length - afterDash.trimStart().length)
      const item = parseMapBlock([{ indent: keyIndent, text: inner, isSeq: false }, ...cont], keyIndent)
      if (item === null) return null
      arr.push(item)
      continue
    }

    // 多行标量（折叠成一行）
    arr.push(cont.length > 0 ? [inner, ...cont.map((l) => l.text)].join(' ') : parseScalar(inner))
  }
  return arr
}

function splitKeyValue(text: string): { key: string; value: string } | null {
  const m = KEY_VALUE.exec(text)
  if (!m) return null
  // 值先做一次引号感知的行内注释剥离（`name: x  # 注释`、`description: "含 # 的串"` 都能对上）
  return { key: unquote(m[1]), value: stripInlineComment((m[2] ?? '').trim()).trim() }
}

/** 标量：行内数组 / 行内映射 / 引号串 / 裸串（永不返回 null，展示层兜底） */
function parseScalar(text: string): YamlNode {
  const t = stripInlineComment(text).trim()
  if (t.startsWith('[') && t.endsWith(']')) {
    return splitFlow(t.slice(1, -1)).map((part) => parseScalar(part))
  }
  if (t.startsWith('{') && t.endsWith('}')) {
    const map: { [key: string]: YamlNode } = {}
    for (const part of splitFlow(t.slice(1, -1))) {
      const kv = splitKeyValue(part.trim())
      if (!kv) return t
      map[kv.key] = parseScalar(kv.value)
    }
    return map
  }
  return unquote(t)
}

/** 按顶层逗号切分（尊重引号），用于 `[a, b]` / `{a: b}` */
function splitFlow(inner: string): string[] {
  const parts: string[] = []
  let buf = ''
  let quote = ''
  for (let i = 0; i < inner.length; i++) {
    const ch = inner[i]
    if (quote) {
      buf += ch
      if (ch === '\\' && quote === '"') {
        buf += inner[i + 1] ?? ''
        i++
      } else if (ch === quote) quote = ''
      continue
    }
    if (ch === '"' || ch === "'") {
      quote = ch
      buf += ch
      continue
    }
    if (ch === ',') {
      parts.push(buf)
      buf = ''
      continue
    }
    buf += ch
  }
  if (buf !== '') parts.push(buf)
  return parts.map((p) => p.trim()).filter((p) => p !== '')
}

/** 去掉行内注释：引号串截到配对的引号为止；裸串截到空白 + `#` */
function stripInlineComment(text: string): string {
  const quote = text[0]
  if (quote === '"' || quote === "'") {
    for (let i = 1; i < text.length; i++) {
      if (text[i] === '\\' && quote === '"') {
        i++
        continue
      }
      if (text[i] === quote) return text.slice(0, i + 1)
    }
    return text
  }
  const at = text.search(/\s#/)
  return at === -1 ? text : text.slice(0, at)
}

function unquote(text: string): string {
  const t = text.trim()
  if (t.length >= 2 && t.startsWith('"') && t.endsWith('"')) {
    try {
      return JSON.parse(t) as string
    } catch {
      return t.slice(1, -1)
    }
  }
  if (t.length >= 2 && t.startsWith("'") && t.endsWith("'")) {
    return t.slice(1, -1).replace(/''/g, "'")
  }
  return t
}

/* ---------- 映射到元数据模型 ---------- */

/** 已映射字段（不再进 extras） */
const MAPPED_KEYS = new Set([
  'title', 'name', 'type', 'status', 'description', 'tags',
  'observed_at', 'observedAt', 'updated_at', 'updatedAt',
  'sources', 'source', 'frag_ids', 'fragIds',
  'generated', 'generated_by', 'generatedBy', 'generated_at', 'generatedAt', 'model'
])

function pick(record: { [k: string]: YamlNode }, ...keys: string[]): YamlNode | undefined {
  for (const key of keys) {
    const v = record[key]
    if (v !== undefined) return v
  }
  return undefined
}

/** 标量 → 展示文本；数组/映射串行化（不丢信息） */
function scalarText(node: YamlNode | undefined): string {
  if (node === undefined) return ''
  if (typeof node === 'string') return node
  if (Array.isArray(node)) return node.map(itemText).filter(Boolean).join(', ')
  return Object.entries(node)
    .map(([k, v]) => `${k}=${itemText(v)}`)
    .join(', ')
}

function itemText(node: YamlNode): string {
  if (typeof node === 'string') return node
  if (Array.isArray(node)) return node.map(itemText).filter(Boolean).join(' ')
  return Object.entries(node)
    .map(([k, v]) => `${k}=${itemText(v)}`)
    .join(' ')
}

function stringList(node: YamlNode | undefined): string[] {
  if (node === undefined) return []
  const arr = Array.isArray(node) ? node : [node]
  return arr.map((it) => (typeof it === 'string' ? it : itemText(it))).filter((s) => s !== '')
}

function toSources(node: YamlNode | undefined): FrontmatterSourceRef[] {
  if (node === undefined) return []
  const arr = Array.isArray(node) ? node : [node]
  const out: FrontmatterSourceRef[] = []
  for (const item of arr) {
    if (typeof item === 'string') {
      if (item !== '') out.push({ resource: item, title: null, lastModified: null })
      continue
    }
    if (Array.isArray(item)) continue
    const resource = scalarText(pick(item, 'resource', 'path', 'file', 'url'))
    const title = scalarText(pick(item, 'title', 'name'))
    const lastModified = scalarText(pick(item, 'last_modified', 'lastModified', 'modified'))
    if (resource === '' && title === '') continue
    out.push({
      resource: resource || title,
      title: title || null,
      lastModified: lastModified || null
    })
  }
  return out
}

function toMeta(record: { [k: string]: YamlNode }, raw: string): FrontmatterMeta {
  const generated = pick(record, 'generated')
  const generatedMap =
    generated && typeof generated === 'object' && !Array.isArray(generated) ? generated : undefined
  // `generated` 写成标量（非 OKF 的 `by/at/model` 结构）时按生成者展示，别静默丢掉
  const generatedScalar = typeof generated === 'string' ? generated : ''

  const extras: FrontmatterExtra[] = []
  for (const [key, value] of Object.entries(record)) {
    if (MAPPED_KEYS.has(key)) continue
    const items = Array.isArray(value) && value.every((v) => typeof v === 'string') ? (value as string[]) : []
    extras.push({ key, value: scalarText(value), items: items.filter((s) => s !== '') })
  }

  return {
    title: scalarText(pick(record, 'title', 'name')),
    type: scalarText(pick(record, 'type')),
    status: scalarText(pick(record, 'status')),
    description: scalarText(pick(record, 'description')),
    tags: stringList(pick(record, 'tags')),
    observedAt: scalarText(pick(record, 'observed_at', 'observedAt')),
    updatedAt: scalarText(pick(record, 'updated_at', 'updatedAt')),
    sources: toSources(pick(record, 'sources', 'source')),
    fragIds: stringList(pick(record, 'frag_ids', 'fragIds')),
    generatedBy:
      scalarText(pick(generatedMap ?? {}, 'by')) ||
      generatedScalar ||
      scalarText(pick(record, 'generated_by', 'generatedBy')),
    generatedAt: scalarText(pick(generatedMap ?? {}, 'at')) || scalarText(pick(record, 'generated_at', 'generatedAt')),
    model: scalarText(pick(generatedMap ?? {}, 'model')) || scalarText(pick(record, 'model')),
    extras,
    raw
  }
}

/** 有可展示内容？（空 frontmatter 不进元数据块） */
function hasContent(meta: FrontmatterMeta): boolean {
  return Boolean(
    meta.title ||
      meta.type ||
      meta.status ||
      meta.description ||
      meta.tags.length > 0 ||
      meta.observedAt ||
      meta.updatedAt ||
      meta.sources.length > 0 ||
      meta.fragIds.length > 0 ||
      meta.generatedBy ||
      meta.generatedAt ||
      meta.model ||
      meta.extras.length > 0
  )
}

/** 空元数据（记忆页把后端 DTO 映射过来时的安全初值） */
export function emptyFrontmatterMeta(): FrontmatterMeta {
  return {
    title: '',
    type: '',
    status: '',
    description: '',
    tags: [],
    observedAt: '',
    updatedAt: '',
    sources: [],
    fragIds: [],
    generatedBy: '',
    generatedAt: '',
    model: '',
    extras: [],
    raw: ''
  }
}
