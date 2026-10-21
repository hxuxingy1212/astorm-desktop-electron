import { defineStore } from 'pinia'
import type { ThemePresetId } from '@/theme/presets'
import type { TerminalStyleId } from '@/theme/terminal'
import type { LocaleKey } from '@/i18n'

export const SETTINGS_KEY = 'astorm:settings'

/** 侧栏宽度:可拖拽调整,范围可在此配置 */
export const SIDEBAR_MIN_WIDTH = 200
export const SIDEBAR_MAX_WIDTH = 550
export const DEFAULT_SIDEBAR_WIDTH = 260

/** 页签面板宽度:下限 320(拖拽范围由 Workspace 按可用宽度比例动态钳制,见 SPLIT_*) */
export const PANEL_MIN_WIDTH = 320

/** 对话区最小宽度:右侧页签区打开时也不得把对话区压缩到该值以下 */
export const CHAT_MIN_WIDTH = 480

/**
 * 轻量面板的初始宽度:文件树 / 代码审查这类面板不需要把可用宽吃满
 * （默认打开会拉到「聊天区退让到最小」的宽度，占屏近半，对一棵树太宽）。
 * 两档**默认宽度**里的窄档（见 `panels::widthModeOfTab`）：相关资料 / 相关记忆 / 终端 /
 * 代码审查 / 工作区文件 / 会话轨迹 / 插件页 / 普通文件预览等在**激活时**用它；
 * 只有浏览器与 office/PDF 预览的默认宽度是「拉满可用宽度」。拖拽不受档位限制。
 */
export const PANEL_COMPACT_WIDTH = 480

export const DEFAULT_SETTINGS = {
  themeMode: 'system' as 'light' | 'dark' | 'system',
  themePreset: 'blue' as ThemePresetId,
  /** 终端配色风格:follow = 跟随应用明暗/主题色,其余为固定经典方案 */
  terminalStyle: 'follow' as TerminalStyleId,
  locale: 'zh-CN' as LocaleKey,
  zoom: 1,
  /** 侧栏折叠偏好:null = 跟随断点(窄窗口自动折叠);true/false = 用户显式指定,任何宽度都生效 */
  sidebarCollapsed: null as boolean | null,
  sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
  panelWidth: 480,
  /**
   * 右侧页签面板最大化(占满整个内容区,隐藏侧栏与对话区)。
   * **纯运行期状态**,不持久化:重启复位(与页签同为运行期)。
   */
  panelMaximized: false,
  /**
   * 打开网页页签时自动从本机 Chrome 导入 Cookie(自动化控制页可关)。
   * 默认关:macOS 首次导入会弹钥匙串授权框,不该在用户没预期时弹。
   */
  autoImportChromeCookies: false,
  /** 保持电脑运行:开启后阻止系统因空闲休眠(手动睡眠/合盖仍生效),桌面端全局 */
  keepAwake: false
}

export type SettingsState = typeof DEFAULT_SETTINGS

export function loadPersistedSettings(): Partial<SettingsState> {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as Partial<SettingsState>
    // 侧栏折叠不跨启动记忆(启动后侧栏始终展开):持久化值一律丢弃。
    // 必须删掉而不是保留 —— 若保留上次的 true,启动时 store 已是 true 而界面按展开渲染,
    // 再点收缩按钮写回 true 不产生变更,按钮看起来就是坏的(Composable 侧不再有运行期标记兜底)。
    delete parsed.sidebarCollapsed
    // 面板最大化是纯运行期状态:持久化值一律丢弃(重启复位)
    delete parsed.panelMaximized
    // 忽略已持久化的旧 sidebarWidth(旧默认 310),让新默认 260 生效;
    // 用户后续拖拽仍会重新写入(此时以拖拽为准)
    delete parsed.sidebarWidth
    return parsed
  } catch {
    return {}
  }
}

/**
 * 应用偏好设置:主题(明暗/主题色)、语言、缩放、侧栏折叠、页签宽度。
 * 所有字段变更后整体持久化到 localStorage。
 */
export const useSettingsStore = defineStore('settings', {
  state: (): SettingsState => ({ ...DEFAULT_SETTINGS, ...loadPersistedSettings() }),
  actions: {
    setThemeMode(mode: SettingsState['themeMode']): void {
      this.themeMode = mode
    },
    setThemePreset(preset: ThemePresetId): void {
      this.themePreset = preset
    },
    setTerminalStyle(style: TerminalStyleId): void {
      this.terminalStyle = style
    },
    setZoom(zoom: number): void {
      this.zoom = Math.min(2, Math.max(0.5, Math.round(zoom * 100) / 100))
    },
    setLocale(locale: LocaleKey): void {
      this.locale = locale
    },
    /** 显式指定侧栏折叠态(此后任何窗口宽度都遵循,不再受断点限制) */
    setSidebarCollapsed(collapsed: boolean): void {
      this.sidebarCollapsed = collapsed
    },
    setSidebarWidth(width: number): void {
      this.sidebarWidth = Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(width)))
    },
    setPanelWidth(width: number): void {
      // 上限不在此钳制:由 Workspace 按可用宽度的比例动态决定(页签区三态范围)
      this.panelWidth = Math.max(PANEL_MIN_WIDTH, Math.round(width))
    },
    /** 切换右侧面板最大化(占满内容区)。纯运行期,不落盘。 */
    togglePanelMaximized(): void {
      this.panelMaximized = !this.panelMaximized
    },
    setPanelMaximized(v: boolean): void {
      this.panelMaximized = v
    },
    persist(): void {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(this.$state))
    }
  }
})
