<script setup lang="ts">
import { ref, watch } from 'vue'
import type { PluginConfigField } from '@/services/dto'

/**
 * 插件配置动态表单：按 manifest.configSchema 渲染（text/textarea/password/secret/number/boolean/select），
 * 行式布局与「设置 → 系统设置」一致（标签左、控件右）。
 * 本地 draft 为唯一数据源；变更经 update:modelValue 上抛由父组件持有。
 * 每选中一个插件即重新挂载（父组件 v-if 控制），无需监听 modelValue 回流。
 */
const props = defineProps<{ fields: PluginConfigField[]; modelValue: Record<string, unknown> }>()
const emit = defineEmits<{ (e: 'update:modelValue', v: Record<string, unknown>): void }>()

const draft = ref<Record<string, unknown>>({})

function defaultValueFor(f: PluginConfigField): unknown {
  if (f.type === 'boolean') return false
  if (f.type === 'number') return 0
  return ''
}

function init(): void {
  const next: Record<string, unknown> = {}
  for (const f of props.fields) {
    next[f.key] = props.modelValue[f.key] ?? f.default ?? defaultValueFor(f)
  }
  draft.value = next
}

watch(() => props.fields, () => init(), { immediate: true })

function setValue(key: string, value: unknown): void {
  draft.value = { ...draft.value, [key]: value }
  emit('update:modelValue', { ...draft.value })
}

/** 必填校验（保存前调用）：任何必填字段为空即失败 */
function validate(): boolean {
  for (const f of props.fields) {
    const v = draft.value[f.key]
    if (f.required && (v === undefined || v === null || v === '')) return false
  }
  return true
}

defineExpose({ validate })
</script>

<template>
  <div class="plug-config-form">
    <div v-for="f in fields" :key="f.key" class="pc-row">
      <div class="pc-label-wrap">
        <span class="pc-label">
          {{ f.label }}
          <span v-if="f.required" class="pc-required">*</span>
        </span>
        <span v-if="f.description" class="pc-desc">{{ f.description }}</span>
      </div>
      <div class="pc-control">
        <a-switch
          v-if="f.type === 'boolean'"
          :model-value="Boolean(draft[f.key])"
          size="small"
          @update:model-value="(v) => setValue(f.key, v)"
        />
        <a-select
          v-else-if="f.type === 'select'"
          :model-value="String(draft[f.key] ?? '')"
          :options="(f.options ?? []).map((o) => ({ label: o.label, value: o.value }))"
          allow-clear
          style="width: 210px"
          @update:model-value="(v) => setValue(f.key, v)"
        />
        <a-input-number
          v-else-if="f.type === 'number'"
          :model-value="Number(draft[f.key] ?? 0)"
          size="small"
          style="width: 110px"
          @update:model-value="(v) => setValue(f.key, v)"
        />
        <a-textarea
          v-else-if="f.type === 'textarea'"
          :model-value="String(draft[f.key] ?? '')"
          :placeholder="f.placeholder"
          :auto-size="{ minRows: 2, maxRows: 4 }"
          style="width: 300px"
          @update:model-value="(v) => setValue(f.key, v)"
        />
        <a-input
          v-else
          :model-value="String(draft[f.key] ?? '')"
          :type="f.type === 'secret' || f.type === 'password' ? 'password' : 'text'"
          :placeholder="f.placeholder"
          allow-clear
          style="width: 260px"
          @update:model-value="(v) => setValue(f.key, v)"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 行式布局:与「设置 → 系统设置」的 .settings-row 完全一致的 macOS 两行式(标题加粗 + 描述灰字) */
.plug-config-form {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.pc-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 8px 0;
}

.pc-label-wrap {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.pc-label {
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.pc-required {
  color: var(--danger);
  margin-left: 2px;
}

.pc-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

.pc-control {
  flex-shrink: 0;
  display: flex;
  align-items: center;
}

/* 开关:macOS 绿色滑块 */
.pc-control :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.pc-control :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}

/* 输入框 / 下拉 / 数字:更柔和的圆角 */
.pc-control :deep(.arco-input-wrapper),
.pc-control :deep(.arco-input-outer),
.pc-control :deep(.arco-textarea-wrapper),
.pc-control :deep(.arco-select),
.pc-control :deep(.arco-select-view) {
  border-radius: 8px;
}
</style>