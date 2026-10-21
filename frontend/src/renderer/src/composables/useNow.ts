import { onBeforeUnmount, ref } from 'vue'
import type { Ref } from 'vue'

/**
 * 一个**全应用共享**的「当前时间」ref，按固定间隔自增，用于让相对时间文案自己走。
 *
 * 为什么共享：看板一列十几张卡片，每张自己开一个 `setInterval` 就是十几个定时器 + 十几次渲染；
 * 这里只有一个 interval，所有订阅者读同一个 ref（引用计数归零时清掉定时器）。
 *
 * 用法：`const now = useNow()` → 模板/计算属性里读 `now.value`，读到的组件就会按间隔重渲染。
 */
const nowRef = ref(Date.now())
let timer: ReturnType<typeof setInterval> | null = null
let subscribers = 0
let intervalMs = 0

export function useNow(ms = 60_000): Ref<number> {
  subscribers += 1
  // 首个订阅者决定间隔；后续订阅者若要求更短，重开一个更快的定时器
  if (!timer || ms < intervalMs) {
    if (timer) clearInterval(timer)
    intervalMs = ms
    timer = setInterval(() => {
      nowRef.value = Date.now()
    }, intervalMs)
  }

  onBeforeUnmount(() => {
    subscribers -= 1
    if (subscribers <= 0 && timer) {
      clearInterval(timer)
      timer = null
      intervalMs = 0
    }
  })

  return nowRef
}
