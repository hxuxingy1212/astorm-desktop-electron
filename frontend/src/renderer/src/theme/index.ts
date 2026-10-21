import type { ThemePresetId } from './presets'

export type ThemeMode = 'light' | 'dark' | 'system'

export function resolveSystemDark(): boolean {
  return window.matchMedia('(prefers-color-scheme: dark)').matches
}

/**
 * 应用主题:
 * - 明暗:body[arco-theme] 驱动 Arco 内置 dark token;
 *   html[data-theme] 驱动自定义设计 token(themes/color-scheme.css)
 * - 主题色:body[data-theme-preset] 驱动 tokens.css 中的主色变量覆盖
 * - 原生侧(主进程 nativeTheme)同步成同一模式 —— macOS 红黄绿失焦色、原生菜单/弹窗
 *   都由它决定,不同步会出现「深色主题 + 浅色系统 → 失焦三个点近黑(看不见)」(见 main/ipc.ts)
 */
export function applyTheme(mode: ThemeMode, preset: ThemePresetId): void {
  const dark = mode === 'system' ? resolveSystemDark() : mode === 'dark'
  if (dark) {
    document.body.setAttribute('arco-theme', 'dark')
    document.documentElement.setAttribute('data-theme', 'dark')
  } else {
    document.body.removeAttribute('arco-theme')
    document.documentElement.setAttribute('data-theme', 'light')
  }
  document.body.dataset.themePreset = preset
  // 非 Electron(浏览器部署)没有这层,静默跳过
  window.api?.appearance?.setThemeMode?.(mode)
}
