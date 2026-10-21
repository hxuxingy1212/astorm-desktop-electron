/**
 * 正文「文件引用」解析：**唯一实现**（模型在正文里提到的文件 → 可点击的引用，点开文件页签）。
 *
 * 两种触发形态（其余内容一律按原文渲染，不做猜测）：
 * - **行内代码**：`` `auth.py` ``、`` `auth.py:47` ``、`` `auth.py:47:5` ``、`` `auth.py#L47` ``、
 *   `` `auth.py:47-52` ``、`` `auth.py (line 47)` ``——模型引用文件天然会写反引号，这是主入口；
 * - **链接目标**：markdown 链接的 URL 是本地文件（`file:///abs/path#L47` / 绝对路径 / 相对路径），
 *   链接文字忽略不计（chip 自己按文件名 + 行号排版）。
 *
 * 判定用**扩展名白名单**（已知代码 / 配置 / 文档 / 数据 / 图片扩展名，或特名文件）：
 * 反引号里出现 `Math.max`、`pnpm -C frontend build`、`src/renderer/src`（目录）、`*.ts`（glob）
 * 都是常事，没有白名单会满篇假引用；真实文件引用几乎总带已知扩展名，未收录的宁可退化成普通行内代码。
 *
 * 解析只产出「路径 + 行号」；相对路径**不在这里拼接**——基准目录由运行环境注入
 * （对话正文 = 会话工作区，文件预览 = 该文件所在目录，见 `FILE_REF_BASE`），
 * 因此同一份正文在不同上下文里都能落到正确位置。
 */

/** 引用解析结果 */
export interface FileRef {
  /** 原始路径文本（相对路径保持相对，绝对路径原样） */
  path: string
  /** 起始行（1 起；无行号引用则缺省） */
  line?: number
  /** 结束行（引用形如 `f.py:47-52` 时给出，且大于起始行） */
  endLine?: number
}

/**
 * 已知文件扩展名（判定「像不像文件路径」）。
 * 只收常见的代码 / 配置 / 文档 / 表格 / 图片 / 归档类型——判据宁窄勿宽，不认识就退化成普通行内代码。
 */
const KNOWN_EXTS = new Set([
  // 代码
  'ts', 'tsx', 'mts', 'cts', 'js', 'jsx', 'mjs', 'cjs', 'vue', 'svelte', 'astro',
  'py', 'pyi', 'rs', 'go', 'java', 'kt', 'kts', 'scala', 'rb', 'php', 'swift', 'dart',
  'c', 'h', 'cc', 'cpp', 'cxx', 'hpp', 'hh', 'cs', 'lua', 'r', 'jl', 'pl', 'pm',
  'ex', 'exs', 'erl', 'hrl', 'clj', 'cljs', 'hs', 'elm', 'fs', 'ml', 'mli', 'nim', 'zig', 'groovy',
  'sh', 'bash', 'zsh', 'fish', 'ps1', 'bat', 'cmd', 'sql', 'graphql', 'gql',
  'proto', 'thrift', 'sol', 'asm', 's', 'v', 'sv', 'vhd', 'vhdl', 'ipynb', 'tf', 'tfvars', 'hcl',
  // 配置 / 数据
  'json', 'jsonc', 'json5', 'yaml', 'yml', 'toml', 'ini', 'cfg', 'conf', 'env', 'properties',
  'xml', 'plist', 'csv', 'tsv', 'lock', 'gradle', 'bzl', 'mk', 'sln', 'csproj', 'diff', 'patch',
  // 文档 / 办公
  'md', 'markdown', 'mdx', 'txt', 'log', 'rst', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'pdf', 'rtf',
  // 图片 / 媒体 / 归档
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg', 'ico', 'avif',
  'mp3', 'wav', 'ogg', 'mp4', 'mov', 'webm',
  'zip', 'tar', 'gz', 'bz2', 'xz', '7z', 'rar', 'jar', 'war', 'wasm', 'map'
])

