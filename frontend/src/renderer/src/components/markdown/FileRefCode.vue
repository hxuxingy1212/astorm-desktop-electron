<script setup lang="ts">
/**
 * 行内代码节点渲染器：内容像**文件引用**（`` `auth.py:47` ``）时渲染为可点击的 FileRefChip，
 * 其余一律转交库内置的行内代码渲染器（同一套 class → 外观零漂移）。
 *
 * 相对路径按注入的基准目录（`FILE_REF_BASE`）解析为绝对路径；没有基准目录
 * （游离的 markdown 渲染，如记忆 / 轨迹面板）时相对引用无从定位，退回普通行内代码，
 * 绝对路径与 `file://` 引用不受影响。
 */
import { computed, inject, type Ref } from 'vue'
import { NODE_RENDERERS, type InlineCodeNodeRendererProps } from 'vue-stream-markdown'
import FileRefChip from '@/components/markdown/FileRefChip.vue'
import { FILE_REF_BASE, parseInlineFileRef, resolveFilePath } from '@/utils/fileRefs'

const props = defineProps<InlineCodeNodeRendererProps>()

const base = inject<Ref<string> | undefined>(FILE_REF_BASE, undefined)
const BuiltinInlineCode = NODE_RENDERERS.inlineCode

const refInfo = computed(() => parseInlineFileRef(props.node.value ?? ''))
/** 打开用绝对路径（空串 = 无法定位 → 退化成普通行内代码） */
const absPath = computed(() =>
  refInfo.value ? resolveFilePath(refInfo.value.path, base?.value ?? '') : ''
)
</script>

<template>
  <FileRefChip v-if="refInfo && absPath" :ref-info="refInfo" :abs-path="absPath" />
  <component :is="BuiltinInlineCode" v-else v-bind="props" />
</template>
