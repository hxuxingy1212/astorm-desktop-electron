import { findElementBlocks } from '@/utils/elementBlock'
import { parseMentionSegments } from './parse'
import type { MentionType } from './types'

/**
 * 由最终发送文本派生结构化 mentions(随请求下发后端)。
 *
 * - 复用 [`parseMentionSegments`]:`$`/`/` 经注册表反查、`@` 走启发式;
 * - **跳过 ```element 围栏块**(拾取元素的 XPath/CSS 里含 `/`、`@` 会被误判);
 * - 跳过 command(`/`,后端暂不处理);agent 的 id 去掉 `agent/` 前缀(后端要裸 id);
 * - 按 token 去重,保持出现顺序。
 */
export interface MentionRefPayload {
  kind: MentionType
  /** skill 名 / agentId(裸) / 文件相对路径 / command id */
  id: string
  /** 原文 token,如 '$agent/builtin-explore'、'@src/foo.ts' */
  token: string
  label?: string
}

export function deriveMentionRefs(text: string): MentionRefPayload[] {
  if (!text) return []
  const out: MentionRefPayload[] = []
  const seen = new Set<string>()

  const collect = (chunk: string): void => {
    for (const seg of parseMentionSegments(chunk)) {
      if (seg.type !== 'mention') continue
      const item = seg.item
      if (item.type === 'command') continue
      if (seen.has(seg.token)) continue
      seen.add(seg.token)
      out.push({
        kind: item.type,
        id: item.type === 'agent' ? item.id.replace(/^agent\//, '') : item.id,
        token: seg.token,
        label: item.label
      })
    }
  }

  let last = 0
  for (const block of findElementBlocks(text)) {
    if (block.start > last) collect(text.slice(last, block.start))
    last = block.end
  }
  if (last < text.length) collect(text.slice(last))
  return out
}
