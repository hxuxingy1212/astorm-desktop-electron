<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { ComponentPublicInstance } from 'vue'
import { useI18n } from 'vue-i18n'
import { useMailStore, folderKind, type MailMessage } from '@/stores/mail'
import MailAvatar from '@/components/mail/MailAvatar.vue'

const { t } = useI18n()
const mail = useMailStore()
const scroller = ref<HTMLElement | null>(null)

/**
 * 懒加载正文摘要：行进入可视区域才请求（Web 端同款 IntersectionObserver 策略），
 * 避免一次性对整页 30 封做 IMAP 拉取；命中后即 unobserve。
 *
 * `observed` 去重：函数式 `:ref` 在行每次 patch（如写入 preview/seen）都会被再次调用，
 * 若不去重会对同一行反复 observe/unobserve。切文件夹/账号/分类时清空。
 */
let observer: IntersectionObserver | null = null
const observed = new Set<number>()
function observeRow(el: Element | ComponentPublicInstance | null): void {
  if (!(el instanceof HTMLElement) || !observer) return
  const uid = Number(el.dataset.uid)
  if (!uid || observed.has(uid)) return
  observed.add(uid)
  observer.observe(el)
}
watch(
  () => [mail.curFolder, mail.curAccount, mail.curCategory],
  () => observed.clear()
)
onMounted(() => {
  observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        const node = entry.target as HTMLElement
        const uid = Number(node.dataset.uid)
        if (uid) mail.requestPreview(uid, node.dataset.folder || '')
        observer?.unobserve(node)
      }
    },
    { root: scroller.value, rootMargin: '150px 0px' }
  )
})
onBeforeUnmount(() => {
  observer?.disconnect()
  observer = null
})

const FOLDER_LABEL: Record<string, string> = {
  inbox: 'mail.inbox',
  draft: 'mail.draft',
  sent: 'mail.sent',
  junk: 'mail.junk',
  trash: 'mail.trash'
}

const title = computed(() => {
  if (mail.curCategory) {
    const key: Record<string, string> = { todo: 'mail.todo', doing: 'mail.doing', done: 'mail.done' }
    return t(key[mail.curCategory] ?? 'mail.inbox')
  }
  const kind = folderKind(mail.curFolder)
  return FOLDER_LABEL[kind] ? t(FOLDER_LABEL[kind]) : mail.curFolder
})

function displayName(from: string): string {
  if (!from) return t('mail.unknownSender')
  const m = from.match(/^(.*?)\s*<.*>$/)
  return (m?.[1] || from.split('@')[0] || from).replace(/^"|"$/g, '').trim()
}

/** 已发送/草稿显示收件人（对方），收到的邮件显示发件人。 */
function outgoing(m: MailMessage): boolean {
  const k = folderKind(m.folder || mail.curFolder)
  return k === 'sent' || k === 'draft'
}

