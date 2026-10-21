/**
 * 后端配置同步(后端配置 → 前端适配):把「后端自身支持的配置」组装为完整 napi 配置
 * (log / workspace / llm / summary / webfetch / browser / security / memory),在启动与保存时
 * 推给 Rust 后端热更新(运行中 updateConfig,未运行则 start)。
 *
 * 后端配置以 Rust AppConfig 为准(camelCase,缺失字段回退默认);前端只保存/读取
 * 后端真实支持的字段——会话摘要 `summary`(tokenThreshold / minMessages /
 * keepTokenThreshold / keepTailRounds)。主题色 / 语言属前端偏好,一律不进后端。
 */
import { app } from 'electron'
import { existsSync } from 'fs'
import { join } from 'path'
import { readFile, stat } from 'fs/promises'
import { napiCall, syncNapiConfig, type NapiAppConfig, type NapiCompactConfig, type NapiGoalConfig, type NapiInstructionConfig, type NapiMemoryCategoryConfig, type NapiMemoryConfig, type NapiSecurityConfig, type NapiSummaryConfig } from './napi'
import { configDir, systemDir } from './systemPaths'
import { writeFileAtomic } from './atomicWrite'
import { getProvidersSnapshot, ensureProvidersLoaded } from './providers'

/** 会话摘要配置默认值(与后端 SummaryConfig::default 一致) */
export const DEFAULT_SUMMARY = {
  token_threshold: 200000,
  min_messages: 8,
  keep_token_threshold: 20000,
  keep_tail_rounds: 5
} as const

export type SummarySettings = typeof DEFAULT_SUMMARY

/** 会话消息剪支配置默认值(与后端 CompactConfig::default 一致) */
export const DEFAULT_COMPACT = {
  compact_threshold: 32768,
  compact_recent_rounds: 5,
  compact_byte_count: 10240,
  compact_head_chars: 200,
  compact_tail_chars: 400
} as const

export type CompactSettings = typeof DEFAULT_COMPACT

/** 目标循环配置默认值(与后端 GoalConfig::default 一致) */
export const DEFAULT_GOAL = {
  max_tokens: 10000000,
  max_iterations: 5
} as const

export type GoalSettings = typeof DEFAULT_GOAL

/** 网络搜索配置(与后端 WebSearchConfig 一致;多引擎免 key 抓取) */
export interface WebSearchSettings {
  defaultEngine: string
  timeoutSec: number
}

/** 网络搜索配置默认值(与后端 WebSearchConfig::default 一致;国内优先默认百度) */
export const DEFAULT_WEB_SEARCH: WebSearchSettings = {
  defaultEngine: 'baidu',
  timeoutSec: 30
}

/** 安全配置默认值(与后端 SecurityConfig::default 一致;camelCase 直接对齐,不做键映射)
 *
 * 仅作「后端取不到默认值」时的**兜底**：`file.readBlack/writeBlack` 保持空表，真正的默认值
 * 由后端 `security_defaults` 提供（非空：凭据路径清单，见 `astorm_core::config::credential_paths`），
 * 免得在后端未加载时用前端常量把默认清单写死/漂移。
 */
export const DEFAULT_SECURITY: NapiSecurityConfig = {
  enabled: true,
  file: { readBlack: [], readWhite: [], writeBlack: [], writeWhite: [] },
  net: { useWhitelist: true, inBlack: [], inWhite: [], outBlack: [], outWhite: [] },
  danger: { useBuiltin: true, builtinPatterns: null, black: [] },
  resource: { memoryMb: 4096, diskMb: 1024, cpuSeconds: 600, cpuPercent: 95, gpu: true },
  toolchainDisabled: [],
  execWhite: []
}

/**
 * 后端安全配置默认值(napi `security_defaults`；取不到返回 null，不缓存失败)。
 *
 * 用于「文件里缺读/写黑名单键」的场景：用户从未保存过安全配置时，这里给出的默认清单
 * 就是设置页看到的初值；保存时随配置落盘，因此**保存一次后清单仍在**。
 */
let securityDefaultsCache: NapiSecurityConfig | null = null

