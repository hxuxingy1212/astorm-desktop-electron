import type { MentionItem } from './types'
import { resolveToken } from './registry'
import { getFileIconUrl, getFolderIconUrl } from '@/utils/fileIcons'

/**
 * 纯文本 → 提及片段(输入框回显重建 chip 与消息侧渲染共用)。
 *
 * 识别规则:
 *   - `/xxx`、`$xxx`、`$agent/xxx`:仅当注册表能反查到条目才成为提及(否则是正文路径/命令名)
 *   - `@path`:启发式 —— 位于行首/空白后且形如路径(含 `/` 或扩展名)才成为文件提及,
 *     无需注册表即可还原(刷新后仍成立);邮箱等词中 `@` 因不满足前置空白被排除
 */

export interface FileMentionItem extends MentionItem {
  type: 'file'
}

export type ParsedSegment =
  | { type: 'text'; text: string }
  | { type: 'mention'; token: string; item: MentionItem }

/** 前置空白/行首 + 触发符 + 标识符(`$`/`@` 允许路径含 `/`) */
const TOKEN_RE = /(^|\s)(\/[\w.-]+|\$[A-Za-z0-9_.\-/]+|@[A-Za-z0-9_.\-/]+)/g

/** 形如路径:含目录分隔或带扩展名(用于排除 `@everyone` 这类非文件引用) */
function looksLikeFileRef(body: string): boolean {
  return body.includes('/') || /\.[A-Za-z0-9]+$/.test(body)
}

/** 由 `@` token 合成文件提及条目(无需注册表) */
export function fileItemFromToken(token: string): FileMentionItem {
  const body = token.startsWith('@') ? token.slice(1) : token
  const isFolder = body.endsWith('/')
  const trimmed = body.replace(/\/+$/, '')
  const name = trimmed.split('/').pop() || trimmed
  return {
    type: 'file',
    id: body,
    label: isFolder ? `${name}/` : name,
    icon: isFolder ? 'icon-folder' : 'icon-file',
    iconUrl: isFolder ? getFolderIconUrl() : getFileIconUrl(name),
    color: 'var(--accent-text)'
  }
}

/** 反查 token 对应条目;`@` 走启发式合成,`/`/`$` 走注册表 */
export function resolveMentionItem(token: string): MentionItem | null {
  const trigger = token[0]
  if (trigger === '@') {
    const body = token.slice(1)
    return looksLikeFileRef(body) ? fileItemFromToken(token) : null
  }
  if (trigger === '/' || trigger === '$') return resolveToken(token)
  return null
}

/** 把纯文本切分为文本/提及片段(相邻文本自动合并) */
export function parseMentionSegments(src: string): ParsedSegment[] {
  const out: ParsedSegment[] = []
  const pushText = (text: string): void => {
    if (!text) return
    const prev = out[out.length - 1]
    if (prev?.type === 'text') prev.text += text
    else out.push({ type: 'text', text })
  }

  let last = 0
  TOKEN_RE.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = TOKEN_RE.exec(src))) {
    const token = m[2]
    // 只消费 token 本身,前置空白留给文本片段
    const tokenStart = m.index + m[1].length
    pushText(src.slice(last, tokenStart))
    const item = resolveMentionItem(token)
    if (item) out.push({ type: 'mention', token, item })
    else pushText(token)
    last = tokenStart + token.length
  }
  pushText(src.slice(last))
  return out
}
