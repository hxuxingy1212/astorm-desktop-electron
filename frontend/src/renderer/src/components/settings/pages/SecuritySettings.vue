<script setup lang="ts">
import SettingsSection from '../SettingsSection.vue'
import SettingsConfigFileRow from '../SettingsConfigFileRow.vue'
import { reactive, ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@/services/backend'
import { Message } from '@arco-design/web-vue'
import { isWebShell } from '@/utils/platform'
import JsonEditorPane from '@/components/common/JsonEditorPane.vue'
import type { AppSettings, SecuritySettings } from '@/services/dto'

const { t } = useI18n()
const saving = ref(false)

/** 去掉 Electron IPC 包装，页面上只显示真正的错误原因 */
function humanError(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e)
  return text.replace(/^Error invoking remote method '[^']+':\s*(?:Error:\s*)?/, '')
}

/** 后端真实支持的安全配置默认值(**仅兜底**:`invoke('security_defaults')` 取不到时用) */
const DEFAULTS: SecuritySettings = {
  enabled: true,
  file: { readBlack: [], readWhite: [], writeBlack: [], writeWhite: [] },
  net: { useWhitelist: true, inBlack: [], inWhite: [], outBlack: [], outWhite: [] },
  danger: { useBuiltin: true, builtinPatterns: null, black: [] },
  resource: { memoryMb: 4096, diskMb: 1024, cpuSeconds: 600, cpuPercent: 95, gpu: true },
  toolchainDisabled: [],
  execWhite: []
}

/**
 * 后端默认安全配置(**未配置时的初值底座**)。
 *
 * 默认值里有非空项(file.readBlack/writeBlack = 凭据路径清单),前端常量兜底无法表达,
 * 必须由后端导出:设置页以它为底、把已保存配置叠加其上(已保存优先);用户从没保存过
 * 安全配置时,凭据清单保留在本地配置文件中(用户显式清空 = 关闭该防护)。
 */
const backendDefaults = ref<SecuritySettings>(JSON.parse(JSON.stringify(DEFAULTS)) as SecuritySettings)

async function loadDefaults(): Promise<void> {
  try {
    const raw = (await invoke('security_defaults')) as unknown
    const parsed = (typeof raw === 'string' ? JSON.parse(raw) : raw) as Partial<SecuritySettings> | null
    if (parsed && typeof parsed === 'object' && parsed.file) {
      backendDefaults.value = {
        ...DEFAULTS,
        ...parsed,
        file: { ...DEFAULTS.file, ...parsed.file },
        net: { ...DEFAULTS.net, ...(parsed.net ?? {}) },
        danger: { ...DEFAULTS.danger, ...(parsed.danger ?? {}) },
        resource: { ...DEFAULTS.resource, ...(parsed.resource ?? {}) },
        toolchainDisabled: Array.isArray(parsed.toolchainDisabled)
          ? parsed.toolchainDisabled
          : DEFAULTS.toolchainDisabled,
        execWhite: Array.isArray(parsed.execWhite) ? parsed.execWhite : DEFAULTS.execWhite
      } as SecuritySettings
    }
  } catch {
    /* 后端未加载 / 命令未注册:退回常量兜底(空表) */
  }
}

const sec = reactive<SecuritySettings>(JSON.parse(JSON.stringify(DEFAULTS)))

const configPath = ref('')
const openingConfig = ref(false)

/** 页内 JSON 编辑态：不弹窗，直接把设置内容替换成编辑器 */
const editingConfig = ref(false)
const configText = ref('')
const configLoading = ref(false)
const savingConfig = ref(false)
const configError = ref('')

/** 独立安全配置文件（config/security.json）路径；名单/开关/资源限制都在这个文件里。 */
async function loadConfigPath(): Promise<void> {
  try {
    configPath.value = (await invoke('security_config_file')).path
  } catch (e) {
    Message.error(humanError(e))
  }
}

async function openConfig(): Promise<void> {
  openingConfig.value = true
  try {
    await invoke('open_security_config_file')
  } catch (e) {
    Message.error(humanError(e))
  } finally {
    openingConfig.value = false
  }
}

/** 进入页内 JSON 编辑器（文件不存在时后端会先按当前生效值创建） */
async function openConfigEditor(): Promise<void> {
  editingConfig.value = true
  configLoading.value = true
  configError.value = ''
  try {
    const r = (await invoke('read_security_config_file')) as { path: string; content: string }
    configPath.value = r.path
    configText.value = r.content
  } catch (e) {
    configError.value = humanError(e)
  } finally {
    configLoading.value = false
  }
}

function closeConfigEditor(): void {
  editingConfig.value = false
  configError.value = ''
}

async function saveConfigText(): Promise<void> {
  savingConfig.value = true
  configError.value = ''
  try {
    await invoke('write_security_config_file', { content: configText.value })
    Message.success(t('common.saveSuccess'))
    editingConfig.value = false
    await loadAll()
  } catch (e) {
    configError.value = humanError(e)
  } finally {
    savingConfig.value = false
  }
}

