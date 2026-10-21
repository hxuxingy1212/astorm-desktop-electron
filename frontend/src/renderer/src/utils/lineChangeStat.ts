/**
 * 行级增删统计（与 ZCode 的 lineChangeStat 同算法）：去公共前后缀 + 行级 LCS。
 *
 * 过程行的「编辑 +N -N」「写入 +N」需要**真实改动行数**，不能把 before/after 的
 * 总行数直接当成 +/-。本模块是该统计的唯一实现，供过程轨迹行复用。
 *
 * 超大文件强行算完整 LCS 会卡顿 → 超过单元格上限时退化为「全删再全加」的保守估算，
 * 优先保证交互流畅（与参考实现同口径）。
 */

/** LCS 单元格上限（超出退化为保守估算）。 */
const MAX_LCS_CELLS = 400_000

export interface LineChangeStat {
  added: number
  removed: number
}

/** 拆逻辑行：末尾单个换行不算多出一行（与编辑器行号一致）。 */
export function splitLogicalLines(content: string | null | undefined): string[] {
  if (!content) return []
  const lines = content.split('\n')
  if (lines[lines.length - 1] === '') lines.pop()
  return lines
}

/** 计算 before → after 的真实增删行数。 */
export function computeLineChangeStat(
  beforeContent: string | null | undefined,
  afterContent: string | null | undefined
): LineChangeStat {
  const beforeLines = splitLogicalLines(beforeContent)
  const afterLines = splitLogicalLines(afterContent)

  // 公共前缀
  let prefix = 0
  while (
    prefix < beforeLines.length &&
    prefix < afterLines.length &&
    beforeLines[prefix] === afterLines[prefix]
  ) {
    prefix += 1
  }

  // 公共后缀
  let beforeTail = beforeLines.length - 1
  let afterTail = afterLines.length - 1
  while (
    beforeTail >= prefix &&
    afterTail >= prefix &&
    beforeLines[beforeTail] === afterLines[afterTail]
  ) {
    beforeTail -= 1
    afterTail -= 1
  }

  const trimmedBefore = beforeLines.slice(prefix, beforeTail + 1)
  const trimmedAfter = afterLines.slice(prefix, afterTail + 1)

  if (trimmedBefore.length === 0) return { added: trimmedAfter.length, removed: 0 }
  if (trimmedAfter.length === 0) return { added: 0, removed: trimmedBefore.length }

  if (trimmedBefore.length * trimmedAfter.length > MAX_LCS_CELLS) {
    return { added: trimmedAfter.length, removed: trimmedBefore.length }
  }

  // 行级 LCS（滚动一维数组，previousDiagonal 保存上一行左上角的值）
  const lcs = new Array<number>(trimmedAfter.length + 1).fill(0)
  for (let i = 1; i <= trimmedBefore.length; i += 1) {
    let previousDiagonal = 0
    for (let j = 1; j <= trimmedAfter.length; j += 1) {
      const previousRow = lcs[j]
      if (trimmedBefore[i - 1] === trimmedAfter[j - 1]) {
        lcs[j] = previousDiagonal + 1
      } else {
        lcs[j] = Math.max(lcs[j], lcs[j - 1])
      }
      previousDiagonal = previousRow
    }
  }

  const unchanged = lcs[trimmedAfter.length] ?? 0
  return {
    added: trimmedAfter.length - unchanged,
    removed: trimmedBefore.length - unchanged
  }
}
