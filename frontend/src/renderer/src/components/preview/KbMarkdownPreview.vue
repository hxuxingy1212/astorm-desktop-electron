<script setup lang="ts">
/**
 * md-editor-v3 自定义预览(previewComponent)：渲染编译后的 html，并把相对路径的
 * `<img src>` 异步升级为 data URL（知识库场景，源文本保持原文）。
 *
 * 现由记忆详情页（MemoryView 的 MdPreview）使用；文件预览已改用与对话正文同一个
 * `MarkdownBody`，不再走这里。`kbImgRelDir` / `kbFrontmatter` 由调用方按需 provide，
 * 未 provide 时行为与改动前一致（不做图片升级 / 不渲染元数据块）。
 */
import { inject, onBeforeUnmount, ref, watch, type Ref } from 'vue'
import { upgradeRelativeImages } from '@/utils/markdownImages'
import MarkdownFrontmatterHeader from '@/components/common/MarkdownFrontmatterHeader.vue'
import type { FrontmatterMeta } from '@/utils/frontmatter'

defineProps<{ html: string; id?: string; className?: string }>()

const root = ref<HTMLElement | null>(null)
/** 知识库相对目录(imageBaseRel);为空则不做图片升级(文件树页签) */
const relDir = inject<Ref<string>>('kbImgRelDir', ref(''))
/** 文件预览面板注入的 frontmatter 元数据(未注入 = 不渲染元数据块) */
const fmMeta = inject<Ref<FrontmatterMeta | null>>('kbFrontmatter', ref<FrontmatterMeta | null>(null))

/** 相对图片内联:容器挂载 / 目录变化时重建观察器(异步渲染器可能晚几帧才给出 <img>) */
let stopImgUpgrade: (() => void) | null = null
watch(
  [root, relDir],
  ([el, dir]) => {
    stopImgUpgrade?.()
    stopImgUpgrade = el && dir ? upgradeRelativeImages(el, dir) : null
  },
  { flush: 'post', immediate: true }
)
onBeforeUnmount(() => {
  stopImgUpgrade?.()
  stopImgUpgrade = null
})
</script>

<template>
  <div ref="root" :id="id" :class="className">
    <MarkdownFrontmatterHeader v-if="fmMeta" :meta="fmMeta" />
    <div v-html="html" />
  </div>
</template>
