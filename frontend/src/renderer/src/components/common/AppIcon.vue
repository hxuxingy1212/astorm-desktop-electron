<script setup lang="ts">
import { computed } from 'vue'
import { uiGlyphMarkup, UI_ICON_STROKE_WIDTH } from '@/utils/uiIconGallery'

/**
 * 手写 UI 图标渲染器:把 assets/ui-icons/*.svg 内联到 DOM。
 *
 * - 内联(而非 <img>)才能让 `stroke="currentColor"` 跟随父级 color —— hover / 选中态 /
 *   `.spin` 旋转 / `:deep(svg)` 着色等既有样式原样生效,与替换前的 lucide 组件行为一致
 * - 线宽按 lucide 的 24 网格语义传入(全局标准见 UI_ICON_STROKE_WIDTH),内部 ×2 换算到 48 网格
 */
const props = withDefaults(
  defineProps<{
    /** 字形名(= assets/ui-icons/<glyph>.svg) */
    glyph: string
    /** 显示尺寸(px) */
    size?: number | string
    /** 线宽(单位同 lucide 的 24 网格:默认使用全局标准) */
    strokeWidth?: number | string
    /** 显式颜色;不传则继承父级 color */
    color?: string
  }>(),
  { size: 18, strokeWidth: UI_ICON_STROKE_WIDTH, color: '' }
)

/** 手写图标是 48 网格(见生成器约定),而调用方沿用 lucide 的 24 网格语义 → 线宽 ×2 */
const markup = computed(() => uiGlyphMarkup(props.glyph, props.size, Number(props.strokeWidth) * 2))
</script>

<template>
  <span class="app-icon" :style="color ? { color } : undefined" v-html="markup" />
</template>

<style scoped>
/* 尺寸由内联 SVG 的 width/height 决定,外壳只负责居中与不收缩 */
.app-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  line-height: 0;
}

.app-icon :deep(svg) {
  display: block;
}
</style>
