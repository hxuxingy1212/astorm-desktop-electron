<script setup lang="ts">
/**
 * 对话框工具行的裸图标按钮（计划 / 目标 / 子会话等入口共用）：
 * 图标 + 可选计数/短标签，悬浮给底色；容器变窄时只留图标。
 */
defineProps<{
  /** 原生 title（悬停原生提示；有自定义悬浮卡时可不传） */
  title?: string
  /** 无障碍标签；缺省回退 title */
  ariaLabel?: string
  /** 图标右侧的短标签（如 `3/5`）；不传则只显示图标 */
  label?: string
  disabled?: boolean
}>()

defineEmits<{ (e: 'click', event: MouseEvent): void }>()
</script>

<template>
  <button
    type="button"
    class="foot-icon-btn"
    :title="title"
    :aria-label="ariaLabel ?? title"
    :disabled="disabled"
    @click="$emit('click', $event)"
  >
    <slot />
    <span v-if="label" class="foot-icon-label">{{ label }}</span>
  </button>
</template>

<style scoped>
.foot-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  height: 28px;
  max-width: 190px;
  padding: 0 6px;
  border: none;
  border-radius: var(--radius-sm);
  background-color: transparent;
  color: var(--text-secondary);
  font: inherit;
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.foot-icon-btn:hover {
  background-color: var(--bg-3);
  color: var(--text-primary);
}

.foot-icon-btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.foot-icon-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-xs);
  line-height: 1;
  font-variant-numeric: tabular-nums;
}

/* 对话框宽度不足时只显示图标（隐藏计数/标签）；容器为 .chat-section（全局声明） */
@container (max-width: 720px) {
  .foot-icon-label {
    display: none;
  }
}
</style>
