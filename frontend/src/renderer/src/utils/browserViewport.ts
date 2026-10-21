/**
 * 内置浏览器页签的缩放（唯一事实源）。
 *
 * 页签状态里存**精确百分比**——画布四角的拖拽锚点可以产生任意值；
 * `ZOOM_LEVELS` 只用于 ⋯ 菜单的 − / + 步进档位（以及 Office 预览的缩放下拉）。
 * 后端 `tabs` 工具同样按百分比收发，无需档位表。
 */
import type { WebViewportState } from '@/types'

export const ZOOM_LEVELS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 2]

/** 拖拽 / 入参的百分比边界（步进档位仍只在 ZOOM_LEVELS 内） */
export const ZOOM_MIN_PERCENT = 25
export const ZOOM_MAX_PERCENT = 200

/** 百分比落库前归一：取整并夹在边界内；非法值回落到 100% */
export function clampZoomPercent(percent: number): number {
  if (!Number.isFinite(percent)) return 100
  return Math.min(ZOOM_MAX_PERCENT, Math.max(ZOOM_MIN_PERCENT, Math.round(percent)))
}

/**
 * **打开浏览器页签的统一视口**：桌面 1280（1280×800）+ 缩放 50%。
 *
 * `presetIndex = 1` 对应 BrowserPanel `SIZE_PRESETS` 的「桌面 1280」。
 * 所有打开浏览器页签的入口都使用它。
 */
export const BROWSER_DESKTOP_1280: WebViewportState = {
  mode: 'custom',
  w: 1280,
  h: 800,
  presetIndex: 1,
  zoomPercent: 50
}
