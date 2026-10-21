<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import UserInputForm from './UserInputForm.vue'
import type { UserChatForm, UserFormField } from '@/types'

const props = defineProps<{ form: UserChatForm }>()
const emit = defineEmits<{ (e: 'submit', data: string): void; (e: 'ignore'): void }>()

const { t } = useI18n()
const formRef = ref<InstanceType<typeof UserInputForm> | null>(null)
const canSubmit = computed(() => formRef.value?.getValid() ?? false)
const canPrev = computed(() => formRef.value?.canPrev() ?? false)
const canNext = computed(() => formRef.value?.canNext() ?? false)
const stepLabel = computed(() => {
  const total = formRef.value?.totalSteps() ?? 0
  if (total <= 1) return ''
  return t('userForm.stepLabel', { step: formRef.value?.step() ?? 1, total })
})

/** 当前页字段:一页一字段,字段的 description 就是这张卡的标题(见下) */
const currentField = computed<UserFormField | null>(() => formRef.value?.currentField() ?? null)
/**
 * 卡片标题 = 当前字段的问题 +（必填/选填）。
 * 不再展示 tip/整表提示语,也不展示字段名(name 只作回传键);
 * 字段没给 description 时回退通用文案(userForm.question)。
 */
const formTitle = computed(() => {
  const f = currentField.value
  if (!f) return ''
  const question = f.description?.trim() || t('userForm.question')
  return question + (f.required ? t('userForm.requiredMark') : t('userForm.optionalMark'))
})

function onSubmit(): void {
  const data = formRef.value?.collect()
  if (!data) return
  emit('submit', JSON.stringify(data))
}
</script>

<template>
  <!-- chat_with_user 待答表单:占据发送框位置(替代输入框),macOS 风格与审批卡同一套配方 -->
  <div class="chat-form-bar">
    <!-- 标题:每个字段的问题(一页一字段,标题随页码切换)+ 必填/选填 -->
    <div class="chat-form-q">
      <span class="chat-form-icon"><icon-message :size="12" /></span>
      <span class="chat-form-qtext">{{ formTitle }}</span>
    </div>

    <UserInputForm ref="formRef" :form="form" />

    <!-- 每字段一页:上一页/下一页(小按钮)+ 步数;其余右侧 忽略/提交 -->
    <div class="chat-form-actions">
      <button type="button" class="chat-form-nav" :disabled="!canPrev" @click="formRef?.prev()">
        {{ t('userForm.previousStep') }}
      </button>
      <button type="button" class="chat-form-nav" :disabled="!canNext" @click="formRef?.next()">
        {{ t('userForm.nextStep') }}
      </button>
      <span v-if="stepLabel" class="chat-form-step">{{ stepLabel }}</span>
      <span class="flex-1" />
      <button type="button" class="chat-form-btn ghost" @click="emit('ignore')">
        {{ t('userForm.ignore') }}
      </button>
      <button type="button" class="chat-form-btn primary" :disabled="!canSubmit" @click="onSubmit">
        {{ t('userForm.submit') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
/*
 * macOS 风格:与审批卡(ApprovalBar)同一套配方 —— 半透明材质 + 大圆角 + 柔和投影,
 * 分区用发丝线(0.5px),按钮为 macOS 形状(28px 高 / 7px 圆角 / 中性灰次级 + 主色主按钮)。
 * 三者同处输入栈(审批卡 / 提问表单 / 发送框),圆角与观感保持统一。
 */
.chat-form-bar {
  box-sizing: border-box;
  width: 100%;
  display: flex;
  flex-direction: column;
  padding: 12px 14px 10px;
  /* 无外边框（按需求去掉；靠背景/投影区分层次） */
  border: none;
  border-radius: 16px;
  background-color: color-mix(in srgb, var(--bg-2) 90%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  box-shadow:
    0 8px 28px rgba(0, 0, 0, 0.16),
    0 1px 2px rgba(0, 0, 0, 0.08);
  animation: form-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
  /* 让卡片内 UA 渲染的部分(光标、选区、选项列表滚动条等)跟随应用主题,
     否则暗色下仍是亮色原生外观;控件本体的配色仍由各自样式定义 */
  color-scheme: light;
}

[data-theme='dark'] .chat-form-bar {
  color-scheme: dark;
}

@keyframes form-in {
  from {
    opacity: 0;
    transform: translateY(5px) scale(0.99);
  }
}

@media (prefers-reduced-motion: reduce) {
  .chat-form-bar {
    animation: none;
  }
}

.chat-form-q {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  margin-bottom: 10px;
}

/* 圆角方块图标(SF Symbols 风格的着色块,与审批卡同款) */
.chat-form-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex: none;
  margin-top: 1px;
  border-radius: 6px;
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 16%, transparent);
}

.chat-form-qtext {
  font-size: var(--font-size-sm);
  font-weight: 600;
  letter-spacing: 0.01em;
  color: var(--text-primary);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

/* 按钮组:发丝线分隔,macOS 右对齐(忽略在左、默认按钮在右) */
.chat-form-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  padding-top: 10px;
  border-top: 0.5px solid var(--border-base);
}

/* 上一页 / 下一页:macOS 小号按钮(无边框填充,与次级按钮同一语言) */
.chat-form-nav {
  height: 22px;
  padding: 0 9px;
  font-family: inherit;
  font-size: var(--font-size-xs);
  font-weight: 500;
  line-height: 1;
  border: 0;
  border-radius: 6px;
  background-color: color-mix(in srgb, var(--bg-5) 14%, transparent);
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.chat-form-nav:hover:not(:disabled) {
  background-color: color-mix(in srgb, var(--bg-5) 26%, transparent);
  color: var(--text-primary);
}

.chat-form-nav:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.chat-form-step {
  font-size: var(--font-size-2xs);
  line-height: 1.4;
  color: var(--text-tertiary);
  margin: 0 2px;
  white-space: nowrap;
}

.chat-form-btn {
  min-width: 68px;
  height: 28px;
  padding: 0 14px;
  font-family: inherit;
  font-size: var(--font-size-sm);
  font-weight: 500;
  line-height: 1;
  border: 0;
  border-radius: 7px;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    filter var(--transition-fast),
    box-shadow var(--transition-fast);
}

/* 忽略:中性灰填充(macOS 非默认按钮) */
.chat-form-btn.ghost {
  color: var(--text-primary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
}

.chat-form-btn.ghost:hover:not(:disabled) {
  background-color: color-mix(in srgb, var(--bg-5) 32%, transparent);
}

/* 提交:主色实心 + 极轻投影(macOS 默认按钮) */
.chat-form-btn.primary {
  color: #fff;
  background-color: var(--accent-text);
  box-shadow: 0 1px 2.5px rgba(0, 0, 0, 0.18);
}

.chat-form-btn.primary:hover:not(:disabled) {
  filter: brightness(1.06);
}

.chat-form-btn:active:not(:disabled) {
  filter: brightness(0.94);
}

/* 禁用态:默认按钮回落为中性灰(macOS 语义:不可用的默认按钮不着主色) */
.chat-form-btn.primary:disabled {
  color: var(--text-tertiary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
  box-shadow: none;
  cursor: not-allowed;
}

.chat-form-btn.ghost:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* 键盘聚焦环(仅键盘可达时显示) */
.chat-form-btn:focus-visible,
.chat-form-nav:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--accent-text) 32%, transparent);
}
</style>