function backendSecurityDefaults(): NapiSecurityConfig | null {
  if (securityDefaultsCache) return securityDefaultsCache
  try {
    const raw = napiCall('securityDefaults')
    const parsed = typeof raw === 'string' ? JSON.parse(raw) : (raw as unknown)
    if (parsed && typeof parsed === 'object' && (parsed as NapiSecurityConfig).file) {
      securityDefaultsCache = parsed as NapiSecurityConfig
      return securityDefaultsCache
    }
  } catch {
    // 后端未加载 / 旧 dll 无此导出：退回 DEFAULT_SECURITY(空表兜底)
  }
  return null
}

function settingsFile(): string {
  return join(configDir(), 'backend-settings.json')
}

/** 安全配置文件（与 backend-settings.json 分离；安全页的路径/打开按钮指向它）。 */
export function securityFile(): string {
  return join(configDir(), 'security.json')
}

/**
 * 记忆分类默认表(与后端 `MemoryConfig::default_categories` 一致)。
 *
 * 顺序即提示词里的取值集顺序(信号强的分类排在 `fact` 前,否则 fact 会吸走步骤/约束/约定),
 * 也决定设置页表格的初始行序;`fact`/`other` 共用 concepts/。
 * `hint` 会被注入提取与编译两处提示词,是唯一直接作用于分类准确率的面。
 */
export const DEFAULT_MEMORY_CATEGORIES: NapiMemoryCategoryConfig[] = [
  { id: 'decision', dir: 'decisions', type: 'Decision', hint: '一次性的选择及其理由' },
  { id: 'preference', dir: 'preferences', type: 'Preference', hint: '用户稳定的倾向与习惯' },
  { id: 'instruction', dir: 'instructions', type: 'Instruction', hint: '用户要求长期遵守的规则' },
  {
    id: 'convention',
    dir: 'conventions',
    type: 'Convention',
    hint: '项目约定：目录/命名/流程/工具链的固定做法'
  },
  {
    id: 'procedure',
    dir: 'procedures',
    type: 'Procedure',
    hint: '可复用的操作步骤、命令序列或排查流程'
  },
  {
    id: 'constraint',
    dir: 'constraints',
    type: 'Constraint',
    hint: '环境或外部强加的硬限制：平台、版本、不可改的接口'
  },
  { id: 'fact', dir: 'concepts', type: 'Fact', hint: '项目里客观成立且不属于以上分类的知识' },
  { id: 'relationship', dir: 'entities', type: 'Entity', hint: '命名实体及其属性与关联' },
  { id: 'other', dir: 'concepts', type: 'Other', hint: '无法归入以上分类的其他记忆' }
]

/** 长期记忆配置默认值(与后端 MemoryConfig::default 一致) */
export const DEFAULT_MEMORY: NapiMemoryConfig = {
  enabled: true,
  autoCompile: false,
  provider: null,
  model: null,
  categories: DEFAULT_MEMORY_CATEGORIES,
  maxUnitsPerCompileCall: 20,
  rawCharBudget: 24000,
  toolResultChars: 1200,
  judgeCandidates: 8,
  extractSelfCritique: false,
  curate: true,
  curateUnitsPerCall: 60,
  minConfidence: 0.6,
  maxUnitsPerSession: 40,
  candidateMinOverlap: 0.25,
  maxTags: 8,
  maxAppendSections: 3,
  relatedConcepts: 3,
  injectMemories: true,
  injectedConcepts: 5
}

/** 默认会话空间基目录:未显式设置时的绝对路径(相对日期工作区会归入其下) */
export function defaultWorkspaceDir(): string {
  return join(app.getPath('home'), 'astorm')
}

/* ---------- 约定文件(AGENTS.md) ---------- */

/** 约定文件名:全局(用户目录根)与会话空间(会话工作区根)同名。 */
export const AGENTS_FILE = 'AGENTS.md'

/** 注入字符上限。**必须与 Rust `agent/prompt.rs::AGENTS_MAX_CHARS` 一致**。 */
export const AGENTS_MAX_CHARS = 8000

/** 编辑器读取的字节上限:超过则不读进 UI(后端另有 1 MiB 守卫)。 */
const AGENTS_UI_MAX_BYTES = 1024 * 1024

/** 默认约定文件注入配置(与后端 `InstructionConfig::default` 一致,均开启) */
export const DEFAULT_INSTRUCTIONS: NapiInstructionConfig = {
  injectGlobalAgents: true,
  injectWorkspaceAgents: true
}

