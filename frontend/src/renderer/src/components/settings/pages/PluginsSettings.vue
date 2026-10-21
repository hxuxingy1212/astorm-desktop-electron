<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { usePluginsStore } from '@/stores/plugins'
import { invoke } from '@/services/backend'
import type { PluginView } from '@/services/dto'
import PluginConfigForm from '@/components/plugins/PluginConfigForm.vue'

const { t } = useI18n()
const plugins = usePluginsStore()

const selectedId = ref<string | null>(null)
const saving = ref(false)
const pendingConfig = ref<Record<string, unknown>>({})
const configFormRef = ref<InstanceType<typeof PluginConfigForm> | null>(null)

/** 点卡片：展开配置区并载入当前配置快照；再次点击已选中的卡片则收起 */
function selectPlugin(v: PluginView): void {
  if (selectedId.value === v.manifest.id) {
    selectedId.value = null
    return
  }
  selectedId.value = v.manifest.id
  pendingConfig.value = { ...v.config }
}

/** 从本地目录安装插件 */
async function install(): Promise<void> {
  try {
    const dir = await window.api.dialog.openDirectory()
    if (!dir) return
    await plugins.installFromDir(dir)
    Message.success(t('plugins.installSuccess'))
  } catch (e) {
    Message.error(`${t('plugins.installFailed')}: ${e instanceof Error ? e.message : String(e)}`)
  }
}

function toggle(v: PluginView, enabled: boolean): void {
  void plugins.setEnabled(v.manifest.id, enabled).catch((e) => {
    Message.error(e instanceof Error ? e.message : String(e))
  })
}

