<script setup lang="ts">
import { computed, onBeforeUnmount, watch } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { useSessionsStore } from '@/stores/sessions'
import { useTabsStore } from '@/stores/tabs'
import { useModelsStore } from '@/stores/models'
import { usePluginsStore } from '@/stores/plugins'
import { usePluginSessionStore } from '@/stores/pluginSession'
import { i18n, getArcoLocale } from '@/i18n'
import { applyTheme } from '@/theme'
import { initTabsSync } from '@/services/tabsSync'
import { initSchedulesSync } from '@/services/schedulesSync'
import { initWebShellLinkRouting } from '@/services/webLinks'
import { initMemorySync } from '@/stores/memory'
import { useZoom } from '@/composables/useZoom'
import { isWebShell } from '@/utils/platform'
import AppLayout from '@/layouts/AppLayout.vue'

const settings = useSettingsStore()
const sessions = useSessionsStore()
const models = useModelsStore()
const tabs = useTabsStore()
const plugins = usePluginsStore()
const pluginSession = usePluginSessionStore()

sessions.init()
sessions.initChatStream()
// 页签状态绑定会话:跟随 activeId 换挡,各会话独立一份页签(须在会话列表就绪后绑定)
tabs.bindSessions()
// 页签 ↔ 后端同步(tabs 工具的镜像上报 + tabs_command 下发;须在 bindSessions 之后)
initTabsSync()
// 浏览器部署:window.open / target=_blank → 内置浏览器页签(桌面由主进程接管,web 壳没有)
initWebShellLinkRouting()
// 定时任务定义变更的实时通道(后端 scheduled_tasks_changed → 定时任务页/看板各自重拉)
initSchedulesSync()
// 记忆编译进度/结果(后端 type:"memory" → 记忆页状态卡与进度条)
initMemorySync()
// 插件会话级状态同样跟随 activeId 换挡(按会话 localStorage 持久化)
pluginSession.bindSessions()
// 有 Rust 后端时用真实会话列表替换 mock 种子;失败保留 mock
void sessions.bootstrapFromBackend()
void models.bootstrap()
// 插件列表(含首次运行自动安装的 demo;侧边栏/工具栏/设置页共用)
// 浏览器部署没有插件运行时（服务端返回“暂未实现”），跳过以免每次加载都报错
if (!isWebShell) void plugins.bootstrap()
useZoom()

// Arco 组件库语言包跟随应用语言
const arcoLocale = computed(() => getArcoLocale(settings.locale))

// 主题:模式/主题色变化时应用;跟随系统时监听系统明暗变化
watch(
  () => [settings.themeMode, settings.themePreset] as const,
  ([mode, preset]) => applyTheme(mode, preset),
  { immediate: true }
)
const systemDarkMedia = window.matchMedia('(prefers-color-scheme: dark)')
const syncSystemTheme = (): void => {
  if (settings.themeMode === 'system') applyTheme('system', settings.themePreset)
}
systemDarkMedia.addEventListener('change', syncSystemTheme)
onBeforeUnmount(() => systemDarkMedia.removeEventListener('change', syncSystemTheme))

// 语言:切换 vue-i18n 全局 locale 与 document.lang
watch(
  () => settings.locale,
  (locale) => {
    i18n.global.locale.value = locale
    document.documentElement.lang = locale
  },
  { immediate: true }
)

// 保持电脑运行:桌面端全局(主进程 powerSaveBlocker);启动即按持久化偏好应用
watch(
  () => settings.keepAwake,
  (enabled) => {
    void window.api?.power?.setKeepAwake(enabled)
  },
  { immediate: true }
)

// 持久化
watch(settings.$state, () => settings.persist(), { deep: true })
// 会话列表节流落盘:流式期间每 token 一次 JSON.stringify + localStorage 会卡死主线程
// （终态 / 标题变更等需要立刻可见的场合由 store 内 flushPersist 立即落盘）
watch(() => sessions.sessions, () => sessions.persistThrottled(), { deep: true })
// 供应商配置:启动 bootstrap 后,变更走防抖同步(表单逐键输入避免每次都写盘+重推 napi)
let modelsPersistTimer: ReturnType<typeof setTimeout> | undefined
watch(
  models.$state,
  () => {
    clearTimeout(modelsPersistTimer)
    modelsPersistTimer = setTimeout(() => models.persist(), 300)
  },
  { deep: true }
)
</script>

<template>
  <a-config-provider :locale="arcoLocale">
    <AppLayout />
  </a-config-provider>
</template>
