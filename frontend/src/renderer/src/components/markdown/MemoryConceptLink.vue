<script setup lang="ts">
/**
 * 记忆正文里的链接渲染：Bundle 内的概念互链 / 原文链接走站内跳转，
 * 其余链接沿用 MarkdownLink（外链弹窗）。
 *
 * 相对路径的基准目录由父级 MemoryConceptPanel 通过 `memoryConceptDir` 注入。
 */
import { computed, inject, ref, type Ref } from 'vue'
import { NodeList, type LinkNodeRendererProps } from 'vue-stream-markdown'
import MarkdownLink from '@/components/markdown/MarkdownLink.vue'
import { resolveMemoryLink } from '@/utils/memoryLinks'
import { useTabsStore } from '@/stores/tabs'

const props = defineProps<LinkNodeRendererProps>()

const tabs = useTabsStore()
const dir = inject<Ref<string>>('memoryConceptDir', ref(''))

const target = computed(() => resolveMemoryLink(props.node.url ?? '', dir.value))

/** 链接的可见文字（用作概念页签标题） */
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

function onClick(event: MouseEvent): void {
  const tg = target.value
  if (!tg) return
  event.preventDefault()
  event.stopPropagation()
  if (tg.kind === 'concept') tabs.openMemoryConceptTab(tg.id, linkText() || undefined)
  else tabs.openMemoryRawTab(tg.stem, tg.stem)
}
</script>

<template>
  <MarkdownLink v-if="!target" v-bind="props" />
  <span v-else data-stream-markdown="link-container" class="inline">
    <a
      data-stream-markdown="link"
      class="text-primary underline cursor-pointer [overflow-wrap:anywhere]"
      @click="onClick"
    >
      <NodeList :parent-node="node" :nodes="node.children" :deep="deep + 1" />
    </a>
  </span>
</template>