/**
 * 全局 AGENTS.md 的绝对路径:`<用户目录>/AGENTS.md`。
 *
 * 用户目录 = 设置页「默认会话空间基目录」(未设置回退 `~/astorm`)。后端由
 * `AppDir::new(config).user_dir` 按同一规则推导,两侧保持一致。
 */
export async function globalAgentsPath(): Promise<string> {
  return join(await readWorkspace(), AGENTS_FILE)
}

/** 读取全局 AGENTS.md(`exists=false` 表示文件不存在;超大文件不回传内容)。 */
export async function readGlobalAgents(): Promise<{
  path: string
  content: string
  exists: boolean
  tooLarge: boolean
}> {
  const path = await globalAgentsPath()
  try {
    const st = await stat(path)
    if (!st.isFile()) return { path, content: '', exists: false, tooLarge: false }
    if (st.size > AGENTS_UI_MAX_BYTES) return { path, content: '', exists: true, tooLarge: true }
    return { path, content: await readFile(path, 'utf8'), exists: true, tooLarge: false }
  } catch {
    // 不存在 / 不可读:以空内容呈现,保存时再创建
    return { path, content: '', exists: false, tooLarge: false }
  }
}

/** 写入全局 AGENTS.md(自动创建用户目录)。 */
export async function writeGlobalAgents(content: string): Promise<{ path: string; ok: boolean }> {
  const path = await globalAgentsPath()
  await writeFileAtomic(path, content)
  return { path, ok: true }
}

/** 读取默认会话空间基目录(缺失 / 损坏回退默认值)。 */
export async function readWorkspace(): Promise<string> {
  const raw = await readJsonFile(settingsFile())
  return typeof raw?.workspace === 'string' && raw.workspace.trim() ? raw.workspace : defaultWorkspaceDir()
}

async function readJsonFile(file: string): Promise<Record<string, unknown> | null> {
  try {
    // Windows 记事本等编辑器会写 UTF-8 BOM；JSON.parse 会因此抛错并静默回退默认值。
    const parsed = JSON.parse((await readFile(file, 'utf8')).replace(/^\uFEFF/, '')) as unknown
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : null
  } catch {
    return null
  }
}

/** 读取用户语言偏好(backend-settings.json 的 locale;缺失 / 非法回退 zh-CN)。 */
export async function readLocale(): Promise<string> {
  const raw = await readJsonFile(settingsFile())
  const locale = raw?.locale
  return typeof locale === 'string' && locale.trim() ? locale.trim() : 'zh-CN'
}

/** 整包写 backend-settings.json(前端偏好与后端摘要配置同文件,主题色不在此文件)。 */
export async function writeSettingsJson(payload: Record<string, unknown>): Promise<void> {
  await writeFileAtomic(settingsFile(), JSON.stringify(payload, null, 2))
}

/** 读取本地持久化的会话摘要配置(缺失 / 损坏回退后端默认值)。 */
export async function readSummary(): Promise<SummarySettings> {
  const raw = await readJsonFile(settingsFile())
  const s = (raw?.summary ?? {}) as Record<string, unknown>
  const out: Record<string, number> = { ...DEFAULT_SUMMARY }
  for (const k of Object.keys(DEFAULT_SUMMARY)) {
    const v = s[k]
    if (typeof v === 'number' && Number.isFinite(v)) out[k] = v
  }
  return out as SummarySettings
}

/** 读取本地持久化的会话消息剪支配置(缺失 / 损坏回退后端默认值)。 */
export async function readCompact(): Promise<CompactSettings> {
  const raw = await readJsonFile(settingsFile())
  const s = (raw?.compact ?? {}) as Record<string, unknown>
  const out: Record<string, number> = { ...DEFAULT_COMPACT }
  for (const k of Object.keys(DEFAULT_COMPACT)) {
    const v = s[k]
    if (typeof v === 'number' && Number.isFinite(v)) out[k] = v
  }
  return out as CompactSettings
}

/** 读取本地持久化的目标循环配置(缺失 / 损坏回退后端默认值)。 */
export async function readGoal(): Promise<GoalSettings> {
  const raw = await readJsonFile(settingsFile())
  const s = (raw?.goal ?? {}) as Record<string, unknown>
  const out: Record<string, number> = { ...DEFAULT_GOAL }
  for (const k of Object.keys(DEFAULT_GOAL)) {
    const v = s[k]
    if (typeof v === 'number' && Number.isFinite(v)) out[k] = v
  }
  return out as GoalSettings
}

