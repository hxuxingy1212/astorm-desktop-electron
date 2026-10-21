<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useMailStore } from '@/stores/mail'
import { sanitizeHtml } from '@/utils/sanitizeHtml'
import { getFileIconUrl } from '@/utils/fileIcons'
import MailAvatar from '@/components/mail/MailAvatar.vue'

const { t } = useI18n()
const mail = useMailStore()

const emit = defineEmits<{
  (e: 'compose', payload: { to: string; subject: string; body: string }): void
  (e: 'preview', payload: { name: string; path: string }): void
}>()

const detail = computed(() => mail.curDetail)
const curMessage = computed(() => mail.messages.find((x) => x.uid === detail.value?.uid))
const isFlagged = computed(() => curMessage.value?.flagged ?? false)
const isPinned = computed(() => curMessage.value?.pinned ?? false)
const curCategory = computed(() => curMessage.value?.category ?? '')
const isSeen = computed(() => curMessage.value?.seen ?? detail.value?.seen ?? true)

// 附件分页展示：默认 5 个，顶部「更多」每次再显示 5 个；切换邮件时重置
const ATT_PAGE = 5
const attVisible = ref(ATT_PAGE)
const visibleAttachments = computed(() => detail.value?.attachments.slice(0, attVisible.value) ?? [])
const hasMoreAtts = computed(() => (detail.value?.attachments.length ?? 0) > attVisible.value)
watch(
  () => detail.value?.uid,
  () => {
    attVisible.value = ATT_PAGE
  }
)

/** 发件人拆分：显示名 + 邮箱地址（用于头像与两行展示）。 */
const fromName = computed(() => {
  const f = detail.value?.from ?? ''
  const name = (f.match(/^(.*?)\s*<.*>$/)?.[1] ?? '').replace(/^"|"$/g, '').trim()
  if (name) return name
  const email = f.match(/<([^>]+)>/)?.[1] ?? f
  return email.split('@')[0] || t('mail.unknownSender')
})
const fromEmail = computed(() => {
  const f = detail.value?.from ?? ''
  return f.match(/<([^>]+)>/)?.[1] ?? (f.includes('@') ? f.trim() : '')
})

/**
 * 正文 HTML 按 uid 缓存：`sanitizeHtml` 会同步解析整段 DOM，大邮件很贵。
 * 会话内重复打开同一封（详情 LRU 命中）时直接复用，不再重新清洗。
 */
const bodyCache = new Map<number, string>()
const BODY_CACHE_MAX = 30

const bodyHtml = computed(() => {
  const d = detail.value
  if (!d) return ''
  const hit = bodyCache.get(d.uid)
  if (hit !== undefined) return hit
  // HTML 邮件优先；纯文本用 body-text（pre-wrap）——与原 Web 端 renderBody 一致
  let out = ''
  if (d.html && d.html.trim()) {
    out = `<div class="body-html">${sanitizeHtml(d.html)}</div>`
  } else {
    const text = d.text ?? ''
    out = text ? `<div class="body-text">${escapeHtml(text)}</div>` : ''
  }
  bodyCache.set(d.uid, out)
  if (bodyCache.size > BODY_CACHE_MAX) {
    const oldest = bodyCache.keys().next().value
    if (oldest !== undefined) bodyCache.delete(oldest)
  }
  return out
})

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

async function onFlag(): Promise<void> {
  if (!detail.value) return
  await mail.setMeta(detail.value.uid, { flagged: !isFlagged.value })
}

async function onPin(): Promise<void> {
  if (!detail.value) return
  await mail.setMeta(detail.value.uid, { pinned: !isPinned.value })
}

async function onCategory(category: string): Promise<void> {
  if (!detail.value) return
  await mail.setMeta(detail.value.uid, { category })
}

function onCategorySelect(value: unknown): void {
  void onCategory(typeof value === 'string' ? value : String(value ?? ''))
}