/** 无扩展名但确实是文件的特名（大小写不敏感，按最后一段文件名比对） */
const SPECIAL_NAMES = new Set([
  'dockerfile', 'makefile', 'cmakelists.txt', 'license', 'notice',
  '.gitignore', '.gitattributes', '.dockerignore', '.npmrc', '.editorconfig'
])

/** 绝对路径：Windows 盘符 / UNC 或 POSIX 根 */
export function isAbsolutePath(path: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(path) || /^[\\/]{1,2}[^\\/]/.test(path)
}

/** 路径 → 文件名（展示用） */
export function fileRefName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

/**
 * 引用路径 → 打开用路径：绝对路径原样返回，相对路径接在 `base`（基准目录）下。
 * `base` 为空（未注入基准目录，如游离的 markdown 预览）时相对路径无从定位，返回空串。
 */
export function resolveFilePath(path: string, base: string): string {
  if (isAbsolutePath(path)) return path
  const b = (base ?? '').trim().replace(/[\\/]+$/, '')
  if (!b) return ''
  return `${b}/${path.replace(/^[\\/]+/, '')}`
}

/**
 * 路径文本是否**像**文件引用。
 * 排除项：空白（命令行 / 句子）、glob 与花括号（`*.ts`）、引号尖括号管道（代码片段）、
 * 变量与波浪号（`$HOME` / `~/x.py`）、等号（`--flag=1`）、带协议的 URL、`//host`、`#锚点`、目录（末段为空）。
 * `allowSpace` 供 `file://` URI 用：URI 里的空格本来就是百分号编码，解出来是真实路径的一部分。
 */
