<script setup lang="ts">
/**
 * 链接节点渲染器：应用内**唯一的链接渲染入口**，三条分支——
 *
 * ① 目标是本地文件（`file://…` / 绝对路径 / 相对路径，可带行号后缀）→ `FileRefChip`
 *    （文件名 + 行号，点了开文件预览页签并定位到行；相对路径按注入的 `FILE_REF_BASE` 解析）；
 * ② 文字是**编号**且目标是 http(s) → `WebRefBadge`（网页引用角标，点了在应用内浏览器打开来源）；
 * ③ 其余 → `MarkdownLink`：外链确认弹窗、站内浏览器页签、异常 URL 错误态，行为完全不变。
 *
 * URL 清洗（harden）与 MarkdownLink 同一套 `useSanitizers`：被拦下的 URL 不当作引用，
 * 交回 MarkdownLink 渲染成错误态。
 */
import { computed, inject, type Ref } from 'vue'
import { useContext, useSanitizers, type LinkNodeRendererProps } from 'vue-stream-markdown'
import FileRefChip from '@/components/markdown/FileRefChip.vue'
import WebRefBadge from '@/components/markdown/WebRefBadge.vue'
import MarkdownLink from '@/components/markdown/MarkdownLink.vue'
import { FILE_REF_BASE, parseFileHref, resolveFilePath } from '@/utils/fileRefs'

const props = defineProps<LinkNodeRendererProps>()

const base = inject<Ref<string> | undefined>(FILE_REF_BASE, undefined)
const { hardenOptions } = useContext()

const rawUrl = computed(() => props.node.url ?? '')
const loading = computed(
  () =>
    !!props.node.loading ||
    !!props.markdownParser?.hasLoadingNode?.(props.node.children) ||
    !rawUrl.value
)
const { transformedUrl, isHardenUrl } = useSanitizers({ url: rawUrl, hardenOptions, loading })

/* ① 本地文件引用 */
const refInfo = computed(() => parseFileHref(rawUrl.value))
/** 打开用绝对路径（空串 = 无法定位 → 按普通链接走 MarkdownLink） */
const absPath = computed(() =>
  refInfo.value ? resolveFilePath(refInfo.value.path, base?.value ?? '') : ''
)

/* ② 网页引用角标：链接文字是纯编号（`[1](url)` / `[[1]](url)`），目标是 http(s) */

/** 链接的可见文字（编号判据） */
function linkText(): string {
  const walk = (nodes: readonly unknown[]): string =>
    nodes
      .map((n) => {
        const node = n as { value?: unknown; children?: unknown[] }
        if (typeof node.value === 'string') return node.value
        if (Array.isArray(node.children)) return walk(node.children)
        return ''
      })
      .join('')
  return walk(props.node.children ?? []).trim()
}

const webRef = computed(() => {
  const url = transformedUrl.value
  if (isHardenUrl.value || typeof url !== 'string' || !/^https?:\/\//i.test(url)) return null
  const m = /^\[?(\d{1,3})\]?$/.exec(linkText())
  return m ? { index: m[1], url } : null
})
</script>

<template>
  <FileRefChip v-if="refInfo && absPath" :ref-info="refInfo" :abs-path="absPath" />
  <WebRefBadge v-else-if="webRef" :index="webRef.index" :url="webRef.url" />
  <MarkdownLink v-else v-bind="props" />
</template>