/** 切换已读/未读(可主动标记未读)。 */
async function onToggleSeen(): Promise<void> {
  if (!detail.value) return
  try {
    await mail.setMeta(detail.value.uid, { seen: !isSeen.value })
  } catch (e) {
    Message.error((e as Error).message)
  }
}

async function onTrash(): Promise<void> {
  if (!detail.value) return
  try {
    await mail.moveToKind(detail.value.uid, 'trash')
    Message.success(t('mail.movedTrash'))
  } catch (e) {
    Message.error((e as Error).message)
  }
}

async function onJunk(): Promise<void> {
  if (!detail.value) return
  try {
    await mail.moveToKind(detail.value.uid, 'junk')
    Message.success(t('mail.movedJunk'))
  } catch (e) {
    Message.error((e as Error).message)
  }
}

async function onDownload(index: number): Promise<void> {
  if (!detail.value) return
  try {
    await mail.downloadAttachment(detail.value.uid, index)
  } catch (e) {
    Message.error((e as Error).message)
  }
}

/** 点击附件 → 拉取字节写临时文件 → 交给文件预览图层展示 */
const previewing = ref(-1)
async function onPreview(index: number): Promise<void> {
  if (!detail.value || previewing.value >= 0) return
  previewing.value = index
  try {
    const { name, path } = await mail.attachmentToTemp(detail.value.uid, index)
    emit('preview', { name, path })
  } catch (e) {
    Message.error((e as Error).message)
  } finally {
    previewing.value = -1
  }
}

function onReply(): void {
  const d = detail.value
  if (!d) return
  const email = (d.from.match(/<([^>]+)>/)?.[1] ?? d.from).trim()
  emit('compose', {
    to: email,
    subject: d.subject.startsWith('Re:') ? d.subject : `Re: ${d.subject}`,
    body: `\n\n--- 原邮件 ---\n发件人: ${d.from}\n时间: ${d.date}\n主题: ${d.subject}\n\n${d.text ?? ''}`
  })
}
</script>

