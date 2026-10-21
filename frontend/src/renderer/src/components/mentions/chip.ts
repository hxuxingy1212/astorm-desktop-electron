import type { MentionItem, MentionTrigger } from './types'
import { TRIGGER_BY_TYPE } from './types'
import { parseMentionSegments } from './parse'

/**
 * 输入框 chip 的纯 DOM 工具(contenteditable 内无法用 Vue 渲染,统一走这里):
 *   - getCaretToken  检测光标前是否正在输入一个提及 token
 *   - commitChip     把 token 文本替换为 chip 元素并恢复光标
 *   - createChipElement / serializeEditor  chip 构造与编辑器序列化
 *
 * chip 有两种形态(样式见 MentionInput.vue 的全局块):
 *   - 文件引用(mention-chip-file):彩色文件图标 + 主色文字,无胶囊底,读起来像链接
 *   - 命令/技能/智能体:标签色胶囊,图标用内联 SVG(自绘 feather 路径,未收录回退彩色圆点)
 */

export interface CaretToken {
  trigger: MentionTrigger
  /** 触发符后已输入部分 */
  query: string
}

/** 光标前紧邻的触发符 + 标识符(如 '/go'、'$code-rev'、'@backend/crates';文件路径含 '/') */
const TOKEN_TAIL_RE = /([\/$@][\w.\-/]*)$/

/**
 * 取光标前文本(仅当前段落内的连续文本节点)并检测是否以
 * 「空白或行首 + 触发符 + 标识符」紧邻光标结束。
 * chip / 块级元素 / BR 都是分段边界:chip 是原子标签,不能与后续输入拼成 token。
 */
export function getCaretToken(editor: HTMLElement): CaretToken | null {
  const sel = window.getSelection()
  if (!sel || !sel.rangeCount) return null
  const range = sel.getRangeAt(0)
  if (!range.collapsed) return null
  if (!editor.contains(range.startContainer)) return null

  let node: Node | null = range.startContainer
  // 光标落在元素边界(如 chip 之后)时,回溯到承载文本的前一个节点
  if (node.nodeType === Node.ELEMENT_NODE) {
    const el = node as Element
    node = el.childNodes[range.startOffset - 1] ?? el.previousSibling
  }

  let text = ''
  while (node && node !== editor) {
    if (node.nodeType === Node.TEXT_NODE) {
      text = (node as Text).data + text
      node = node.previousSibling
    } else {
      break // BR / chip / 其他元素均为分界
    }
  }

  const m = text.match(/(^|\s)([/$@])([\p{L}\p{N}_.\-\/]*)$/u)
  if (!m) return null
  return { trigger: m[2] as CaretToken['trigger'], query: m[3] }
}

/**
 * 把光标前的 token 文本替换为 chip,可在其后追加 apply 模板文本,并恢复光标到 chip 之后。
 * `insertChip=false` 时只**清除已输入的 token 文本**（不插 chip）——用于技能由外部胶囊承载的场景。
 */
export function commitChip(item: MentionItem, label: string, insertChip = true): void {
  const sel = window.getSelection()
  if (!sel || !sel.rangeCount) return
  const range = sel.getRangeAt(0)

  let textNode: Text | null = null
  const container = range.startContainer
  const offset = range.startOffset
  if (container.nodeType === Node.TEXT_NODE) {
    textNode = container as Text
  } else {
    const prev = (container as Element).childNodes[offset - 1]
    if (prev && prev.nodeType === Node.TEXT_NODE) textNode = prev as Text
    else {
      // 光标前无文本节点(纯 chip/块级边界):补一个空文本节点作为插入锚点
      textNode = document.createTextNode('')
      ;(container as Element).insertBefore(textNode, (container as Element).childNodes[offset] ?? null)
    }
  }

  // 去掉文本节点尾部的 token(如 '/goal'),保留前面的空格
  const m = textNode.data.match(TOKEN_TAIL_RE)
  if (m) textNode.data = textNode.data.slice(0, textNode.data.length - m[1].length)

  // insertChip=false:只清除已输入的 token 文本(技能由外部胶囊承载)
  let caretAnchor: Node = textNode
  if (insertChip) {
    const chip = createChipElement(item, label)
    textNode.parentNode!.insertBefore(chip, textNode.nextSibling)
    caretAnchor = chip
    // apply 模板文本紧随 chip(可选)
    const applied = item.apply ? item.apply({ query: m ? m[1].slice(1) : '' }) : ''
    if (applied) {
      const t = document.createTextNode(applied)
      textNode.parentNode!.insertBefore(t, chip.nextSibling)
      caretAnchor = t
    }
  }

  // 光标锚点文本节点(便于继续编辑与检测后续 token)
  const caretText = document.createTextNode('')
  textNode.parentNode!.insertBefore(caretText, caretAnchor.nextSibling)
  const r = document.createRange()
  r.setStart(caretText, 0)
  r.setEnd(caretText, 0)
  sel.removeAllRanges()
  sel.addRange(r)
}

