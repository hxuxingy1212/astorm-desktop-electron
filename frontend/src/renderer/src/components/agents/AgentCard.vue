<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { AgentCard } from '@/services/dto'

const props = defineProps<{ agent: AgentCard }>()

const emit = defineEmits<{
  (e: 'edit', agent: AgentCard): void
  (e: 'delete', agent: AgentCard): void
  (e: 'preview', agent: AgentCard): void
}>()

const { t } = useI18n()

/** 执行模式文案键(与后端 AgentExecMode 值一致) */
const MODE_LABEL: Record<string, string> = {
  read_only: 'settings.agents.modeReadOnlyShort',
  read_write: 'settings.agents.modeReadWriteShort',
  auto: 'settings.agents.modeAutoShort',
  inherit: 'settings.agents.modeInheritShort'
}

/** 权限模式文案键 */
const PERMISSION_LABEL: Record<string, string> = {
  manual: 'settings.agents.permissionManualShort',
  auto: 'settings.agents.permissionAutoShort',
  inherit: 'settings.agents.permissionInheritShort'
}

const modeLabel = computed(() => t(MODE_LABEL[props.agent.mode] ?? MODE_LABEL.inherit))
const permissionLabel = computed(
  () => t(PERMISSION_LABEL[props.agent.permission] ?? PERMISSION_LABEL.inherit)
)
const isReadOnly = computed(() => props.agent.mode === 'read_only')

/** 工具集摘要（空 = 无工具：智能体不能调用任何工具） */
const toolSummary = computed(() =>
  props.agent.toolIds.length
    ? t('settings.agents.toolCount', { count: props.agent.toolIds.length })
    : t('settings.agents.noTool')
)

/** 技能集摘要（空 = 环境提示词不列技能；仍可经 skills 工具加载） */
const skillSummary = computed(() =>
  props.agent.skillIds.length
    ? t('settings.agents.skillCount', { count: props.agent.skillIds.length })
    : t('settings.agents.noSkill')
)

/** 前几个工具名（卡片上直接可见，避免只能看数量） */
const toolPreview = computed(() => props.agent.toolIds.slice(0, 4))

/** 内置智能体不可编辑 → 整卡点击即进入只读预览（自定义卡片仍走底部「编辑」） */
function onCardClick(): void {
  if (props.agent.builtin) emit('preview', props.agent)
}
</script>

<template>
  <div
    class="agent-card"
    :class="{ clickable: agent.builtin }"
    :role="agent.builtin ? 'button' : undefined"
    :title="agent.builtin ? t('settings.agents.previewTitle') : undefined"
    @click="onCardClick"
  >
    <div class="card-header">
      <div class="card-name-wrap">
        <span class="card-name">{{ agent.name }}</span>
        <a-tag v-if="agent.builtin" size="small" class="builtin-tag">
          {{ t('settings.agents.builtin') }}
        </a-tag>
      </div>
    </div>

    <p class="card-desc">{{ agent.description || t('settings.agents.noDescription') }}</p>

    <div class="card-meta">
      <a-tag v-if="isReadOnly" size="small" class="readonly-tag">{{ t('settings.agents.readOnlyTag') }}</a-tag>
      <span class="meta-item">{{ modeLabel }}</span>
      <span class="meta-divider">·</span>
      <span class="meta-item">{{ permissionLabel }}</span>
      <span class="meta-divider">·</span>
      <span class="meta-item">{{ toolSummary }}</span>
      <span class="meta-divider">·</span>
      <span class="meta-item">{{ skillSummary }}</span>
      <span v-if="agent.guidance" class="meta-divider">·</span>
      <span v-if="agent.guidance" class="meta-item">{{ t('settings.agents.hasGuidance') }}</span>
    </div>

    <div v-if="toolPreview.length" class="tool-preview">
      <a-tag v-for="tool in toolPreview" :key="tool" size="small">{{ tool }}</a-tag>
      <span v-if="agent.toolIds.length > toolPreview.length" class="more-tools">
        +{{ agent.toolIds.length - toolPreview.length }}
      </span>
    </div>

    <div class="card-footer">
      <template v-if="!agent.builtin">
        <a-button type="text" size="mini" @click="emit('edit', agent)">
          <template #icon><icon-edit :size="13" /></template>
          {{ t('common.edit') }}
        </a-button>
        <a-button type="text" size="mini" status="danger" @click="emit('delete', agent)">
          <template #icon><icon-delete :size="13" /></template>
          {{ t('common.delete') }}
        </a-button>
      </template>
      <span v-else class="builtin-hint">{{ t('settings.agents.builtinHint') }}</span>
      <a-button v-if="agent.builtin" type="text" size="mini" @click.stop="emit('preview', agent)">
        <template #icon><icon-eye :size="13" /></template>
        {{ t('settings.agents.view') }}
      </a-button>
    </div>
  </div>
</template>

<style scoped>
/* macOS 卡片:白底 + 细边框 + 大圆角 */
.agent-card {
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-lg);
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  transition: border-color var(--transition-fast), box-shadow var(--transition-fast);
}

.agent-card:hover {
  border-color: var(--accent-text);
  box-shadow: var(--shadow-sm);
}

/* 内置卡片整卡可点:进只读预览 */
.agent-card.clickable {
  cursor: pointer;
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

.card-name {
  font-weight: 600;
  font-size: var(--font-size-title);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.builtin-tag {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
}

.readonly-tag {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
}

.card-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.5;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  min-height: 20px;
}

.card-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.meta-divider {
  color: var(--border-base);
}

.tool-preview {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
}

.more-tools {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
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

.builtin-hint {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  margin-right: auto;
}

.card-footer :deep(.arco-btn) {
  font-size: var(--font-size-xs);
  border-radius: 6px;
}

.card-footer :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}
</style>