<template>
  <section class="mail-reader">
    <template v-if="detail">
      <div class="reader-toolbar">
        <a-tooltip :content="t('mail.reply')">
          <button type="button" class="reader-btn" @click="onReply"><icon-reply :size="16" /></button>
        </a-tooltip>
        <a-tooltip :content="isFlagged ? t('mail.unflag') : t('mail.flag')">
          <button type="button" class="reader-btn" :class="{ on: isFlagged }" @click="onFlag">
            <icon-star :size="16" />
          </button>
        </a-tooltip>
        <a-tooltip :content="t('mail.pin')">
          <button type="button" class="reader-btn" :class="{ on: isPinned }" @click="onPin">
            <icon-pushpin :size="16" />
          </button>
        </a-tooltip>
        <a-tooltip :content="isSeen ? t('mail.markUnread') : t('mail.markRead')">
          <button type="button" class="reader-btn" :class="{ on: !isSeen }" @click="onToggleSeen">
            <icon-mail :size="16" />
          </button>
        </a-tooltip>
        <a-radio-group
          :model-value="curCategory"
          class="reader-cat"
          type="button"
          size="small"
          :title="t('mail.category')"
          @change="onCategorySelect"
        >
          <a-radio value="">{{ t('mail.uncategorized') }}</a-radio>
          <a-radio value="todo">{{ t('mail.todo') }}</a-radio>
          <a-radio value="doing">{{ t('mail.doing') }}</a-radio>
          <a-radio value="done">{{ t('mail.done') }}</a-radio>
        </a-radio-group>
        <span class="reader-spacer" />
        <a-tooltip :content="t('mail.markJunk')">
          <button type="button" class="reader-btn" @click="onJunk"><icon-alert-circle :size="16" /></button>
        </a-tooltip>
        <a-tooltip :content="t('mail.delete')">
          <button type="button" class="reader-btn" @click="onTrash"><icon-delete :size="16" /></button>
        </a-tooltip>
      </div>

      <div class="reader-scroll">
        <div class="reader-header">
          <div class="reader-from-row">
            <MailAvatar :name="fromName" :email="fromEmail || detail.from" :size="36" />
            <div class="reader-from-info">
              <span class="reader-from-name">{{ fromName }}</span>
              <span v-if="fromEmail" class="reader-from-email">{{ fromEmail }}</span>
            </div>
            <span class="reader-date">{{ detail.date }}</span>
          </div>
          <h2 class="reader-subject">{{ detail.subject || t('mail.noSubject') }}</h2>
          <div class="reader-to">{{ t('mail.to') }}: {{ detail.to }}</div>
          <div v-if="detail.cc" class="reader-to">{{ t('mail.cc') }}: {{ detail.cc }}</div>
        </div>

        <!-- 正文过大被截断时的提示 -->
        <div v-if="detail.truncated" class="reader-truncated">
          <icon-exclamation-circle :size="13" />
          <span>{{ t('mail.bodyTruncated') }}</span>
        </div>

        <!-- 正文 -->
        <div class="reader-body" v-html="bodyHtml" />

        <!-- 附件：置于正文之后（与原 Web 端一致） -->
        <div v-if="detail.attachments.length" class="reader-attachments">
          <div class="att-head">
            <div class="att-title">
              <icon-folder :size="14" />{{ t('mail.attachments') }}
              <span class="att-count">{{ detail.attachments.length }}</span>
            </div>
            <button v-if="hasMoreAtts" type="button" class="att-more" @click="attVisible += ATT_PAGE">
              {{ t('mail.showMore') }}<icon-down :size="12" />
            </button>
          </div>
          <div class="att-grid">
            <div
              v-for="(a, i) in visibleAttachments"
              :key="i"
              class="att-card"
              role="button"
              tabindex="0"
              :class="{ loading: previewing === i }"
              @click="onPreview(i)"
              @keydown.enter="onPreview(i)"
            >
              <img class="att-glyph" :src="getFileIconUrl(a.filename)" width="18" height="18" alt="" />
              <span class="att-name">{{ a.filename }}</span>
              <span class="att-size">{{ fmtSize(a.size) }}</span>
              <button
                type="button"
                class="att-dl"
                :title="t('mail.download')"
                @click.stop="onDownload(i)"
              >
                <icon-download :size="13" />
              </button>
            </div>
          </div>
        </div>
      </div>
    </template>

    <div v-else-if="mail.loadingDetail" class="reader-loading">
      <a-spin :size="22" />
      <span>{{ t('mail.loadingMail') }}</span>
    </div>

    <div v-else class="reader-empty">
      <icon-mail :size="44" />
      <span>{{ t('mail.emptyRead') }}</span>
    </div>
  </section>
</template>

<style scoped>
.mail-reader {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  /* 与邮件正文的白底一致，避免正文里邮件自带的白色背景形成「白块」 */
  background: var(--bg-base);
}
.reader-toolbar {
  display: flex;
  align-items: center;
  gap: 2px;
  height: 48px;
  padding: 0 10px;
  border-bottom: 1px solid var(--panel-border);
}
.reader-spacer {
  flex: 1;
}
.reader-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.reader-btn:hover {
  background: var(--bg-hover);
}
.reader-btn.on {
  color: var(--accent-text);
}
/* 分类:分段单选(无 / 待办 / 进行中 / 已完成),对齐系统设置的 macOS 分段控件 */
.reader-cat {
  flex: 0 0 auto;
  margin-left: 2px;
}
.reader-cat :deep(.arco-radio-group-button) {
  padding: 2px;
  background-color: var(--color-fill-3) !important;
  border-radius: 9px;
}
.reader-cat :deep(.arco-radio-button) {
  margin: 2px;
  padding: 0 12px;
  line-height: 24px;
  border-radius: 7px;
  border: none !important;
  background-color: transparent;
  color: var(--color-text-2) !important;
}
/* 去掉分段控件默认的分隔竖线 */
.reader-cat :deep(.arco-radio-button)::before,
.reader-cat :deep(.arco-radio-button)::after {
  display: none !important;
}
.reader-cat :deep(.arco-radio-button:hover) {
  color: var(--color-text-1) !important;
  background-color: var(--color-fill-4) !important;
}
.reader-cat :deep(.arco-radio-button.arco-radio-checked) {
  color: var(--accent-text) !important;
  background-color: var(--color-bg-5) !important;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}
