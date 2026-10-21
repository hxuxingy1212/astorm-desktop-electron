<script setup lang="ts">
/**
 * 「继续填写表单」入口：该轮里有一次被放弃的提问（忽略 / 被新消息取代 / 被中断）时，
 * 在收纳块下方单独占一行（不受「已处理 Xs」折叠影响），点一下把**原表单**重新弹回输入框位置。
 *
 * 为什么需要它：用户点「忽略」后，那条待决中断已从后端台账取走（旧 request_id 失效），
 * 但表单定义仍保留在 tool 消息的 `metadata.interrupt.request.form` 里——后端 `reopenInterrupt`
 * 据此换新 request_id 重新登记，回答仍写回**同一条** tool 结果（与首次回答同一条链路）。
 *
 * 视觉：与审批卡 / 提问表单卡同一套 macOS 配方（半透明材质 + 16px 圆角 + 发丝线 + 柔和投影），
 * 但压成单行（图标 + 说明 + 右侧中性灰按钮），避免和输入框上方的真表单卡抢视觉。
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import type { ToolCallView } from '@/types'

const props = defineProps<{
  /** 被放弃的那次提问（工具卡；表单定义在 tool.interrupt.request.form 里） */
  tool: ToolCallView | null
  /** 该行所属会话（子会话页签传各自会话 id，否则决议会落到主会话） */
  sessionId: string
}>()

const { t } = useI18n()
const sessions = useSessionsStore()

/** 请求在途：禁用按钮，避免连点弹出多张表单卡 */
const busy = ref(false)

const visible = computed(() => !!props.tool && props.sessionId !== '')

async function onResume(): Promise<void> {
  if (busy.value || !props.tool) return
  busy.value = true
  try {
    await sessions.reopenForm(props.tool.id, props.sessionId)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div v-if="visible" class="form-resume-bar" role="status">
    <span class="form-resume-icon"><icon-question-circle :size="12" /></span>
    <span class="form-resume-text">{{ t('chat.resumeFormHint') }}</span>
    <span class="flex-1" />
    <button type="button" class="form-resume-btn" :disabled="busy" @click="onResume">
      {{ t('chat.resumeForm') }}
    </button>
  </div>
</template>

<style scoped>
/* macOS 材质卡片（与 ChatFormBar / ApprovalBar 同一配方），压成单行 */
.form-resume-bar {
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 8px;
  /* 消息列本身有 8px 行间距，这里只补一点，让该行与上方收纳块略分开（行为上它属于该轮的收尾） */
  margin-top: 2px;
  padding: 8px 12px;
  border: 0.5px solid var(--border-base);
  border-radius: 12px;
  background-color: color-mix(in srgb, var(--bg-2) 80%, transparent);
  backdrop-filter: blur(16px) saturate(180%);
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.08);
  animation: form-resume-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}

@keyframes form-resume-in {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
}

@media (prefers-reduced-motion: reduce) {
  .form-resume-bar {
    animation: none;
  }
}

/* 圆角方块图标（SF Symbols 风格的着色块，与表单卡同款但小一号） */
.form-resume-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  flex: none;
  border-radius: 5px;
  color: var(--text-tertiary);
  background-color: color-mix(in srgb, var(--bg-5) 18%, transparent);
}

.form-resume-text {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.4;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 中性灰次级按钮（macOS 非默认按钮；与表单卡「忽略」同款形状与配色） */
.form-resume-btn {
  flex: none;
  height: 26px;
  min-width: 64px;
  padding: 0 12px;
  font-family: inherit;
  font-size: var(--font-size-code);
  font-weight: 500;
  line-height: 1;
  border: 0;
  border-radius: 7px;
  color: var(--text-primary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    filter var(--transition-fast);
}

.form-resume-btn:hover:not(:disabled) {
  background-color: color-mix(in srgb, var(--bg-5) 32%, transparent);
}

.form-resume-btn:active:not(:disabled) {
  filter: brightness(0.94);
}

.form-resume-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.form-resume-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--accent-text) 32%, transparent);
}
</style>
