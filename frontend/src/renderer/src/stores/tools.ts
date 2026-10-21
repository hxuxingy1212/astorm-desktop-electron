import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@/services/backend'
import type { ToolInfo } from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/**
 * 工具 store —— 内置/插件/ MCP 工具,经 invoke() 走 mock 后端。
 */
export const useToolsStore = defineStore('tools', () => {
  const tools = ref<ToolInfo[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const serverToolsCache = ref<Record<string, ToolInfo[]>>({})
  const serverToolsLoading = ref<Record<string, boolean>>({})

  const builtinTools = computed(() => tools.value.filter((t) => t.source?.type === 'builtin'))
  const pluginTools = computed(() => tools.value.filter((t) => t.source?.type === 'plugin'))
  const mcpTools = computed(() => tools.value.filter((t) => t.source?.type === 'mcp'))
  /**
   * 动态（按需加载）工具：后端 `dynamic` 标记（清单真相在 agent::dynamic_tools::DYNAMIC_TOOLS）。
   * 初始不下发 LLM，模型经 tool_load 加载后生效；设置页独立页签展示。
   */
  const dynamicTools = computed(() => tools.value.filter((t) => t.dynamic === true))

  async function fetchTools(category: string | null = null): Promise<void> {
    loading.value = true
    error.value = null
    try {
      tools.value = (await invoke('list_tools', { category })) as ToolInfo[]
    } catch (e) {
      error.value = toErrorMessage(e)
    } finally {
      loading.value = false
    }
  }

  async function fetchServerTools(server: string): Promise<ToolInfo[]> {
    if (serverToolsCache.value[server]) return serverToolsCache.value[server]
    serverToolsLoading.value = { ...serverToolsLoading.value, [server]: true }
    try {
      const result = (await invoke('list_mcp_server_tools', { server })) as ToolInfo[]
      serverToolsCache.value = { ...serverToolsCache.value, [server]: result }
      return result
    } catch (e) {
      error.value = toErrorMessage(e)
      console.error(`获取 MCP 服务器工具失败 (${server}):`, e)
      return []
    } finally {
      serverToolsLoading.value = { ...serverToolsLoading.value, [server]: false }
    }
  }

  function getServerTools(server: string): ToolInfo[] | null {
    return serverToolsCache.value[server] || null
  }

  function clearServerToolsCache(): void {
    serverToolsCache.value = {}
    serverToolsLoading.value = {}
  }

  async function searchTools(query: string, limit = 20): Promise<ToolInfo[]> {
    try {
      return (await invoke('search_tools', { query, limit })) as ToolInfo[]
    } catch (e) {
      error.value = toErrorMessage(e)
      return []
    }
  }

  return {
    tools,
    loading,
    error,
    builtinTools,
    pluginTools,
    dynamicTools,
    mcpTools,
    serverToolsCache,
    serverToolsLoading,
    fetchTools,
    fetchServerTools,
    getServerTools,
    clearServerToolsCache,
    searchTools
  }
})