/** 读取本地持久化的网络搜索配置(缺失 / 非法引擎回退默认值)。 */
export async function readWebSearch(): Promise<WebSearchSettings> {
  const raw = await readJsonFile(settingsFile())
  const s = (raw?.webSearch ?? {}) as Record<string, unknown>
  const engines = ['baidu', 'sogou', 'bing', 'duckduckgo', 'google']
  const engine = typeof s.defaultEngine === 'string' && engines.includes(s.defaultEngine)
    ? s.defaultEngine
    : DEFAULT_WEB_SEARCH.defaultEngine
  const timeout =
    typeof s.timeoutSec === 'number' && Number.isFinite(s.timeoutSec) && s.timeoutSec > 0
      ? Math.min(120, Math.max(1, Math.round(s.timeoutSec)))
      : DEFAULT_WEB_SEARCH.timeoutSec
  return { defaultEngine: engine, timeoutSec: timeout }
}

/** 读取本地持久化的安全配置(缺失 / 字段损坏逐项回退默认值)。 */
export async function readSecurity(): Promise<NapiSecurityConfig> {
  // 独立 security.json 优先；旧版本把 security 段存在 backend-settings.json 里，作迁移回退
  const dedicated = await readJsonFile(securityFile())
  const legacy = dedicated ? null : await readJsonFile(settingsFile())
  const s = (dedicated ?? legacy?.security ?? {}) as Partial<NapiSecurityConfig>
  const arr = (v: unknown): string[] =>
    Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string' && x.trim().length > 0) : []
  const bool = (v: unknown, d: boolean): boolean => (typeof v === 'boolean' ? v : d)
  const num = (v: unknown): number | null =>
    typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : null
  // CPU 使用率：1-100 之外的值视为不限
  const pct = (v: unknown): number | null => {
    const n = num(v)
    return n !== null && n <= 100 ? n : null
  }
  // 资源限制默认值（与后端 SecurityResourceConfig::default 一致）：
  // 键**缺失**（未配置/旧配置文件）→ 回退默认；键**显式存在**（含 null = 用户选择不限）→ 尊重用户值
  const res = (s.resource ?? {}) as Record<string, unknown>
  const numOr = (key: string, fallback: number): number | null =>
    key in res ? num(res[key]) : fallback
  // 文件名单：键**缺失** = 从未保存过安全配置 → 用后端默认值（凭据路径清单）；
  // 键**显式存在**（含空数组 = 用户清空 = 关闭该防护）→ 尊重用户值，绝不把默认清单合并回去。
  const file = (s.file ?? {}) as Record<string, unknown>
  const fileDefaults = backendSecurityDefaults()?.file
  const fileArr = (
    key: 'readBlack' | 'readWhite' | 'writeBlack' | 'writeWhite',
    fallback: string[]
  ): string[] => (key in file ? arr(file[key]) : (fileDefaults?.[key] ?? fallback))
  return {
    enabled: bool(s.enabled, true),
    file: {
      readBlack: fileArr('readBlack', DEFAULT_SECURITY.file.readBlack),
      readWhite: fileArr('readWhite', DEFAULT_SECURITY.file.readWhite),
      writeBlack: fileArr('writeBlack', DEFAULT_SECURITY.file.writeBlack),
      writeWhite: fileArr('writeWhite', DEFAULT_SECURITY.file.writeWhite)
    },
    net: {
      useWhitelist: bool(s.net?.useWhitelist, true),
      inBlack: arr(s.net?.inBlack),
      inWhite: arr(s.net?.inWhite),
      outBlack: arr(s.net?.outBlack),
      outWhite: arr(s.net?.outWhite)
    },
    danger: {
      useBuiltin: bool(s.danger?.useBuiltin, true),
      // 内置清单编辑副本：键缺失 = 跟随代码内置；显式数组 = 用户副本（可为空数组 = 清空内置）
      builtinPatterns: Array.isArray(s.danger?.builtinPatterns) ? arr(s.danger.builtinPatterns) : null,
      black: arr(s.danger?.black)
    },
    resource: {
      memoryMb: numOr('memoryMb', 4096),
      diskMb: numOr('diskMb', 1024),
      cpuSeconds: numOr('cpuSeconds', 600),
      // CPU 使用率:键缺失 → 回退默认 95%（整机占比）;显式 null / 越界 → 不限
      cpuPercent: 'cpuPercent' in res ? pct(res.cpuPercent) : 95,
      gpu: bool(res.gpu, true)
    },
    toolchainDisabled: arr(s.toolchainDisabled),
    execWhite: arr(s.execWhite)
  }
}