/** 引号字形(24 网格,lucide「quote」);内联 chip 与顶部胶囊共用 */
export const QUOTE_ICON_PATH =
  '<path d="M3 21c3 0 7-1 7-8V5c0-1.25-.756-2.017-2-2H4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2 1 0 1 0 1 1v1c0 1-1 2-2 2s-1 .008-1 1.031V20c0 1 0 1 1 1z"/><path d="M15 21c3 0 7-1 7-8V5c0-1.25-.757-2.017-2-2h-4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2h.75c0 2.25.25 4-2.75 4v3c0 1 0 1 1 1z"/>'

/**
 * 对话正文引用的**内联 chip**:label 为截断预览,key 关联 quotePick store 条目
 * (增删与输入框上方胶囊同步)。完整文本 + 来源消息作为「说明」走消息元数据,不进 chip。
 */
export function createQuoteChip(label: string, key: number): HTMLElement {
  const span = document.createElement('span')
  span.className = 'mention-chip'
  span.contentEditable = 'false'
  span.dataset.type = 'quote'
  span.dataset.token = label
  span.dataset.quoteKey = String(key)
  span.style.setProperty('--chip-color', 'var(--accent-text)')
  span.innerHTML =
    `<svg class="mention-chip-icon" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${QUOTE_ICON_PATH}</svg>` +
    `<span class="mention-chip-label">${escapeHtml(label)}</span>`
  return span
}

export function createChipElement(item: MentionItem, label: string): HTMLElement {
  const span = document.createElement('span')
  // 文件引用走「图标 + 主色文字」的链接式样式(mention-chip-file),命令/技能/智能体仍是彩色胶囊
  span.className = item.type === 'file' ? 'mention-chip mention-chip-file' : 'mention-chip'
  span.contentEditable = 'false'
  span.dataset.type = item.type
  span.dataset.id = item.id
  // 序列化「原文」:文件→路径、智能体→名称、命令→/token。
  // 机器可读的引用说明另行收集(见 MentionInput.collectMentions),发送时作为元数据下发。
  span.dataset.token =
    item.type === 'file'
      ? item.id
      : item.type === 'agent'
        ? (item.label ?? item.id)
        : TRIGGER_BY_TYPE[item.type] + item.id
  span.style.setProperty('--chip-color', item.color)
  span.innerHTML =
    chipGlyphHtml(item) + `<span class="mention-chip-label">${escapeHtml(label)}</span>`
  return span
}

/**
 * 浏览器拾取元素的**内联 chip**：token 为 ```element 围栏（前置换行,自成一段）；
 * `key` 关联 elementPick store 条目（增删与输入框上方胶囊同步）。
 */
export function createElementChip(token: string, label: string, key: number): HTMLElement {
  const span = document.createElement('span')
  span.className = 'mention-chip'
  span.contentEditable = 'false'
  span.dataset.type = 'element'
  // 原文=编号+元素名(用户可见);围栏数据作为「说明」存 data-fence,发送时按元数据下发
  span.dataset.token = label
  span.dataset.fence = token
  span.dataset.elementKey = String(key)
  span.style.setProperty('--chip-color', 'var(--accent-text)')
  span.innerHTML =
    `<svg class="mention-chip-icon" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${CHIP_ICON_PATHS['icon-pick-element'] ?? ''}</svg>` +
    `<span class="mention-chip-label">${escapeHtml(label)}</span>`
  return span
}

