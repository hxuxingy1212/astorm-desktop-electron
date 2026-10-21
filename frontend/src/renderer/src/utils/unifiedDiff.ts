/**
 * before/after → 标准 unified diff 文本（供 `CodeDiffView` 渲染）。
 *
 * 与 `lineChangeStat` 同源：去公共前后缀 + 行级 LCS，产出单个覆盖全文的 hunk。
 * 这里处理的是**编辑片段**（old_text/new_text / 写入 content），不是整份文件，
 * 因此行号为片段内相对行号；整份文件带真实上下文由后端 `file_diff` 提供。
 *
 * 超过单元格上限时退化为「全删再全加」（仍是合法 diff，只是上下文丢失）。
 */
import { splitLogicalLines } from './lineChangeStat'

/** LCS 单元格上限（超出退化为全删全加）。 */
const MAX_DIFF_LCS_CELLS = 60_000

export interface UnifiedDiff {
  /** unified diff 文本 */
  patch: string
  /** after 侧逻辑行数（`CodeDiffView` 的 newLines，用于补尾部折叠条） */
  newLines: number
}

function hunkRange(lineCount: number): string {
  return lineCount === 0 ? '0,0' : `1,${lineCount}`
}

/** 把 before/after 的行差追加成 ` ` / `+` / `-` 前缀行。 */
function appendDiffBody(out: string[], beforeLines: string[], afterLines: string[]): void {
  if (beforeLines.length === 0) {
    for (const line of afterLines) out.push(`+${line}`)
    return
  }
  if (afterLines.length === 0) {
    for (const line of beforeLines) out.push(`-${line}`)
    return
  }
  if (beforeLines.length * afterLines.length > MAX_DIFF_LCS_CELLS) {
    for (const line of beforeLines) out.push(`-${line}`)
    for (const line of afterLines) out.push(`+${line}`)
    return
  }

  const n = beforeLines.length
  const m = afterLines.length
  // lcs[i][j] = before[i..] 与 after[j..] 的最长公共子序列长度
  const lcs = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i][j] =
        beforeLines[i] === afterLines[j]
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1])
    }
  }

  let i = 0
  let j = 0
  while (i < n || j < m) {
    if (i < n && j < m && beforeLines[i] === afterLines[j]) {
      out.push(` ${beforeLines[i]}`)
      i += 1
      j += 1
      continue
    }
    if (i >= n) {
      out.push(`+${afterLines[j]}`)
      j += 1
      continue
    }
    if (j >= m) {
      out.push(`-${beforeLines[i]}`)
      i += 1
      continue
    }
    // 优先保留能延长公共子序列的一侧（与参考实现同口径）
    if (lcs[i][j + 1] >= lcs[i + 1][j]) {
      out.push(`+${afterLines[j]}`)
      j += 1
    } else {
      out.push(`-${beforeLines[i]}`)
      i += 1
    }
  }
}

/**
 * 构造单个 hunk 的 unified diff。`fileLabel` 用于 `--- a/…` / `+++ b/…` 头。
 * before 为空 = 新建文件（`--- /dev/null`），after 为空 = 删除。
 */
export function buildUnifiedDiff(
  before: string | null | undefined,
  after: string | null | undefined,
  fileLabel: string
): UnifiedDiff {
  const beforeLines = splitLogicalLines(before)
  const afterLines = splitLogicalLines(after)
  const created = beforeLines.length === 0 && afterLines.length > 0
  const deleted = beforeLines.length > 0 && afterLines.length === 0
  const out = [
    `--- ${created ? '/dev/null' : `a/${fileLabel}`}`,
    `+++ ${deleted ? '/dev/null' : `b/${fileLabel}`}`,
    `@@ -${hunkRange(beforeLines.length)} +${hunkRange(afterLines.length)} @@`
  ]
  appendDiffBody(out, beforeLines, afterLines)
  return { patch: out.join('\n'), newLines: afterLines.length }
}
