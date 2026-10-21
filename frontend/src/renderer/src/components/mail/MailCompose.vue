<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import RichTextEditor from '@/components/mail/RichTextEditor.vue'
import { useMailStore } from '@/stores/mail'

const props = defineProps<{
  open: boolean
  initial?: { to?: string; cc?: string; bcc?: string; subject?: string; body?: string }
}>()
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'sent'): void
}>()

const { t } = useI18n()
const mail = useMailStore()

const form = reactive({ to: '', cc: '', bcc: '', subject: '' })
/** 正文富文本 HTML（v-model:html 双向绑定） */
const bodyHtml = ref('')
const attachments = ref<string[]>([])
const sending = ref(false)
const showCc = ref(false)

/** 纯文本正文 → HTML：转义 + 换行转 <br>（回复/转发预填的是纯文本） */
function textToHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\n/g, '<br>')
}

/** HTML → 纯文本回退（剥离标签，块级元素转换行），随 HTML 一起发给后端 */
function htmlToText(html: string): string {
  const el = document.createElement('div')
  el.innerHTML = html
  el.querySelectorAll('br').forEach((b) => b.replaceWith('\n'))
  el.querySelectorAll('p,div,li,blockquote,pre,h1,h2,h3,h4').forEach((n) =>
    n.append(document.createTextNode('\n'))
  )
  return (el.textContent ?? '').replace(/\n{3,}/g, '\n\n').trim()
}

watch(
  () => props.open,
  (open) => {
    if (!open) return
    form.to = props.initial?.to ?? ''
    form.cc = props.initial?.cc ?? ''
    form.bcc = props.initial?.bcc ?? ''
    form.subject = props.initial?.subject ?? ''
    bodyHtml.value = props.initial?.body ? textToHtml(props.initial.body) : ''
    attachments.value = []
    showCc.value = Boolean(form.cc || form.bcc)
    sending.value = false
    // 打开后聚焦收件人（省一次点击）
    nextTick(() => {
      const el = document.querySelector<HTMLInputElement>('.mail-compose .field-to input')
      el?.focus()
    })
  }
)

const hasContent = computed(
  () =>
    Boolean(
      form.to.trim() || form.subject.trim() || htmlToText(bodyHtml.value) || attachments.value.length
    )
)

function fileName(path: string | { path?: string } | null | undefined): string {
  const p = typeof path === 'string' ? path : (path?.path ?? '')
  return p.split(/[\\/]/).pop() ?? p
}

async function pickAttachments(): Promise<void> {
  const paths = await window.api.dialog.openAttachments()
  const valid = (paths ?? []).filter(Boolean)
  if (valid.length) attachments.value = [...new Set([...attachments.value, ...valid])]
}

function removeAttachment(i: number): void {
  attachments.value.splice(i, 1)
}

function close(): void {
  emit('update:open', false)
}

async function onSend(): Promise<void> {
  const split = (s: string): string[] =>
    s.split(',').map((x) => x.trim()).filter(Boolean)
  const to = split(form.to)
  if (!to.length) {
    Message.warning(t('mail.needRecipient'))
    return
  }
  sending.value = true
  try {
    await mail.send({
      to,
      cc: split(form.cc),
      bcc: split(form.bcc),
      subject: form.subject,
      body: htmlToText(bodyHtml.value),
      html: bodyHtml.value || undefined,
      attachments: attachments.value
    })
    Message.success(t('mail.sentOk'))
    emit('sent')
    close()
  } catch (e) {
    Message.error((e as Error).message)
  } finally {
    sending.value = false
  }
}
</script>

