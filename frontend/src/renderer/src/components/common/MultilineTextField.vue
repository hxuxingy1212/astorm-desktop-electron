<script setup lang="ts">
/**
 * 大段文档输入框（多行纯文本）。
 *
 * 用于约定 / 智能体指引 / 技能指令等「写一大段文字」的场景；不再提供 Markdown
 * 工具栏与分屏预览——内容仍是 Markdown 文本，只是编辑面收敛为普通多行输入。
 * 样式见 `styles/inputs.css` 第 5 节（描边 + 填充 + 原生纵向缩放柄）。
 */
withDefaults(
  defineProps<{
    modelValue: string
    placeholder?: string
    disabled?: boolean
    maxLength?: number
    /** 最小高度（px，默认 200）；用户可拖动右下角继续拉高 */
    minHeight?: number
    /** 撑满父容器剩余高度（弹性布局里用，如定时任务右栏） */
    fill?: boolean
  }>(),
  { placeholder: '', disabled: false, maxLength: undefined, minHeight: 200, fill: false }
)

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
</script>

<template>
  <a-textarea
    class="multiline-text-field"
    :class="{ 'is-fill': fill }"
    :model-value="modelValue"
    :placeholder="placeholder"
    :disabled="disabled"
    :max-length="maxLength"
    :style="{ '--multiline-min-height': `${minHeight}px` }"
    @update:model-value="emit('update:modelValue', $event)"
  />
</template>
