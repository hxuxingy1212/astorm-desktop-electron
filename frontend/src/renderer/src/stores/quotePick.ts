import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

/**
 * 对话正文「添加到当前任务」→ 聊天发送框的引用桥(与浏览器元素选择同一范式)。
 *
 * 在对话正文里选中一段文字并点「添加到当前任务」,把该段文本推入本 store;
 * ChatInput 在发送框上方渲染一枚聚合胶囊(图标 + 「N 条对话引用」+ 移除),
 * 编辑器里同步为一枚内联 chip。发送时:正文显示截断预览,完整文本 + 来源消息
 * 作为「引用说明」进消息元数据(不渲染),喂模型 / 冷启动重建时再拼回 user 消息。
 *
 * 与 elementPick 一致:两个面板非父子关系(消息区 / 输入框),故用 store 承载。
 */
export type QuoteRole = 'assistant' | 'user'

export interface PickedQuote {
  /** 选中的正文原文(完整) */
  text: string
  /** 来源消息 id(去重 / 定位) */
  messageId?: string
  /** 来源消息序号(#seq,卡片角标展示) */
  seq?: number
  /** 来源角色 */
  role?: QuoteRole
  /** 列表 key(会话内单调递增,唯一;不要用 Date.now(),同毫秒会重复) */
  ts: number
}

/** 拾取 key 计数器(模块级:跨组件保持唯一) */
let quoteSeq = 0

/** 正文预览:折叠空白并截断(内联 chip / 正文「原文」用) */
export function quotePreview(text: string, max = 40): string {
  const one = text.replace(/\s+/g, ' ').trim()
  return one.length > max ? `${one.slice(0, max)}…` : one
}

export const useQuotePickStore = defineStore('quotePick', () => {
  const items = ref<PickedQuote[]>([])
  const count = computed(() => items.value.length)

  /** 追加一条引用;空白文本忽略,同来源同文本去重(返回是否新增) */
  function push(payload: {
    text: string
    messageId?: string
    seq?: number
    role?: QuoteRole
  }): boolean {
    const text = payload.text.replace(/\s+/g, ' ').trim()
    if (!text) return false
    const dup = items.value.some(
      (i) => i.messageId === payload.messageId && i.text === text
    )
    if (dup) return false
    items.value = [...items.value, { ...payload, text, ts: ++quoteSeq }]
    return true
  }

  /** 移除第 index 个(胶囊上的 × 清空全部,传 undefined) */
  function remove(index?: number): void {
    if (index == null) {
      items.value = []
      return
    }
    items.value = items.value.filter((_, i) => i !== index)
  }

  /** 按 key 移除(输入框内联 chip 被删除时对账用) */
  function removeByKey(ts: number): void {
    items.value = items.value.filter((i) => i.ts !== ts)
  }

  return { items, count, push, remove, removeByKey }
})