/* 阅读区纵向滚动;邮件正文里的固定宽表格过宽时,允许整块横向滚动查看(不裁切) */
.reader-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 20px 24px 40px;
}
.reader-header {
  padding-bottom: 14px;
  border-bottom: 1px solid var(--panel-border);
}
.reader-subject {
  margin: 0 0 10px;
  font-size: var(--font-size-lg);
  font-weight: 600;
  line-height: 1.35;
  color: var(--text-primary);
}
/* 发件人：头像 + 名称/邮箱 + 时间 */
.reader-from-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
}
.reader-from-info {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
  flex: 1;
}
.reader-from-name {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.reader-from-email {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.reader-date {
  flex-shrink: 0;
  color: var(--text-tertiary);
  font-size: var(--font-size-xs);
}
.reader-to {
  margin-top: 8px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.reader-attachments {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 18px 0;
}
.att-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.att-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.att-count {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
/* 顶部「更多」：再展开 5 个 */
.att-more {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  font-size: var(--font-size-xs);
}
.att-more:hover {
  color: var(--text-primary);
}
/* 紧凑网格：窄卡片，自适应每行多个 */
.att-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
  gap: 6px;
}
/* 单行高度、无边框卡片：图标 + 文件名 + 大小 + 下载 */
.att-card {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  padding: 5px 8px;
  border: none;
  border-radius: 7px;
  background: var(--bg-2);
  color: var(--text-primary);
  cursor: pointer;
  text-align: left;
  transition: background-color 0.12s ease;
}
.att-card:hover {
  background: var(--bg-3);
}
.att-card:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 1px;
}
.att-card.loading {
  opacity: 0.6;
  pointer-events: none;
}
.att-glyph {
  width: 18px;
  height: 18px;
  flex-shrink: 0;
}
.att-name {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.att-size {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
.att-dl {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  flex-shrink: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-tertiary);
  transition:
    background-color 0.12s ease,
    color 0.12s ease;
}
/* 仅当鼠标直接悬停在下载按钮上时才反馈；悬停卡片本体不改变按钮样式 */
.att-dl:hover {
  background: var(--bg-4);
  color: var(--text-primary);
}
/* 正文过大被截断的提示条 */
.reader-truncated {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 14px;
  padding: 7px 11px;
  border-radius: 8px;
  background: var(--fill-2);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}
.reader-body {
  margin-top: 18px;
  max-width: 100%;
  font-size: var(--font-size-md);
  line-height: 1.7;
  color: var(--text-primary);
  word-break: break-word;
  overflow-wrap: anywhere;
}
/* 邮件 HTML 常带固定宽度的图片：限制到容器宽,图片不撑出横向滚动条(表格保留可横向滚动) */
.reader-body :deep(img) {
  max-width: 100%;
  height: auto;
}
.reader-body :deep(table) {
  /* 不限制表格宽度:固定宽表格过宽时保留原宽,由外层横向滚动查看 */
  max-width: none;
}
.reader-body :deep(p) {
  margin: 0 0 10px;
}
.reader-body :deep(h1),
.reader-body :deep(h2),
.reader-body :deep(h3) {
  margin: 0 0 8px;
}
.reader-body :deep(a) {
  color: var(--accent-text);
}
.reader-body :deep(blockquote) {
  margin: 8px 0;
  padding-left: 12px;
  border-left: 2px solid var(--border-base);
  color: var(--text-secondary);
}
.reader-body :deep(pre) {
  max-width: 100%;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-family: 'SF Mono', Monaco, 'Cascadia Code', monospace;
}
.reader-body :deep(.body-text) {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: inherit;
}
/* 空状态 / 加载中：撑满阅读区并在水平+垂直方向居中 */
.reader-empty,
.reader-loading {
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  width: 100%;
  min-height: 0;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}
</style>