/** 格式化编辑器里的 JSON（解析失败时只提示，不改内容） */
function formatConfig(): void {
  try {
    const parsed: unknown = JSON.parse(configText.value)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      configError.value = t('settings.security.jsonMustBeObject')
      return
    }
    configText.value = JSON.stringify(parsed, null, 2)
    configError.value = ''
  } catch (e) {
    configError.value = t('settings.security.jsonInvalid', {
      error: e instanceof Error ? e.message : String(e)
    })
  }
}

/** Arco input-number 空值(undefined) 与「不限(null)」互转 */
function toNumOrNull(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

async function loadAll(): Promise<void> {
  await loadDefaults()
  // 先落到「后端默认配置」:用户从没保存过安全配置时,get_settings 里的空表不会把
  // 凭据默认清单(readBlack/writeBlack)覆盖掉;已保存配置随后叠加(优先,含显式空表 = 清空)
  Object.assign(sec, JSON.parse(JSON.stringify(backendDefaults.value)))
  try {
    const s = (await invoke('get_settings')) as AppSettings
    if (s.security) {
      sec.enabled = s.security.enabled
      Object.assign(sec.file, s.security.file)
      Object.assign(sec.net, s.security.net)
      Object.assign(sec.danger, s.security.danger)
      Object.assign(sec.resource, s.security.resource)
      // 非本页编辑项;保留以免保存时覆盖「本地环境」区块维护的禁用列表
      sec.toolchainDisabled = Array.isArray(s.security.toolchainDisabled)
        ? s.security.toolchainDisabled
        : []
    }
  } catch {
    /* 保持默认值 */
  }
  // 云端 Web UI 没有「本机配置文件」概念，不调用桌面命令（配置行也隐藏）
  if (!isWebShell) await loadConfigPath()
}

function resetAll(): void {
  // 重置页面可编辑控件；文件中的名单仍由本地配置文件管理。
  Object.assign(sec, JSON.parse(JSON.stringify(backendDefaults.value)))
  Message.info(t('settings.security.resetDone'))
}

async function saveAll(): Promise<void> {
  saving.value = true
  try {
    const s = (await invoke('get_settings')) as AppSettings
    // 名单由本地文件管理，保存前读取最新配置，只更新页面可编辑的字段。
    const latest = s.security ?? backendDefaults.value
    s.security = {
      ...latest,
      enabled: sec.enabled,
      net: { ...latest.net, useWhitelist: sec.net.useWhitelist },
      danger: { ...latest.danger, useBuiltin: sec.danger.useBuiltin },
      resource: { ...sec.resource }
    }
    await invoke('save_settings', { settings: s })
    Message.success(t('common.saveSuccess'))
  } catch (e) {
    Message.error(humanError(e))
  } finally {
    saving.value = false
  }
}

onMounted(() => {
  loadAll()
})
</script>

<template>
  <!-- 页内 JSON 编辑（不弹窗）：整页替换为编辑器，保存/取消在底部 -->
  <div v-if="editingConfig" class="page config-editor-page">
    <div class="page-header">
      <div class="editor-heading">
        <h2 class="page-title">{{ t('settings.security.editConfigTitle') }}</h2>
        <span class="config-editor-path">{{ configPath }}</span>
        <span class="config-editor-hint">{{ t('settings.security.editConfigHint') }}</span>
      </div>
    </div>
    <div class="page-body config-editor-body">
      <div v-if="configError" class="config-error">{{ configError }}</div>
      <div v-if="configLoading" class="config-loading">{{ t('common.loading') }}</div>
      <JsonEditorPane v-else v-model="configText" />
    </div>
    <div class="page-footer">
      <div class="footer-actions">
        <a-button size="small" @click="formatConfig">
          {{ t('settings.security.formatConfig') }}
        </a-button>
        <a-button size="small" @click="closeConfigEditor">{{ t('common.cancel') }}</a-button>
        <a-button type="primary" size="small" :loading="savingConfig" @click="saveConfigText">
          {{ t('common.save') }}
        </a-button>
      </div>
    </div>
  </div>

  <div v-else class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.security.title') }}</h2>
    </div>

    <div class="page-body">
      <!-- 防护总开关 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.security.general') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.enabled') }}</div>
              <div class="row-desc">{{ t('settings.security.enabledDesc') }}</div>
            </div>
            <a-switch v-model="sec.enabled" />
          </div>

      </SettingsSection>

      <!-- 系统自动注入（只读；引导用户去本地环境页禁用工具注入） -->
      <SettingsSection>
        <template #title>
          {{ t('settings.security.autoInjected') }}
        </template>

          <SettingsConfigFileRow
            :path="configPath"
            :loading="openingConfig"
            @open="openConfig"
            @edit="openConfigEditor"
          />

      </SettingsSection>

      <!-- 文件访问名单 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.security.file') }}
        </template>

          <SettingsConfigFileRow
            :path="configPath"
            :loading="openingConfig"
            @open="openConfig"
            @edit="openConfigEditor"
          />

      </SettingsSection>

      <!-- 网络访问名单 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.security.net') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.netUseWhitelist') }}</div>
              <div class="row-desc">{{ t('settings.security.netUseWhitelistDesc') }}</div>
            </div>
            <a-switch v-model="sec.net.useWhitelist" />
          </div>
          <SettingsConfigFileRow
            :path="configPath"
            :loading="openingConfig"
            @open="openConfig"
            @edit="openConfigEditor"
          />

      </SettingsSection>

      <!-- 危险命令黑名单 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.security.danger') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.dangerUseBuiltin') }}</div>
              <div class="row-desc">{{ t('settings.security.dangerUseBuiltinDesc') }}</div>
            </div>
            <a-switch v-model="sec.danger.useBuiltin" />
          </div>
          <SettingsConfigFileRow
            :path="configPath"
            :loading="openingConfig"
            @open="openConfig"
            @edit="openConfigEditor"
          />


      </SettingsSection>

      <!-- 资源限制 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.security.resource') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.memoryMb') }}</div>
              <div class="row-desc">{{ t('settings.security.memoryMbDesc') }}</div>
            </div>
            <a-input-number
              :model-value="sec.resource.memoryMb ?? undefined"
              @update:model-value="(v: unknown) => (sec.resource.memoryMb = toNumOrNull(v))"
              :min="64"
              :max="1048576"
              :step="256"
              size="small"
              class="res-input"
            />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.diskMb') }}</div>
              <div class="row-desc">{{ t('settings.security.diskMbDesc') }}</div>
            </div>
            <a-input-number
              :model-value="sec.resource.diskMb ?? undefined"
              @update:model-value="(v: unknown) => (sec.resource.diskMb = toNumOrNull(v))"
              :min="16"
              :max="1048576"
              :step="64"
              size="small"
              class="res-input"
            />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.cpuPercent') }}</div>
              <div class="row-desc">{{ t('settings.security.cpuPercentDesc') }}</div>
            </div>
            <a-input-number
              :model-value="sec.resource.cpuPercent ?? undefined"
              @update:model-value="(v: unknown) => (sec.resource.cpuPercent = toNumOrNull(v))"
              :min="5"
              :max="100"
              :step="5"
              size="small"
              class="res-input"
            />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.cpuSeconds') }}</div>
              <div class="row-desc">{{ t('settings.security.cpuSecondsDesc') }}</div>
            </div>
            <a-input-number
              :model-value="sec.resource.cpuSeconds ?? undefined"
              @update:model-value="(v: unknown) => (sec.resource.cpuSeconds = toNumOrNull(v))"
              :min="1"
              :max="86400"
              :step="10"
              size="small"
              class="res-input"
            />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <div class="row-title">{{ t('settings.security.gpu') }}</div>
              <div class="row-desc">{{ t('settings.security.gpuDesc') }}</div>
            </div>
            <a-switch v-model="sec.resource.gpu" />
          </div>

      </SettingsSection>
    </div>

    <div class="page-footer">
      <div class="footer-actions">
        <a-button size="small" @click="resetAll">{{ t('settings.security.reset') }}</a-button>
        <a-button type="primary" size="small" :loading="saving" @click="saveAll">
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
  /* 预占滚动条位置,内容增删时视图不晃动 */
  scrollbar-gutter: stable;
}

