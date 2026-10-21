import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import type { PickedElementData } from '@/types'
import { withCard } from '@/utils/elementBlock'

/**
 * 内置浏览器「选择元素」→ 聊天发送框的附件桥。
 *
 * BrowserPanel 拾取成功后 push(),ChatInput 在发送框上方渲染一枚 macOS 风格
 * 附件胶囊(图标 + 「N 个网页元素」+ 移除),悬停弹出卡片(内容与页面内高亮卡片同源);
 * 发送时把各元素对应的 ```element 文本块追加到消息末尾,随后 clear()。
 *
 * 与 plugins.injectedInput 同一范式:两个面板不是父子关系,且 BrowserPanel
 * 切页签即卸载,故用 store 承载。
 */
export interface PickedElement {
  data: PickedElementData
  /** 该元素序列化进消息的完整文本块(```element 围栏) */
  token: string
  /**
   * 列表 key(会话内单调递增,唯一)。**不要用 `Date.now()`**——同毫秒的多次拾取
   * (连续点击 / 批量回填)会得到相同 key,导致输入框内联 chip 按 key 去重后数量对不上胶囊。
   */
  ts: number
}

/** 拾取 key 计数器(模块级:跨组件/跨批量回填保持唯一) */
let pickSeq = 0

export const useElementPickStore = defineStore('elementPick', () => {
  const items = ref<PickedElement[]>([])
  const count = computed(() => items.value.length)

  /** 拾取成功:追加一个待发送元素(data 一律补全 card,弹层渲染不依赖主进程版本) */
  function push(payload: { data: PickedElementData; token: string }): void {
    items.value = [...items.value, { ...payload, data: withCard(payload.data), ts: ++pickSeq }]
  }

  /** 移除第 index 个(胶囊上的 × 清空全部,传 undefined) */
  function remove(index?: number): void {
    if (index == null) {
      items.value = []
      return
    }
    items.value = items.value.filter((_, i) => i !== index)
  }

  /** 按时间戳 key 移除(输入框内联 chip 被删除时对账用) */
  function removeByKey(ts: number): void {
    items.value = items.value.filter((i) => i.ts !== ts)
  }

  /** 一次发送用到的全部文本块(按拾取顺序,块间空行分隔) */
  function tokens(): string {
    return items.value.map((i) => i.token).join('\n\n')
  }

  return { items, count, push, remove, removeByKey, tokens }
})
