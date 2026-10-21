import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { ComputedRef, Ref } from 'vue'

export type Breakpoint = 'md' | 'lg' | 'xl'

/** 屏幕断点:xl = 完整侧栏(≥1440,覆盖 Mac 1440 与主流桌面);lg 及以下自动折叠侧栏;md 额外钳制页签宽度 */
export const BREAKPOINTS = {
  xl: 1440,
  lg: 960
} as const

export interface BreakpointComposable {
  width: Ref<number>
  bp: ComputedRef<Breakpoint>
  isXl: ComputedRef<boolean>
}

/**
 * 监听窗口尺寸并派生断点。
 * 布局组件据此自适应:侧栏折叠、页签区宽度钳制等。
 */
export function useBreakpoint(): BreakpointComposable {
  const width = ref(window.innerWidth)
  let raf = 0

  const onResize = (): void => {
    cancelAnimationFrame(raf)
    raf = requestAnimationFrame(() => {
      width.value = window.innerWidth
    })
  }

  onMounted(() => window.addEventListener('resize', onResize))
  onBeforeUnmount(() => {
    window.removeEventListener('resize', onResize)
    cancelAnimationFrame(raf)
  })

  const bp = computed<Breakpoint>(() => {
    if (width.value >= BREAKPOINTS.xl) return 'xl'
    if (width.value >= BREAKPOINTS.lg) return 'lg'
    return 'md'
  })

  const isXl = computed(() => bp.value === 'xl')

  return { width, bp, isXl }
}