/**
 * office 预览拾取的**内联 chip**：token 为 ```office-ref 围栏；`key` 关联
 * officePick store 条目（增删与输入框上方胶囊同步）。
 * 样式与浏览器元素 chip([`createElementChip`])完全一致:mention-chip 内联样式 +
 * accent 蓝 + 拾取光标图标(lucide 同源图形)。
 */
export function createOfficeRefChip(token: string, label: string, key: number): HTMLElement {
  const span = document.createElement('span')
  span.className = 'mention-chip'
  span.contentEditable = 'false'
  span.dataset.type = 'office'
  span.dataset.token = label
  span.dataset.fence = token
  span.dataset.officeKey = String(key)
  span.style.setProperty('--chip-color', 'var(--accent-text)')
  span.innerHTML =
    `<svg class="mention-chip-icon" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${CHIP_ICON_PATHS['icon-pick-element'] ?? ''}</svg>` +
    `<span class="mention-chip-label">${escapeHtml(label)}</span>`
  return span
}

/** chip 内图标:文件/文件夹用手写 SVG 图标(16px),否则回退内联 SVG / 彩色圆点 */
function chipGlyphHtml(item: MentionItem): string {
  if (item.iconUrl) {
    return `<img class="mention-chip-glyph" src="${item.iconUrl}" width="16" height="16" alt="">`
  }
  return chipIconHtml(item.icon)
}

