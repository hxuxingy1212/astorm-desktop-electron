<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useMailStore, folderKind } from '@/stores/mail'

const { t } = useI18n()
const mail = useMailStore()

const emit = defineEmits<{
  (e: 'compose'): void
}>()

const FOLDER_META: Record<string, { icon: string; labelKey: string }> = {
  inbox: { icon: 'icon-inbox', labelKey: 'mail.inbox' },
  draft: { icon: 'icon-file-text', labelKey: 'mail.draft' },
  sent: { icon: 'icon-send', labelKey: 'mail.sent' },
  junk: { icon: 'icon-alert-circle', labelKey: 'mail.junk' },
  trash: { icon: 'icon-delete', labelKey: 'mail.trash' }
}

const CATEGORIES = [
  { key: 'todo', icon: 'icon-check-circle', labelKey: 'mail.todo' },
  { key: 'doing', icon: 'icon-clock-circle', labelKey: 'mail.doing' },
  { key: 'done', icon: 'icon-check', labelKey: 'mail.done' }
]

interface FolderEntry {
  name: string
  kind: string
  unread: number
  total: number
}

/** 标准文件夹（每类一个，按收件箱/草稿/已发送/垃圾/废纸篓顺序）。 */
const standardFolders = computed<FolderEntry[]>(() => {
  const order = ['inbox', 'draft', 'sent', 'junk', 'trash']
  const out: FolderEntry[] = []
  for (const kind of order) {
    const c = mail.counts.find((x) => !x.folder.startsWith('#') && folderKind(x.folder) === kind)
    if (c) out.push({ name: c.folder, kind, unread: c.unread, total: c.total })
  }
  return out
})

const categoryCounts = computed(() =>
  CATEGORIES.map((c) => {
    const found = mail.counts.find((x) => x.folder === `#${c.key}`)
    return { ...c, count: found?.total ?? 0 }
  })
)

const activeFolder = computed(() => mail.curFolder)
const activeCategory = computed(() => mail.curCategory)

function isFolderActive(name: string): boolean {
  return !activeCategory.value && activeFolder.value === name
}

function accountLabel(a: { displayName: string | null; name: string; username: string }): string {
  return a.displayName || a.name || a.username
}

function onSelectAccount(value: string | number | Record<string, unknown> | undefined): void {
  if (typeof value !== 'string') return
  void mail.selectAccount(value)
}
</script>

<template>
  <aside class="mail-sidebar">
    <div class="mail-sidebar-head">
      <a-dropdown trigger="click" position="bl" @select="onSelectAccount">
        <button type="button" class="account-btn">
          <icon-mail :size="16" />
          <span class="account-name">
            {{
              mail.state.accounts.find((a) => a.name === mail.curAccount)
                ? accountLabel(mail.state.accounts.find((a) => a.name === mail.curAccount)!)
                : t('mail.noAccount')
            }}
          </span>
          <icon-down :size="12" class="account-arrow" />
        </button>
        <template #content>
          <a-doption v-for="a in mail.state.accounts" :key="a.name" :value="a.name">
            <span class="flex items-center justify-between gap-3">
              <span>{{ accountLabel(a) }}</span>
              <icon-check v-if="a.name === mail.state.active" :size="13" />
            </span>
          </a-doption>
        </template>
      </a-dropdown>
      <button type="button" class="compose-btn" :title="t('mail.compose')" @click="emit('compose')">
        <icon-edit :size="16" />
      </button>
    </div>

    <div class="mail-sidebar-scroll">
      <div class="mail-section">
        <button
          v-for="f in standardFolders"
          :key="f.name"
          type="button"
          class="folder-row"
          :class="{ active: isFolderActive(f.name) }"
          @click="mail.selectFolder(f.name)"
        >
          <component :is="FOLDER_META[f.kind]?.icon ?? 'icon-folder'" :size="16" class="shrink-0" />
          <span class="folder-label">{{ FOLDER_META[f.kind] ? t(FOLDER_META[f.kind].labelKey) : f.name }}</span>
          <span v-if="f.unread" class="badge">{{ f.unread > 99 ? '99+' : f.unread }}</span>
        </button>
      </div>

      <div class="mail-section">
        <div class="mail-section-title">{{ t('mail.categories') }}</div>
        <button
          v-for="c in categoryCounts"
          :key="c.key"
          type="button"
          class="folder-row"
          :class="{ active: activeCategory === c.key }"
          @click="mail.selectCategory(c.key)"
        >
          <component :is="c.icon" :size="16" class="shrink-0" />
          <span class="folder-label">{{ t(c.labelKey) }}</span>
          <span v-if="c.count" class="badge badge-muted">{{ c.count }}</span>
        </button>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.mail-sidebar {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--bg-2);
  border-right: 1px solid var(--panel-border);
}
.mail-sidebar-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px;
}
.account-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
  height: 34px;
  padding: 0 10px;
  border: 1px solid var(--panel-border);
  border-radius: 8px;
  background: var(--bg-1);
  color: var(--text-primary);
  cursor: pointer;
}
.account-btn:hover {
  background: var(--bg-hover);
}
.account-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-md);
  text-align: left;
}
.account-arrow {
  color: var(--text-tertiary);
}
.compose-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  transition: background-color 0.15s ease;
}
.compose-btn:hover {
  background: var(--bg-hover);
}
.compose-btn:active {
  background: var(--bg-active);
}
.mail-sidebar-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 4px 6px 8px;
}
.mail-section {
  margin-bottom: 10px;
}
.mail-section-title {
  padding: 6px 10px 4px;
  font-size: var(--font-size-md);
  font-weight: 400;
  color: var(--text-tertiary);
}
.folder-row {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  height: 34px;
  padding: 0 10px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  font-size: var(--font-size-md);
  transition: background-color var(--transition-fast);
}
.folder-row:hover {
  background: var(--bg-hover);
}
.folder-row.active {
  background: var(--bg-3);
  color: var(--text-primary);
  font-weight: 500;
}
.folder-row-plain {
  color: var(--text-tertiary);
}
.folder-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: left;
}
.badge {
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  border-radius: 9px;
  background: var(--bg-3);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
  line-height: 18px;
  text-align: center;
}
.badge-muted {
  background: var(--bg-3);
  color: var(--text-secondary);
}
</style>
