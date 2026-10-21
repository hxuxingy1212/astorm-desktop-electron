<script setup lang="ts">
/**
 * MCP 服务器卡片：与工具卡片同一套卡片网格。
 *
 * 卡片主体（名称 / 传输方式 / 地址 / 状态）点击进入该服务器的工具卡片页；
 * 连接、断开、启用开关与「工具」入口放在底部操作区，点击不触发整卡跳转。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { McpServerSummary } from '@/services/dto'

const props = defineProps<{
  server: McpServerSummary
  connecting?: boolean
  /** 已缓存的工具数量（未连接 / 未加载时为 0，不显示） */
  toolCount?: number
}>()

const emit = defineEmits<{
  open: []
  connect: []
  disconnect: []
  auth: []
  toggle: [value: boolean]
}>()

const { t } = useI18n()

const transport = computed(() => (props.server.transportType === 'http' ? 'HTTP' : 'STDIO'))
const statusLabel = computed(() => {
  if (props.server.authRequired) return t('settings.mcp.needsAuth')
  return props.server.connected
    ? t('settings.mcp.connected')
    : props.server.enabled
      ? t('settings.mcp.disconnected')
      : t('settings.mcp.disabled')
})
const statusColor = computed(() =>
  props.server.authRequired
    ? 'orange'
    : props.server.connected
      ? 'green'
      : props.server.enabled
        ? 'gray'
        : 'red'
)
</script>

<template>
  <div
    class="mcp-server-card"
    :class="{ 'is-disabled': !server.enabled }"
    role="button"
    tabindex="0"
    @click="emit('open')"
    @keydown.enter="emit('open')"
  >
    <div class="card-header">
      <div class="card-name-wrap">
        <icon-link :size="15" class="card-icon" />
        <span class="card-name">{{ server.name }}</span>
        <span class="card-badge">{{ transport }}</span>
      </div>
    </div>

    <p class="card-desc">{{ server.command || server.url || '—' }}</p>

    <div class="card-meta">
      <a-tag size="small" :color="statusColor">{{ statusLabel }}</a-tag>
      <span v-if="toolCount" class="card-source">
        {{ t('settings.mcp.toolCount', { n: toolCount }) }}
      </span>
    </div>

    <div class="card-footer" @click.stop>
      <a-button
        v-if="server.authRequired"
        type="text"
        size="mini"
        status="warning"
        @click="emit('auth')"
      >
        <template #icon><icon-lock :size="13" /></template>
        {{ t('settings.mcp.authorize') }}
      </a-button>
      <a-button
        v-else-if="server.enabled && !server.connected"
        type="text"
        size="mini"
        :loading="connecting"
        @click="emit('connect')"
      >
        <template #icon><icon-link :size="13" /></template>
        {{ t('settings.mcp.connect') }}
      </a-button>
      <a-button
        v-else-if="server.connected"
        type="text"
        size="mini"
        status="warning"
        @click="emit('disconnect')"
      >
        <template #icon><icon-unlink :size="13" /></template>
        {{ t('settings.mcp.disconnect') }}
      </a-button>
      <a-switch
        :model-value="server.enabled"
        size="small"
        @change="(val) => emit('toggle', Boolean(val))"
      />
      <a-button
        v-if="!server.authRequired"
        class="view-tools"
        type="text"
        size="mini"
        @click="emit('open')"
      >
        {{ t('settings.mcp.viewTools') }}
        <icon-right :size="12" class="view-tools-chevron" />
      </a-button>
    </div>
  </div>
</template>

<style scoped>
/* 底色 / 圆角 / hover 由 styles/cards.css 的 .mcp-server-card 配方提供 */
.mcp-server-card {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 8px;
  padding: 16px;
  cursor: pointer;
}

.mcp-server-card:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 2px;
}

.mcp-server-card.is-disabled {
  opacity: 0.5;
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
  font-family: var(--font-mono);
  font-size: var(--font-size-code);
  line-height: 1.55;
  color: var(--text-secondary);
  overflow-wrap: anywhere;
}

.card-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: auto;
  padding-top: 4px;
}

.card-source {
  margin-left: auto;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.card-footer {
  display: flex;
  align-items: center;
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

.view-tools {
  margin-left: auto;
}

.view-tools-chevron {
  margin-left: 2px;
}

/* 启用开关:macOS 绿色滑块 */
.mcp-server-card :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.mcp-server-card :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}
</style>
