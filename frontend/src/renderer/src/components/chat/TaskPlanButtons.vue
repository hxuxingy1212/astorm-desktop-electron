<script setup lang="ts">
/**
 * 对话框工具行里的「计划」「子会话」两个入口：
 * 用**图标按钮 + 悬浮卡片**替代原先悬浮在消息区上方的规划胶囊。
 *
 * - 计划图标：当前会话有规划大纲文件时显示，悬浮/点击在其上方弹出计划卡片；
 * - 子会话图标：当前会话派生了子智能体（含本轮尚未开始的委派）时显示，弹出子会话卡片。
 * 两者都遵循「当前会话有才显示，无则不显示」。
 *
 * 数据与刷新逻辑同原规划胶囊（从 ChatPanel 迁入）：计划经 napi 读取、
 * 子会话由会话集合 + 本轮 sub_session 调用派生。
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { useSessionsStore } from '@/stores/sessions'
import { invoke } from '@/services/backend'
import { mergeChildItems, type ChildSessionItem } from './childSessions'
import ChildSessionsList from './ChildSessionsList.vue'
import FooterIconButton from './FooterIconButton.vue'
import HoverCard from './HoverCard.vue'
import type { ToolCallView } from '@/types'

/** 规划步骤(与后端 taskplan 大纲模型对齐) */
interface PlanStep {
  taskId: string
  taskName: string
  taskDescription?: string
  status: string
  createdAt?: string
  updatedAt?: string
}
interface PlanFile {
  taskId: string
  title: string
  description?: string
  createdAt?: string
  updatedAt?: string
  nodes: PlanStep[]
}

const { t } = useI18n()
const route = useRoute()
const sessions = useSessionsStore()

/** 子会话卡片引用：点击条目跳转后主动收起卡片 */
const childCardRef = ref<{ close: () => void } | null>(null)

/** 仅主聊天视图显示（新建会话页 / 其它路由不显示） */
const isChatView = computed(() => route.path.startsWith('/chat'))
const sid = computed(() => sessions.activeId)
const activeSession = computed(() => sessions.sessions.find((s) => s.id === sid.value))
const messages = computed(() => sessions.messages[sid.value] ?? [])
const isStreaming = computed(() => sessions.streamingIds.includes(sid.value))

// ---------- 子会话条目 ----------
/**
 * 子智能体条目：已创建子会话 + 本轮尚未开始的委派（串行执行，未轮到的没有会话记录）。
 * 「未开始」只在父轮运行中派生，避免历史里失败/未配对调用留下幽灵条目。
 */
const activeChildItems = computed<ChildSessionItem[]>(() => {
  if (!isChatView.value) return []
  const backendId = activeSession.value?.backendId
  const children = sessions.childSessionsOf(backendId)
  if (!backendId || !isStreaming.value) return mergeChildItems(children, [])
  const calls = collectSubSessionCalls().filter((c) => c.status !== 'error')
  const pending = calls.slice(children.length).map((c, k) => ({
    toolCallId: c.id,
    parentBackendId: backendId,
    callIndex: children.length + k,
    task: c.task
  }))
  return mergeChildItems(children, pending)
})

/** 消息里按时间顺序的 sub_session 工具调用（id / 任务文本 / 状态） */
function collectSubSessionCalls(): Array<{ id: string; task: string; status: ToolCallView['status'] }> {
  const out: Array<{ id: string; task: string; status: ToolCallView['status'] }> = []
  for (const m of messages.value) {
    for (const tool of m.tools ?? []) {
      if (tool.name !== 'sub_session') continue
      const task = (tool.params as { task?: unknown } | null | undefined)?.task
      out.push({ id: tool.id, task: typeof task === 'string' ? task : '', status: tool.status })
    }
  }
  return out
}

// ---------- 计划 ----------
const activePlan = ref<PlanFile | null>(null)
let planTimer: ReturnType<typeof setTimeout> | null = null

const nodes = computed(() => activePlan.value?.nodes ?? [])
const completedCount = computed(() => nodes.value.filter((n) => n.status === 'completed').length)