function looksLikeFile(path: string, allowSpace = false): boolean {
  const p = path.trim()
  if (!p || p.length > 200) return false
  if (allowSpace ? /["'`<>|*?{}=$~]/.test(p) : /[\s"'`<>|*?{}=$~]/.test(p)) return false
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(p)) return false
  if (p.startsWith('//') || p.startsWith('#')) return false
  const name = fileRefName(p)
  if (!name || name === '.' || name === '..') return false
  const lower = name.toLowerCase()
  if (SPECIAL_NAMES.has(lower) || lower.startsWith('.env')) return true
  const dot = lower.lastIndexOf('.')
  if (dot <= 0) return false
  return KNOWN_EXTS.has(lower.slice(dot + 1))
}

/** 行号合法（1 起） */
function validLine(n: number | undefined): number | undefined {
  return typeof n === 'number' && Number.isInteger(n) && n >= 1 ? n : undefined
}

/** 结束行：只在给出且大于起始行时保留 */
function resolveRange(line?: number, end?: number): Pick<FileRef, 'line' | 'endLine'> {
  const start = validLine(line)
  const stop = validLine(end)
  return { line: start, endLine: start && stop && stop > start ? stop : undefined }
}

/**
 * 剥掉行号后缀，返回路径本体与行号。
 * 支持的写法：`path:12`、`path:12:5`（列号忽略）、`path:12-20`、`path#L12`、`path#L12-L20`、
 * `path (line 12)` / `path (lines 12-20)`。Windows 盘符（`C:\dir\f.py`）不会被误当行号后缀。
 * 行号写了但非法（0 起 / 区间倒挂）→ 返回 null：整段不像正经引用，交给调用方按原文渲染。
 */
function splitLineSuffix(text: string): { path: string; line?: number; endLine?: number } | null {
  // 路径本体用惰性匹配：`a.py:47:5` 要在第一个冒号处切开（贪婪会把 `a.py:47` 当路径）
  let m = /^(.*?):(\d+)(?::\d+)?(?:-(\d+))?$/.exec(text)
  if (m) return invalidSuffix(Number(m[2])) ? null : { path: m[1], ...resolveRange(Number(m[2]), m[3] ? Number(m[3]) : undefined) }
  m = /^(.+)#L(\d+)(?:-L?(\d+))?$/i.exec(text)
  if (m) return invalidSuffix(Number(m[2])) ? null : { path: m[1], ...resolveRange(Number(m[2]), m[3] ? Number(m[3]) : undefined) }
  m = /^(.*?)\s*\((?:lines?|行)\s*(\d+)(?:\s*[-–—]\s*(\d+))?\)$/i.exec(text)
  if (m) return invalidSuffix(Number(m[2])) ? null : { path: m[1], ...resolveRange(Number(m[2]), m[3] ? Number(m[3]) : undefined) }
  return { path: text }
}

/** 行号后缀里的数字是否非法（`path:0`） */
function invalidSuffix(n: number): boolean {
  return !Number.isInteger(n) || n < 1
}

/**
 * 行内代码 → 文件引用（不像文件路径返回 null，调用方按普通行内代码渲染）。
 * 整段才算数：`见 auth.py 的登录逻辑` 这类夹在文字里的行内代码不会被截取。
 */
export function parseInlineFileRef(raw: string): FileRef | null {
  const text = (raw ?? '').trim()
  if (!text) return null
  const split = splitLineSuffix(text)
  if (!split || !looksLikeFile(split.path)) return null
  return { path: split.path.trim(), line: split.line, endLine: split.endLine }
}

/**
 * 链接 URL → 文件引用（非本地文件返回 null，调用方按普通链接渲染）。
 * `file://` URI 解出路径并按 RFC 去掉百分号编码；其余协议（http / mailto / vscode…）一律不算文件。
 */
export function parseFileHref(href: string): FileRef | null {
  const raw = (href ?? '').trim()
  if (!raw) return null

  let text = raw
  let uriPath = false
  if (/^file:\/\//i.test(text)) {
    // 只接 `file:///abs/path`（三斜杠 = 无主机）；`file://host/share` 形式不认
    if (!/^file:\/\/\//i.test(text)) return null
    try {
      text = decodeURIComponent(text.slice('file://'.length))
    } catch {
      return null
    }
    // file:///C:/Users/x/a.md 解出来是 /C:/Users/x/a.md；Windows 盘符路径要
    // 去掉前导斜杠（POSIX 的 /home/... 保持不变）。
    if (/^\/[A-Za-z]:[\\/]/.test(text)) text = text.slice(1)
    uriPath = true
  } else if (
    /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(text) || // http(s) / vscode / …（带主机部分）
    /^(?:mailto|tel|data|javascript|blob|about):/i.test(text) // 无主机的少数协议
  ) {
    return null
  }

  // 锚点里的行号（`#L47`）优先；随后丢弃查询串与其余锚点
  let line: number | undefined
  let endLine: number | undefined
  const hash = text.indexOf('#')
  if (hash >= 0) {
    const anchor = text.slice(hash + 1)
    text = text.slice(0, hash)
    const m = /^L(\d+)(?:-L?(\d+))?$/i.exec(anchor)
    if (m) ({ line, endLine } = resolveRange(Number(m[1]), m[2] ? Number(m[2]) : undefined))
  }
  const query = text.indexOf('?')
  if (query >= 0) text = text.slice(0, query)

  const split = splitLineSuffix(text)
  if (!split) return null
  const merged = { path: split.path, ...resolveRange(line ?? split.line, endLine ?? split.endLine) }
  if (!looksLikeFile(merged.path, uriPath)) return null
  return { path: merged.path.trim(), line: merged.line, endLine: merged.endLine }
}

/**
 * provide/inject 键：文件引用的**基准目录**（相对路径按它解析）。
 * 对话正文注入会话工作区（ChatPanel），文件预览注入所在目录（FilePreviewPanel）——
 * 注入的是 `Ref<string>`（工作区是异步解析出来的，注入引用才能跟着更新）。
 */
export const FILE_REF_BASE = Symbol('astorm:file-ref-base')