.page-footer {
  flex-shrink: 0;
  padding: 8px 0 0;
  margin-top: 0;
}

.footer-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
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

/* ---------- 系统自动注入(只读) ---------- */
.info-icon {
  color: var(--text-tertiary);
  cursor: pointer;
  flex-shrink: 0;
  transition: color 0.15s;
}

.info-icon:hover {
  color: var(--text-secondary);
}

.goto-hint {
  max-width: 280px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.goto-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.goto-desc {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  line-height: 1.5;
}

/* 资源限制输入框:macOS 浅底无边框、加宽 */
.res-input {
  width: 190px;
}

.res-input :deep(.arco-input),
.res-input :deep(.arco-input-number) {
  border-radius: 8px;
}

.res-input :deep(.arco-input-number-input) {
  background-color: var(--color-fill-2);
}

/* ---------- 页内 JSON 编辑器 ---------- */
.config-editor-page .page-header {
  align-items: flex-start;
}

.editor-heading {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.config-editor-path {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  overflow-wrap: anywhere;
}

.config-editor-hint {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

.page-body.config-editor-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
  padding: 0 0 12px;
  overflow: hidden;
}

.config-error {
  flex-shrink: 0;
  color: var(--danger);
  font-size: var(--font-size-sm);
}

.config-loading {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

/* ---------- macOS 风格控件覆盖 ---------- */

/* 开关(macOS 绿色滑块):圆角胶囊轨道 + 白色圆钮(与系统设置页完全一致) */
.settings-row :deep(.arco-switch) {
  border-radius: 999px;
}

.settings-row :deep(.arco-switch-checked) {
  background-color: var(--accent-text) !important;
}

.settings-row :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}

.settings-row :deep(.arco-input-number),
.settings-row :deep(.arco-select) {
  border-radius: 6px;
}

.footer-actions :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

.footer-actions :deep(.arco-btn) {
  border-radius: 6px;
}
</style>