/** 写独立安全配置文件（整段 security 落盘；不在 backend-settings.json 留副本）。 */
export async function writeSecurityConfig(security: unknown): Promise<void> {
  await writeFileAtomic(securityFile(), JSON.stringify(security, null, 2))
}

/**
 * 旧配置迁移（幂等）：把 backend-settings.json 的 security 段搬到 security.json，
 * 并从原文件删除，保证安全配置只有一份来源。security.json 已存在时不覆盖。
 */
export async function migrateSecurityConfig(): Promise<void> {
  if (existsSync(securityFile())) return
  const raw = await readJsonFile(settingsFile())
  if (!raw) return
  const legacy = raw.security
  if (!legacy || typeof legacy !== 'object' || Array.isArray(legacy)) return
  await writeSecurityConfig(legacy)
  delete raw.security
  await writeSettingsJson(raw)
}

/**
 * 确保安全配置文件存在后返回其路径：先迁移旧配置；从未配置过则按当前生效值
 * （含后端默认凭据名单）落盘，用户打开/手改时看到的就是真实生效配置。
 */
export async function ensureSecurityConfigFile(): Promise<string> {
  await migrateSecurityConfig()
  if (!existsSync(securityFile())) await writeSecurityConfig(await readSecurity())
  return securityFile()
}

/** 读取本地持久化的长期记忆配置(缺失 / 字段损坏逐项回退默认值;分类表空表回退默认表)。 */
export async function readMemory(): Promise<NapiMemoryConfig> {
  const raw = await readJsonFile(settingsFile())
  const s = (raw?.memory ?? {}) as Record<string, unknown>
  const bool = (v: unknown, d: boolean): boolean => (typeof v === 'boolean' ? v : d)
  const num = (v: unknown, d: number): number => (typeof v === 'number' && Number.isFinite(v) ? v : d)
  // 数字区间钳制：设置页与手改文件都走这里，越界值直接收敛到可用范围
  const clamped = (v: unknown, d: number, min: number, max: number): number =>
    Math.min(max, Math.max(min, num(v, d)))
  const str = (v: unknown): string => (typeof v === 'string' ? v.trim() : '')
  const optStr = (v: unknown): string | null => (str(v) ? str(v) : null)

  const categories: NapiMemoryCategoryConfig[] = Array.isArray(s.categories)
    ? s.categories
        .map((c) => {
          const row = (c ?? {}) as Record<string, unknown>
          return { id: str(row.id), dir: str(row.dir), type: str(row.type), hint: str(row.hint) }
        })
        .filter((c) => c.id.length > 0)
    : []
  return {
    enabled: bool(s.enabled, DEFAULT_MEMORY.enabled),
    autoCompile: bool(s.autoCompile, DEFAULT_MEMORY.autoCompile),
    provider: optStr(s.provider),
    model: optStr(s.model),
    // 空表 / 全非法 → 后端 `effective_categories` 会回退默认表；前端一并对齐，
    // 免得设置页显示"没有分类"而后端其实在用默认 9 条
    categories: categories.length > 0 ? categories : DEFAULT_MEMORY_CATEGORIES,
    maxUnitsPerCompileCall: clamped(s.maxUnitsPerCompileCall, DEFAULT_MEMORY.maxUnitsPerCompileCall, 1, 200),
    rawCharBudget: clamped(s.rawCharBudget, DEFAULT_MEMORY.rawCharBudget, 4000, 200000),
    toolResultChars: clamped(s.toolResultChars, DEFAULT_MEMORY.toolResultChars, 200, 50000),
    judgeCandidates: clamped(s.judgeCandidates, DEFAULT_MEMORY.judgeCandidates, 0, 50),
    extractSelfCritique: bool(s.extractSelfCritique, DEFAULT_MEMORY.extractSelfCritique),
    curate: bool(s.curate, DEFAULT_MEMORY.curate),
    curateUnitsPerCall: clamped(s.curateUnitsPerCall, DEFAULT_MEMORY.curateUnitsPerCall, 1, 500),
    minConfidence: clamped(s.minConfidence, DEFAULT_MEMORY.minConfidence, 0, 1),
    maxUnitsPerSession: clamped(s.maxUnitsPerSession, DEFAULT_MEMORY.maxUnitsPerSession, 0, 500),
    candidateMinOverlap: clamped(s.candidateMinOverlap, DEFAULT_MEMORY.candidateMinOverlap, 0, 1),
    maxTags: clamped(s.maxTags, DEFAULT_MEMORY.maxTags, 0, 100),
    maxAppendSections: clamped(s.maxAppendSections, DEFAULT_MEMORY.maxAppendSections, 0, 100),
    relatedConcepts: clamped(s.relatedConcepts, DEFAULT_MEMORY.relatedConcepts, 0, 20),
    injectMemories: bool(s.injectMemories, DEFAULT_MEMORY.injectMemories),
    injectedConcepts: clamped(s.injectedConcepts, DEFAULT_MEMORY.injectedConcepts, 0, 20)
  }
}

