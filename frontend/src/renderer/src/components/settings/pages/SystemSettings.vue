<script setup lang="ts">
import SettingsSection from '../SettingsSection.vue'
import { ref, reactive, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSettingsStore } from '@/stores/settings'
import { invoke } from '@/services/backend'
import { Message } from '@arco-design/web-vue'
import { TERMINAL_STYLE_OPTIONS } from '@/theme/terminal'
import type { TerminalStyleId } from '@/theme/terminal'
import { pickDirectory } from '@/composables/useDirectoryPicker'
import type { AppSettings, WebSearchSettings } from '@/services/dto'

const { t } = useI18n()
const settings = useSettingsStore()

const workspaceDir = ref('')
/** 应用系统目录（config/data/log/plugins 的父目录；Web 形态无此概念，保持空并隐藏该行） */
const appSystemDir = ref('')
const saving = ref(false)

/** 系统权限:状态由主进程查询(macOS 真实状态;其他平台 unsupported,引导到系统设置) */
type PermissionItem = Awaited<ReturnType<typeof window.api.permission.status>>[number]
const permissions = ref<PermissionItem[]>([])
const permissionsLoading = ref(false)
const isMacPlatform = window.api?.platform === 'darwin'

/** 网络搜索配置(与 Rust AppConfig.webSearch 对齐);多引擎免 key 抓取,失败按内置顺序(国内优先)自动回退 */
const WEB_SEARCH_DEFAULTS: WebSearchSettings = { defaultEngine: 'baidu', timeoutSec: 30 }
const webSearch = reactive<WebSearchSettings>({ ...WEB_SEARCH_DEFAULTS })
/** 默认引擎可选项(顺序即设置页展示顺序,国内优先) */
const ENGINE_OPTIONS = ['baidu', 'sogou', 'bing', 'duckduckgo', 'google'] as const

function onWindowFocus(): void {
  void loadPermissions()
}

async function loadPermissions(): Promise<void> {
  if (!window.api?.permission) return
  permissionsLoading.value = true
  try {
    permissions.value = await window.api.permission.status()
  } catch {
    permissions.value = []
  } finally {
    permissionsLoading.value = false
  }
}

async function openPermissionSettings(kind: PermissionItem['kind']): Promise<void> {
  if (!window.api?.permission) return
  const ok = await window.api.permission.openSettings(kind)
  if (!ok) Message.info(t('settings.system.permissionManual'))
}

function permissionStateText(state: PermissionItem['state']): string {
  return t(`settings.system.permState.${state}`)
}

function changeLocale(val: string | number | boolean): void {
  settings.setLocale(val as 'zh-CN' | 'en-US')
}

function changeTheme(val: string | number | boolean): void {
  settings.setThemeMode(val as 'light' | 'dark' | 'system')
}

function changeTerminalStyle(val: unknown): void {
  settings.setTerminalStyle((Array.isArray(val) ? val[0] : val) as TerminalStyleId)
}

/** 选择默认会话空间基目录(相对日期工作区归入其下;会话自身存储的路径不受切换影响) */
async function pickWorkspace(): Promise<void> {
  // 桌面=原生目录对话框；浏览器部署=应用内模态（服务端目录浏览）
  const dir = await pickDirectory()
  if (dir) workspaceDir.value = dir
}

async function loadAll(): Promise<void> {
  try {
    const s = (await invoke('get_settings')) as AppSettings
    workspaceDir.value = s.workspace ?? ''
    if (s.webSearch) Object.assign(webSearch, s.webSearch)
  } catch {
    /* 保持默认值 */
  }
}

/** 应用系统目录只读路径（桌面壳才有；Web 形态保持空） */
async function loadAppSystemDir(): Promise<void> {
  try {
    appSystemDir.value = (await window.api?.system?.appDir?.()) ?? ''
  } catch {
    appSystemDir.value = ''
  }
}

/** 用系统文件管理器打开应用系统目录 */
async function openAppSystemDir(): Promise<void> {
  if (!appSystemDir.value || !window.api?.fs?.openFolder) return
  const err = await window.api.fs.openFolder(appSystemDir.value)
  if (err) Message.error(err)
}

async function saveAll(): Promise<void> {
  saving.value = true
  try {
    const s = (await invoke('get_settings')) as AppSettings
    s.workspace = workspaceDir.value
    s.webSearch = { ...webSearch }
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
  loadAppSystemDir()
  loadPermissions()
  window.addEventListener('focus', onWindowFocus)
})

