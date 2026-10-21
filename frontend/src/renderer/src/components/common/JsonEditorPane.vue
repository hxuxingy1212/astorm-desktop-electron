<script setup lang="ts">
/**
 * 共享 JSON 编辑器（安全配置 / MCP 配置共用的唯一实现）。
 *
 * 基于 json-editor-vue 文本模式：行号 + 折叠 + 语法高亮，隐藏菜单/导航/状态栏；
 * 外观与设置页的代码块/正文同一套令牌（等宽 13px、1.65 行高、行号槽与编辑器同底）。
 */
import { computed } from 'vue'
import JsonEditorVue from 'json-editor-vue'
import { Mode } from 'vanilla-jsoneditor'
import 'vanilla-jsoneditor/themes/jse-theme-dark.css'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'

withDefaults(defineProps<{ modelValue: string }>(), { modelValue: '' })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const settings = useSettingsStore()
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)
</script>

<template>
  <JsonEditorVue
    class="json-editor-pane"
    :class="{ 'jse-theme-dark': isDark }"
    :model-value="modelValue"
    :mode="Mode.text"
    :debounce="0"
    :main-menu-bar="false"
    :navigation-bar="false"
    :status-bar="false"
    :ask-to-format="false"
    @update:model-value="emit('update:modelValue', $event)"
  />
</template>

<style scoped>
.json-editor-pane,
.json-editor-pane.jse-theme-dark {
  /* 与代码预览共用字体/字号；行高放宽到阅读舒适的 1.65 */
  --jse-font-family-mono: var(--font-mono);
  --jse-font-size-mono: 13px;
  --jse-line-height: 1.65;

  /* 编辑器与行号槽同底，不画内部分隔线 */
  --jse-background-color: var(--bg-base);
  --jse-text-color: var(--text-primary);
  --jse-delimiter-color: var(--text-secondary);
  --jse-main-border: 0;
  --jse-panel-background: var(--bg-base);
  --jse-panel-border: 0;
  --jse-panel-color-readonly: var(--text-tertiary);

  /* 编辑反馈收敛到选中态，去掉整行底色 */
  --jse-active-line-background-color: transparent;
  --jse-selection-background-color: color-mix(in srgb, var(--accent-text) 18%, transparent);
  --jse-hover-background-color: var(--fill-1);

  flex: 1;
  min-height: 0;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-xl);
  overflow: hidden;
}

.json-editor-pane :deep(.jse-main),
.json-editor-pane :deep(.jse-text-mode) {
  height: 100%;
}

/* 行号槽：与代码同底、无右边线；行号右对齐留出呼吸位 */
.json-editor-pane :deep(.cm-editor) {
  background: var(--bg-base);
}

.json-editor-pane :deep(.cm-gutters) {
  background: var(--bg-base);
  border-right: none;
  /* 行号/折叠箭头是 gutter 文本，选中会整列变蓝 */
  user-select: none;
  -webkit-user-select: none;
}

/* 去掉当前行的整行/整槽高亮 */
.json-editor-pane :deep(.cm-activeLineGutter) {
  background: transparent !important;
}

/* 空的 lint 槽不占宽（CodeMirror 用 !important 固定 display:flex） */
.json-editor-pane :deep(.cm-gutter-lint) {
  display: none !important;
}

.json-editor-pane :deep(.cm-lineNumbers .cm-gutterElement) {
  padding: 0 10px 0 12px;
}

/* 折叠箭头：CSS 画的居中 chevron（隐藏 ⌄/› 文本字形） */
.json-editor-pane :deep(.cm-foldGutter .cm-gutterElement) {
  color: var(--text-tertiary);
  cursor: pointer;
  padding: 0 5px;
}

.json-editor-pane :deep(.cm-foldGutter .cm-gutterElement span) {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 10px;
  height: 100%;
  font-size: 0;
  outline: none;
}

.json-editor-pane :deep(.cm-foldGutter .cm-gutterElement span::before) {
  content: '';
  width: 7px;
  height: 7px;
  border-right: 1.5px solid currentColor;
  border-bottom: 1.5px solid currentColor;
  transform: rotate(45deg) translate(-1px, -1px);
}

.json-editor-pane :deep(.cm-foldGutter .cm-gutterElement span[title='Unfold line']::before) {
  transform: rotate(-45deg) translate(1px, -1px);
}

.json-editor-pane :deep(.cm-foldGutter .cm-gutterElement:hover) {
  color: var(--text-primary);
}

.json-editor-pane :deep(.cm-line) {
  padding: 0 12px 0 8px;
}

/* 去掉 JSON 缩进参考竖线（缩进标记画在行首伪元素上） */
.json-editor-pane :deep(.cm-indent-markers::before) {
  display: none !important;
}

/* 自带错误横幅是英文且占一整条；错误统一显示在编辑器上方 */
.json-editor-pane :deep(.jse-message.jse-error) {
  display: none;
}
</style>
