<script setup lang="ts">
/**
 * 侧栏会话行的悬浮卡片（悬停 500ms 弹出）。
 *
 * 行本身仍由调用方通过默认插槽渲染（任务行 / 项目子会话行结构不同）；本组件只负责
 * 延迟、右侧定位与卡片内容：**标题（最多两行）+ 最近一条用户消息（最多四行）**。
 * 最近消息优先取 store 已加载的历史；没加载过时在卡片真正展开后按需拉取——
 * 鼠标快速扫过列表不会为每一行预取。
 * 不做行内操作按钮——侧栏的悬浮图标已按产品要求移除。
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { Session } from '@/types'
import { useSessionsStore } from '@/stores/sessions'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import HoverCard from '@/components/chat/HoverCard.vue'

const props = withDefaults(
  defineProps<{
    session?: Session | null
    /** 右键菜单打开等场景禁用悬浮卡 */
    disabled?: boolean
  }>(),
  { session: null, disabled: false }
)

const { t } = useI18n()
const sessions = useSessionsStore()

const title = computed(() => localizedSessionTitle(props.session) || t('common.unnamed'))

/** 卡片预览最多 2000 字：先规范空白再截断，避免超长用户消息整段进 DOM */
const PREVIEW_TEXT_LIMIT = 2000
function previewText(content: string | null | undefined): string {
  const text = (content ?? '').replace(/\s+/g, ' ').trim()
  return text.length > PREVIEW_TEXT_LIMIT ? `${text.slice(0, PREVIEW_TEXT_LIMIT)}…` : text
}

/** 从后往前找最后一条**有正文**的 user 消息 */
function lastUserTextOf(list: Array<{ role?: string; content?: string | null }>): string {
  for (let i = list.length - 1; i >= 0; i -= 1) {
    const msg = list[i]
    if (msg.role !== 'user') continue
    const text = previewText(msg.content)
    if (text) return text
  }
  return ''
}

const cardRef = ref<InstanceType<typeof HoverCard> | null>(null)
const isOpen = computed(() => cardRef.value?.isOpen ?? false)
/** 未加载过历史时按需拉取的最近用户消息（已加载的会话直接用 store，随流式实时更新） */
const fetchedLastUser = ref('')
let fetchToken = 0

const lastUserText = computed(() => {
  const id = props.session?.id
  if (!id) return ''
  return lastUserTextOf(sessions.messages[id] ?? []) || fetchedLastUser.value
})

async function fetchLastUser(): Promise<void> {
  const session = props.session
  if (!session?.backendId || !window.api?.chat?.listMessages) return
  if (lastUserTextOf(sessions.messages[session.id] ?? [])) return
  const token = (fetchToken += 1)
  try {
    const records = (await window.api.chat.listMessages(session.backendId)) as Array<{
      role?: string
      content?: string | null
    }>
    if (token !== fetchToken) return
    fetchedLastUser.value = lastUserTextOf(records)
  } catch {
    if (token === fetchToken) fetchedLastUser.value = ''
  }
}

watch(isOpen, (open) => {
  if (open) void fetchLastUser()
})
watch(
  () => props.session?.id,
  () => {
    fetchedLastUser.value = ''
    fetchToken += 1
    if (isOpen.value) void fetchLastUser()
  }
)

/** 点击行（切换会话）后收起卡片，避免它悬在新页面上 */
function closeCard(): void {
  cardRef.value?.close()
}
</script>

<template>
  <HoverCard
    ref="cardRef"
    :open-delay-ms="500"
    placement="right"
    block
    :disabled="disabled || !session"
  >
    <template #trigger>
      <div class="session-row-trigger" @click="closeCard">
        <slot />
      </div>
    </template>
    <div v-if="session" class="session-hover">
      <div class="session-hover-title">{{ title }}</div>
      <div v-if="lastUserText" class="session-hover-message">{{ lastUserText }}</div>
    </div>
  </HoverCard>
</template>

<style scoped>
.session-row-trigger {
  display: block;
  width: 100%;
  min-width: 0;
}

.session-hover {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 6px 12px;
}

.session-hover-title {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  font-size: var(--font-size-md);
  line-height: 1.45;
  font-weight: 500;
  color: var(--text-primary);
  word-break: break-word;
}

.session-hover-message {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 4;
  overflow: hidden;
  font-size: var(--font-size-sm);
  line-height: 1.55;
  color: var(--text-secondary);
  word-break: break-word;
}
</style>
