<script setup lang="ts">
import { useSettingsStore } from '@/stores/settings'
import { useResizeDrag } from '@/composables/useResizeDrag'

const props = defineProps<{
  /** 页签区可拖拽范围(Workspace 按可用宽度比例动态计算:次要 ~28% ~ 主要 ~72%) */
  min: number
  max: number
}>()

const settings = useSettingsStore()

// 分割线由本组件左侧的 1px 竖线统一提供,拖拽条本体完全隐形
// invert: 手柄在页签区左缘,分割线跟随鼠标(往左拖 → 页签区变宽)
// min/max 用 getter:随窗口宽度变化的比例范围在拖拽过程中实时生效
const { onPointerDown, onPointerMove, onPointerUp } = useResizeDrag({
  min: () => props.min,
  max: () => props.max,
  get: () => settings.panelWidth,
  set: (w) => settings.setPanelWidth(w),
  invert: true
})
</script>

<template>
  <div
    class="relative z-10 h-full w-1.5 shrink-0 cursor-col-resize select-none"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
  >
    <!-- 单条分割线:紧贴页签区左缘,与页签栏下边框在拐角处相接,不留缺口 -->
    <div class="pointer-events-none absolute inset-y-0 right-0 w-px bg-[var(--panel-border)]" />
  </div>
</template>
