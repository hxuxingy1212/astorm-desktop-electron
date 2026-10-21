import type { PickedElementCard, PickedElementCardRow, PickedElementData } from '@/types'

/**
 * 内置浏览器拾取元素的文本表示:
 *
 *   拾取结果在发送框里是一枚原子 chip(图标 + 名字),但它序列化进消息的
 *   是一整块 ```element 围栏文本 —— chip 是「整体删除」的,文本按光标位置
 *   正常拼接,模型拿到的是结构化字段。
 *
 * MentionText 侧按 ELEMENT_BLOCK_RE 先切出围栏块(否则 XPath 里的 /html
 * 会被提及正则误判成 token),再对剩余文本跑提及渲染。
 */

export const ELEMENT_FENCE = 'element'

/** 匹配 ```element ... ``` 围栏块(用于消息侧切分) */
export const ELEMENT_BLOCK_RE = /```element\r?\n([\s\S]*?)```/g

export interface ElementBlockRange {
  /** 围栏块在原文中的起止下标(含围栏) */
  start: number
  end: number
  /** 围栏内的正文(行式 `标签: 值`) */
  body: string
}

/** 找出文本里所有元素围栏块(供 MentionText 先切分再渲染提及) */
export function findElementBlocks(text: string): ElementBlockRange[] {
  const out: ElementBlockRange[] = []
  ELEMENT_BLOCK_RE.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = ELEMENT_BLOCK_RE.exec(text))) {
    out.push({ start: m.index, end: m.index + m[0].length, body: m[1] ?? '' })
  }
  return out
}

/** 把围栏块正文解析为 `标签: 值` 行(值内首个冒号之后原样保留) */
export function parseElementBlockRows(body: string): { label: string; value: string }[] {
  return body
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const at = line.indexOf(': ')
      return at < 0
        ? { label: line, value: '' }
        : { label: line.slice(0, at), value: line.slice(at + 2) }
    })
}

/** 卡片模型里按 key 取值(尺寸/样式/组件/源码都从这里来,与悬停卡片同源不另算) */
function cardValue(data: PickedElementData, key: string): string {
  return data.card?.rows.find((r) => r.key === key)?.value ?? ''
}

/** 已知的元素字段 key(用于把 ```element 围栏里固化的本地化标签反查回 card row.key) */
const ELEMENT_ROW_KEYS = [
  'tag', 'id', 'class', 'xpath', 'css', 'text', 'size',
  'color', 'background', 'font', 'padding', 'margin', 'component', 'path', 'page'
] as const
/** 值应以高亮路径样式展示的字段 */
const ELEMENT_PATH_KEYS = new Set(['xpath', 'css', 'path', 'page'])

/** 本地化标签 → 字段 key(两套 i18n 命名空间都试;查不到返回 null) */
export function elementRowKeyOfLabel(label: string, t: (key: string) => string): string | null {
  for (const key of ELEMENT_ROW_KEYS) {
    if (t(`panel.browser.elementBlock.${key}`) === label) return key
    if (t(`panel.browser.inspect.${key}`) === label) return key
  }
  return null
}

/**
 * 由 ```element 围栏正文重建**最小悬停卡片**(结构化数据在消息里只剩文本):
 * 用现有 i18n 把行标签反查回 row.key,命中才保留(宁缺勿错);
 * 卡片名取「标签」行,缺失回退 `网页元素`。
 */
export function elementCardFromBlock(body: string, t: (key: string) => string): PickedElementCard {
  const rows: PickedElementCardRow[] = []
  let name = ''
  for (const { label, value } of parseElementBlockRows(body)) {
    const key = elementRowKeyOfLabel(label, t)
    if (!key) continue
    if (key === 'tag' && !name) name = value
    rows.push({ key, value, path: ELEMENT_PATH_KEYS.has(key) })
  }
  return { name: name || '网页元素', size: '', rows }
}

/** chip 展示名:组件名优先,回退 标签#id → 标签.首个class → 标签 */
export function elementChipLabel(data: PickedElementData): string {
  if (data.component?.name) return data.component.name
  const tag = data.tagName || 'element'
  if (data.id) return `${tag}#${data.id}`
  const first = data.classList[0]
  return first ? `${tag}.${first}` : tag
}

/**
 * 生成元素文本块(```element 围栏);空字段整行省略。
 * 结构:标识(标签/id/class/xpath/css/文本)→ 尺寸与计算样式(颜色/背景/字体/内边距/外边距)
 * → 来源(组件/源码)→ 页面。尺寸与样式行直接取悬停卡片模型的值,两处内容一致。
 * `t` 为 i18n 翻译函数(字段标签随语言),在拾取时刻固化进文本。
 */
export function buildElementBlock(data: PickedElementData, t: (key: string) => string): string {
  const rows: [string, string][] = []
  const add = (key: string, value: string): void => {
    if (value) rows.push([t(key), value])
  }
  // 标识
  add('panel.browser.elementBlock.tag', data.tagName)
  add('panel.browser.elementBlock.id', data.id)
  add('panel.browser.elementBlock.class', data.classList.join(' '))
  add('panel.browser.elementBlock.xpath', data.xpath)
  add('panel.browser.elementBlock.css', data.cssSelector)
  add('panel.browser.elementBlock.text', data.text)
  // 尺寸 + 计算样式(与悬停卡片同源)
  add('panel.browser.elementBlock.size', data.card?.size ?? '')
  for (const key of ['color', 'background', 'font', 'padding', 'margin']) {
    add(`panel.browser.inspect.${key}`, cardValue(data, key))
  }
  // 来源
  add('panel.browser.inspect.component', cardValue(data, 'component'))
  add('panel.browser.inspect.path', cardValue(data, 'path'))
  add('panel.browser.elementBlock.page', data.url)
  const body = rows.map(([label, value]) => `${label}: ${value}`).join('\n')
  return `\`\`\`${ELEMENT_FENCE}\n${body}\n\`\`\``
}

/**
 * 保证拾取结果带卡片数据。
 *
 * 主进程 cardModel 正常都会返回 card;但旧版主进程(未重建)或异常分支可能没有,
 * 而发送框弹层要按 card.name / card.rows 渲染 —— 缺字段会让整块渲染抛错,
 * 表现为「拾取后发送框坏掉」。此处兜底拼一张最小卡片,只保证不抛错。
 */
export function withCard(data: PickedElementData): PickedElementData {
  if (data.card) return data
  const rows: PickedElementCardRow[] = []
  const push = (key: string, value: string): void => {
    if (value) rows.push({ key, value })
  }
  push('id', data.id)
  push('xpath', data.xpath)
  const card: PickedElementCard = { name: elementChipLabel(data), size: '', rows }
  return { ...data, card }
}