function shortDate(date: string, ts: number): string {
  const d = ts ? new Date(ts * 1000) : new Date(date)
  if (Number.isNaN(d.getTime())) return date || ''
  const now = new Date()
  const sameYear = d.getFullYear() === now.getFullYear()
  const pad = (n: number): string => String(n).padStart(2, '0')
  if (d.toDateString() === now.toDateString()) return `${pad(d.getHours())}:${pad(d.getMinutes())}`
  if (sameYear) return `${d.getMonth() + 1}/${d.getDate()}`
  return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`
}

/**
 * 每行展示字段预计算：原来模板每行重复调用 `senderLabel`/`outgoing`/`shortDate`
 * （各含正则 / folderKind / Date 构造）多次，列表一长就明显。这里在 messages 变化时
 * 一次性算好（`row.sender` / `row.email` / `row.dateText`），渲染只读字段。
 */
const rows = computed(() =>
  mail.messages.map((m) => {
    const out = outgoing(m)
    const who = out ? m.to : m.from
    const sender = who
      ? displayName(who)
      : out
        ? t('mail.noRecipient')
        : t('mail.unknownSender')
    return { m, email: who, sender, dateText: shortDate(m.date, m.ts) }
  })
)

function onScroll(): void {
  const el = scroller.value
  if (!el) return
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 120) void mail.loadMore()
}

function onRefresh(): void {
  void mail.refreshMessages()
  void mail.refreshSidebar()
}

function openMessage(m: MailMessage): void {
  void mail.selectMessage(m.uid)
}

function toggleUnread(): void {
  mail.unreadOnly = !mail.unreadOnly
  void mail.loadMessages(true)
}
</script>

<template>
  <section class="mail-list">
    <div class="mail-list-head">
      <div class="mail-list-title-wrap">
        <span class="mail-list-title">{{ title }}</span>
        <span class="mail-list-sub">
          {{ t('mail.headerSub', { total: mail.total, unread: mail.unread }) }}
        </span>
      </div>
      <div class="mail-list-actions">
        <button
          type="button"
          class="list-icon-btn"
          :class="{ active: mail.unreadOnly }"
          :title="t('mail.unreadOnly')"
          @click="toggleUnread"
        >
          <icon-eye :size="15" />
        </button>
        <button type="button" class="list-icon-btn" :title="t('mail.refresh')" @click="onRefresh">
          <icon-refresh :size="15" />
        </button>
      </div>
    </div>

    <div ref="scroller" class="mail-list-body" @scroll="onScroll">
      <div
        v-if="!mail.messages.length && !mail.loadingList && !mail.syncing"
        class="mail-empty"
      >
        <icon-empty :size="36" />
        <span>{{ t('mail.emptyList') }}</span>
      </div>

      <button
        v-for="row in rows"
        :key="row.m.uid"
        :ref="observeRow"
        :data-uid="row.m.uid"
        :data-folder="row.m.folder"
        type="button"
        class="msg-row"
        :class="{ active: mail.curUid === row.m.uid, unread: !row.m.seen }"
        @click="openMessage(row.m)"
      >
        <span class="msg-dot" :class="{ read: row.m.seen }" />
        <MailAvatar :name="row.sender" :email="row.email" :size="34" />
        <span class="msg-main">
          <span class="msg-top">
            <span class="msg-from">{{ row.sender }}</span>
            <span class="msg-badges">
              <icon-pushpin v-if="row.m.pinned" :size="13" class="badge-icon pin" />
              <icon-star v-if="row.m.flagged" :size="13" class="badge-icon flag" />
              <span v-if="row.m.category" class="cat-badge" :class="`cat-${row.m.category}`">
                {{ t(`mail.${row.m.category}`) }}
              </span>
            </span>
            <span class="msg-date">{{ row.dateText }}</span>
          </span>
          <span class="msg-subject">{{ row.m.subject || t('mail.noSubject') }}</span>
          <span v-if="row.m.preview" class="msg-preview">{{ row.m.preview }}</span>
        </span>
      </button>

      <div v-if="mail.loadingList || (mail.syncing && !mail.messages.length)" class="mail-loading">
        <a-spin :size="16" />
      </div>
      <button
        v-else-if="mail.hasMore"
        type="button"
        class="load-more"
        @click="mail.loadMore()"
      >{{ t('mail.loadMore') }}</button>
    </div>
  </section>
</template>

<style scoped>
.mail-list {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--bg-1);
  border-right: 1px solid var(--panel-border);
}
.mail-list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  height: 48px;
  padding: 0 12px;
  border-bottom: 1px solid var(--panel-border);
}
.mail-list-title-wrap {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}
.mail-list-title {
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}
.mail-list-sub {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
.mail-list-actions {
  display: flex;
  align-items: center;
  gap: 2px;
}
.list-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.list-icon-btn:hover {
  background: var(--bg-hover);
}
.list-icon-btn.active {
  color: var(--accent-text);
}
.mail-list-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
.msg-row {
  display: flex;
  gap: 10px;
  width: 100%;
  padding: 11px 14px;
  border: none;
  border-bottom: 1px solid var(--panel-border);
  background: transparent;
  cursor: pointer;
  text-align: left;
  transition: background-color 0.12s ease;
  /* 浏览器原生「视口外行跳过布局/绘制」：列表很长时显著降低渲染成本（无需引入虚拟滚动库） */
  content-visibility: auto;
  contain-intrinsic-size: auto 64px;
}
.msg-row:hover {
  background: var(--bg-hover);
}
.msg-row.active {
  background: var(--bg-3);
  box-shadow: inset 3px 0 0 var(--accent-text);
}
/* 未读圆点（行首，read 时透明占位保持对齐） */
.msg-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--accent-text);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-text) 16%, transparent);
}
.msg-dot.read {
  background: transparent;
  box-shadow: none;
}
.msg-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  flex: 1;
}
.msg-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
.msg-from {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.msg-row.unread .msg-from {
  font-weight: 600;
}
/* 行内徽标：置顶 / 标记 / 分类（与原 Web 端一致，紧邻发件人右侧、日期之前） */
.msg-badges {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}
.badge-icon {
  flex-shrink: 0;
}
.badge-icon.pin {
  color: var(--accent-text);
}
.badge-icon.flag {
  color: var(--warning);
}
.cat-badge {
  font-size: var(--font-size-2xs);
  line-height: 1;
  padding: 3px 6px;
  border-radius: 6px;
  white-space: nowrap;
}
.cat-badge.cat-todo {
  background: color-mix(in srgb, var(--warning) 18%, transparent);
  color: var(--warning);
}
.cat-badge.cat-doing {
  background: color-mix(in srgb, var(--accent-text) 14%, transparent);
  color: var(--accent-text);
}
.cat-badge.cat-done {
  background: color-mix(in srgb, var(--success) 18%, transparent);
  color: var(--success);
}
.msg-date {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
.msg-subject {
  font-size: var(--font-size-code);
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.msg-row.unread .msg-subject {
  color: var(--text-primary);
}
.msg-preview {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mail-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  height: 60%;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}
.mail-loading,
.load-more {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  padding: 12px;
  color: var(--text-tertiary);
}
.load-more {
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: var(--font-size-xs);
}
.load-more:hover {
  color: var(--accent-text);
}
</style>