onUnmounted(() => {
  window.removeEventListener('focus', onWindowFocus)
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.system.title') }}</h2>
    </div>
    <div class="page-body">
      <!-- Appearance -->
      <SettingsSection>
        <template #title>
          {{ t('settings.system.appearance') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.themeMode') }}</span>
              <span class="row-desc">{{ t('settings.system.themeModeDesc') }}</span>
            </div>
            <a-radio-group :model-value="settings.themeMode" type="button" @change="changeTheme">
              <a-radio value="light">{{ t('settings.system.themeLight') }}</a-radio>
              <a-radio value="dark">{{ t('settings.system.themeDark') }}</a-radio>
              <a-radio value="system">{{ t('settings.system.themeAuto') }}</a-radio>
            </a-radio-group>
          </div>
          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.language') }}</span>
              <span class="row-desc">{{ t('settings.system.languageDesc') }}</span>
            </div>
            <a-radio-group :model-value="settings.locale" type="button" @change="changeLocale">
              <a-radio value="zh-CN">{{ t('settings.system.localeZh') }}</a-radio>
              <a-radio value="en-US">{{ t('settings.system.localeEn') }}</a-radio>
            </a-radio-group>
          </div>
          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.terminalStyle') }}</span>
              <span class="row-desc">{{ t('settings.system.terminalStyleDesc') }}</span>
            </div>
            <a-select :model-value="settings.terminalStyle" :style="{ width: '210px' }" @change="changeTerminalStyle">
              <a-option v-for="opt in TERMINAL_STYLE_OPTIONS" :key="opt.id" :value="opt.id">
                <span class="term-style-opt">
                  <span class="term-style-swatch" :style="{ background: opt.previewBg, color: opt.previewFg }">Aa</span>
                  {{ t(opt.titleKey) }}
                </span>
              </a-option>
            </a-select>
          </div>

      </SettingsSection>

      <!-- 电源:保持电脑运行(桌面端全局,主进程 powerSaveBlocker) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.system.power') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.keepAwake') }}</span>
              <span class="row-desc">{{ t('settings.system.keepAwakeDesc') }}</span>
            </div>
            <a-switch v-model="settings.keepAwake" />
          </div>

      </SettingsSection>

      <!-- 系统权限授权(跨平台:macOS 可查询/请求,其他平台引导到系统设置) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.system.permissions') }}
        </template>

          <div v-if="!isMacPlatform" class="permission-hint">
            {{ t('settings.system.permissionPlatformHint') }}
          </div>
          <div v-if="permissionsLoading && !permissions.length" class="permission-hint">
            {{ t('common.loading') }}
          </div>
          <div v-if="permissions.length" class="permission-list">
            <div v-for="p in permissions" :key="p.kind" class="permission-item">
              <div class="permission-info">
                <span class="permission-name">{{ t(`settings.system.perm.${p.kind}`) }}</span>
                <span class="permission-desc">{{ t(`settings.system.perm.${p.kind}Desc`) }}</span>
              </div>
              <div class="permission-actions">
                <span class="permission-badge" :class="`perm-${p.state}`">
                  {{ permissionStateText(p.state) }}
                </span>
                <button
                  v-if="p.canOpenSettings"
                  type="button"
                  class="permission-btn"
                  @click="openPermissionSettings(p.kind)"
                >
                  {{ t('settings.system.permissionOpen') }}
                </button>
              </div>
            </div>
          </div>

      </SettingsSection>

      <!-- 默认会话空间(后端 AppConfig.workspace:相对日期工作区归入其下) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.system.workspace') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.workspaceDir') }}</span>
              <span class="row-desc">{{ t('settings.system.workspaceDirDesc') }}</span>
            </div>
            <div class="dir-pick">
              <span class="dir-path" :title="workspaceDir">{{ workspaceDir || '—' }}</span>
              <a-button size="small" @click="pickWorkspace">{{ t('settings.system.workspacePick') }}</a-button>
            </div>
          </div>

          <!-- 应用系统目录：只读展示 + 用系统文件管理器打开 -->
          <div v-if="appSystemDir" class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.appDir') }}</span>
              <span class="row-desc">{{ t('settings.system.appDirDesc') }}</span>
            </div>
            <div class="dir-pick">
              <span class="dir-path" :title="appSystemDir">{{ appSystemDir }}</span>
              <a-button size="small" @click="openAppSystemDir">{{ t('settings.system.appDirOpen') }}</a-button>
            </div>
          </div>

      </SettingsSection>

      <!-- 网络搜索(后端 AppConfig.webSearch:默认引擎;多引擎免 key 抓取,失败自动回退) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.system.webSearch') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.system.webSearchEngine') }}</span>
              <span class="row-desc">{{ t('settings.system.webSearchEngineDesc') }}</span>
            </div>
            <a-select v-model="webSearch.defaultEngine" :style="{ width: '210px' }">
              <a-option v-for="eng in ENGINE_OPTIONS" :key="eng" :value="eng">
                {{ t(`settings.system.webSearchEngines.${eng}`) }}
              </a-option>
            </a-select>
          </div>

      </SettingsSection>
    </div>

    <div class="page-footer">
      <div class="footer-actions">
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
  /* 预占滚动条位置,内容增删时视图不晃动 */
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

