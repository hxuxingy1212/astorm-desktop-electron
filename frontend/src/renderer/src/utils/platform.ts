/**
 * 运行平台判断。
 * Electron 下取 preload 暴露的 process.platform;纯浏览器预览(无 preload)按 UA 兜底,
 * 以便开发期在浏览器里验证布局。
 */

const uaPlatform: string = navigator.platform?.toLowerCase() ?? ''
const raw: string = window.api?.platform ?? (/mac/i.test(uaPlatform) ? 'darwin' : '')

/** macOS:使用系统原生红黄绿按钮,右侧 Windows 风格窗口按钮隐藏 */
export const isMac = raw === 'darwin'

/** 浏览器部署(web-bridge 桥接):没有 Electron 窗口装饰——红黄绿与自绘窗口按钮都不存在 */
export const isWebShell = window.api?.__web === true

/** 原生红黄绿按钮:仅 Electron 桌面壳的 macOS 有;浏览器部署即便跑在 Mac 上也无窗口装饰 */
export const hasTrafficLights = isMac && !isWebShell

/** 自绘窗口按钮(最小化/最大化/关闭):仅 Electron 桌面壳的非 macOS 平台显示;浏览器里由浏览器自身提供 */
export const showWindowControls = !isMac && !isWebShell

/** 本机桌面玻璃：macOS 原生 vibrancy / Windows 原生 acrylic；其他环境回落实底。 */
export const hasSidebarGlass = !isWebShell && (window.api?.platform === 'darwin' || window.api?.platform === 'win32')
