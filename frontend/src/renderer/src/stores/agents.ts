import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@/services/backend'
import type { AgentCard, CreateAgentRequest, AgentUpdate } from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/**
 * 智能体管理 store —— 数据经 invoke() 走后端 napi（sqlite `agents` 表）。
 *
 * 卡片构成：名称 / 描述 / 额外用户指引（system 角色，追加在系统提示词之后）/
 * 工具集（**强制过滤**：空 = 无工具）/ 技能集（仅进环境提示词，不限制 skills 工具加载）。
 * 内置卡片（`builtin`）不可编辑 / 删除，由后端拒绝（前端只做隐藏 + 提示）。
 */
export const useAgentsStore = defineStore('agents', () => {
  const agents = ref<AgentCard[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  /** 自定义（可编辑）智能体 */
  const customAgents = computed(() => agents.value.filter((a) => !a.builtin))
  /** 内置智能体 */
  const builtinAgents = computed(() => agents.value.filter((a) => a.builtin))

  async function fetchAgents(): Promise<void> {
    loading.value = true
    error.value = null
    try {
      agents.value = (await invoke('list_agents')) as AgentCard[]
    } catch (e) {
      error.value = toErrorMessage(e)
    } finally {
      loading.value = false
    }
  }

  async function getAgent(id: string): Promise<AgentCard | null> {
    try {
      return (await invoke('get_agent', { id })) as AgentCard | null
    } catch (e) {
      error.value = toErrorMessage(e)
      return null
    }
  }

  /** 新建：成功后插入列表并返回卡片（失败返回 null，错误写入 error） */
  async function createAgent(data: CreateAgentRequest): Promise<AgentCard | null> {
    error.value = null
    try {
      const card = (await invoke('create_agent', { req: { ...data } })) as AgentCard
      await fetchAgents()
      return card
    } catch (e) {
      error.value = toErrorMessage(e)
      return null
    }
  }

  async function updateAgent(id: string, updates: AgentUpdate): Promise<AgentCard | null> {
    error.value = null
    try {
      const card = (await invoke('update_agent', { id, updates: { ...updates } })) as AgentCard
      await fetchAgents()
      return card
    } catch (e) {
      error.value = toErrorMessage(e)
      return null
    }
  }

  async function removeAgent(id: string): Promise<boolean> {
    error.value = null
    try {
      await invoke('remove_agent', { id })
      agents.value = agents.value.filter((a) => a.agentId !== id)
      return true
    } catch (e) {
      error.value = toErrorMessage(e)
      return false
    }
  }

  return {
    agents,
    loading,
    error,
    customAgents,
    builtinAgents,
    fetchAgents,
    getAgent,
    createAgent,
    updateAgent,
    removeAgent
  }
})
