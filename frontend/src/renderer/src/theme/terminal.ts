/**
 * 终端配色方案(xterm.js theme):「跟随主题」随应用明暗/主题色实时解析,
 * 其余为固定经典终端配色(黑/亮/奶油/磷光绿/深海蓝)。
 * 设置页「系统设置 → 外观 → 终端风格」选择,TerminalPanel 消费。
 */

export type TerminalStyleId = 'follow' | 'classic' | 'light' | 'cream' | 'green' | 'arc'

/** xterm.js ITheme 所需的配色对象(含 16 色 ANSI 调色板) */
export interface TerminalScheme {
  background: string
  foreground: string
  cursor: string
  cursorAccent: string
  selectionBackground: string
  black: string
  red: string
  green: string
  yellow: string
  blue: string
  magenta: string
  cyan: string
  white: string
  brightBlack: string
  brightRed: string
  brightGreen: string
  brightYellow: string
  brightBlue: string
  brightMagenta: string
  brightCyan: string
  brightWhite: string
}

/** 设置页选项元信息(预览色块用) */
export interface TerminalStyleOption {
  id: TerminalStyleId
  titleKey: string
  previewBg: string
  previewFg: string
}

/** 读取 CSS 变量(跟随主题时取应用当前的背景/文字/强调色) */
function cssVar(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}

/** 是否为暗色主题(html[data-theme='dark']) */
function isDarkTheme(): boolean {
  return document.documentElement.getAttribute('data-theme') === 'dark'
}

/** 经典黑:纯黑底 + Windows 终端风格 16 色 */
const CLASSIC: TerminalScheme = {
  background: '#0c0c0c',
  foreground: '#cccccc',
  cursor: '#ffffff',
  cursorAccent: '#0c0c0c',
  selectionBackground: 'rgba(86, 156, 214, 0.4)',
  black: '#000000',
  red: '#cd3131',
  green: '#0dbc79',
  yellow: '#e5e510',
  blue: '#2472c8',
  magenta: '#bc3fbc',
  cyan: '#11a8cd',
  white: '#e5e5e5',
  brightBlack: '#666666',
  brightRed: '#f14c4c',
  brightGreen: '#23d18b',
  brightYellow: '#f5f543',
  brightBlue: '#3b8eea',
  brightMagenta: '#d670d6',
  brightCyan: '#29b8db',
  brightWhite: '#e5e5e5'
}

/** 明亮:白底深字(浅色应用下的终端) */
const LIGHT: TerminalScheme = {
  background: '#ffffff',
  foreground: '#000000',
  cursor: '#000000',
  cursorAccent: '#ffffff',
  selectionBackground: 'rgba(22, 93, 255, 0.22)',
  black: '#000000',
  red: '#c0342d',
  green: '#0f7b3c',
  yellow: '#8f6b00',
  blue: '#2254d7',
  magenta: '#7a2eb0',
  cyan: '#0a6e75',
  white: '#9aa1ab',
  brightBlack: '#4e5969',
  brightRed: '#d93a34',
  brightGreen: '#178a47',
  brightYellow: '#a07a00',
  brightBlue: '#2f61e0',
  brightMagenta: '#8b38c0',
  brightCyan: '#0c7d85',
  brightWhite: '#c9cdd4'
}

/** 奶油:暖色纸感底,低饱和棕灰字 */
const CREAM: TerminalScheme = {
  background: '#f5efe3',
  foreground: '#403a2e',
  cursor: '#8a5a00',
  cursorAccent: '#f5efe3',
  selectionBackground: 'rgba(181, 138, 0, 0.25)',
  black: '#403a2e',
  red: '#b0413e',
  green: '#5f7a2d',
  yellow: '#9a6b00',
  blue: '#4a6fb5',
  magenta: '#8a5a9d',
  cyan: '#2f7f76',
  white: '#a9a294',
  brightBlack: '#77705f',
  brightRed: '#c2504c',
  brightGreen: '#6f8f35',
  brightYellow: '#b37e00',
  brightBlue: '#577fcc',
  brightMagenta: '#9d6ab2',
  brightCyan: '#37918a',
  brightWhite: '#e8e2d3'
}

