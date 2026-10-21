import { onBeforeUnmount, ref } from 'vue'
import type { Ref } from 'vue'

export interface ResizeDragOptions {
  /** 最小/最大宽度:静态数值或动态 getter(随窗口尺寸变化的比例钳制,如页签区三态范围) */
  min: number | (() => number)
  max: number | (() => number)
  get: () => number
  set: (width: number) => void
  /**
   * 手柄位于目标面板「左缘」时传 true(如页签分栏):
   * 拖拽方向反向,使分割线跟随鼠标(往左拖 → 面板变宽)。
   * 手柄位于右缘(如侧栏)时保持默认。
   */
  invert?: boolean
}

export interface ResizeDragControls {
  dragging: Ref<boolean>
  onPointerDown: (e: PointerEvent) => void
  onPointerMove: (e: PointerEvent) => void
  onPointerUp: (e: PointerEvent) => void
}

/**
 * 指针拖拽调宽(侧栏、分栏通用):
 * 按下记录起始宽度,移动时按位移调整并钳制在 [min, max] 内。
 */
export function useResizeDrag({ min, max, get, set, invert = false }: ResizeDragOptions): ResizeDragControls {
  const dragging = ref(false)
  let startX = 0
  let startWidth = 0
  const dir = invert ? -1 : 1
  let frame: number | null = null
  let pendingWidth: number | null = null

  const flush = (): void => {
    if (frame !== null) cancelAnimationFrame(frame)
    frame = null
    if (pendingWidth !== null) {
      const width = pendingWidth
      pendingWidth = null
      if (width !== get()) set(width)
    }
  }

  onBeforeUnmount(() => {
    if (frame !== null) cancelAnimationFrame(frame)
    frame = null
    pendingWidth = null
  })

  const onPointerDown = (e: PointerEvent): void => {
    dragging.value = true
    startX = e.clientX
    startWidth = get()
    const el = e.currentTarget as HTMLElement
    el.setPointerCapture(e.pointerId)
  }

  const onPointerMove = (e: PointerEvent): void => {
    if (!dragging.value) return
    const lo = typeof min === 'function' ? min() : min
    const hi = typeof max === 'function' ? max() : max
    pendingWidth = Math.min(hi, Math.max(lo, Math.round(startWidth + dir * (e.clientX - startX))))
    // 高频指针事件只提交一次布局更新，避免同帧多次触发 Vue / grid 重排。
    if (frame === null) frame = requestAnimationFrame(flush)
  }

  const onPointerUp = (e: PointerEvent): void => {
    if (!dragging.value) return
    onPointerMove(e)
    flush()
    dragging.value = false
    const el = e.currentTarget as HTMLElement
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId)
  }

  return { dragging, onPointerDown, onPointerMove, onPointerUp }
}
