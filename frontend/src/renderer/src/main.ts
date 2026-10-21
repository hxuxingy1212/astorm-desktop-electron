import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ArcoVue from '@arco-design/web-vue'
import ArcoVueIcon from '@arco-design/web-vue/es/icon'
import '@arco-design/web-vue/dist/arco.css'
import 'virtual:uno.css'
import '@/styles/global.css'
import '@/styles/inputs.css'
import '@/styles/settings.css'
import '@/styles/cards.css'
import '@/styles/themes/base.css'
import '@/styles/themes/color-scheme.css'
import '@/theme/tokens.css'
import 'vue-stream-markdown/index.css'
// Arco 弹窗统一 macOS 骨架(`modal-class="mac-modal"`);必须晚于 arco.css 加载才能覆盖其默认值
import '@/components/common/mac-modal.css'
import 'md-editor-v3/lib/style.css'
import 'vue-stream-markdown/theme.css'
import App from './App.vue'
import { hasSidebarGlass } from '@/utils/platform'
import OnboardingApp from './onboarding/OnboardingApp.vue'
import { i18n } from './i18n'
import { applyTheme } from './theme'
import { loadPersistedSettings } from './stores/settings'
// 图标集:手写 SVG 字形(appIcons),覆盖 ArcoVueIcon 的默认注册;模板里的 <icon-xxx /> 不变
import { installAppIcons } from './plugins/appIcons'
import { hydrateMentions } from './components/mentions/registry'
import { installScrollbars } from './services/scrollbars'
import router from './router'

document.documentElement.dataset.sidebarGlass = String(hasSidebarGlass)

// 挂载前先应用持久化的主题与缩放,避免闪烁
const persisted = loadPersistedSettings()
applyTheme(persisted.themeMode ?? 'system', persisted.themePreset ?? 'blue')
window.api.zoom.setFactor(persisted.zoom ?? 1)

const pinia = createPinia()

/**
 * 连接方式:壳子这次加载的目标是**引导页**(首次启动 / 菜单点「连接方式…」)时只挂引导页
 * ——不挂 App.vue、不启任何 store 的会话链路与后端调用,让「还没选本机还是云端」的启动期
 * 绝对干净;其余情况(本机模式)照旧挂完整应用。`welcome` 由主进程按本次加载目标给
 * (preload 同步读)。
 */
const onboardingOnly = window.api?.shellConnection?.info?.welcome === true

if (onboardingOnly) {
  createApp(OnboardingApp)
    .use(pinia)
    .use(ArcoVue)
    .use(ArcoVueIcon)
    .use(installAppIcons)
    .use(i18n)
    .mount('#app')
} else {
  createApp(App)
    .use(router)
    .use(pinia)
    .use(ArcoVue)
    .use(ArcoVueIcon)
    .use(installAppIcons)
    .use(i18n)
    .mount('#app')

  // 启动预取技能/智能体并 seed 提及注册表:刷新后历史消息里的 `$` token 也能渲染为 chip(仅一次,失败静默)
  void hydrateMentions()
}

// 细滚动条行为(空闲隐藏 / 滚动时显示 / 预留占位):挂载后立刻装,与首帧同任务
installScrollbars()
