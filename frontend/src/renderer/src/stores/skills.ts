import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@/services/backend'
import type { SkillSummary, SkillDoc, DiscoveredSkill, CreateSkillRequest, SkillUpdate } from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

function normalizeCategory(category: string | string[]): string {
  if (Array.isArray(category)) return category.join(',')
  return category ?? ''
}

/**
 * 技能管理 store —— 数据经 invoke() 走 mock 后端(localStorage 持久化)。
 */
export const useSkillsStore = defineStore('skills', () => {
  const skills = ref<SkillSummary[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const discovered = ref<DiscoveredSkill[]>([])
  const sessionSkillNames = ref<string[]>([])

  const enabledSkills = computed(() => skills.value.filter((s) => s.enabled))
  const skillsByCategory = computed(() => {
    const map: Record<string, SkillSummary[]> = {}
    for (const s of skills.value) {
      const cat = s.category || 'general'
      if (!map[cat]) map[cat] = []
      map[cat].push(s)
    }
    return map
  })

  async function fetchSkills(enabledOnly = false, category: string | null = null): Promise<void> {
    loading.value = true
    error.value = null
    try {
      skills.value = (await invoke('list_skills', { enabledOnly, category })) as SkillSummary[]
    } catch (e) {
      error.value = toErrorMessage(e)
    } finally {
      loading.value = false
    }
  }

  async function searchSkills(query: string, limit = 20): Promise<SkillSummary[]> {
    try {
      return (await invoke('search_skills', { query, limit })) as SkillSummary[]
    } catch (e) {
      error.value = toErrorMessage(e)
      return []
    }
  }

  async function getSkill(name: string): Promise<SkillDoc | null> {
    try {
      return (await invoke('get_skill', { name })) as SkillDoc | null
    } catch (e) {
      error.value = toErrorMessage(e)
      return null
    }
  }

  async function discoverSkills(): Promise<void> {
    loading.value = true
    try {
      discovered.value = (await invoke('discover_skills')) as DiscoveredSkill[]
    } catch (e) {
      error.value = toErrorMessage(e)
    } finally {
      loading.value = false
    }
  }

  async function createSkill(data: CreateSkillRequest): Promise<SkillSummary[]> {
    loading.value = true
    try {
      const skill = (await invoke('create_skill', { req: { ...data } })) as SkillDoc
      skills.value.push(skill as unknown as SkillSummary)
      return [skill as unknown as SkillSummary]
    } catch (e) {
      error.value = toErrorMessage(e)
      return []
    } finally {
      loading.value = false
    }
  }

  async function importSkills(paths: string[]): Promise<SkillSummary[]> {
    loading.value = true
    try {
      const imported = (await invoke('import_skills', { paths })) as SkillDoc[]
      mergeSkills(imported as unknown as SkillSummary[])
      return imported as unknown as SkillSummary[]
    } catch (e) {
      error.value = toErrorMessage(e)
      return []
    } finally {
      loading.value = false
    }
  }

  async function importSkillPackage(path: string): Promise<SkillSummary[]> {
    loading.value = true
    try {
      const imported = (await invoke('import_skill_package', { path })) as SkillDoc[]
      mergeSkills(imported as unknown as SkillSummary[])
      return imported as unknown as SkillSummary[]
    } catch (e) {
      error.value = toErrorMessage(e)
      return []
    } finally {
      loading.value = false
    }
  }

  function mergeSkills(imported: SkillSummary[]): void {
    for (const s of imported) {
      const idx = skills.value.findIndex((x) => x.name === s.name)
      if (idx === -1) skills.value.push(s)
      else skills.value[idx] = s
    }
  }

  async function removeSkill(name: string): Promise<void> {
    try {
      await invoke('remove_skill', { name })
      skills.value = skills.value.filter((s) => s.name !== name)
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  async function toggleSkill(name: string, enabled: boolean): Promise<void> {
    try {
      const updated = (await invoke('toggle_skill', { name, enabled })) as SkillSummary
      const idx = skills.value.findIndex((s) => s.name === name)
      if (idx !== -1) skills.value[idx] = { ...skills.value[idx], ...updated }
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  async function updateSkill(
    name: string,
    updates: Omit<SkillUpdate, 'category'> & { category?: string | string[] }
  ): Promise<SkillDoc | null> {
    try {
      const updated = (await invoke('update_skill', {
        name,
        updates: { ...updates, category: normalizeCategory(updates.category ?? '') }
      })) as SkillDoc
      const idx = skills.value.findIndex((s) => s.name === name)
      if (idx !== -1) skills.value[idx] = { ...skills.value[idx], ...updated }
      return updated
    } catch (e) {
      error.value = toErrorMessage(e)
      return null
    }
  }

  async function fetchSessionSkills(): Promise<void> {
    // 会话级技能:mock 阶段直接使用全部已启用技能
    sessionSkillNames.value = skills.value.filter((s) => s.enabled).map((s) => s.name)
  }

  function setSessionSkills(names: string[]): void {
    sessionSkillNames.value = names
  }

  return {
    skills,
    loading,
    error,
    discovered,
    sessionSkillNames,
    enabledSkills,
    skillsByCategory,
    fetchSkills,
    searchSkills,
    getSkill,
    createSkill,
    discoverSkills,
    importSkills,
    importSkillPackage,
    removeSkill,
    toggleSkill,
    updateSkill,
    fetchSessionSkills,
    setSessionSkills
  }
})