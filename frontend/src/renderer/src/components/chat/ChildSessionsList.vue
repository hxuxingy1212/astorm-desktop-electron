<script setup lang="ts">
/**
 * 子智能体列表：对话区右上角**规划胶囊**展开面板里的一节（计划列表下方）。
 *
 * 行结构参考 DeepSeek Harness：左列「标题 + 副行 meta（智能体名 · 状态）」，
 * 右列「token 数 + 耗时」，最右 chevron。已创建的子会话点条目在**右侧打开页签**
 * （实时输出 / 审批 / 表单）；「未开始」的委派点条目只展示任务文本（用户消息）。
 */
import { onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import { useTabsStore } from '@/stores/tabs'
import { useAgentsStore } from '@/stores/agents'
import { formatCount } from '@/utils/number'
import { formatDuration } from '@/utils/time'
import type { ChildSessionItem } from './childSessions'
import { localizedSessionTitle } from '@/utils/sessionTitle'

const props = defineProps<{ items: ChildSessionItem[] }>()

const emit = defineEmits<{
  /** 点击某个子会话条目（已跳转/打开页签）→ 供外层收起悬浮卡片 */
  (e: 'select'): void
}>()

const { t } = useI18n()
const sessionsStore = useSessionsStore()
const tabsStore = useTabsStore()
const agentsStore = useAgentsStore()

onMounted(() => {
  if (agentsStore.agents.length === 0) void agentsStore.fetchAgents()
})

type ChildStatus =
  | 'running'
  | 'form'
  | 'interrupted'
  | 'ignored'
  | 'cancelled'
  | 'error'
  | 'done'
  | 'idle'
  | 'pending'

/** 状态口径与侧栏会话行一致：流式 > 等你处理 > 终态（含「已忽略提问」灰点） > 空闲；未开始 = pending */
function statusOf(item: ChildSessionItem): ChildStatus {
  if (item.kind === 'pending') return 'pending'
  const id = item.session.id
  if (sessionsStore.isStreamingSession(id)) return 'running'
  const flag = sessionsStore.taskFlags[id]
  if (flag?.formWaiting || flag?.awaitingApproval) return 'form'
  if (flag?.end === 'interrupted') return 'interrupted'
  if (flag?.end === 'ignored') return 'ignored'
  if (flag?.end === 'cancelled') return 'cancelled'
  if (flag?.end === 'error') return 'error'
  if (flag?.end === 'done') return 'done'
  return 'idle'
}

/** 状态词（副行 meta 用简短文案） */
function statusWord(status: ChildStatus): string {
  if (status === 'form') return t('chat.subStatus.waiting')
  return t(`chat.subStatus.${status}`)
}

/** 副行 meta：智能体名 · 状态 */
function metaOf(item: ChildSessionItem): string {
  const status = statusWord(statusOf(item))
  if (item.kind === 'pending') return status
  const agentType = item.session.agentType
  const agent = agentType
    ? agentsStore.agents.find((a) => a.agentId === agentType)?.name ?? agentType
    : ''
  return [agent, status].filter(Boolean).join(' · ')
}

/** 右列 token 数：子会话累计用量（聚合后的自身用量；为 0 时不显示） */
function tokOf(item: ChildSessionItem): string {
  if (item.kind === 'pending') return ''
  const total = sessionsStore.sessionInfo[item.session.id]?.sessionUsage?.total ?? 0
  return total > 0 ? t('chat.subAgentTok', { n: formatCount(total) }) : ''
}

/** 右列耗时：创建 → 最近更新（子会话每轮更新 updatedAt，耗时随之增长） */
function durOf(item: ChildSessionItem): string {
  if (item.kind === 'pending') return ''
  const created = item.session.createdAt
  const updated = item.session.updatedAt ?? created
  return formatDuration(updated - created)
}

function open(item: ChildSessionItem): void {
  if (item.kind === 'pending') {
    tabsStore.openPendingSubSessionTab(item.toolCallId, item.title, item.task, item.parentBackendId, item.callIndex)
  } else {
    tabsStore.openSessionTab(item.session.id, localizedSessionTitle(item.session))
  }
  // 点击即完成跳转，通知外层收起悬浮卡片
  emit('select')
}
</script>

<template>
  <div class="subs-list">
    <div
      v-for="item in props.items"
      :key="item.key"
      class="subs-item"
      @click="open(item)"
    >
      <span class="subs-slot">
        <icon-loading v-if="statusOf(item) === 'running'" class="subs-spin" :size="13" />
        <span v-else class="subs-dot" :class="`subs-dot-${statusOf(item)}`" />
      </span>
      <span class="subs-main">
        <span class="subs-title">{{ item.title || t('common.unnamed') }}</span>
        <span class="subs-meta">{{ metaOf(item) }}</span>
      </span>
      <span class="subs-stats">
        <span class="subs-tok">{{ tokOf(item) }}</span>
        <span class="subs-dur">{{ durOf(item) }}</span>
      </span>
      <icon-right :size="14" class="subs-chevron" />
    </div>
  </div>
</template>

<style scoped>
/* 行度量与计划子项一致：右缘对齐、悬停同底色，两节读起来是一份列表 */
.subs-list {
  display: flex;
  flex-direction: column;
  max-height: min(264px, 45vh);
  overflow-y: auto;
  padding: 2px 0 4px;
}

.subs-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 14px;
  cursor: pointer;
  transition: background-color 0.15s;
}

.subs-item:hover {
  background: var(--bg-hover);
}

/* 状态位：与计划子项状态图标同宽，标题列对齐 */
.subs-slot {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  flex-shrink: 0;
}

.subs-spin {
  color: var(--accent-text);
  animation: subs-spin 0.9s linear infinite;
}

@keyframes subs-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

.subs-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background-color: var(--text-tertiary);
}

/* 未开始：空心圆环 */
.subs-dot-pending {
  background: transparent;
  box-shadow: inset 0 0 0 1.5px var(--text-tertiary);
}

/* 空闲 / 上次被进程打断 / 已忽略：中性灰点 */
.subs-dot-idle,
.subs-dot-interrupted,
.subs-dot-ignored {
  background-color: var(--text-tertiary);
}

.subs-dot-done {
  background-color: var(--success);
}

/* 等你处理（待审批 / 待回答）：橙点 */
.subs-dot-form {
  background-color: var(--warning);
}

/* 异常（用户打断 / 执行失败）：红点 */
.subs-dot-cancelled,
.subs-dot-error {
  background-color: var(--danger);
}

/* 左列：标题 + 副行 meta（meta 不省略，可换行） */
.subs-main {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 1px;
}

.subs-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  font-weight: 500;
}

.subs-meta {
  color: var(--text-tertiary);
  font-size: var(--font-size-xs);
  line-height: 16px;
}

/* 右列：token（上）/ 耗时（下），等宽数字、右对齐 */
.subs-stats {
  display: flex;
  flex-shrink: 0;
  flex-direction: column;
  align-items: flex-end;
  gap: 1px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.subs-tok {
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}

.subs-dur {
  color: var(--text-tertiary);
  font-size: var(--font-size-xs);
}

.subs-chevron {
  flex-shrink: 0;
  color: var(--text-tertiary);
}
</style>