/** 读取本地持久化的约定文件配置(缺失 / 字段损坏逐项回退默认值)。 */
export async function readInstructions(): Promise<NapiInstructionConfig> {
  const raw = await readJsonFile(settingsFile())
  const s = (raw?.instructions ?? {}) as Record<string, unknown>
  const bool = (v: unknown, d: boolean): boolean => (typeof v === 'boolean' ? v : d)
  return {
    injectGlobalAgents: bool(s.injectGlobalAgents, DEFAULT_INSTRUCTIONS.injectGlobalAgents),
    injectWorkspaceAgents: bool(s.injectWorkspaceAgents, DEFAULT_INSTRUCTIONS.injectWorkspaceAgents)
  }
}

/** 摘要 → 后端 summary(camelCase 键,与后端 AppConfig.summary 对齐)。 */
export function mapSummary(summary: SummarySettings): NapiSummaryConfig {
  return {
    tokenThreshold: summary.token_threshold,
    minMessages: summary.min_messages,
    keepTokenThreshold: summary.keep_token_threshold,
    keepTailRounds: summary.keep_tail_rounds
  }
}

/** 剪支 → 后端 compact(camelCase 键,与后端 AppConfig.compact 对齐)。 */
export function mapCompact(compact: CompactSettings): NapiCompactConfig {
  return {
    compactThreshold: compact.compact_threshold,
    compactRecentRounds: compact.compact_recent_rounds,
    compactByteCount: compact.compact_byte_count,
    compactHeadChars: compact.compact_head_chars,
    compactTailChars: compact.compact_tail_chars
  }
}

/** 目标循环 → 后端 goal(camelCase 键,与后端 AppConfig.goal 对齐)。 */
export function mapGoal(goal: GoalSettings): NapiGoalConfig {
  return {
    maxTokens: goal.max_tokens,
    maxIterations: goal.max_iterations
  }
}

/** 组装后端启动 / 热更配置:只含后端字段,不含主题 / 语言。 */
/**
 * agent-browser 可执行文件路径(browser 工具用)。
 *
 * dev 取仓库内 `frontend/resources/agent-browser/<os>-<arch>/`(scripts/sync-agent-browser.mjs 同步);
 * 打包取 `process.resourcesPath/agent-browser/<os>-<arch>/`(electron-builder extraResources)。
 * 找不到返回 undefined —— Rust 侧再回退 ASTORM_AGENT_BROWSER_BIN 与 PATH,工具会给出可读指引。
 */
function resolveAgentBrowserBinary(): string | undefined {
  const exe = process.platform === 'win32' ? 'agent-browser.exe' : 'agent-browser'
  const rel = join('agent-browser', `${process.platform}-${process.arch}`, exe)
  const candidates = app.isPackaged
    ? [join(process.resourcesPath, rel)]
    : [join(__dirname, '../../resources', rel), join(process.cwd(), 'resources', rel)]
  return candidates.find((p) => existsSync(p))
}

