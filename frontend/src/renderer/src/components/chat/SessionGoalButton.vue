<script setup lang="ts">
/**
 * 对话框工具行里的「会话目标」入口（计划右侧、上下文用量左侧）。
 *
 * - 只有当前会话有目标时显示（`sessionInfo.goal`，后端每次落库广播）；
 * - 悬浮/点击弹出目标卡片：目标全文 + 自动推进次数 / token 消耗；
 * - 卡片内支持**修改**（内联编辑）与**删除**，不必发消息，后端立即写活记录。
 */
import { computed, nextTick, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { useSessionsStore } from '@/stores/sessions'
import FooterIconButton from './FooterIconButton.vue'
import HoverCard from './HoverCard.vue'

const { t } = useI18n()
const route = useRoute()
const sessions = useSessionsStore()

const isChatView = computed(() => route.path.startsWith('/chat'))
const sid = computed(() => sessions.activeId)
const goal = computed(() => sessions.sessionInfo[sid.value]?.goal ?? null)
const visible = computed(() => isChatView.value && !!goal.value?.text?.trim())
/** 目标循环是否仍在推进：会话正在跑（含等你审批/填表）→ 进行中，否则视为已暂停 */
const goalStatusKey = computed(() => sessions.statusKeyOf(sid.value))
const goalLifecycle = computed(() => goal.value?.status ?? 'active')
const goalRunning = computed(
  () =>
    goalLifecycle.value === 'active' &&
    (goalStatusKey.value === 'running' || goalStatusKey.value === 'form')
)
const goalStatusLabel = computed(() => {
  if (goalLifecycle.value === 'completed') return t('chat.goalStatusCompleted')
  if (goalLifecycle.value === 'stopped') return t('chat.goalStatusStopped')
  return goalRunning.value ? t('chat.goalStatusActive') : t('chat.goalStatusPaused')
})
const goalIconClass = computed(() => {
  if (goalLifecycle.value === 'completed') return 'goal-icon-completed'
  if (goalLifecycle.value === 'stopped' || !goalRunning.value) return 'goal-icon-paused'
  return 'goal-icon-active'
})
const goalDotClass = computed(() => {
  if (goalLifecycle.value === 'completed') return 'is-completed'
  if (goalLifecycle.value === 'stopped') return 'is-stopped'
  return goalRunning.value ? 'is-active' : ''
})

const cardRef = ref<InstanceType<typeof HoverCard> | null>(null)
const editing = ref(false)
const draft = ref('')
const busy = ref(false)
const textareaRef = ref<HTMLTextAreaElement | null>(null)

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

function startEdit(): void {
  if (!goal.value || busy.value) return
  draft.value = goal.value.text ?? ''
  editing.value = true
  nextTick(() => textareaRef.value?.focus())
}

function cancelEdit(): void {
  editing.value = false
  draft.value = ''
}

async function save(): Promise<void> {
  const text = draft.value.trim()
  if (!text) {
    Message.warning(t('chat.goalNeedText'))
    return
  }
  busy.value = true
  try {
    await sessions.updateSessionGoal(sid.value, text)
    editing.value = false
    draft.value = ''
    Message.success(t('chat.goalUpdated'))
  } catch (err) {
    Message.error(t('chat.goalActionFailed', { reason: errorText(err) }))
  } finally {
    busy.value = false
  }
}

async function remove(): Promise<void> {
  if (busy.value) return
  busy.value = true
  try {
    await sessions.updateSessionGoal(sid.value, null)
    editing.value = false
    draft.value = ''
    Message.success(t('chat.goalDeleted'))
    cardRef.value?.close()
  } catch (err) {
    Message.error(t('chat.goalActionFailed', { reason: errorText(err) }))
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <HoverCard v-if="visible" ref="cardRef">
    <template #trigger="{ toggle }">
      <FooterIconButton
        :aria-label="t('chat.goal')"
        :data-goal="goal?.text ?? ''"
        @click="toggle"
      >
        <icon-goal :size="16" :class="goalIconClass" />
      </FooterIconButton>
    </template>

    <div class="goal-card">
      <template v-if="!editing">
        <div class="goal-body">
          <icon-goal :size="15" class="goal-head-icon" />
          <p class="goal-text">{{ goal?.text }}</p>
        </div>
        <div class="goal-footer">
          <div class="goal-meta">
            <span class="goal-state">
              <span class="goal-state-dot" :class="goalDotClass" />
              {{ goalStatusLabel }}
            </span>
            <span>{{ t('chat.goalAdvanced', { n: goal?.iterations ?? 0 }) }}</span>
          </div>
          <div class="goal-actions">
            <button type="button" class="goal-act" :disabled="busy" @click="startEdit">
              {{ t('chat.goalEdit') }}
            </button>
            <button
              type="button"
              class="goal-act goal-act-danger"
              :disabled="busy"
              @click="remove"
            >
              {{ t('chat.goalDelete') }}
            </button>
          </div>
        </div>
      </template>

      <template v-else>
        <textarea
          ref="textareaRef"
          v-model="draft"
          class="goal-edit"
          rows="3"
          :aria-label="t('chat.goal')"
          @keydown.meta.enter.prevent="save"
          @keydown.ctrl.enter.prevent="save"
          @keydown.esc.prevent="cancelEdit"
        />
        <div class="goal-footer goal-footer--edit">
          <div class="goal-actions">
            <button
              type="button"
              class="goal-act goal-act-primary"
              :disabled="busy || !draft.trim()"
              @click="save"
            >
              {{ t('chat.goalSave') }}
            </button>
            <button type="button" class="goal-act" :disabled="busy" @click="cancelEdit">
              {{ t('chat.goalCancel') }}
            </button>
          </div>
        </div>
      </template>
    </div>
  </HoverCard>
</template>

<style scoped>
.goal-body {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  padding: 2px 14px 0;
}

.goal-head-icon {
  flex-shrink: 0;
  margin-top: 2px;
  color: var(--warning);
}

.goal-text {
  flex: 1;
  min-width: 0;
  max-height: 30vh;
  margin: 0;
  overflow-y: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-size: var(--font-size-sm);
  line-height: 1.6;
  color: var(--text-primary);
}

.goal-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 14px 4px;
}

.goal-footer--edit {
  justify-content: flex-end;
  padding-top: 10px;
}

.goal-icon-active {
  color: var(--warning);
}

.goal-icon-completed {
  color: var(--success);
}

.goal-icon-paused {
  color: var(--text-tertiary);
}

.goal-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

.goal-state {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.goal-state-dot {
  width: 6px;
  height: 6px;
  flex: none;
  border-radius: 50%;
  background: var(--text-tertiary);
}

.goal-state-dot.is-active {
  background: var(--accent-text);
}

.goal-state-dot.is-completed {
  background: var(--success);
}

.goal-state-dot.is-stopped {
  background: var(--text-tertiary);
}

.goal-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: none;
  margin-left: auto;
}

.goal-act {
  height: 26px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font: inherit;
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    border-color 0.15s ease,
    color 0.15s ease;
}

.goal-act:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.goal-act:disabled {
  opacity: 0.5;
  cursor: default;
}

.goal-act-danger {
  color: var(--danger);
  border-color: color-mix(in srgb, var(--danger) 35%, var(--border-base));
}

.goal-act-danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--danger) 10%, transparent);
  color: var(--danger);
}

.goal-act-primary {
  color: var(--accent-text);
  border-color: color-mix(in srgb, var(--accent-text) 35%, var(--border-base));
}

.goal-edit {
  box-sizing: border-box;
  width: calc(100% - 28px);
  min-height: 76px;
  max-height: 30vh;
  margin: 0 14px;
  padding: 8px 10px;
  resize: vertical;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-sm);
  outline: none;
  background: var(--bg-1);
  color: var(--text-primary);
  font: inherit;
  font-size: var(--font-size-sm);
  line-height: 1.55;
}

.goal-edit:focus {
  border-color: var(--accent-text);
}
</style>
