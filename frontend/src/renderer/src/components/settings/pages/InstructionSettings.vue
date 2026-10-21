<script setup lang="ts">
import SettingsSection from '../SettingsSection.vue'
/**
 * 全局约定设置页（AGENTS.md 合并注入）。
 *
 * 只管**全局**那份约定文件（`<用户目录>/AGENTS.md`，用户目录 = 「默认会话空间基目录」设置）：
 * 两个注入开关（全局 / 会话空间）+ 全局文件的编辑与打开。
 *
 * 会话空间那份（`<会话工作区>/AGENTS.md`）是工作区里的普通文件，不进设置页编辑；
 * 本页只展示合并顺序，让用户知道它会以什么优先级参与拼装。
 *
 * 保存 = 写文件（`write_global_agents`）+ 保存开关（`get_settings` → 改 `instructions` 段 →
 * `save_settings`，主进程随即 `syncBackendConfig()` 热更后端）。注入内容在**下一轮**生效。
 */
import { onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { invoke } from '@/services/backend'
import MultilineTextField from '@/components/common/MultilineTextField.vue'
import type { AppSettings, InstructionSettings } from '@/services/dto'
import { isWebShell } from '@/utils/platform'

const { t } = useI18n()

/** 与后端 `InstructionConfig::default` 一致（均开启） */
const DEFAULTS: InstructionSettings = { injectGlobalAgents: true, injectWorkspaceAgents: true }

/** 编辑器字符上限。**必须与 Rust `agent/prompt.rs::AGENTS_MAX_CHARS` 一致** */
const AGENTS_MAX_CHARS = 8000

const form = reactive<InstructionSettings>({ ...DEFAULTS })
const content = ref('')
const filePath = ref('')
const fileExists = ref(false)
const tooLarge = ref(false)
const saving = ref(false)
const loading = ref(false)

type ReadResult = { path: string; content: string; exists: boolean; tooLarge: boolean }

async function loadAll(): Promise<void> {
  loading.value = true
  try {
    const s = (await invoke('get_settings')) as AppSettings
    if (s.instructions) Object.assign(form, s.instructions)
  } catch {
    /* 保持默认值 */
  }
  try {
    const r = (await invoke('read_global_agents')) as ReadResult
    filePath.value = r.path
    fileExists.value = r.exists
    tooLarge.value = r.tooLarge
    content.value = r.content
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    loading.value = false
  }
}

async function openFile(): Promise<void> {
  // openPath 对文件同样有效（交给系统默认应用打开）
  const err = await window.api.fs.openFolder(filePath.value)
  if (err) Message.error(err)
}

async function saveAll(): Promise<void> {
  if (content.value.length > AGENTS_MAX_CHARS) {
    Message.error(t('settings.instructions.tooLong', { max: AGENTS_MAX_CHARS }))
    return
  }
  saving.value = true
  try {
    const written = (await invoke('write_global_agents', { content: content.value })) as {
      path: string
      ok: boolean
    }
    filePath.value = written.path
    fileExists.value = true
    tooLarge.value = false

    const s = (await invoke('get_settings')) as AppSettings
    s.instructions = { ...form }
    await invoke('save_settings', { settings: s })
    Message.success(t('common.saveSuccess'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    saving.value = false
  }
}

onMounted(() => {
  loadAll()
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.instructions.title') }}</h2>
    </div>
    <div class="page-body">
      <!-- 注入开关 -->
      <SettingsSection>
        <template #title>{{ t('settings.instructions.injection') }}</template>
        <div class="settings-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.instructions.injectGlobal') }}</span>
            <span class="row-desc">{{ t('settings.instructions.injectGlobalDesc') }}</span>
          </div>
          <a-switch v-model="form.injectGlobalAgents" />
        </div>
        <div class="settings-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.instructions.injectWorkspace') }}</span>
            <span class="row-desc">{{ t('settings.instructions.injectWorkspaceDesc') }}</span>
          </div>
          <a-switch v-model="form.injectWorkspaceAgents" />
        </div>
      </SettingsSection>

      <!-- 合并顺序（只读说明） -->
      <SettingsSection>
        <template #title>{{ t('settings.instructions.mergeOrder') }}</template>
        <div class="merge-order">
          <span class="merge-item">{{ t('settings.instructions.orderStatic') }}</span>
          <span class="merge-arrow">→</span>
          <span class="merge-item">{{ t('settings.instructions.orderGlobal') }}</span>
          <span class="merge-arrow">→</span>
          <span class="merge-item">{{ t('settings.instructions.orderWorkspace') }}</span>
          <span class="merge-arrow">→</span>
          <span class="merge-item">{{ t('settings.instructions.orderGuidance') }}</span>
        </div>
        <span class="row-desc">{{ t('settings.instructions.mergeOrderDesc') }}</span>
      </SettingsSection>

      <!-- 全局约定文件 -->
      <SettingsSection>
        <template #title>{{ t('settings.instructions.globalFile') }}</template>
        <div class="settings-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.instructions.filePath') }}</span>
            <span class="row-desc">
              {{ fileExists ? t('settings.instructions.fileExists') : t('settings.instructions.fileMissing') }}
            </span>
          </div>
          <div class="dir-pick">
            <span class="dir-path" :title="filePath">{{ filePath || '—' }}</span>
            <!-- 系统默认应用打开：浏览器部署无此能力，隐藏按钮 -->
            <a-button v-if="!isWebShell" size="small" :disabled="!fileExists" @click="openFile">
              {{ t('settings.instructions.openFile') }}
            </a-button>
          </div>
        </div>
        <div class="editor-wrap">
          <!-- 大段文本用多行输入框（底部保存；内容仍是 Markdown 文本） -->
          <MultilineTextField
            v-model="content"
            :max-length="AGENTS_MAX_CHARS"
            :min-height="480"
            :placeholder="t('settings.instructions.editorPlaceholder')"
          />
          <span v-if="tooLarge" class="warn">{{ t('settings.instructions.tooLarge') }}</span>
        </div>
      </SettingsSection>
    </div>

    <div class="page-footer">
      <div class="footer-actions">
        <a-button :loading="loading" @click="loadAll">{{ t('settings.instructions.reload') }}</a-button>
        <a-button type="primary" :loading="saving" @click="saveAll">
          {{ t('common.save') }}
        </a-button>
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
}

.page-title {
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
}

.page-body {
  flex: 1;
  overflow: auto;
  padding: 0 56px 4px 0;
  scrollbar-gutter: stable;
}

.settings-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 8px 0;
}

.row-text {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.row-title {
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.row-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

/* 合并顺序：四个环节用箭头串成一行 */
.merge-order {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}

.merge-item {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  background-color: var(--bg-2);
  border-radius: 3px;
  padding: 4px 8px;
}

.merge-arrow {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.dir-pick {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.dir-path {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  font-family: 'SF Mono', Monaco, 'Cascadia Code', monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 360px;
  background-color: var(--color-fill-2);
  border: 1px solid transparent;
  border-radius: 8px;
  padding: 5px 10px;
  line-height: 1.4;
}

.dir-pick :deep(.arco-btn) {
  border-radius: 6px;
}

/* 开关(macOS 绿色滑块):与其它设置页一致 */
.settings-row :deep(.arco-switch) {
  border-radius: 999px;
}

.settings-row :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.settings-row :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}

/* 多行文本输入：与设置页其它输入同一套主题令牌，书写区自带描边 */
.editor-wrap {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.warn {
  font-size: var(--font-size-xs);
  color: rgb(var(--warning-6));
}

.page-footer {
  flex-shrink: 0;
  padding: 8px 0 0;
}

.footer-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.footer-actions :deep(.arco-btn) {
  border-radius: 6px;
}

.footer-actions :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}
</style>
