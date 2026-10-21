<script setup lang="ts">
/**
 * 写邮件确认卡（就地内嵌在对话输入区，替代输入框）。
 *
 * 模型调用 `mail send` 时，后端返回 `mail_compose` 中断信封；本组件展示模型草稿的
 * **只读预览**（收件人 / 主题 / 附件 / 正文 HTML 渲染），用户点「发送」或「取消」。
 * 发送/取消即 `submitInterrupt`（answer=草稿 / ignored）。发送由后端在决议处理时执行，
 * 前端不直接发信（保证 tool 结果与审计统一）。
 *
 * 保持现有确认卡结构；表面、文字和控件颜色跟随主题令牌。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { sanitizeHtml } from '@/utils/sanitizeHtml'
import { getFileIconUrl } from '@/utils/fileIcons'
import { useTabsStore } from '@/stores/tabs'

/** 附件项（路径 / 文件名 / 大小） */
export interface MailComposeAttachment {
  path: string
  name: string
  size: number
}

/** 模型草稿（来自中断信封 `mail_compose`） */
export interface MailComposeDraft {
  to: string
  cc: string
  bcc: string
  subject: string
  body: string
  html: string
  attachments: string[]
  attachment_meta?: MailComposeAttachment[]
}

const props = defineProps<{
  draft: MailComposeDraft
  /** 发送中（决议已提交、等待后端落定） */
  sending?: boolean
}>()
const emit = defineEmits<{ (e: 'send'): void; (e: 'cancel'): void }>()

const { t } = useI18n()
const tabs = useTabsStore()

/** 点击附件：在文件预览页签中打开本地文件（附件是模型引用的本地路径） */
function openAttachment(a: MailComposeAttachment): void {
  if (!a.path) return
  tabs.openFileTab({ path: a.path, name: a.name })
}

/** 正文：优先 HTML 渲染（sanitize 后），否则纯文本按换行展示 */
const bodyHtml = computed(() => {
  const d = props.draft
  if (d.html && d.html.trim()) return sanitizeHtml(d.html)
  return ''
})
const bodyText = computed(() => (!bodyHtml.value ? (props.draft.body ?? '') : ''))

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}

/**
 * 附件展示列表：优先用后端给的 `attachment_meta`（文件名 + 大小）；
 * 缺失（历史记录）时回退到 `attachments` 路径，按路径取文件名、大小未知。
 */
const attachments = computed<MailComposeAttachment[]>(() => {
  const meta = props.draft.attachment_meta
  if (meta?.length) return meta
  return (props.draft.attachments ?? []).map((p) => ({ path: p, name: fileName(p), size: 0 }))
})