async function loadPlan(): Promise<void> {
  if (!isChatView.value) return
  const backendId = sessions.sessions.find((s) => s.id === sid.value)?.backendId
  activePlan.value = null
  if (!backendId) return
  try {
    const plans = (await invoke('list_plan_files', { session_id: backendId })) as Array<{
      taskId: string
      createdAt?: string
    }> | null
    if (!plans?.length) return
    const latest = [...plans].sort((a, b) =>
      String(b.createdAt ?? '').localeCompare(String(a.createdAt ?? ''))
    )[0]
    activePlan.value = (await invoke('read_plan_file', {
      session_id: backendId,
      task_id: latest.taskId
    })) as PlanFile | null
  } catch {
    activePlan.value = null
  }
}

// 会话切换立即加载;后端 task_list 变更事件即时刷新;消息变化兜底(非流式防抖)
watch(sid, () => void loadPlan(), { immediate: true })
watch(
  () => sessions.planVersion[sid.value],
  () => void loadPlan()
)
watch(messages, () => {
  if (isStreaming.value) return
  if (planTimer) clearTimeout(planTimer)
  planTimer = setTimeout(() => void loadPlan(), 800)
})
</script>

<template>
  <template v-if="isChatView">
    <!-- 计划：当前会话有规划大纲才显示 -->
    <HoverCard v-if="activePlan">
      <template #trigger="{ toggle }">
        <FooterIconButton
          :title="t('chat.planSection')"
          :label="`${completedCount}/${nodes.length}`"
          @click="toggle"
        >
          <icon-plan :size="16" />
        </FooterIconButton>
      </template>
      <div class="card-head">
        <span class="card-count">{{ completedCount }}/{{ nodes.length }}</span>
        <span class="card-title">{{ activePlan.title }}</span>
      </div>
      <div class="plan-list">
        <div
          v-for="(node, i) in nodes"
          :key="node.taskId || i"
          class="plan-node"
          :class="`status-${node.status}`"
        >
          <span class="plan-node-icon">
            <span v-if="node.status === 'pending'" class="dot" />
            <icon-loading v-else-if="node.status === 'in_progress'" :size="14" class="spin" />
            <icon-check v-else-if="node.status === 'completed'" :size="14" />
            <icon-close v-else-if="node.status === 'failed'" :size="14" />
          </span>
          <span class="plan-node-name">{{ node.taskName }}</span>
        </div>
      </div>
    </HoverCard>

    <!-- 子会话：当前会话派生了子智能体才显示 -->
    <HoverCard v-if="activeChildItems.length" ref="childCardRef">
      <template #trigger="{ toggle }">
        <FooterIconButton
          :title="t('chat.childSessions')"
          :label="String(activeChildItems.length)"
          @click="toggle"
        >
          <icon-user-group :size="16" />
        </FooterIconButton>
      </template>
      <div class="card-head">
        <span class="card-count">{{ activeChildItems.length }}</span>
        <span class="card-title">{{ t('chat.childSessions') }}</span>
      </div>
      <ChildSessionsList :items="activeChildItems" @select="childCardRef?.close()" />
    </HoverCard>
  </template>
</template>

<style scoped>
/* 卡片标题行：与 SessionInfoCard 的用量标题同款 */
.card-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 14px 8px;
}
.card-count {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: var(--accent-text);
  flex-shrink: 0;
}
.card-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

/* 计划步骤列表 */
.plan-list {
  display: flex;
  flex-direction: column;
  max-height: min(280px, 45vh);
  overflow-y: auto;
  padding: 2px 0 4px;
}
.plan-node {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  font-size: var(--font-size-sm);
  transition: background-color 0.15s;
}
.plan-node:hover {
  background: var(--bg-hover);
}
.plan-node-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  flex-shrink: 0;
}
.plan-node-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  line-height: 1.4;
}
.status-completed .plan-node-name {
  color: var(--text-secondary);
}
.plan-node-icon .dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-tertiary);
}
.status-in_progress .plan-node-icon {
  color: var(--info);
}
.status-completed .plan-node-icon {
  color: var(--success);
}
.status-failed .plan-node-icon {
  color: var(--danger);
}
.spin {
  animation: plan-spin 1s linear infinite;
}
@keyframes plan-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
</style>