/** 把编辑器内容序列化为纯文本:chip → 其 token,块级元素 → 换行 */
export function serializeEditor(editor: HTMLElement): string {
  const out: string[] = []
  const walk = (node: Node): void => {
    if (node.nodeType === Node.TEXT_NODE) {
      out.push((node as Text).data)
      return
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return
    const el = node as HTMLElement
    if (el.classList.contains('mention-chip')) {
      out.push(el.dataset.token ?? '')
      // 命令 chip（/goal、/plan…）后若无空白，补一个空格：
      // 用户从菜单选中命令后常直接接着写目标，DOM 里会紧贴成 `/goal目标`；
      // 不补空格会让 `/goal <目标>` / `/plan <需求>` 的识别失败。
      if (el.dataset.type === 'command') {
        const next = el.nextSibling
        const startsWithSpace =
          next?.nodeType === Node.TEXT_NODE && /^\s/.test((next as Text).data)
        if (!startsWithSpace) out.push(' ')
      }
      return
    }
    if (el.tagName === 'BR') {
      out.push('\n')
      return
    }
    el.childNodes.forEach(walk)
    if (el.tagName === 'DIV' || el.tagName === 'P') out.push('\n')
  }
  walk(editor)
  return out.join('').replace(/\n{3,}/g, '\n\n').trim()
}

/**
 * 把纯文本写入编辑器并重建 chip(编辑队列消息回显用):
 * `/`/`$` 经注册表、`@` 经启发式解析为 chip,其余文本按 `\n` 转 `<br>`。
 * labelOf 缺省用条目自身 label/id(命令类条目的本地化名需由调用方传入)。
 */
export function renderTextToEditor(
  editor: HTMLElement,
  text: string,
  labelOf?: (item: MentionItem) => string
): void {
  editor.innerHTML = ''
  const frag = document.createDocumentFragment()
  const appendText = (value: string): void => {
    value.split('\n').forEach((part, i) => {
      if (i > 0) frag.appendChild(document.createElement('br'))
      if (part) frag.appendChild(document.createTextNode(part))
    })
  }
  for (const seg of parseMentionSegments(text)) {
    if (seg.type === 'text') appendText(seg.text)
    else frag.appendChild(createChipElement(seg.item, labelOf ? labelOf(seg.item) : (seg.item.label ?? seg.item.id)))
  }
  editor.appendChild(frag)
}

/** chip 内联图标:未收录的图标名回退为彩色圆点 */
function chipIconHtml(icon?: string): string {
  const d = icon ? CHIP_ICON_PATHS[icon] : undefined
  if (!d) {
    return '<span class="mention-chip-dot"></span>'
  }
  return `<svg class="mention-chip-icon" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`
}

/**
 * 自绘 SVG 路径(24x24,feather 风格)。键与 config 中 Arco 图标名对应;
 * 新增条目如果图标未收录,chip 内会用彩色圆点兜底(弹层/消息侧仍用 Arco 图标)。
 */
export const CHIP_ICON_PATHS: Record<string, string> = {
  'icon-exclamation-circle':
    '<circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>',
  'icon-launch':
    '<path d="M22 2L11 13"/><path d="M22 2l-7 20-4-9-9-4 20-7z"/>',
  'icon-thunderbolt': '<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>',
  'icon-check-circle':
    '<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/>',
  'icon-eye': '<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>',
  'icon-file':
    '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/>',
  'icon-tool':
    '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>',
  'icon-search': '<circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>',
  'icon-user-group':
    '<path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  'icon-safe': '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>',
  'icon-folder':
    '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>',
  'icon-star': '<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/>',
  'icon-calendar':
    '<rect x="3" y="4" width="18" height="18" rx="2" ry="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/>',
  'icon-history':
    '<polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/>',
  'icon-undo':
    '<path d="M9 14L4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>',
  'icon-goal':
    '<path d="M19.8 8.5A9 9 0 1 1 12 4"/><path d="M16.55 10.4A5.25 5.25 0 1 1 12 7.75"/><path d="M12 13V2.5"/><path d="M12 2.5l6.5 3-6.5 3z"/>',
  'icon-init':
    '<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="M9 15h6"/><path d="M12 12v6"/>',
  'icon-sparkles':
    '<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594zM20 2v4m2-2h-4"/><circle cx="4" cy="20" r="2"/>',
  'icon-compass':
    '<circle cx="12" cy="12" r="10"/><path d="m16.24 7.76l-1.804 5.411a2 2 0 0 1-1.265 1.265L7.76 16.24l1.804-5.411a2 2 0 0 1 1.265-1.265z"/>',
  'icon-robot':
    '<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>',
  'icon-plan':
    '<path d="M13 5.5h8"/><path d="M13 12h8"/><path d="M13 18.5h8"/><path d="M3 16.5l2 2 4-4"/><path d="M3 6l2 2 4-4"/>',
  'icon-compact':
    '<path d="M3 12h5"/><path d="M6 8.5l3.5 3.5-3.5 3.5"/><path d="M21 12h-5"/><path d="M18 8.5l-3.5 3.5 3.5 3.5"/>',
  'icon-fork':
    '<circle cx="12" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><circle cx="18" cy="6" r="3"/><path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9m6 3v3"/>',
  'icon-pick-element':
    // lucide MousePointerClick 原版(与 <icon-pick-element> 组件同源,ElementPill 胶囊同图形)
    '<path d="M14 4.1 12 6"/><path d="m5.1 8-2.9-.8"/><path d="m6 12-1.9 2"/><path d="M7.2 2.2 8 5.1"/><path d="M9.037 9.69a.498.498 0 0 1 .653-.653l11 4.5a.5.5 0 0 1-.074.949l-4.349 1.041a1 1 0 0 0-.74.739l-1.04 4.35a.5.5 0 0 1-.95.074z"/>',
  'icon-branch':
    '<line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>',
  'icon-download':
    '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/>',
  'icon-sync':
    '<polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>',
  'icon-copy':
    '<rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
  'icon-message':
    '<path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/>',
  'icon-brush':
    '<path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/>',
  'icon-settings':
    '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>',
  'icon-bar-chart':
    '<line x1="12" y1="20" x2="12" y2="10"/><line x1="18" y1="20" x2="18" y2="4"/><line x1="6" y1="20" x2="6" y2="16"/>'
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`)
}