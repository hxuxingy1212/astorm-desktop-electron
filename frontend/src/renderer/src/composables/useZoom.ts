import { onBeforeUnmount, onMounted, watch } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { isWebShell } from '@/utils/platform'

/**
 * 全局缩放:设置变化时通过 IPC 调用 webContents.setZoomFactor,
 * 并监听 Ctrl/Cmd + =/-/0 快捷键。
 */
export function useZoom(): void {
  const settings = useSettingsStore()

  watch(
    () => settings.zoom,
    (zoom) => {
      window.api.zoom.setFactor(zoom)
    },
    { immediate: true }
  )

  const onKeydown = (event: KeyboardEvent): void => {
    // 浏览器部署没有 webContents.setZoomFactor；不要吞掉浏览器原生缩放快捷键。
    if (isWebShell) return
    if (!(event.ctrlKey || event.metaKey)) return
    const key = event.key.toLowerCase()
    if (key === '=' || key === '+') {
      event.preventDefault()
      settings.setZoom(settings.zoom + 0.1)
    } else if (key === '-') {
      event.preventDefault()
      settings.setZoom(settings.zoom - 0.1)
    } else if (key === '0') {
      event.preventDefault()
      settings.setZoom(1)
    }
  }

  onMounted(() => window.addEventListener('keydown', onKeydown))
  onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))
}
