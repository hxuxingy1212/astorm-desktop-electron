/**
 * 悬浮标题跑马灯（侧栏任务行 / 页签标题共用）。
 *
 * 为什么用 `scrollLeft` 而不是 `text-indent`：Chromium 在 flex + ellipsis 的标题上
 * 仍会更新 `text-indent` 的计算值，但不重绘文字；`scrollLeft` 是真实滚动位置，稳定可见。
 *
 * - 约 30px/s 匀速滚到末尾（dsh 参考）；
 * - 溢出 ≤8px 视为视觉抖动，不启动；
 * - `prefers-reduced-motion` 直接跳到末尾；
 * - 同一时刻只保留一个活动项：进入新行 / 离开时取消并回到开头。
 *
 * 用法：行元素 `@mouseenter="enter($event, key)" @mouseleave="leave(key)"`；
 * 标题元素加 `marquee-title` class 与 `:class="{ 'is-overflowing': active[key] }"`。
 */
import { onBeforeUnmount, reactive } from 'vue'

export function useHoverMarquee(selector = '.marquee-title') {
  const active = reactive<Record<string, boolean>>({})
  let raf: number | null = null
  let el: HTMLElement | null = null
  let currentKey: string | null = null
  let start = 0
  let duration = 0
  let shift = 0

  function stop(): void {
    if (raf !== null) {
      cancelAnimationFrame(raf)
      raf = null
    }
    if (el) el.scrollLeft = 0
    if (currentKey) delete active[currentKey]
    el = null
    currentKey = null
  }

  function enter(e: MouseEvent, key: string): void {
    const box = (e.currentTarget as HTMLElement | null)?.querySelector<HTMLElement>(selector)
    if (!box) return
    stop()
    const overflow = Math.max(0, box.scrollWidth - box.clientWidth)
    if (overflow <= 8) return
    active[key] = true
    el = box
    currentKey = key
    shift = overflow
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      box.scrollLeft = shift
      return
    }
    start = performance.now()
    duration = shift / 0.03
    const step = (now: number): void => {
      if (!el) return
      const progress = Math.min(1, (now - start) / duration)
      el.scrollLeft = shift * progress
      if (progress < 1) raf = requestAnimationFrame(step)
      else raf = null
    }
    raf = requestAnimationFrame(step)
  }

  function leave(key: string): void {
    if (currentKey === key) stop()
  }

  onBeforeUnmount(stop)

  return { active, enter, leave, stop }
}