/** 磷光绿:深绿底色荧光绿字 */
const GREEN: TerminalScheme = {
  background: '#001a00',
  foreground: '#33ff66',
  cursor: '#33ff66',
  cursorAccent: '#001a00',
  selectionBackground: 'rgba(51, 255, 102, 0.3)',
  black: '#000000',
  red: '#f14c4c',
  green: '#0dbc79',
  yellow: '#b8bb00',
  blue: '#3b8eea',
  magenta: '#bc3fbc',
  cyan: '#21c6c6',
  white: '#c8c8c8',
  brightBlack: '#2e7d32',
  brightRed: '#ff6b6b',
  brightGreen: '#33ff66',
  brightYellow: '#e5f543',
  brightBlue: '#66b3ff',
  brightMagenta: '#e066e0',
  brightCyan: '#4de6e6',
  brightWhite: '#f0f0f0'
}

/** 深海蓝:深蓝底冰蓝字 */
const ARC: TerminalScheme = {
  background: '#0b1d33',
  foreground: '#dfe7f2',
  cursor: '#4da3ff',
  cursorAccent: '#0b1d33',
  selectionBackground: 'rgba(77, 163, 255, 0.3)',
  black: '#10263f',
  red: '#ff5f56',
  green: '#3fdd78',
  yellow: '#f5c542',
  blue: '#4da3ff',
  magenta: '#c678dd',
  cyan: '#2dd4bf',
  white: '#c9d6e6',
  brightBlack: '#4a5f78',
  brightRed: '#ff7b72',
  brightGreen: '#5be08f',
  brightYellow: '#f8d469',
  brightBlue: '#74b9ff',
  brightMagenta: '#d99ae9',
  brightCyan: '#5ce6d3',
  brightWhite: '#e8eef7'
}

/** 各固定方案的完整配色 */
const FIXED: Record<Exclude<TerminalStyleId, 'follow'>, TerminalScheme> = {
  classic: CLASSIC,
  light: LIGHT,
  cream: CREAM,
  green: GREEN,
  arc: ARC
}

/** 跟随主题:背景/文字取自应用 CSS 变量,强调色为 --accent-text;ANSI 调色板按明暗选用。
 *  背景用 --panel-bg(与右侧页签区底色一致),终端与内容区无缝铺满 */
function followScheme(): TerminalScheme {
  const accent = cssVar('--accent-text', '#165dff')
  const bg = cssVar('--panel-bg', '#0c0c0c')
  const palette = isDarkTheme() ? CLASSIC : LIGHT
  return {
    background: bg,
    // 前景色:浅色应用取纯黑、深色应用取亮白,保证最高对比(不再跟随 --text-primary 的偏浅灰)
    foreground: isDarkTheme() ? '#f2f3f5' : '#000000',
    cursor: accent,
    cursorAccent: bg,
    selectionBackground: isDarkTheme() ? 'rgba(48, 111, 255, 0.35)' : 'rgba(22, 93, 255, 0.2)',
    black: palette.black,
    red: palette.red,
    green: palette.green,
    yellow: palette.yellow,
    blue: palette.blue,
    magenta: palette.magenta,
    cyan: palette.cyan,
    white: palette.white,
    brightBlack: palette.brightBlack,
    brightRed: palette.brightRed,
    brightGreen: palette.brightGreen,
    brightYellow: palette.brightYellow,
    brightBlue: palette.brightBlue,
    brightMagenta: palette.brightMagenta,
    brightCyan: palette.brightCyan,
    brightWhite: palette.brightWhite
  }
}

/** 解析指定风格为 xterm 配色(跟随主题即时读 CSS 变量) */
export function resolveTerminalScheme(id: TerminalStyleId): TerminalScheme {
  return id === 'follow' ? followScheme() : FIXED[id]
}

/** 设置页选项列表(预览色块为该风格的背景/前景) */
export const TERMINAL_STYLE_OPTIONS: TerminalStyleOption[] = [
  { id: 'follow', titleKey: 'settings.terminalStyle.follow', previewBg: cssVar('--panel-bg', '#0c0c0c'), previewFg: cssVar('--text-primary', '#f2f3f5') },
  { id: 'classic', titleKey: 'settings.terminalStyle.classic', previewBg: CLASSIC.background, previewFg: CLASSIC.foreground },
  { id: 'light', titleKey: 'settings.terminalStyle.light', previewBg: LIGHT.background, previewFg: LIGHT.foreground },
  { id: 'cream', titleKey: 'settings.terminalStyle.cream', previewBg: CREAM.background, previewFg: CREAM.foreground },
  { id: 'green', titleKey: 'settings.terminalStyle.green', previewBg: GREEN.background, previewFg: GREEN.foreground },
  { id: 'arc', titleKey: 'settings.terminalStyle.arc', previewBg: ARC.background, previewFg: ARC.foreground }
]