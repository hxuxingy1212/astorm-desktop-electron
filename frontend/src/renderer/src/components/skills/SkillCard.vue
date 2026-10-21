<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SkillSummary } from '@/services/dto'

const props = defineProps<{ skill: SkillSummary }>()

const emit = defineEmits<{
  (e: 'toggle', name: string, enabled: boolean): void
  (e: 'edit', skill: SkillSummary): void
  (e: 'delete', skill: SkillSummary): void
}>()

const { t } = useI18n()

const categories = computed(() => {
  if (!props.skill.category) return []
  return props.skill.category
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
})

const tagList = computed(() => {
  if (!Array.isArray(props.skill.tags)) return []
  return props.skill.tags
})

const sourceLabels: Record<string, string> = {
  user: 'settings.skills.sourceUser',
  project: 'settings.skills.sourceProject',
  global: 'settings.skills.sourceGlobal',
  builtin: 'settings.skills.sourceBuiltin'
}
</script>

<template>
  <div class="skill-card" :class="{ disabled: !skill.enabled }">
    <div class="card-header">
      <div class="card-name-wrap">
        <icon-sparkles :size="15" class="card-icon" />
        <span class="card-name">{{ skill.name }}</span>
        <span v-if="skill.version" class="card-version">v{{ skill.version }}</span>
      </div>
      <a-switch
        :model-value="skill.enabled"
        size="small"
        @change="(v) => emit('toggle', skill.name, v as boolean)"
      />
    </div>

    <p class="card-desc">{{ skill.description }}</p>

    <div class="card-meta">
      <a-tag v-for="cat in categories" :key="cat" size="small">
        {{ cat }}
      </a-tag>
      <a-tag v-for="tag in tagList" :key="tag" size="small">
        {{ tag }}
      </a-tag>
      <span class="card-source">{{ t(sourceLabels[skill.source] || 'settings.skills.sourceUser') }}</span>
    </div>

    <div class="card-footer">
      <a-button type="text" size="mini" @click="emit('edit', skill)">
        <template #icon><icon-edit :size="13" /></template>
        {{ t('common.edit') }}
      </a-button>
      <a-button
        v-if="skill.source === 'user'"
        type="text"
        size="mini"
        status="danger"
        @click="emit('delete', skill)"
      >
        <template #icon><icon-delete :size="13" /></template>
        {{ t('common.delete') }}
      </a-button>
    </div>
  </div>
</template>

<style scoped>
/* macOS 卡片:白底 + 细边框 + 大圆角 */
.skill-card {
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-lg);
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  transition: border-color var(--transition-fast), box-shadow var(--transition-fast), opacity var(--transition-fast);
}

.skill-card:hover {
  border-color: var(--accent-text);
  box-shadow: var(--shadow-sm);
}

.skill-card.disabled {
  opacity: 0.65;
}

.card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.card-name-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.card-icon {
  flex-shrink: 0;
  color: var(--text-primary);
}

.card-name {
  font-weight: 600;
  font-size: var(--font-size-title);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.card-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.5;
  display: -webkit-box;
  -webkit-line-clamp: 1;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.card-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.card-version {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  font-family: monospace;
  background: var(--bg-3);
  padding: 1px 5px;
  border-radius: 4px;
}

.card-source {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  margin-left: auto;
}

.card-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  margin-top: auto;
  padding-top: 4px;
  border-top: 1px solid var(--border-light);
}

.card-footer :deep(.arco-btn) {
  font-size: var(--font-size-xs);
  border-radius: 6px;
}

/* 按钮图标:垂直居中对齐,与文字尺寸协调 */
.card-footer :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}

/* ---------- macOS 风格控件覆盖 ---------- */

/* 开关:macOS 绿色滑块 + 白色圆钮 */
.skill-card :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.skill-card :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}
</style>