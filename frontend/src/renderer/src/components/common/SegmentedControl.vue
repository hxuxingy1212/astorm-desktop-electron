<script setup lang="ts" generic="T extends string">
/**
 * 分段控件（docs/ui-style.md §3.4）：一排胶囊、容器**不套槽**，选中 `--fill-3`。
 *
 * - `md`（默认）：高 34px / 15px，页面级（设置页时间范围等）。
 * - `sm`：高 26px / 13px，紧凑菜单内使用。
 * - `block`：整排铺满父容器（表单行 / 菜单里需要等宽两格时）。
 */
withDefaults(
  defineProps<{
    options: { label: string; value: T }[]
    size?: 'sm' | 'md'
    block?: boolean
  }>(),
  { size: 'md', block: false }
)

const model = defineModel<T>({ required: true })
</script>

<template>
  <div class="seg-control" :class="[`is-${size}`, { 'is-block': block }]">
    <button
      v-for="opt in options"
      :key="opt.value"
      type="button"
      class="seg-item"
      :class="{ on: model === opt.value }"
      :aria-pressed="model === opt.value"
      @click="model = opt.value"
    >
      {{ opt.label }}
    </button>
  </div>
</template>

<style scoped>
.seg-control {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.seg-control.is-block {
  display: flex;
  width: 100%;
}

.seg-control.is-block .seg-item {
  flex: 1;
}

.seg-item {
  height: 34px;
  padding: 0 14px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-title);
  font-weight: 500;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.seg-item:hover {
  background: var(--fill-1);
  color: var(--text-primary);
}

.seg-item.on {
  background: var(--fill-3);
  color: var(--text-primary);
}

/* 紧凑档：菜单里的两格模式切换 */
.is-sm {
  gap: 4px;
}

.is-sm .seg-item {
  height: 26px;
  padding: 0 10px;
  border-radius: 7px;
  font-size: var(--font-size-sm);
}
</style>
