import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

/**
 * office 预览「拾取元素路径」→ 聊天发送框的附件桥（对标浏览器拾取的 elementPick）。
 *
 * OfficeViewerRenderer 拾取成功后 push(),ChatInput 在发送框上方渲染附件胶囊
 * (图标 + 「N 个文档路径」+ 移除),悬停弹卡片;发送时把各条对应的 ```office-ref
 * 文本块按元数据下发(后端拼回 user 消息,模型拿 CLI 路径直接 edit)。
 *
 * 与 elementPick 同一范式:预览面板与 ChatInput 非父子,面板随页签切换卸载,故用 store 承载。
 */

/** 拾取的数据:文件 + CLI 树形路径(+ viewer 悬浮卡片同源的元素摘要) */
export interface OfficeRefData {
  /** 文件绝对路径 */
  file: string
  /** 格式:docx / xlsx / pptx / pdf */
  kind: string
  /** CLI 树形路径(如 /part[1]/paragraph[2];来自 viewer @select 第一参数) */
  path: string
  /** 元素类型(如 paragraph / cell / shape / text) */
  type?: string
  /** 名称摘要(可选:PPT 形状名 / 工作表名等) */
  name?: string
  /** 文本摘要(可选) */
  text?: string
  /** 位置/尺寸等附加信息(可选,卡片逐条展示) */
  meta?: Record<string, string | number | undefined>
}

export interface PickedOfficeRef {
  data: OfficeRefData
  /** 该引用序列化进消息元数据的完整文本块(```office-ref 围栏) */
  token: string
  /** 列表 key(会话内单调递增,唯一;**不要用 Date.now()**——同毫秒连点会撞 key) */
  ts: number
}

/** 拾取 key 计数器(模块级:跨组件/跨批量回填保持唯一) */
let pickSeq = 0

export const useOfficePickStore = defineStore('officePick', () => {
  const items = ref<PickedOfficeRef[]>([])
  const count = computed(() => items.value.length)

  /** 拾取成功:追加一个待发送文档路径引用 */
  function push(payload: { data: OfficeRefData; token: string }): void {
    items.value = [...items.value, { ...payload, ts: ++pickSeq }]
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

  return { items, count, push, remove, removeByKey }
})
