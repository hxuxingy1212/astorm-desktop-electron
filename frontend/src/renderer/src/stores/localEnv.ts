import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@/services/backend'
import type {
  AppSettings,
  LocalEnv,
  ToolchainAllowanceReport,
  ToolchainAllowanceTool
} from '@/services/dto'

function toErrorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** 检测结果 localStorage 键（跨重启持久化；探测逐工具 --version 开销大，不做启动/进页自动重测） */
const LOCAL_ENV_KEY = 'astorm:local-env:v1'
interface LocalEnvCache {
  savedAt: number
  items: LocalEnv[]
}

function readCache(): LocalEnv[] | null {
  try {
    const raw = localStorage.getItem(LOCAL_ENV_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as LocalEnvCache
    return Array.isArray(parsed.items) ? parsed.items : null
  } catch {
    return null
  }
}

function writeCache(items: LocalEnv[]): void {
  try {
    const cache: LocalEnvCache = { savedAt: Date.now(), items }
    localStorage.setItem(LOCAL_ENV_KEY, JSON.stringify(cache))
  } catch {
    /* 配额/隐私模式等写入失败不影响本次展示 */
  }
}

/** 「本地环境」卡片行：检测结果 + 注入状态按 name 合并后的渲染模型 */
export interface LocalEnvRow extends LocalEnv {
  /** 工具 bin 名（注入开关 write/remove 的键） */
  bin: string
  /** 是否已被用户禁用注入（true = 已取消注入） */
  disabled: boolean
  /** 禁用该工具后其目录是否仍被其他启用工具共享留在白名单 */
  sharedDir: boolean
  /** PATH 命中的真实所在目录（canonicalize；未检测到为 null） */
  resolvedDir: string | null
  /** 附加注入目录（rustup 真实工具链等；无则空） */
  extraDirs: string[]
}

/**
 * 本地编程环境 store。
 *
 * 检测结果**前端持久化**（localStorage `astorm:local-env:v1`）：`detect_local_env`
 * （逐工具 `--version`，开销大）结果写入本地存储，应用重启 / 切换设置菜单再进入本页
 * **都不会**重复探测——直接展示上次结果；只有用户点「刷新」才强制重新检测并回写。
 * 注入状态（`toolchain_allowances`）开销小且随开关变化，每次进入页面/开关后都拉最新。
 */
export const useLocalEnvStore = defineStore('localEnv', () => {
  const items = ref<LocalEnv[]>([])
  /** 探测是否已完成过（检测结果的缓存标记） */
  const loaded = ref(false)
  const loading = ref(false)
  const error = ref('')
  const allowances = ref<ToolchainAllowanceReport | null>(null)

  /** 注入状态按 name 索引（合并时 O(1) 取用） */
  const allowanceByName = computed<Map<string, ToolchainAllowanceTool>>(() => {
    const m = new Map<string, ToolchainAllowanceTool>()
    for (const t of allowances.value?.tools ?? []) m.set(t.name, t)
    return m
  })

  /** 合并检测结果与注入状态后的渲染行 */
  const rows = computed<LocalEnvRow[]>(() => {
    const byName = allowanceByName.value
    return items.value.map((it) => {
      const a = byName.get(it.name)
      return {
        ...it,
        bin: a?.bin ?? it.name,
        disabled: a?.disabled ?? false,
        sharedDir: a?.sharedDir ?? false,
        resolvedDir: a?.resolvedDir ?? null,
        extraDirs: a?.extraDirs ?? []
      }
    })
  })

  /** 当前生效的 exec 放行目录（安全配置只读展示复用） */
  const execDirs = computed(() => allowances.value?.execDirs ?? [])
  /** 当前生效的只读放行 globs */
  const readGlobs = computed(() => allowances.value?.readGlobs ?? [])
  /** 系统技能目录（固定只读注入） */
  const skillsDir = computed<string | null>(() => allowances.value?.skillsDir ?? null)

  /**
   * 探测工具列表。`force=false` 时命中缓存则跳过——缓存优先 localStorage（跨重启，
   * 重启后进页直接展示上次结果，不自动重测），其次本运行期内存；仅无任何缓存时调后端。
   * `force=true`（用户手动「刷新」）强制重测并回写 localStorage。
   */
  async function loadDetection(force = false): Promise<void> {
    if (!force) {
      if (loaded.value) return
      const cached = readCache()
      if (cached) {
        items.value = cached
        loaded.value = true
        return
      }
    }
    loading.value = true
    error.value = ''
    try {
      items.value = (await invoke('detect_local_env')) as LocalEnv[]
      writeCache(items.value)
    } catch (e) {
      error.value = toErrorMessage(e)
      // 失败不清空已有列表（手动刷新失败保留旧数据 + 错误提示）
    } finally {
      loaded.value = true
      loading.value = false
    }
  }

  /** 拉取当前注入状态（反映配置里 toolchain_disabled 的即时值；失败静默保留上次）。 */
  async function loadAllowances(): Promise<void> {
    try {
      allowances.value = (await invoke('toolchain_allowances')) as ToolchainAllowanceReport
    } catch (e) {
      // 后端未加载时保留上次值，不阻断页面（检测可能仍可用）
      // eslint-disable-next-line no-console
      console.error('[localEnv] load toolchain_allowances failed:', toErrorMessage(e))
    }
  }

  /** 进入页面：复用检测缓存 + 拉最新注入状态。 */
  async function bootstrap(): Promise<void> {
    await Promise.all([loadDetection(false), loadAllowances()])
  }

  /** 用户主动刷新：强制重新检测 + 拉最新注入状态。 */
  async function refresh(): Promise<void> {
    await Promise.all([loadDetection(true), loadAllowances()])
  }

  /**
   * 切换某工具的注入开关（inject=true 表示「注入沙箱默认白名单」）。
   * 改往 security.toolchainDisabled（本地环境页维护）→ save_settings（主进程落盘 + 热更后端）→ 刷新注入态。
   */
  async function toggleInjection(bin: string, inject: boolean): Promise<void> {
    const s = (await invoke('get_settings')) as AppSettings
    if (!s.security) throw new Error('安全配置缺失，无法切换注入')
    const cur = Array.isArray(s.security.toolchainDisabled)
      ? s.security.toolchainDisabled
      : []
    const next = inject
      ? cur.filter((x) => x !== bin)
      : Array.from(new Set([...cur, bin]))
    s.security.toolchainDisabled = next
    await invoke('save_settings', { settings: s })
    await loadAllowances()
  }

  return {
    items,
    loaded,
    loading,
    error,
    allowances,
    rows,
    execDirs,
    readGlobs,
    skillsDir,
    bootstrap,
    refresh,
    toggleInjection,
    loadAllowances
  }
})
