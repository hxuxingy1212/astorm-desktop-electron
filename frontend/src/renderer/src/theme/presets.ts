export type ThemePresetId = 'blue' | 'green' | 'purple' | 'orange' | 'cyan'

export interface ThemePreset {
  id: ThemePresetId
  /** 色板圆点展示色 */
  color: string
  /** 主题色名称的 i18n key */
  titleKey: string
}

export const THEME_PRESETS: ThemePreset[] = [
  { id: 'blue', color: '#165dff', titleKey: 'theme.presets.blue' },
  { id: 'green', color: '#00b42a', titleKey: 'theme.presets.green' },
  { id: 'purple', color: '#722ed1', titleKey: 'theme.presets.purple' },
  { id: 'orange', color: '#ff7d00', titleKey: 'theme.presets.orange' },
  { id: 'cyan', color: '#14c9c9', titleKey: 'theme.presets.cyan' }
]
