import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@/services/backend'
import type { PluginView } from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/**
 * 插件系统 store —— 已安装插件列表 + 配置/启停/安装/卸载，经 invoke() 走主进程
 * （userData/plugins 目录 + plugins.json 注册表，见 src/main/plugins.ts）。
 *
 * 另持有「跨组件注入」桥：插件 SDK `chat.appendInput` 写入 injectedInput，
 * ChatPanel 监听后填入发送框（复用 MentionInput.setContent 路径）。
 */
export interface InjectedInput {
  text: string
  /** 时间戳用于去重（同文本连续注入仍触发 watch） */
  ts: number
}

export const usePluginsStore = defineStore('plugins', () => {
  const list = ref<PluginView[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  /** 插件 → 聊天发送框的注入桥 */
  const injectedInput = ref<InjectedInput | null>(null)

  const byId = computed(() => new Map(list.value.map((v) => [v.manifest.id, v])))
  const enabledPlugins = computed(() => list.value.filter((v) => v.enabled && v.entryUrl))

  async function bootstrap(): Promise<void> {
    await fetchPlugins()
  }

  /** 扫描已安装插件（主进程读 userData/plugins；目录缺失的注册记录返回占位条目） */
  async function fetchPlugins(): Promise<void> {
    loading.value = true
    error.value = null
    try {
      list.value = (await invoke('plugin_scan')) as PluginView[]
    } catch (e) {
      error.value = toErrorMessage(e)
      console.error('获取插件列表失败:', e)
    } finally {
      loading.value = false
    }
  }

  /** 从本地目录导入安装（调用方先经 dialog 拿到目录路径） */
  async function installFromDir(dir: string): Promise<void> {
    try {
      await invoke('plugin_install', { dir })
      await fetchPlugins()
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  async function remove(id: string): Promise<void> {
    try {
      await invoke('plugin_remove', { id })
      await fetchPlugins()
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  async function setEnabled(id: string, enabled: boolean): Promise<void> {
    try {
      const updated = (await invoke('plugin_set_enabled', { id, enabled })) as PluginView
      // 用返回的最新视图原位更新对应项,避免整表 fetchPlugins 重渲染导致的卡片抖动
      const idx = list.value.findIndex((v) => v.manifest.id === id)
      if (idx >= 0) list.value[idx] = updated
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  async function saveConfig(id: string, config: Record<string, unknown>): Promise<void> {
    try {
      await invoke('plugin_save_config', { id, config })
      await fetchPlugins()
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  /** 插件把文本填入聊天发送框（不含自动发送；ChatPanel 监听此桥） */
  function setInjectedInput(text: string): void {
    injectedInput.value = { text, ts: Date.now() }
  }

  return {
    list,
    loading,
    error,
    injectedInput,
    byId,
    enabledPlugins,
    bootstrap,
    fetchPlugins,
    installFromDir,
    remove,
    setEnabled,
    saveConfig,
    setInjectedInput
  }
})