<script setup lang="ts">
/**
 * 工具卡片（设置页「工具」页签）：与技能卡片同一套卡片网格。
 *
 * 工具本身是只读目录项，不提供启用/编辑/删除；底部改为「参数 / 复制」，
 * Schema 在弹窗里查看，避免长 JSON 把卡片撑高。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ToolInfo } from '@/services/dto'

const props = defineProps<{ tool: ToolInfo; compact?: boolean; selectable?: boolean }>()

const emit = defineEmits<{
  (e: 'view', tool: ToolInfo): void
  (e: 'select', tool: ToolInfo): void
  (e: 'copy', tool: ToolInfo): void
}>()

const { t } = useI18n()

/** JSON Schema 的参数个数（properties 的键数） */
const paramCount = computed(() => {
  const schema = props.tool.parametersSchema as { properties?: unknown } | undefined
  const properties = schema?.properties
  return properties && typeof properties === 'object' ? Object.keys(properties).length : 0
})

/** 来源：MCP 带服务器名；插件 / 内置分开；动态工具在头部另有「按需加载」徽标 */
const sourceLabel = computed(() => {
  const source = props.tool.source
  if (source?.type === 'mcp') {
    return source.server ? `${t('settings.tools.sourceMcp')} · ${source.server}` : t('settings.tools.sourceMcp')
  }
  if (source?.type === 'plugin') return t('settings.tools.sourcePlugin')
  return t('settings.tools.sourceBuiltin')
})
</script>

<template>
  <div
    class="tool-card"
    :class="{ 'is-compact': compact, 'is-selectable': selectable }"
    :role="selectable ? 'button' : undefined"
    :tabindex="selectable ? 0 : undefined"
    @click="selectable && emit('select', tool)"
    @keydown.enter="selectable && emit('select', tool)"
  >
    <div class="card-header">
      <div class="card-name-wrap">
        <icon-tool :size="15" class="card-icon" />
        <span class="card-name">{{ tool.name }}</span>
        <span v-if="tool.dynamic" class="card-badge">{{ t('settings.tools.dynamicBadge') }}</span>
      </div>
    </div>

    <p v-if="tool.description" class="card-desc">{{ tool.description }}</p>

    <div class="card-meta">
      <span v-if="paramCount" class="card-param">
        {{ t('settings.tools.paramCount', { n: paramCount }) }}
      </span>
      <span class="card-source">{{ sourceLabel }}</span>
    </div>

    <div class="card-footer">
      <a-button
        type="text"
        size="mini"
        @click.stop="selectable ? emit('select', tool) : emit('view', tool)"
      >
        <template #icon><icon-code :size="13" /></template>
        {{ selectable ? t('settings.tools.details') : t('settings.tools.viewSchema') }}
      </a-button>
      <a-button type="text" size="mini" @click.stop="emit('copy', tool)">
        <template #icon><icon-copy :size="13" /></template>
        {{ t('common.copy') }}
      </a-button>
    </div>
  </div>
</template>

<style scoped>
/* 卡片底色 / 圆角 / hover 由 styles/cards.css 的共享卡片配方提供，这里只管布局。 */
.tool-card {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 8px;
  padding: 16px;
}

/* MCP 服务器卡片内嵌：收一档内边距，描述只留一行 */
.tool-card.is-compact {
  padding: 12px;
}

.tool-card.is-compact .card-desc {
  -webkit-line-clamp: 1;
}

/* 可点击进入详情（MCP 工具）：整卡为热区，键盘可达 */
.tool-card.is-selectable {
  cursor: pointer;
}

.tool-card.is-selectable:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 2px;
}

.card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-width: 0;
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
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.card-badge {
  flex-shrink: 0;
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--bg-3);
  color: var(--text-tertiary);
  font-size: var(--font-size-2xs);
}

.card-desc {
  display: -webkit-box;
  overflow: hidden;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  font-size: var(--font-size-sm);
  line-height: 1.5;
  color: var(--text-secondary);
}

.card-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: auto;
  padding-top: 4px;
}

/* 参数数量：轻量填充胶囊，无描边；与右侧来源文字同一水平线 */
.card-param {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  height: 18px;
  padding: 0 8px;
  border-radius: 999px;
  background: var(--bg-2);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
  line-height: 18px;
}

.card-source {
  margin-left: auto;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.card-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  padding-top: 4px;
  border-top: 1px solid var(--border-light);
}

.card-footer :deep(.arco-btn) {
  border-radius: 6px;
  font-size: var(--font-size-xs);
}

.card-footer :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}
</style>
