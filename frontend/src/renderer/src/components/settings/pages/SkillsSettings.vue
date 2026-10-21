<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { useSkillsStore } from '@/stores/skills'
import SkillCard from '@/components/skills/SkillCard.vue'
import SkillFormModal from '@/components/skills/SkillFormModal.vue'
import type { SkillDoc, SkillSummary } from '@/services/dto'

const { t } = useI18n()
const store = useSkillsStore()

const searchQuery = ref('')
const categoryFilter = ref('')
const showFormModal = ref(false)
const editingSkill = ref<SkillDoc | null>(null)
const packageInputRef = ref<HTMLInputElement | null>(null)

const filteredSkills = computed(() => {
  let list = store.skills
  if (searchQuery.value) {
    const q = searchQuery.value.toLowerCase()
    list = list.filter((s) => s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q))
  }
  if (categoryFilter.value) {
    list = list.filter((s) => {
      const cats = toArray(s.category)
      return cats.some((c) => c === categoryFilter.value || c.startsWith(categoryFilter.value + '/'))
    })
  }
  return list
})

const domains = ['办公自动化', '数据分析', '内容创作', '研发效能', '安全合规', '个人助理']

function toArray(v: string | null | undefined): string[] {
  if (!v) return []
  return v
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
}

interface SkillFormPayload {
  name: string
  description: string
  prompt: string
  category: string[]
  version: string
  author: string
  tags: string[]
}

onMounted(() => {
  store.fetchSkills()
})

function handleCreate(): void {
  editingSkill.value = null
  showFormModal.value = true
}

async function handleEdit(skill: SkillSummary): Promise<void> {
  const full = await store.getSkill(skill.name)
  if (full) editingSkill.value = full
  showFormModal.value = true
}

async function handleSave(data: SkillFormPayload): Promise<void> {
  if (editingSkill.value) {
    const updated = await store.updateSkill(editingSkill.value.name, data)
    if (updated) {
      Message.success(t('settings.skills.saveSuccess'))
    }
  } else {
    const imported = await store.createSkill(data)
    if (imported.length) {
      Message.success(t('settings.skills.createSuccess'))
    }
  }
  showFormModal.value = false
}

function handleToggle(name: string, enabled: boolean): void {
  store.toggleSkill(name, enabled)
  Message.success(enabled ? t('settings.skills.enabled') : t('settings.skills.disabled'))
}

function handleDelete(skill: SkillSummary): void {
  void macConfirm({
    title: t('settings.skills.deleteConfirm'),
    content: `${t('settings.skills.deleteWarning')} "${skill.name}"?`,
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      await store.removeSkill(skill.name)
      Message.success(t('settings.skills.deleteSuccess'))
    }
  })
}

async function handleDiscover(): Promise<void> {
  await store.discoverSkills()
  const count = store.discovered.length
  if (count > 0) {
    Message.success(t('settings.skills.discovered', { count }))
  } else {
    Message.info(t('settings.skills.noDiscover'))
  }
}

function handleImportPackage(): void {
  packageInputRef.value?.click()
}

async function onPackageSelected(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  input.value = ''
  // Electron 32+ 已移除 File.path；必须走 webUtils 暴露的 pathForFile。
  const path = window.api.attachment?.pathForFile?.(file) ?? ''
  if (!path) {
    Message.error(t('settings.skills.importFailed'))
    return
  }
  try {
    const imported = await store.importSkillPackage(path)
    if (!imported.length) {
      Message.error(t('settings.skills.importFailed'))
      return
    }
    Message.success(t('settings.skills.importSuccess', { count: imported.length }))
  } catch (err) {
    Message.error(err instanceof Error ? err.message : String(err))
  }
}

async function handleImportDiscovered(path: string): Promise<void> {
  const imported = await store.importSkills([path])
  if (imported.length) {
    store.discovered = store.discovered.filter((d) => d.path !== path)
    Message.success(t('settings.skills.importSuccess', { count: 1 }))
  }
}

