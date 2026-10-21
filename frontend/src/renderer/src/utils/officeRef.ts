import type { OfficeRefData } from '@/stores/officePick'

/**
 * office 预览拾取的文本表示(对标 elementBlock 的 ```element 围栏):
 *
 *   拾取结果在发送框里是一枚原子 chip(图标 + 名字),序列化进消息元数据的
 *   是一整块 ```office-ref 围栏文本。**字段 key 不走 i18n**——path/kind/file
 *   是 office CLI 的契约键,模型直接按它调用 office 工具,本地化反而破坏解析。
 */

export const OFFICE_FENCE = 'office-ref'

/** 匹配 ```office-ref ... ``` 围栏块(用于消息侧切分,与 MentionText 的 element 切分同款) */
export const OFFICE_BLOCK_RE = /```office-ref\r?\n([\s\S]*?)```/g

/**
 * 生成 office-ref 文本块(```office-ref 围栏);空字段整行省略。
 * 结构:文件(绝对路径)→ 格式 → CLI 树形路径 → 元素摘要(name/text)→ 附加 meta。
 */
export function buildOfficeRefBlock(data: OfficeRefData): string {
  const rows: [string, string][] = []
  const add = (key: string, value: string | number | undefined): void => {
    if (value !== undefined && value !== null && `${value}`.trim() !== '') {
      rows.push([key, `${value}`])
    }
  }
  add('file', data.file)
  add('kind', data.kind)
  add('path', data.path)
  add('type', data.type)
  add('name', data.name)
  add('text', data.text)
  for (const [k, v] of Object.entries(data.meta ?? {})) {
    add(k, v)
  }
  const body = rows.map(([label, value]) => `${label}: ${value}`).join('\n')
  return `\`\`\`${OFFICE_FENCE}\n${body}\n\`\`\``
}

/** chip / 胶囊卡片版头共用的短标签:名称 → 文本摘要(截断) → 紧凑路径。
 *  输入框内联 chip 的可见文本与序列化 token 都来自这里——必须紧凑,
 *  与浏览器元素 chip(组件名/tag)同口径;完整内容在 ```office-ref 围栏里。 */
export function officeRefChipLabel(data: OfficeRefData): string {
  const name = data.name?.trim()
  if (name) return trimLabel(name, 20)
  const text = data.text?.trim()
  if (text) return trimLabel(text, 14)
  return compactPath(data.path)
}

/** 单行化 + 截断 */
function trimLabel(s: string, max: number): string {
  const one = s.replace(/\s+/g, ' ')
  return one.length > max ? `${one.slice(0, max)}…` : one
}

/** 紧凑路径:`/part[1]/paragraph[2]` → `part1/para2` */
export function compactPath(path: string): string {
  return path
    .replace(/^\//, '')
    .split('/')
    .map((seg) => {
      const m = seg.match(/^(\w+)\[(.+)\]$/)
      if (!m) return seg
      const name = m[1]
      const key = m[2]
      const abbr: Record<string, string> = {
        paragraph: 'para',
        slide: 'slide',
        sheet: 'sheet',
        part: 'part',
        page: 'page',
        text: 'text',
        cell: 'cell'
      }
      return `${abbr[name] ?? name}${key}`
    })
    .join('/')
}