<template>
  <a-modal
    :visible="open"
    :width="840"
    :top="'8vh'"
    modal-class="mail-compose"
    unmount-on-close
    :footer="false"
    :title="t('mail.compose')"
    @cancel="close"
  >
    <div class="compose">
      <!-- 字段区：发丝线分隔的无边框行（写信的默认姿态） -->
      <div class="fields">
        <div class="field field-to">
          <label>{{ t('mail.to') }}</label>
          <input v-model="form.to" class="ipt" :placeholder="t('mail.phTo')" />
          <button type="button" class="cc-toggle" @click="showCc = !showCc">
            {{ showCc ? t('mail.hideCc') : t('mail.showCc') }}
          </button>
        </div>
        <div class="cc-fields" :class="{ open: showCc }">
          <div class="cc-fields-inner">
            <div class="field">
              <label>{{ t('mail.cc') }}</label>
              <input v-model="form.cc" class="ipt" :placeholder="t('mail.optional')" />
            </div>
            <div class="field">
              <label>{{ t('mail.bcc') }}</label>
              <input v-model="form.bcc" class="ipt" :placeholder="t('mail.optional')" />
            </div>
          </div>
        </div>
        <div class="field">
          <label>{{ t('mail.subject') }}</label>
          <input v-model="form.subject" class="ipt" />
        </div>

        <!-- 附件行：有附件时行内列出芯片，否则只留一颗添加入口 -->
        <div class="field field-attach">
          <label>{{ t('mail.attachments') }}</label>
          <div class="attach-zone">
            <button type="button" class="attach-btn" @click="pickAttachments">
              <icon-link :size="14" />
              <span>{{ t('mail.addAttachment') }}</span>
            </button>
            <TransitionGroup name="chip" tag="div" class="chips">
              <span v-for="(a, i) in attachments" :key="a" class="chip">
                <icon-file :size="13" class="chip-icon" />
                <span class="chip-name">{{ fileName(a) }}</span>
                <button type="button" class="chip-x" @click="removeAttachment(i)">
                  <icon-close :size="11" />
                </button>
              </span>
            </TransitionGroup>
          </div>
        </div>
      </div>

      <!-- 正文：富文本工具条 + 书写区 -->
      <RichTextEditor
        v-model:html="bodyHtml"
        class="body-rte"
        :placeholder="t('mail.body')"
        :min-height="200"
      />

      <div class="compose-foot">
        <span v-if="hasContent" class="foot-hint">{{ t('mail.composeHint') }}</span>
        <span class="spacer" />
        <button type="button" class="btn btn-ghost" @click="close">{{ t('mail.cancel') }}</button>
        <button type="button" class="btn btn-primary" :disabled="sending" @click="onSend">
          <icon-loading v-if="sending" :size="14" class="spin" />
          <span>{{ t('mail.send') }}</span>
        </button>
      </div>
    </div>
  </a-modal>
</template>

<style scoped>
.compose {
  display: flex;
  flex-direction: column;
  min-height: 460px;
}

/* ---------- 字段区 ---------- */
.fields {
  display: flex;
  flex-direction: column;
}
.field {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 44px;
  padding: 6px 2px;
  border-bottom: 1px solid var(--border-light);
  transition: border-color var(--transition-fast);
}
/* label：小字号、次级色、固定列宽 → 形成稳定对齐的文字栏 */
.field label {
  flex: 0 0 56px;
  font-size: var(--font-size-code);
  letter-spacing: 0.01em;
  color: var(--text-tertiary);
  user-select: none;
}
.ipt {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-md);
  font-family: inherit;
  letter-spacing: 0.005em;
}
.ipt::placeholder {
  color: var(--text-disabled);
}
/* 聚焦整行：发丝线亮成强调色，给出「正在编辑这一行」的即时反馈 */
.field:focus-within {
  border-bottom-color: var(--accent-text);
}
.field-to label {
  color: var(--text-secondary);
}
.cc-toggle {
  flex: none;
  border: none;
  background: transparent;
  color: var(--accent-text);
  cursor: pointer;
  font-size: var(--font-size-xs);
  padding: 4px 6px;
  border-radius: 6px;
  transition: background-color var(--transition-fast);
}
.cc-toggle:hover {
  background: var(--bg-hover);
}

/* Cc/Bcc：高度动画展开（空间一致：从 To 行下方生长出来） */
.cc-fields {
  display: grid;
  grid-template-rows: 0fr;
  transition: grid-template-rows 0.28s cubic-bezier(0.32, 0.72, 0, 1);
}
.cc-fields.open {
  grid-template-rows: 1fr;
}
.cc-fields-inner {
  overflow: hidden;
  min-height: 0;
}

