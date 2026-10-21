import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { useSessionsStore } from '@/stores/sessions'

/**
 * 插件「会话级状态」store：按 会话(id) × 插件(id) 保存任意 KV，随会话切换换挡。
 *
 * - 持久化：localStorage 按会话 key `astorm:plugin-state:<sessionId>:v1`（与 tabs 的
 *   按会话持久化同构）；set 即落盘，重启后可恢复。
 * - 切换：跟随 sessions.activeId；页签按会话绑定、插件 iframe 随换挡重建（不保活），
 *   插件每次加载后经 SDK `state.get` 读到**该会话自己**的状态——
 *   「按会话存、切换恢复」由"每会话一份数据 + 插件加载时拉取"完成。
 * - 无会话（newChat 页）时状态为空；同会话内 in-memory 缓存即事实源。
 */

function storageKey(sessionId: string): string {
  return `astorm:plugin-state:${sessionId}:v1`
}

export const usePluginSessionStore = defineStore('pluginSession', () => {
  /** 当前会话 id（跟随 sessions.activeId）；空 = 未绑定会话 */
  const currentSessionId = ref('')
  /** 当前会话的插件状态缓存：pluginId → (key → value) */
  const cache = ref<Record<string, Record<string, unknown>>>({})

  /** 载入指定会话的状态（丢弃当前内存缓存） */
  function load(sessionId: string): void {
    currentSessionId.value = sessionId
    cache.value = {}
    if (!sessionId) return
    try {
      const raw = JSON.parse(localStorage.getItem(storageKey(sessionId)) ?? '{}') as unknown
      if (raw && typeof raw === 'object') {
        cache.value = raw as Record<string, Record<string, unknown>>
      }
    } catch {
      cache.value = {}
    }
  }

  /** 持久化当前会话状态（set 后即时落盘，保证切会话/重启可恢复） */
  function persist(): void {
    const sid = currentSessionId.value
    if (!sid) return
    try {
      localStorage.setItem(storageKey(sid), JSON.stringify(cache.value))
    } catch {
      /* 存储不可用时忽略（与 tabs 持久化一致） */
    }
  }

  /** 当前会话下某插件的状态表（惰性初始化） */
  function stateFor(pluginId: string): Record<string, unknown> {
    if (!cache.value[pluginId]) cache.value[pluginId] = {}
    return cache.value[pluginId]!
  }

  /** 读当前会话某插件的状态：传 key 取单值，不传返回整表 */
  function get(pluginId: string, key?: string): unknown {
    const map = cache.value[pluginId]
    if (!map) return undefined
    return key === undefined ? map : map[key]
  }

  /** 写当前会话某插件的状态（key → value），立即持久化 */
  function set(pluginId: string, key: string, value: unknown): void {
    stateFor(pluginId)[key] = value
    persist()
  }

  /** 删除会话时清掉它的插件状态（内存缓存 + localStorage） */
  function clearSession(sessionId: string): void {
    if (currentSessionId.value === sessionId) {
      cache.value = {}
      currentSessionId.value = ''
    }
    try {
      localStorage.removeItem(storageKey(sessionId))
    } catch {
      /* 忽略 */
    }
  }

  /** 跟随会话切换（应用启动后调用一次，与 tabs.bindSessions 同源） */
  function bindSessions(): void {
    const sessions = useSessionsStore()
    watch(
      () => sessions.activeId,
      (id) => load(id || ''),
      { immediate: true }
    )
  }

  return {
    currentSessionId,
    cache,
    bindSessions,
    load,
    persist,
    stateFor,
    get,
    set,
    clearSession
  }
})