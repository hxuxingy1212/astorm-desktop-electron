import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { invoke } from '@/services/backend'
import type {
  MemoryCompileReport,
  MemoryConcept,
  MemoryConceptDetail,
  MemoryEvent,
  MemoryLogEntry,
  MemoryRawSource,
  MemoryStatus,
  MemoryValidateReport
} from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** 阶段键 → i18n 键后缀（进度条文字） */
export const MEMORY_STAGES = ['parse', 'extract', 'compile', 'merge', 'write'] as const
export type MemoryStage = (typeof MEMORY_STAGES)[number]

/** 后端未就绪（未加载 dll）时的错误前缀，与 services/backend 一致 */
const UNAVAILABLE = 'BACKEND_UNAVAILABLE'

/**
 * 长期记忆 store：状态卡 / 概念浏览 / 检索 / 编译触发与进度。
 *
 * 编译是**异步**的：`compile()` 立即返回，进度与结果由 `initMemorySync()` 订阅的
 * `type === 'memory'` 事件推进（与定时任务的 schedulesSync 同一套模块级单例订阅）。
 */
export const useMemoryStore = defineStore('memory', () => {
  // ---- 状态卡 ----
  const status = ref<MemoryStatus | null>(null)
  const statusLoading = ref(false)

  // ---- 概念列表 / 详情 ----
  const concepts = ref<MemoryConcept[]>([])
  const listLoading = ref(false)
  const query = ref('')
  /** 检索结果（非空 = 当前展示检索结果；清空 query 回到全量列表） */
  const searching = ref(false)
  const selectedId = ref('')
  const detail = ref<MemoryConceptDetail | null>(null)
  const detailLoading = ref(false)

  // ---- 抽屉：变更历史 / 来源会话 ----
  const log = ref<MemoryLogEntry[]>([])
  const rawSources = ref<MemoryRawSource[]>([])

  // ---- 编译进度 ----
  const running = ref(false)
  const stage = ref<MemoryStage | null>(null)
  const current = ref(0)
  const total = ref(0)
  const progressMessage = ref('')
  const lastReport = ref<MemoryCompileReport | null>(null)
  const lastFailedMessage = ref('')

  const error = ref<string | null>(null)

  /** 进度百分比（total 为 0 时退化为 0，避免除零） */
  const percent = computed(() =>
    total.value > 0 ? Math.min(100, Math.round((current.value / total.value) * 100)) : 0
  )

  function backendMissing(e: unknown): boolean {
    return toErrorMessage(e).startsWith(UNAVAILABLE)
  }

  // ---------- 只读查询 ----------

  async function refreshStatus(): Promise<void> {
    statusLoading.value = true
    try {
      status.value = (await invoke('memory_status')) as MemoryStatus
      error.value = null
    } catch (e) {
      error.value = toErrorMessage(e)
    } finally {
      statusLoading.value = false
    }
  }

  async function fetchConcepts(): Promise<void> {
    listLoading.value = true
    try {
      concepts.value = (await invoke('memory_list_concepts')) as MemoryConcept[]
      searching.value = false
      error.value = null
    } catch (e) {
      error.value = toErrorMessage(e)
      concepts.value = []
    } finally {
      listLoading.value = false
    }
  }

  async function searchConcepts(q: string, limit = 50): Promise<void> {
    const trimmed = q.trim()
    query.value = q
    if (!trimmed) {
      await fetchConcepts()
      return
    }
    listLoading.value = true
    try {
      concepts.value = (await invoke('memory_search', { query: trimmed, limit })) as MemoryConcept[]
      searching.value = true
      error.value = null
    } catch (e) {
      error.value = toErrorMessage(e)
      concepts.value = []
    } finally {
      listLoading.value = false
    }
  }

  async function openConcept(conceptId: string): Promise<void> {
    selectedId.value = conceptId
    detailLoading.value = true
    try {
      detail.value = (await invoke('memory_show_concept', { concept_id: conceptId })) as MemoryConceptDetail
      error.value = null
    } catch (e) {
      error.value = toErrorMessage(e)
      detail.value = null
    } finally {
      detailLoading.value = false
    }
  }

  function closeConcept(): void {
    selectedId.value = ''
    detail.value = null
  }

  async function fetchLog(limit = 200): Promise<void> {
    try {
      log.value = (await invoke('memory_read_log', { limit })) as MemoryLogEntry[]
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  async function fetchRawSources(): Promise<void> {
    try {
      rawSources.value = (await invoke('memory_list_raw_sources')) as MemoryRawSource[]
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  async function readRawSource(stem: string): Promise<string> {
    return (await invoke('memory_read_raw_source', { stem })) as string
  }

  /** 概念总数 + 待编译数一次性刷新（编译结束、进入页面时调用） */
  async function refreshAll(): Promise<void> {
    await Promise.all([refreshStatus(), fetchConcepts()])
  }

  // ---------- 写操作 ----------

  /** 触发编译（立即返回；进度与结果经事件推进）。返回是否成功发起。 */
  async function compile(force = false): Promise<boolean> {
    try {
      await invoke('memory_compile', { force })
      // 乐观置位：事件可能比 IPC 返回更快到达，先不置位会更晚才显示进度
      running.value = true
      stage.value = null
      current.value = 0
      total.value = 0
      progressMessage.value = ''
      lastReport.value = null
      lastFailedMessage.value = ''
      return true
    } catch (e) {
      error.value = toErrorMessage(e)
      running.value = false
      return false
    }
  }

  async function cancel(): Promise<void> {
    try {
      await invoke('memory_cancel')
    } catch (e) {
      error.value = toErrorMessage(e)
    }
  }

  async function reindex(): Promise<number> {
    try {
      const n = (await invoke('memory_reindex')) as number
      return n
    } catch (e) {
      error.value = toErrorMessage(e)
      return -1
    }
  }

  async function validateBundle(): Promise<MemoryValidateReport | null> {
    try {
      return (await invoke('memory_validate')) as MemoryValidateReport
    } catch (e) {
      error.value = toErrorMessage(e)
      return null
    }
  }

  /** 用系统文件管理器打开 Bundle 目录；返回 '' = 成功，否则错误信息 */
  async function openRoot(): Promise<string> {
    const root = status.value?.root
    if (!root) return '记忆目录未知（请先刷新状态）'
    const open = window.api?.fs?.openFolder
    if (!open) return '当前环境不支持打开目录'
    return open(root)
  }

  // ---------- 事件应用 ----------

  /** 把一条 `type === 'memory'` 事件应用到本地进度状态 */
  function applyEvent(ev: MemoryEvent): void {
    const kind = ev.kind
    if (kind === 'started') {
      running.value = true
      stage.value = null
      current.value = 0
      total.value = 0
      lastReport.value = null
      lastFailedMessage.value = ''
    } else if (kind === 'stage') {
      running.value = true
      stage.value = (ev.stage as MemoryStage | null) ?? null
      current.value = ev.current
      total.value = ev.total
    }
    // message 在三个分支里都要刷新最后一句说明
    if (ev.message) progressMessage.value = ev.message
    if (kind === 'finished' || kind === 'cancelled') {
      running.value = false
      stage.value = null
      lastReport.value = ev.report
      void refreshAll()
    } else if (kind === 'failed') {
      running.value = false
      stage.value = null
      lastFailedMessage.value = ev.message
    }
  }

  return {
    // state
    status,
    statusLoading,
    concepts,
    listLoading,
    query,
    searching,
    selectedId,
    detail,
    detailLoading,
    log,
    rawSources,
    running,
    stage,
    current,
    total,
    progressMessage,
    lastReport,
    lastFailedMessage,
    error,
    // computed
    percent,
    // actions
    backendMissing,
    refreshStatus,
    fetchConcepts,
    searchConcepts,
    openConcept,
    closeConcept,
    fetchLog,
    fetchRawSources,
    readRawSource,
    refreshAll,
    compile,
    cancel,
    reindex,
    validateBundle,
    openRoot,
    applyEvent
  }
})

/* ---------- 后端记忆事件订阅（模块级单例，App.vue 调用一次） ---------- */

let offEvent: (() => void) | null = null

/**
 * 订阅 `type === 'memory'` 全局事件并驱动 store 进度。
 *
 * 形状与 `tabsSync` / `schedulesSync` 一致：模块级单例订阅，无后端桥（纯浏览器调试）时静默跳过。
 */
export function initMemorySync(): void {
  if (!window.api?.backend?.onEvent || offEvent) return
  offEvent = window.api.backend.onEvent((chunk) => {
    const ev = chunk as { type?: string } & MemoryEvent
    if (ev?.type !== 'memory') return
    // 事件到达时 pinia 已安装（App.vue 在 setup 之后调用本函数）
    useMemoryStore().applyEvent(ev)
  })
}
