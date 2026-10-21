/**
 * 文件操作行（读取/写入/编辑/追加）与目录类结果（列目录/查找/搜索）的展示派生。
 *
 * 数据优先级：
 * 1. 后端结构化元数据（`tool.fileDiff` / `tool.readStats`）—— edit 带整份文件的限上下文
 *    unified diff；read 带精确行/字符数；
 * 2. 前端从入参/结果派生 —— edit 用 `old_text/new_text`、write 用 `content`、
 *    read 数结果文本、list/glob/grep 数结果行。
 *
 * **性能约束**：本模块会被 `ChatStepList` 的 `rows` computed 调用，而该 computed 会随
 * 思考逐 token 重算。因此用 WeakMap 缓存（key 含 status/result 长度，结果到达后失效重算），
 * 常规命中为 O(1)，LCS/diff 只在首次计算。
 */
import type { ToolCallView } from '@/types'
import { getFileIconUrl } from '@/utils/fileIcons'
import { computeLineChangeStat, splitLogicalLines } from '@/utils/lineChangeStat'
import { buildUnifiedDiff } from '@/utils/unifiedDiff'
import { actionOf } from './toolDisplay'

export interface FileChange {
  /** 路径（原样，供 CodeDiffView 按扩展名选语言） */
  path: string
  /** 文件名（含扩展名） */
  fileName: string
  /** 目录部分（含尾部 `/`；无目录为空串） */
  fileDir: string
  /** 文件树同款文件图标 URL */
  fileIconUrl: string
  /** edit：新增行数 */
  added: number
  /** edit：删除行数 */
  removed: number
  /** 写类：unified diff（仅 write；append/truncated 时 undefined） */
  patch?: string
  newLines?: number
  /** 读取（read）/写入（write/append）：行数 / 字符数 */
  lines?: number
  chars?: number
  /** 后端标记 patch 被截断（只显示计数） */
  truncated?: boolean
}

interface CacheEntry {
  key: string
  value: FileChange | null
}

const changeCache = new WeakMap<ToolCallView, CacheEntry>()
const countCache = new WeakMap<ToolCallView, { key: string; value: ToolCount | null }>()

/** 缓存 key：params 不变，只有 status/result 变化（结果到达后重算一次）。 */
function cacheKeyOf(tool: ToolCallView): string {
  return `${tool.status}:${tool.result?.length ?? -1}:${tool.fileDiff ? 1 : 0}:${tool.readStats ? 1 : 0}`
}

function strOf(params: unknown, key: string): string {
  const v = (params as Record<string, unknown> | null | undefined)?.[key]
  return typeof v === 'string' ? v : ''
}

function splitPath(path: string): { fileName: string; fileDir: string } {
  const idx = path.lastIndexOf('/')
  if (idx < 0) return { fileName: path, fileDir: '' }
  return { fileName: path.slice(idx + 1), fileDir: path.slice(0, idx + 1) }
}

/** read/list/glob/grep 结果里的机器附加文本（路径纠正说明 / 截断提示）不计入统计。 */
function stripResultExtras(text: string): string {
  return text
    .replace(/^（请求路径多了一层同名目录[\s\S]*?）\n/, '')
    .replace(/\n?…（内容过长已截断）$/, '')
}

function baseOf(path: string): Omit<FileChange, 'added' | 'removed'> {
  const { fileName, fileDir } = splitPath(path)
  return { path, fileName, fileDir, fileIconUrl: getFileIconUrl(path) }
}

/** 读取：优先后端精确统计，否则从结果文本数行/字符。 */
function readChange(tool: ToolCallView, path: string): FileChange {
  const base = baseOf(path)
  if (tool.readStats) {
    return { ...base, added: 0, removed: 0, lines: tool.readStats.lines, chars: tool.readStats.chars }
  }
  if (!tool.result) return { ...base, added: 0, removed: 0 }
  const text = stripResultExtras(tool.result)
  return {
    ...base,
    added: 0,
    removed: 0,
    lines: splitLogicalLines(text).length,
    chars: [...text].length
  }
}

function derive(tool: ToolCallView): FileChange | null {
  if (tool.name !== 'file_operation') return null
  const action = actionOf(tool.name, tool.params)
  const path = strOf(tool.params, 'path')
  if (!path) return null

  // 编辑：+N -N + diff
  if (action === 'edit') {
    if (tool.fileDiff) {
      const { additions, deletions, patch, newLines, truncated } = tool.fileDiff
      return { ...baseOf(path), added: additions, removed: deletions, patch, newLines, truncated }
    }
    const before = strOf(tool.params, 'old_text')
    const after = strOf(tool.params, 'new_text')
    const stat = computeLineChangeStat(before, after)
    const diff = buildUnifiedDiff(before, after, splitPath(path).fileName)
    return { ...baseOf(path), added: stat.added, removed: stat.removed, ...diff }
  }

  // 写入 / 追加：N行M字符（write 展开另有 diff）
  if (action === 'write' || action === 'append') {
    const content = strOf(tool.params, 'content')
    const page: FileChange = {
      ...baseOf(path),
      added: 0,
      removed: 0,
      lines: splitLogicalLines(content).length,
      chars: [...content].length
    }
    if (action === 'write') {
      const fallback = buildUnifiedDiff('', content, splitPath(path).fileName)
      return {
        ...page,
        patch: tool.fileDiff?.patch ?? fallback.patch,
        newLines: tool.fileDiff?.newLines ?? fallback.newLines,
        truncated: tool.fileDiff?.truncated
      }
    }
    return page
  }

  if (action === 'read' || action === '') return readChange(tool, path)

  // list / glob / grep 无单一文件（计数见 toolCountOf）
  return null
}

const NO_MATCH = '未找到匹配'

/** 目录类结果的计数（列目录/查找文件 → files；搜索文件内容 → matches）。 */
export interface ToolCount {
  count: number
  unit: 'files' | 'matches'
}

/**
 * 列目录 / 查找文件 → `N个文件`，搜索文件内容 → `N处` 的计数。
 * 结果未到达或失败时返回 null（不显示）。
 */
export function toolCountOf(tool: ToolCallView): ToolCount | null {
  if (tool.name !== 'file_operation') return null
  const action = actionOf(tool.name, tool.params)
  if (action !== 'list' && action !== 'glob' && action !== 'grep') return null
  if (tool.status === 'error' || !tool.result) return null

  const key = `${tool.status}:${tool.result.length}`
  const hit = countCache.get(tool)
  if (hit && hit.key === key) return hit.value

  const text = stripResultExtras(tool.result).trim()
  const unit: ToolCount['unit'] = action === 'grep' ? 'matches' : 'files'
  const count =
    !text || text === NO_MATCH
      ? 0
      : text.split('\n').filter((line) => line.trim().length > 0).length
  const value: ToolCount = { count, unit }
  countCache.set(tool, { key, value })
  return value
}

/** 取文件操作行的展示数据（无则 null，调用方回退普通工具行）。 */
export function fileChangeOf(tool: ToolCallView): FileChange | null {
  const key = cacheKeyOf(tool)
  const hit = changeCache.get(tool)
  if (hit && hit.key === key) return hit.value
  const value = derive(tool)
  changeCache.set(tool, { key, value })
  return value
}
