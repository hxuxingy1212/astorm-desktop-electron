<script setup lang="ts">
import { computed } from 'vue'
import type { Component } from 'vue'

/**
 * Mac Finder 风格图标:
 * - folder:双层文件夹(深色后板 + 浅色前板),随所选颜色渐变
 * - document:带折角的页面
 * 可选在正面叠加一个 lucide glyph 作为「选中图标」徽标。
 */
interface Props {
  type: 'folder' | 'document'
  color: string
  size?: number
  /** 选中的图标徽标(lucide 组件);folder 类型默认图标为 'folder' 时不叠加 */
  glyphComp?: Component | null
}
const props = withDefaults(defineProps<Props>(), { size: 48, glyphComp: null })

const uid = computed(() => `mfi-${props.type}-${props.color.replace(/[^a-zA-Z0-9]/g, '')}`)

function clamp(v: number): number {
  return Math.max(0, Math.min(255, Math.round(v)))
}
/** amt<0 加深,amt>0 提亮 */
function shade(hex: string, amt: number): string {
  const n = parseInt(hex.slice(1), 16)
  let r = (n >> 16) & 255
  let g = (n >> 8) & 255
  let b = n & 255
  if (amt < 0) {
    const f = 1 + amt
    r = clamp(r * f)
    g = clamp(g * f)
    b = clamp(b * f)
  } else {
    r = clamp(r + (255 - r) * amt)
    g = clamp(g + (255 - g) * amt)
    b = clamp(b + (255 - b) * amt)
  }
  return `#${((1 << 24) + (r << 16) + (g << 8) + b).toString(16).slice(1)}`
}

const darkC = computed(() => shade(props.color, -0.3))
const darkerC = computed(() => shade(props.color, -0.45))
const lightC = computed(() => shade(props.color, 0.28))
const baseC = computed(() => shade(props.color, 0.04))

/** 徽标文字颜色:按背景亮度选黑/白 */
const glyphDark = computed(() => {
  const hex = props.color.replace('#', '')
  const r = parseInt(hex.slice(0, 2), 16)
  const g = parseInt(hex.slice(2, 4), 16)
  const b = parseInt(hex.slice(4, 6), 16)
  return (r * 0.299 + g * 0.587 + b * 0.114) > 150
})
const glyphColor = computed(() => (glyphDark.value ? '#c26f00' : 'rgba(255,255,255,0.96)'))

const showGlyph = computed(() => !!props.glyphComp)
</script>

<template>
  <span class="mac-item" :style="{ width: `${size}px`, height: `${size}px` }">
    <!-- 文件夹:后板(带标签)+ 前板 -->
    <svg v-if="type === 'folder'" :width="size" :height="size" viewBox="0 0 64 64" class="mac-svg">
      <defs>
        <linearGradient :id="`${uid}-back`" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" :stop-color="darkC" />
          <stop offset="1" :stop-color="darkerC" />
        </linearGradient>
        <linearGradient :id="`${uid}-front`" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" :stop-color="lightC" />
          <stop offset="1" :stop-color="baseC" />
        </linearGradient>
      </defs>
      <!-- 后板(上部带标签) -->
      <path
        :fill="`url(#${uid}-back)`"
        d="M6 15a5 5 0 0 1 5-5h13.2a5 5 0 0 1 3.5 1.4L31.6 15H53a5 5 0 0 1 5 5v3H6z"
      />
      <!-- 前板 -->
      <path :fill="`url(#${uid}-front)`" d="M6 24h52v19a6 6 0 0 1-6 6H12a6 6 0 0 1-6-6z" />
    </svg>

    <!-- 文档:页面 + 折角 -->
    <svg v-else :width="size" :height="size" viewBox="0 0 64 64" class="mac-svg">
      <defs>
        <linearGradient :id="`${uid}-page`" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" :stop-color="lightC" />
          <stop offset="1" :stop-color="baseC" />
        </linearGradient>
      </defs>
      <!-- 折角(更亮) -->
      <path :fill="lightC" d="M39 8l11 11H39z" />
      <!-- 页面主体 -->
      <path :fill="`url(#${uid}-page)`" d="M14 10a4 4 0 0 1 4-4h21l11 11v37a4 4 0 0 1-4 4H18a4 4 0 0 1-4-4z" />
    </svg>

    <component
      v-if="showGlyph"
      :is="glyphComp"
      class="mac-emblem"
      :size="Math.round(size * 0.42)"
      :stroke-width="2.4"
      :color="glyphColor"
    />
  </span>
</template>

<style scoped>
.mac-item {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  line-height: 0;
}
.mac-svg {
  display: block;
}
.mac-emblem {
  position: absolute;
  left: 50%;
  top: 52%;
  transform: translate(-50%, -50%);
}
</style>