/* ---------- 附件 ---------- */
.field-attach {
  align-items: center;
}
.attach-zone {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  min-width: 0;
}
.attach-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 11px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-2);
  color: var(--text-secondary);
  cursor: pointer;
  font-size: var(--font-size-code);
  white-space: nowrap;
  transition:
    background-color var(--transition-fast),
    border-color var(--transition-fast),
    transform 0.1s ease;
}
.attach-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
/* 按下即时反馈（§响应：反馈发生在按下，不是松手） */
.attach-btn:active {
  transform: scale(0.97);
}
.chips {
  position: relative;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 220px;
  padding: 4px 6px 4px 9px;
  border-radius: 8px;
  background: var(--fill-2);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  transition: background-color var(--transition-fast);
}
.chip:hover {
  background: var(--fill-3);
}
.chip-icon {
  flex: none;
  color: var(--text-tertiary);
}
.chip-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chip-x {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex: none;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}
.chip-x:hover {
  background: var(--bg-active);
  color: var(--danger);
}

/* 芯片出现/移除：缩放 + 淡入（材质化，不是单纯透明度） */
.chip-enter-active,
.chip-leave-active {
  transition:
    transform 0.22s cubic-bezier(0.32, 0.72, 0, 1),
    opacity 0.22s ease;
}
.chip-enter-from,
.chip-leave-to {
  opacity: 0;
  transform: scale(0.9);
}
.chip-leave-active {
  position: absolute;
}

/* ---------- 正文（富文本） ---------- */
.body-rte {
  margin-top: 10px;
}

/* ---------- 底栏 ---------- */
.compose-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 12px;
  border-top: 1px solid var(--border-light);
}
.foot-hint {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.spacer {
  flex: 1;
}
/* 自绘按钮：与 Arco 观感一致但能精确控制按下反馈 */
.btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 32px;
  padding: 0 16px;
  border-radius: 8px;
  border: 1px solid transparent;
  font-size: var(--font-size-sm);
  font-family: inherit;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    border-color var(--transition-fast),
    transform 0.1s ease,
    opacity var(--transition-fast);
}
.btn:active {
  transform: scale(0.97);
}
.btn-ghost {
  background: var(--bg-2);
  color: var(--text-primary);
}
.btn-ghost:hover {
  background: var(--bg-hover);
}
.btn-primary {
  background: var(--accent-text);
  color: #fff;
  min-width: 84px;
}
.btn-primary:hover {
  filter: brightness(1.06);
}
.btn-primary:disabled {
  opacity: 0.6;
  cursor: default;
  transform: none;
}
.spin {
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

/* 无障碍：削弱动效偏好下，改用短交叉淡入 */
@media (prefers-reduced-motion: reduce) {
  .cc-fields {
    transition: none;
  }
  .chip-enter-active,
  .chip-leave-active {
    transition: opacity 0.15s ease;
  }
  .chip-enter-from,
  .chip-leave-to {
    transform: none;
  }
  .btn,
  .attach-btn {
    transition: background-color var(--transition-fast);
  }
}
</style>

<style>
/* ===== macOS 风格弹窗骨架（Arco Modal 传送至 body，需非 scoped；mail-compose 前缀防冲突） =====
   与 .fm-modal / .agent-modal 同一套语言：颜色全走主题变量，明暗主题与强调色自动适配 */
.mail-compose.arco-modal,
.mail-compose .arco-modal {
  border-radius: 14px;
  overflow: hidden;
  background: var(--panel-bg);
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.12);
}
.mail-compose .arco-modal-header {
  padding: 15px 18px 11px;
  background: transparent;
  border-bottom: none;
}
.mail-compose .arco-modal-title {
  font-size: var(--font-size-title);
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--text-primary);
  text-align: left;
}
.mail-compose .arco-modal-close-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 7px;
  color: var(--text-tertiary);
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}
.mail-compose .arco-modal-close-btn:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}
.mail-compose .arco-modal-body {
  padding: 0 18px 16px;
  background: transparent;
}
</style>
