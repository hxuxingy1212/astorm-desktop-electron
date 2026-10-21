<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { useAgentsStore } from '@/stores/agents'
import AgentCard from '@/components/agents/AgentCard.vue'
import AgentFormModal from '@/components/agents/AgentFormModal.vue'
import type {
  AgentCard as AgentCardDto,
  AgentExecMode,
  AgentPermissionMode
} from '@/services/dto'

const { t } = useI18n()
const store = useAgentsStore()

const showFormModal = ref(false)
const editingAgent = ref<AgentCardDto | null>(null)
/** 只读预览（内置卡片点击）：同一弹窗切只读视图，避免维护第二套布局 */
const previewMode = ref(false)

interface AgentFormPayload {
  name: string
  description: string
  guidance: string
  toolIds: string[]
  skillIds: string[]
  mode: AgentExecMode
  permission: AgentPermissionMode
}

onMounted(() => {
  store.fetchAgents()
})

/** 重新拉取（后端未加载 / 出错时页面给出可见反馈，而不是空列表） */
function reload(): void {
  store.fetchAgents()
}

function handleCreate(): void {
  editingAgent.value = null
  previewMode.value = false
  showFormModal.value = true
}

function handleEdit(agent: AgentCardDto): void {
  editingAgent.value = agent
  previewMode.value = false
  showFormModal.value = true
}

/** 内置智能体：只读预览（字段锁定 + 指引渲染为 markdown） */
function handlePreview(agent: AgentCardDto): void {
  editingAgent.value = agent
  previewMode.value = true
  showFormModal.value = true
}

async function handleSave(data: AgentFormPayload): Promise<void> {
  const saved = editingAgent.value
    ? await store.updateAgent(editingAgent.value.agentId, data)
    : await store.createAgent(data)
  if (saved) {
    Message.success(
      editingAgent.value
        ? t('settings.agents.saveSuccess')
        : t('settings.agents.createSuccess')
    )
    showFormModal.value = false
  } else {
    // 名称重复 / 内置不可改等后端校验失败原因由 store.error 承载
    Message.error(store.error || t('settings.agents.saveFailed'))
  }
}

function handleDelete(agent: AgentCardDto): void {
  void macConfirm({
    title: t('settings.agents.deleteConfirm'),
    content: `${t('settings.agents.deleteWarning')} "${agent.name}"?`,
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      const ok = await store.removeAgent(agent.agentId)
      if (ok) Message.success(t('settings.agents.deleteSuccess'))
      else Message.error(store.error || t('settings.agents.deleteFailed'))
    }
  })
}
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div class="header-left">
        <h2 class="page-title">{{ t('settings.agents.title') }}</h2>
        <span v-if="store.agents.length" class="page-count">{{ store.agents.length }}</span>
      </div>
      <div class="header-actions">
        <a-button type="primary" size="small" @click="handleCreate">
          <template #icon><icon-plus :size="13" /></template>
          {{ t('settings.agents.create') }}
        </a-button>
      </div>
    </div>

    <p class="page-desc">{{ t('settings.agents.description') }}</p>

    <!-- 后端调用失败（如 dll 未同步 / 应用未重启）必须可见：否则页面只是"空"，看不出是故障 -->
    <a-alert v-if="store.error" type="error" class="page-error">
      {{ t('settings.agents.loadFailed') }}：{{ store.error }}
      <template #action>
        <a-button size="mini" @click="reload">{{ t('settings.agents.retry') }}</a-button>
      </template>
    </a-alert>

    <div class="page-body">
      <a-spin v-if="store.loading" class="loading-spin" />

      <template v-else>
        <div v-if="store.agents.length" class="card-grid">
          <AgentCard
            v-for="agent in store.agents"
            :key="agent.agentId"
            :agent="agent"
            @edit="handleEdit"
            @delete="handleDelete"
            @preview="handlePreview"
          />
        </div>

        <div v-else class="empty-state">
          <p>{{ t('settings.agents.noAgents') }}</p>
          <a-button size="small" @click="handleCreate">
            {{ t('settings.agents.create') }}
          </a-button>
        </div>
      </template>
    </div>
    <AgentFormModal
      v-model:visible="showFormModal"
      :agent="editingAgent"
      :preview="previewMode"
      @save="handleSave"
    />
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
  margin-bottom: 8px;
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

.page-desc {
  margin: 0 0 18px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  line-height: 1.6;
  flex-shrink: 0;
}

.page-body {
  flex: 1;
  overflow: auto;
  /* 预占滚动条位置,内容增删时视图不晃动 */
  scrollbar-gutter: stable;
}

.page-error {
  margin-bottom: 12px;
  flex-shrink: 0;
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

/* ---------- macOS 风格控件覆盖 ---------- */

.page :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

.page :deep(.arco-btn) {
  border-radius: 6px;
}

.page :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}
</style>
