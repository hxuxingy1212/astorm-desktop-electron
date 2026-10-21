import { ref } from 'vue'

/**
 * 全屏状态(模块级共享 ref):
 * StatusBar 订阅主进程事件并调用 applyFullscreen 更新;
 * 布局组件(如 Workspace)读取 ref 自适应(如全屏时放宽对话列上限)。
 * 同时驱动 html[data-fullscreen],供全局 CSS 放大工具栏/侧栏。
 */
const fullscreen = ref(false)

export function useFullscreenState(): typeof fullscreen {
  return fullscreen
}

export function applyFullscreen(f: boolean): void {
  fullscreen.value = f
  document.documentElement.dataset.fullscreen = f ? 'true' : 'false'
}
