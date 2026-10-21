<script setup lang="ts">
/**
 * 正文「文件引用」chip：文件图标 + 文件名（+ 行号），点击打开 / 激活文件预览页签。
 *
 * 只负责「显示 + 打开」：像不像引用由 utils/fileRefs 判定，绝对路径由调用方（节点渲染器）
 * 结合注入的基准目录解析好传进来——两个节点渲染器（行内代码 / 链接）共用这一个 chip，
 * 样式与打开行为因此只有一处。
 *
 * 图标与文件树 / 页签 / 最近文件同款字形（utils/fileIcons），悬浮提示给完整路径
 * （相对引用在正文里看不出去哪了，提示里补全）。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { fileRefName, type FileRef } from '@/utils/fileRefs'
import { useTabsStore } from '@/stores/tabs'
import { getFileIconUrl } from '@/utils/fileIcons'

const props = defineProps<{
  /** 解析结果（原始路径 + 行号） */
  refInfo: FileRef
  /** 已解析的打开路径（绝对；空串 = 无从定位，调用方不渲染 chip） */
  absPath: string
}>()

const { t } = useI18n()
const tabs = useTabsStore()

const name = computed(() => fileRefName(props.refInfo.path))
const iconUrl = computed(() => getFileIconUrl(name.value))

/** 行号文案（引用里写了行号才显示；区间取起始-结束） */
const lineText = computed(() => {
  const { line, endLine } = props.refInfo
  if (!line) return ''
  return endLine
    ? t('chat.fileRef.lineRange', { a: line, b: endLine })
    : t('chat.fileRef.line', { n: line })
})

function open(): void {
  tabs.openFileTab(
    { path: props.absPath, name: name.value },
    props.refInfo.line ? { targetLine: props.refInfo.line } : {}
  )
}
</script>

<template>
  <button type="button" class="fr-chip" :title="absPath" @click="open">
    <img class="fr-icon" :src="iconUrl" alt="" />
    <span class="fr-text">
      <span class="fr-name">{{ name }}</span>
      <span v-if="lineText" class="fr-line">({{ lineText }})</span>
    </span>
  </button>
</template>

<style scoped>
/* 文件引用：纯链接文字，无底色/描边；图标在文字外。 */
.fr-chip {
  display: inline;
  max-width: 100%;
  padding: 0;
  margin: 0;
  border: 0;
  background: transparent;
  color: var(--link-text);
  font-family: var(--font-mono);
  font-size: 0.875em;
  font-weight: 500;
  line-height: 1.5;
  white-space: nowrap;
  cursor: pointer;
  overflow-wrap: normal;
}

.fr-text {
  display: inline;
  padding: 0;
  border: 0;
  background: transparent;
  line-height: inherit;
}

.fr-chip:hover,
.fr-chip:focus-visible {
  color: color-mix(in srgb, var(--link-text) 88%, var(--text-primary));
}

.fr-chip:hover .fr-text,
.fr-chip:focus-visible .fr-text {
  text-decoration: underline dotted var(--link-text);
  text-underline-offset: 3px;
}

.fr-chip:active {
  color: var(--text-primary);
}

.fr-chip:focus-visible {
  outline: 2px solid var(--link-text);
  outline-offset: 2px;
  border-radius: var(--radius-xs);
}

/* 图标在文字外：1.1em，按文字中线对齐 */
.fr-icon {
  display: inline-block;
  width: 1.1em;
  height: 1.1em;
  margin-right: 5px;
  vertical-align: middle;
}

.fr-name {
  white-space: nowrap;
}

/* 行号使用同色弱化，采用括号样式与文件名连读。 */
.fr-line {
  white-space: nowrap;
  color: inherit;
  opacity: 0.7;
}
</style>