/** 人类可读体积（B / KB / MB） */
function humanSize(bytes: number): string {
  if (!bytes || bytes <= 0) return ''
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

const hasRecipients = computed(() => Boolean(props.draft.to?.trim()))
</script>

<template>
  <!-- 写邮件确认：占据输入框位置，只读预览模型草稿 + 发送/取消 -->
  <div class="mail-compose-bar">
    <div class="mcb-head">
      <span class="mcb-icon"><icon-mail :size="13" /></span>
      <span class="mcb-title">{{ t('chat.mailComposeTitle') }}</span>
    </div>

    <!-- 收件信息 -->
    <div class="mcb-fields">
      <div class="mcb-field">
        <span class="mcb-label">{{ t('mail.to') }}</span>
        <span class="mcb-value" :class="{ empty: !hasRecipients }">
          {{ props.draft.to || t('mail.noRecipient') }}
        </span>
      </div>
      <div v-if="props.draft.cc" class="mcb-field">
        <span class="mcb-label">{{ t('mail.cc') }}</span>
        <span class="mcb-value">{{ props.draft.cc }}</span>
      </div>
      <div v-if="props.draft.bcc" class="mcb-field">
        <span class="mcb-label">{{ t('mail.bcc') }}</span>
        <span class="mcb-value">{{ props.draft.bcc }}</span>
      </div>
      <div class="mcb-field">
        <span class="mcb-label">{{ t('mail.subject') }}</span>
        <span class="mcb-value" :class="{ empty: !props.draft.subject }">
          {{ props.draft.subject || t('mail.noSubject') }}
        </span>
      </div>
    </div>

    <!-- 附件：复用文件树的文件图标 + 体积 -->
    <div v-if="attachments.length" class="mcb-attach">
      <icon-link :size="12" class="mcb-attach-icon" />
      <button
        v-for="(a, i) in attachments"
        :key="i"
        type="button"
        class="mcb-chip"
        :title="a.path"
        @click="openAttachment(a)"
      >
        <img class="mcb-chip-glyph" :src="getFileIconUrl(a.name)" width="15" height="15" alt="" />
        <span class="mcb-chip-name">{{ a.name }}</span>
        <span v-if="humanSize(a.size)" class="mcb-chip-size">{{ humanSize(a.size) }}</span>
      </button>
    </div>

    <!-- 正文预览（只读） -->
    <div class="mcb-body">
      <div v-if="bodyHtml" class="mcb-body-html" v-html="bodyHtml" />
      <pre v-else-if="bodyText" class="mcb-body-text">{{ bodyText }}</pre>
      <span v-else class="mcb-body-empty">{{ t('mail.emptyBody') }}</span>
    </div>

    <div class="mcb-actions">
      <span class="flex-1" />
      <button type="button" class="mcb-btn ghost" :disabled="sending" @click="emit('cancel')">
        {{ t('mail.cancel') }}
      </button>
      <button
        type="button"
        class="mcb-btn primary"
        :disabled="sending || !hasRecipients"
        @click="emit('send')"
      >
        {{ t('mail.send') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
/* 结构冻结：邮件内容沿用对话区底色，外层确认卡单独承托。 */
.mail-compose-bar {
  box-sizing: border-box;
  width: 100%;
  display: flex;
  flex-direction: column;
  padding: 10px 12px 8px;
  border: none;
  border-radius: 16px;
  background-color: var(--bg-2);
  animation: mcb-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
  color-scheme: light;
  max-height: 60vh;
}
[data-theme='dark'] .mail-compose-bar {
  color-scheme: dark;
}
@keyframes mcb-in {
  from {
    opacity: 0;
    transform: translateY(5px) scale(0.99);
  }
}
@media (prefers-reduced-motion: reduce) {
  .mail-compose-bar {
    animation: none;
  }
}

.mcb-head {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 8px;
}
.mcb-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex: none;
  border-radius: 6px;
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 16%, transparent);
}
.mcb-title {
  font-size: var(--font-size-title);
  font-weight: var(--font-weight-semibold);
  letter-spacing: 0.01em;
  color: var(--text-primary);
}

.mcb-fields {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.mcb-field {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: var(--font-size-md);
  line-height: var(--line-height-ui);
  min-width: 0;
}
.mcb-label {
  flex: 0 0 44px;
  color: var(--text-secondary);
}
.mcb-value {
  flex: 0 1 auto;
  width: fit-content;
  max-width: calc(100% - 52px);
  min-width: 0;
  padding: 6px 8px;
  border-radius: var(--radius-md);
  background: var(--bg-base);
  color: var(--text-primary);
  word-break: break-word;
}
.mcb-value.empty {
  color: var(--text-tertiary);
}

.mcb-attach {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}
.mcb-attach-icon {
  color: var(--text-tertiary);
}
.mcb-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 260px;
  padding: 3px 9px;
  border: none;
  border-radius: var(--radius-sm);
  background: var(--bg-base);
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition: background-color var(--transition-fast);
}
.mcb-chip:hover {
  background: var(--bg-4);
}
.mcb-chip:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 2px;
}
.mcb-chip-glyph {
  flex: none;
  display: block;
}
.mcb-chip-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mcb-chip-size {
  flex: none;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}

/* 正文预览：只读，滚动，最高占半屏 */
.mcb-body {
  margin-top: 8px;
  padding: 8px 10px;
  border-radius: 10px;
  background: var(--bg-base);
  overflow: auto;
  font-size: var(--font-size-md);
  line-height: var(--line-height-body);
  color: var(--text-primary);
}
.mcb-body-html :deep(img) {
  max-width: 100%;
  height: auto;
}
.mcb-body-html :deep(a) {
  color: var(--accent-text);
}
.mcb-body-html :deep(blockquote) {
  margin: 6px 0;
  padding-left: 10px;
  border-left: 3px solid var(--border-strong);
  color: var(--text-secondary);
}
.mcb-body-html :deep(pre) {
  padding: 8px 10px;
  border-radius: 6px;
  background: var(--bg-base);
  white-space: pre-wrap;
}
.mcb-body-text {
  margin: 0;
  font-family: inherit;
  white-space: pre-wrap;
  word-break: break-word;
}
.mcb-body-empty {
  color: var(--text-tertiary);
}

.mcb-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
  padding-top: 4px;
}
.mcb-btn {
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
    filter var(--transition-fast);
}
.mcb-btn.ghost {
  color: var(--text-primary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
}
.mcb-btn.ghost:hover:not(:disabled) {
  background-color: color-mix(in srgb, var(--bg-5) 32%, transparent);
}
.mcb-btn.primary {
  color: #fff;
  background-color: var(--accent-text);
}
.mcb-btn.primary:hover:not(:disabled) {
  filter: brightness(1.06);
}
.mcb-btn:active:not(:disabled) {
  filter: brightness(0.94);
}
.mcb-btn.primary:disabled {
  color: var(--text-tertiary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
  cursor: not-allowed;
}
.mcb-btn.ghost:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.mcb-btn:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 2px;
}
</style>