/**
 * astorm-office 套件根目录(scripts/sync-office.mjs 拷贝的 office-kit 导出包)。
 *
 * dev 取仓库内 `frontend/resources/office`;打包取 `process.resourcesPath/office`
 * (electron-builder extraResources)。不存在返回 undefined —— office 工具报「未同步」、
 * 预览回退旧渲染器/外部打开,功能整体降级不阻塞。
 */
export function resolveOfficeDir(): string | undefined {
  const candidates = app.isPackaged
    ? [join(process.resourcesPath, 'office')]
    : [join(__dirname, '../../resources/office'), join(process.cwd(), 'resources/office')]
  return candidates.find((p) => existsSync(join(p, 'bin')) && existsSync(join(p, 'skill', 'SKILL.md')))
}

/**
 * 页面渲染助手脚本(page-render.cjs)路径:复用应用自带 Chromium 的隐藏窗口渲染页面,
 * 供 Rust `websearch`(百度/谷歌)与 `webfetch`(正文抓取)使用。
 *
 * dev 取仓库内 `frontend/resources/page-render.cjs`;打包取 `process.resourcesPath/page-render.cjs`
 * (electron-builder extraResources)。找不到返回 undefined —— 后端退回纯 HTTP 抓取。
 */
function resolvePageRendererScript(): string | undefined {
  const rel = 'page-render.cjs'
  const candidates = app.isPackaged
    ? [join(process.resourcesPath, rel)]
    : [join(__dirname, '../../resources', rel), join(process.cwd(), 'resources', rel)]
  return candidates.find((p) => existsSync(p))
}

export function buildFullNapiConfig(
  summary: SummarySettings,
  compact: CompactSettings,
  workspace: string,
  security: NapiSecurityConfig = DEFAULT_SECURITY,
  memory: NapiMemoryConfig = DEFAULT_MEMORY,
  instructions: NapiInstructionConfig = DEFAULT_INSTRUCTIONS,
  goal: GoalSettings = DEFAULT_GOAL,
  webSearch: WebSearchSettings = DEFAULT_WEB_SEARCH,
  locale: string = 'zh-CN'
): NapiAppConfig {
  const providers = getProvidersSnapshot()
  const officeDir = resolveOfficeDir()
  return {
    log: { level: 'info' },
    workspace,
    llm: {
      providers,
      defaultProvider: providers[0]?.name
    },
    summary: mapSummary(summary),
    compact: mapCompact(compact),
    goal: mapGoal(goal),
    browser: { binary: resolveAgentBrowserBinary() },
    // astorm-office 套件:office 工具(spawn 五 CLI)+ 内置 office skill(真实目录注册)
    office: resolveOfficeDir()
      ? {
          binDir: join(officeDir!, 'bin'),
          skillDir: join(officeDir!, 'skill')
        }
      : undefined,
    webSearch: {
      defaultEngine: webSearch.defaultEngine,
      timeoutSec: webSearch.timeoutSec
    },
    // 页面渲染服务(websearch/webfetch 共用):复用 Electron 自带 Chromium 的常驻隐藏窗口
    webRender: {
      bin: process.execPath,
      script: resolvePageRendererScript(),
      // 独立 profile:避免与主应用 Electron / 多进程争用 userData 锁
      profileDir: join(systemDir(), 'render-profile'),
      concurrency: 2,
      queueMax: 16,
      idleSecs: 300,
      minIntervalMs: 800
    },
    security,
    memory,
    instructions,
    locale
  }
}

/** 把当前持久化配置推给后端热更新(未加载绑定 / 未运行则为空操作)。 */
export async function syncBackendConfig(): Promise<void> {
  const summary = await readSummary()
  const compact = await readCompact()
  const workspace = await readWorkspace()
  const security = await readSecurity()
  const memory = await readMemory()
  const instructions = await readInstructions()
  const goal = await readGoal()
  const webSearch = await readWebSearch()
  const locale = await readLocale()
  syncNapiConfig(
    buildFullNapiConfig(summary, compact, workspace, security, memory, instructions, goal, webSearch, locale)
  )
}

/** 启动装配:先确保供应商快照已加载,再按完整配置推后端。 */
export async function bootstrapConfig(): Promise<void> {
  await ensureProvidersLoaded()
  // 旧版本的安全配置存在 backend-settings.json 里，启动即迁移到独立文件
  await migrateSecurityConfig()
  await syncBackendConfig()
}