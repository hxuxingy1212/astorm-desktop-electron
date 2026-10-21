/**
 * 应用预览入口：先装 mock 桥，再挂载真实 renderer。
 *
 * `preview-bridge` 必须早于 `@/main` 求值（E 模块按源码顺序执行 side-effect import），
 * 否则 main.ts 会在 `window.api.zoom.setFactor` 处直接抛错。
 */
import './preview-bridge'
import '@/main'

import { useSettingsStore } from '@/stores/settings'
import { useChatPrefsStore } from '@/stores/chatPrefs'
import { applyTheme } from '@/theme'

// 预览默认「执行模式」：新会话页与输入框直接演示完整能力，而不是停在计划模式
useChatPrefsStore().mode = 'execute'

/**
 * 窄屏（手机上的介绍页 iframe）自动收起侧栏：应用本身没有「窗口宽度 → 自动折叠」规则，
 * 260px 侧栏在 390px 视口里会把正文挤成竖排。恢复宽屏时仅在「是由本逻辑折叠的」情况下展开，
 * 不覆盖用户自己的折叠选择。
 */
const settings = useSettingsStore()
const narrowViewport = window.matchMedia('(max-width: 720px)')
let autoCollapsed = false
function syncSidebarForViewport(): void {
  if (narrowViewport.matches) {
    autoCollapsed = true
    settings.setSidebarCollapsed(true)
  } else if (autoCollapsed) {
    autoCollapsed = false
    settings.setSidebarCollapsed(false)
  }
}
syncSidebarForViewport()
narrowViewport.addEventListener('change', syncSidebarForViewport)

/**
 * 介绍页与 iframe 的约定：
 * - `astorm-preview-theme`：切换应用内明暗（沿用真实设置 store，控件文案/Arco 主题一起变）
 * - `astorm-preview-reset`：清掉预览产生的 localStorage 并重载
 */
window.addEventListener('message', (event) => {
  if (event.origin !== window.location.origin) return
  const data = event.data as { type?: string; theme?: string } | null
  if (!data) return
  if (data.type === 'astorm-preview-theme') {
    const mode = data.theme === 'dark' ? 'dark' : 'light'
    const settings = useSettingsStore()
    settings.setThemeMode(mode)
    applyTheme(mode, settings.themePreset)
    return
  }
  if (data.type === 'astorm-preview-reset') {
    try {
      localStorage.clear()
    } catch {
      /* 隐私模式忽略 */
    }
    window.location.reload()
  }
})

window.parent?.postMessage({ type: 'astorm-preview-ready' }, window.location.origin)