/* ---------- 系统权限授权(macOS 分组列表) ---------- */
.permission-hint {
  font-size: var(--font-size-code);
  line-height: 1.5;
  color: var(--text-tertiary);
}

/* 权限列表:与其它设置模块同款(无外框,纯留白分行的行式列表) */
.permission-list {
  display: flex;
  flex-direction: column;
}

.permission-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 8px 0;
}

.permission-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.permission-name {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}

.permission-desc {
  font-size: var(--font-size-code);
  color: var(--text-secondary);
}

.permission-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

/* 权限状态徽章:macOS 小圆角标签(高度与按钮一致;纯填充分层,不描边) */
.permission-badge {
  display: inline-flex;
  align-items: center;
  height: 24px;
  padding: 0 10px;
  border: 1px solid transparent;
  border-radius: 6px;
  font-size: var(--font-size-xs);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
}

.perm-granted {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 14%, transparent);
}

.perm-denied,
.perm-restricted {
  color: var(--danger);
  background: color-mix(in srgb, var(--danger) 14%, transparent);
}

.perm-not-determined,
.perm-unknown,
.perm-unsupported {
  color: var(--text-secondary);
  background: var(--fill-1);
}

/* 行内小按钮:与全局输入/按钮同款无边框浅填充,悬浮加深 */
.permission-btn {
  height: 24px;
  padding: 0 12px;
  border: none;
  border-radius: 6px;
  background: var(--fill-1);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.permission-btn:hover {
  background: var(--fill-2);
}

/* 默认会话空间目录:路径超长省略,悬浮显示完整 */
.dir-pick {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

/* 默认会话空间目录:路径超长省略,悬浮显示完整;macOS 柔和填充 + 圆角 */
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

/* 终端风格下拉:选项内嵌小色块(该风格背景/前景) */
.term-style-opt {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.term-style-swatch {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 16px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  line-height: 1;
  border: 1px solid var(--border-strong);
  flex-shrink: 0;
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

/* ---------- macOS 风格控件覆盖 ---------- */

/* 分段控件(主题模式 / 语言):浅灰胶囊容器 + 选中白色胶囊 + 阴影 + 系统蓝文字 */
.settings-row :deep(.arco-radio-group-button) {
  padding: 2px;
  background-color: var(--color-fill-3) !important;
  border-radius: 8px;
}

.settings-row :deep(.arco-radio-group-button .arco-radio-button) {
  margin: 2px;
  line-height: 24px;
  border-radius: 6px;
  color: var(--color-text-2) !important;
}

.settings-row :deep(.arco-radio-group-button .arco-radio-button:hover) {
  color: var(--color-text-1) !important;
  background-color: var(--color-fill-4) !important;
}

.settings-row :deep(.arco-radio-group-button .arco-radio-button.arco-radio-checked) {
  color: var(--accent-text) !important;
  background-color: var(--color-bg-5) !important;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

/* 开关(macOS 绿色滑块):圆角胶囊轨道 + 白色圆钮 */
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

/* 数字输入框 / 下拉:更柔和的 macOS 圆角与浅边框 */
.settings-row :deep(.arco-input-number),
.settings-row :deep(.arco-select) {
  border-radius: 6px;
}

/* 底部主按钮(保存):macOS 系统蓝填充 + 白字 */
.footer-actions :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* 底部操作按钮:macOS 圆角 */
.footer-actions :deep(.arco-btn) {
  border-radius: 6px;
}
</style>