async function openPlugin(v: PluginView): Promise<void> {
  try {
    // 打开文件管理器并定位到插件目录（userData/plugins/<id>）
    const dir = (await invoke('plugin_dir', { id: v.manifest.id })) as string
    const err = await window.api.fs.openFolder(dir)
    if (err) Message.error(t('plugins.openFailed'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

function remove(v: PluginView): void {
  void macConfirm({
    title: t('plugins.remove'),
    content: t('plugins.removeConfirm', { name: v.manifest.name }),
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      try {
        await plugins.remove(v.manifest.id)
        if (selectedId.value === v.manifest.id) selectedId.value = null
        Message.success(t('plugins.remove') + ' ✓')
      } catch (e) {
        Message.error(e instanceof Error ? e.message : String(e))
      }
    }
  })
}

async function save(v: PluginView): Promise<void> {
  if (!configFormRef.value?.validate()) {
    Message.warning(t('plugins.configFieldRequired'))
    return
  }
  saving.value = true
  try {
    await plugins.saveConfig(v.manifest.id, pendingConfig.value)
    Message.success(t('plugins.configSaved'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div class="header-left">
        <h2 class="page-title">{{ t('plugins.menuTitle') }}</h2>
        <span v-if="plugins.list.length" class="page-count">{{ plugins.list.length }}</span>
      </div>
      <div class="header-actions">
        <a-button size="small" type="primary" @click="install">
          <template #icon><icon-plus :size="13" /></template>
          {{ t('plugins.installFromDir') }}
        </a-button>
      </div>
    </div>

    <div class="page-body">
      <a-spin v-if="plugins.loading" class="loading-spin" />

      <div v-else-if="!plugins.list.length" class="empty-state">
        <icon-puzzle :size="40" class="empty-icon" />
        <p>{{ t('plugins.empty') }}</p>
        <p class="empty-tip">{{ t('plugins.installFromDirTip') }}</p>
        <a-button size="small" @click="install">{{ t('plugins.installFromDir') }}</a-button>
      </div>

      <div v-else class="plugin-list">
        <div
          v-for="v in plugins.list"
          :key="v.manifest.id"
          class="plugin-card"
          :class="{ selected: selectedId === v.manifest.id, 'is-disabled': !v.enabled }"
        >
          <div class="plugin-card-header" @click="selectPlugin(v)">
            <icon-puzzle class="plugin-card-icon" :size="18" />
            <div class="plugin-card-main">
              <div class="plugin-card-title">
                <span class="plugin-name">{{ v.manifest.name }}</span>
                <span v-if="v.manifest.id === 'demo'" class="builtin-tip">{{ t('plugins.builtinTip') }}</span>
                <span v-if="!v.enabled" class="off-badge">{{ t('plugins.disabled') }}</span>
              </div>
              <div class="plugin-card-meta">
                <span>{{ t('plugins.version') }} {{ v.manifest.version || '0.0.0' }}</span>
                <span v-if="v.manifest.permissions?.length" class="perm-list">
                  {{ t('plugins.permissions') }}: {{ v.manifest.permissions.join(', ') }}
                </span>
              </div>
              <div v-if="v.manifest.description" class="plugin-desc">{{ v.manifest.description }}</div>
            </div>
            <div class="plugin-card-actions" @click.stop>
              <a-switch
                :model-value="v.enabled"
                size="small"
                @update:model-value="(val) => toggle(v, Boolean(val))"
              />
              <a-button size="small" @click="openPlugin(v)">{{ t('plugins.open') }}</a-button>
              <a-button size="small" status="danger" @click="remove(v)">{{ t('plugins.remove') }}</a-button>
            </div>
          </div>

          <!-- 配置区：选中并展开；schema 驱动的动态表单 -->
          <div v-if="selectedId === v.manifest.id" class="plugin-config" @click.stop>
            <template v-if="v.manifest.configSchema?.length">
              <PluginConfigForm
                ref="configFormRef"
                :fields="v.manifest.configSchema"
                :model-value="pendingConfig"
                @update:model-value="pendingConfig = $event"
              />
              <div class="config-actions">
                <a-button type="primary" size="small" :loading="saving" @click="save(v)">
                  {{ t('plugins.saveConfig') }}
                </a-button>
              </div>
            </template>
            <div v-else class="config-empty">{{ t('plugins.configEmpty') }}</div>
          </div>
        </div>
      </div>
    </div>
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

.page-body {
  flex: 1;
  overflow: auto;
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
  gap: 10px;
  padding: 80px 0;
  color: var(--text-tertiary);
}

.empty-icon {
  color: var(--text-tertiary);
}

.empty-tip {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.plugin-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

/* macOS 卡片:白底 + 细边框 + 大圆角 */
.plugin-card {
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-lg);
  overflow: hidden;
  transition:
    border-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

.plugin-card.selected {
  border-color: var(--accent-text);
  box-shadow: var(--shadow-sm);
}

.plugin-card.is-disabled {
  opacity: 0.6;
}

.plugin-card-header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  cursor: pointer;
}

.plugin-card-icon {
  flex-shrink: 0;
  color: var(--accent-text);
}

.plugin-card-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.plugin-card-title {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.plugin-name {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}

.builtin-tip {
  font-size: var(--font-size-2xs);
  padding: 1px 6px;
  border-radius: 6px;
  background: var(--bg-3);
  color: var(--accent-text);
}

.off-badge {
  font-size: var(--font-size-2xs);
  padding: 1px 6px;
  border-radius: 6px;
  background: var(--bg-3);
  color: var(--text-tertiary);
}

.plugin-card-meta {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.plugin-desc {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.plugin-card-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.plugin-config {
  padding: 14px 16px 16px;
  border-top: 1px solid var(--border-base);
  background: var(--bg-1);
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.config-actions {
  display: flex;
  justify-content: flex-end;
}

.config-empty {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* ---------- macOS 风格控件覆盖 ---------- */

/* 主按钮(安装插件/保存配置):macOS 系统蓝填充 + 白字 */
.page :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* 按钮:macOS 圆角 */
.page :deep(.arco-btn) {
  border-radius: 6px;
}

/* 按钮图标:垂直居中 */
.page :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}

/* 插件开关:macOS 绿色滑块 */
.plugin-card :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.plugin-card :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}

/* 配置区(插件配置 JSON 对应的动态表单)控件圆角 */
.plugin-config :deep(.arco-input-wrapper),
.plugin-config :deep(.arco-input-outer),
.plugin-config :deep(.arco-textarea-wrapper),
.plugin-config :deep(.arco-select) {
  border-radius: 8px;
}
</style>