async function handleImportAllDiscovered(): Promise<void> {
  const paths = store.discovered.map((d) => d.path)
  const imported = await store.importSkills(paths)
  if (imported.length) {
    store.discovered = []
    Message.success(t('settings.skills.importSuccess', { count: imported.length }))
  }
}
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div class="header-left">
        <h2 class="page-title">{{ t('settings.skills.title') }}</h2>
        <span v-if="store.skills.length" class="page-count">{{ store.skills.length }}</span>
      </div>
      <div class="header-actions">
        <a-input-search
          v-model="searchQuery"
          :placeholder="t('settings.skills.searchPlaceholder')"
          allow-clear
          style="width: 220px"
        />
        <a-button size="small" @click="handleDiscover">
          <template #icon><icon-refresh :size="13" /></template>
          {{ t('settings.skills.discover') }}
        </a-button>
        <a-button size="small" @click="handleImportPackage">
          <template #icon><icon-download :size="13" /></template>
          {{ t('settings.skills.fromPackage') }}
        </a-button>
        <input
          ref="packageInputRef"
          type="file"
          accept=".zip,.tar.gz,.tgz"
          style="display: none"
          @change="onPackageSelected"
        />
        <a-button type="primary" size="small" @click="handleCreate">
          <template #icon><icon-plus :size="13" /></template>
          {{ t('settings.skills.create') }}
        </a-button>
      </div>
    </div>

    <div class="filter-bar">
      <span class="filter-tag" :class="{ active: !categoryFilter }" @click="categoryFilter = ''">
        {{ t('settings.skills.all') }}
      </span>
      <span
        v-for="dom in domains"
        :key="dom"
        class="filter-tag"
        :class="{ active: categoryFilter === dom }"
        @click="categoryFilter = categoryFilter === dom ? '' : dom"
      >
        {{ dom }}
      </span>
    </div>

    <div class="page-body">
      <a-spin v-if="store.loading" class="loading-spin" />

      <div v-else-if="!store.skills.length && !store.discovered.length" class="empty-state">
        <p>{{ t('settings.skills.noSkills') }}</p>
        <a-button size="small" @click="handleDiscover">
          {{ t('settings.skills.discover') }}
        </a-button>
      </div>

      <template v-else>
        <div v-if="filteredSkills.length" class="card-grid">
          <SkillCard
            v-for="skill in filteredSkills"
            :key="skill.name"
            :skill="skill"
            @toggle="handleToggle"
            @edit="handleEdit"
            @delete="handleDelete"
          />
        </div>

        <a-empty v-else-if="store.skills.length">
          {{ t('settings.skills.noMatch') }}
        </a-empty>

        <div v-if="store.discovered.length" class="discovered-section">
          <div class="discovered-header">
            <h3 class="discovered-title">{{ t('settings.skills.discoveredTitle') }}</h3>
            <a-button size="small" type="primary" @click="handleImportAllDiscovered">
              {{ t('settings.skills.importAll') }} ({{ store.discovered.length }})
            </a-button>
          </div>
          <div class="discovered-list">
            <div v-for="item in store.discovered" :key="item.path" class="discovered-item">
              <div class="discovered-info">
                <span class="discovered-name">{{ item.name }}</span>
                <span v-if="item.error" class="discovered-error">{{ item.error }}</span>
                <span v-else class="discovered-desc">{{ item.description }}</span>
                <code class="discovered-path">{{ item.path }}</code>
              </div>
              <a-button size="small" :disabled="!!item.error" @click="handleImportDiscovered(item.path)">
                {{ t('settings.skills.import') }}
              </a-button>
            </div>
          </div>
        </div>
      </template>
    </div>

    <SkillFormModal v-model:visible="showFormModal" :skill="editingSkill" @save="handleSave" />
  </div>
</template>

<style scoped>
.page {
  height: 100%;
  display: flex;
  flex-direction: column;
  padding: 28px 56px 16px;
}

.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 20px;
  flex-shrink: 0;
  flex-wrap: wrap;
  gap: 12px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 10px;
}

.page-title {
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}

.page-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 20px;
  height: 20px;
  box-sizing: border-box;
  font-size: var(--font-size-2xs);
  font-weight: 500;
  line-height: 1;
  color: var(--text-tertiary);
  background: var(--bg-3);
  border-radius: 50%;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

/* 分类筛选:macOS 分段控件(浅灰胶囊容器 + 选中白色胶囊) */
.filter-bar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  padding: 4px 6px;
  margin-bottom: 16px;
  flex-shrink: 0;
  background: var(--bg-2);
  border-radius: 10px;
  width: fit-content;
  max-width: 100%;
}

.filter-tag {
  font-size: var(--font-size-sm);
  padding: 5px 14px;
  line-height: 1.4;
  cursor: pointer;
  border-radius: 6px;
  color: var(--text-secondary);
  transition: all 0.15s ease;
  user-select: none;
}

.filter-tag:hover {
  color: var(--text-primary);
  background: var(--bg-3);
}

.filter-tag.active {
  color: var(--accent-text);
  background: var(--bg-base);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

.page-body {
  flex: 1;
  overflow: auto;
  /* 预占滚动条位置,内容增删时视图不晃动 */
  scrollbar-gutter: stable;
}

.loading-spin {
  display: flex;
  justify-content: center;
  padding: 80px 0;
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  padding: 80px 0;
  color: var(--text-tertiary);
}

.card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 12px;
}

/* Discovered section */
.discovered-section {
  margin-top: 32px;
  padding-top: 20px;
  border-top: 1px solid var(--border-base);
}

.discovered-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}

.discovered-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}

.discovered-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.discovered-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 14px;
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  transition: border-color var(--transition-fast);
}

.discovered-item:hover {
  border-color: var(--accent-text);
}

.discovered-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.discovered-name {
  font-weight: 600;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

.discovered-desc {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.discovered-error {
  font-size: var(--font-size-xs);
  color: var(--danger);
  word-break: break-all;
}

.discovered-path {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  font-family: monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ---------- macOS 风格控件覆盖 ---------- */

/* 主按钮(新建技能/导入全部):macOS 系统蓝填充 + 白字 */
.page :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* 按钮:macOS 圆角 */
.page :deep(.arco-btn) {
  border-radius: 6px;
}

/* 搜索框:更柔和的圆角 */
.page :deep(.arco-input-wrapper),
.page :deep(.arco-input-outer) {
  border-radius: 8px;
}

/* 按钮图标:垂直居中对齐,与文字尺寸协调 */
.page :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}
</style>