import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@/services/backend'
import type { McpOAuthStart, McpOAuthStatus, McpServerSummary } from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/**
 * MCP 服务器 store —— 服务器列表 + 配置文件内容,经 invoke() 走 mock 后端。
 */
export const useMcpStore = defineStore('mcp', () => {
  const servers = ref<McpServerSummary[]>([])
  const configText = ref('')
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function fetchServers(): Promise<void> {
    loading.value = true
    error.value = null
    try {
      const [list, raw] = await Promise.all([
        invoke('list_mcp_servers'),
        invoke('read_mcp_config_file')
      ])
      servers.value = list as McpServerSummary[]
      configText.value = raw as string
    } catch (e) {
      error.value = toErrorMessage(e)
      console.error('获取 MCP 服务器列表失败:', e)
    } finally {
      loading.value = false
    }
  }

  async function saveConfig(text: string): Promise<void> {
    try {
      await invoke('write_mcp_config_file', { content: text })
      await fetchServers()
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  async function toggleEnabled(name: string, enabled: boolean): Promise<void> {
    try {
      await invoke('toggle_mcp_server', { name, enabled })
      await fetchServers()
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  async function connectServer(name: string): Promise<string | null> {
    try {
      const result = (await invoke('connect_mcp_server', { name })) as string
      await fetchServers()
      return result
    } catch (e) {
      error.value = toErrorMessage(e)
      console.error(`连接 MCP 服务器失败 (${name}):`, e)
      return null
    }
  }

  async function disconnectServer(name: string): Promise<void> {
    try {
      await invoke('disconnect_mcp_server', { name })
      await fetchServers()
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  /** 保存手动授权头（Bearer / 自定义 Header）；后端会按需重连 */
  async function setServerAuth(
    name: string,
    headers: Record<string, string>
  ): Promise<string> {
    try {
      const message = (await invoke('set_mcp_server_auth', {
        name,
        headers: JSON.stringify(headers)
      })) as string
      await fetchServers()
      return message
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  /** 开始 OAuth：返回浏览器授权地址 */
  async function beginOAuth(name: string): Promise<McpOAuthStart> {
    try {
      return (await invoke('begin_mcp_oauth', { name })) as McpOAuthStart
    } catch (e) {
      error.value = toErrorMessage(e)
      throw e
    }
  }

  /** 轮询 OAuth 状态 */
  async function oauthStatus(name: string): Promise<McpOAuthStatus> {
    return (await invoke('mcp_oauth_status', { name })) as McpOAuthStatus
  }

  const connectedServers = computed(() => servers.value.filter((s) => s.connected))

  return {
    servers,
    configText,
    loading,
    error,
    connectedServers,
    fetchServers,
    saveConfig,
    toggleEnabled,
    connectServer,
    disconnectServer,
    setServerAuth,
    beginOAuth,
    oauthStatus
  }
})
