import type { MentionItem } from './types'
import { invoke } from '@/services/backend'

/**
 * `$` 提及的数据源:真实技能(list_skills)与子智能体(list_agents)。
 * 经 invoke() 走主进程 → Rust napi,与 fileSource 一致带 TTL 缓存与并发合并,
 * 避免弹层每次输入都打 IPC。
 *
 * token 约定:
 *   - 技能 `$<name>`(后端技能名即 kebab-case 唯一标识)
 *   - 智能体 `$agent/<agentId>`(加 agent/ 命名空间,与技能区分便于反查)
 */

const CACHE_TTL = 10_000

interface CacheEntry {
  ts: number
  items: MentionItem[]
}

const skillCache = new Map<string, CacheEntry>()
const agentCache = new Map<string, CacheEntry>()
const skillInflight = new Map<string, Promise<MentionItem[]>>()
const agentInflight = new Map<string, Promise<MentionItem[]>>()

/** `$` 条目统一单色,与正文一致(不再按分类/标识取语义色) */
const ITEM_COLOR = 'var(--text-primary)'

/** 逗号分隔的分类串 → 关键词数组(供过滤命中分类名) */
function splitKeywords(...values: (string | string[] | undefined | null)[]): string[] {
  const out: string[] = []
  for (const v of values) {
    if (!v) continue
    const parts = Array.isArray(v) ? v : v.split(',')
    for (const p of parts) {
      const s = p.trim()
      if (s) out.push(s)
    }
  }
  return out
}

interface SkillSummaryLike {
  name?: string
  description?: string
  category?: string | string[]
  tags?: string[]
  enabled?: boolean
}

interface AgentCardLike {
  agentId?: string
  name?: string
  description?: string
  guidance?: string
  builtin?: boolean
}

/** 内置默认主智能体 id:会话缺省时使用,不作为可提及/可委派的子智能体 */
const BUILTIN_MAIN_AGENT_ID = 'builtin-main'
/** 内置子智能体 id → 专属图标(其余自定义智能体用通用人群图标) */
const AGENT_ICON: Record<string, string> = {
  'builtin-explore': 'icon-compass',
  'builtin-general': 'icon-robot'
}

function toSkillItems(raw: SkillSummaryLike[]): MentionItem[] {
  const out: MentionItem[] = []
  for (const s of raw) {
    const name = (s.name ?? '').trim()
    if (!name) continue
    if (s.enabled === false) continue
    out.push({
      type: 'skill',
      id: name,
      label: name,
      description: s.description ?? '',
      // 技能统一用星光图标(与输入框上方胶囊、设置页一致)
      icon: 'icon-sparkles',
      color: ITEM_COLOR,
      keywords: splitKeywords(s.category, s.tags)
    })
  }
  out.sort((a, b) => a.id.localeCompare(b.id))
  return out
}

function toAgentItems(raw: AgentCardLike[]): MentionItem[] {
  const out: MentionItem[] = []
  for (const a of raw) {
    const id = (a.agentId ?? '').trim()
    const name = (a.name ?? '').trim()
    // 默认主智能体是会话缺省,不列出(它只作为默认,不作为子智能体被提及/委派)
    if (!id || !name || id === BUILTIN_MAIN_AGENT_ID) continue
    out.push({
      type: 'agent',
      id: `agent/${id}`,
      label: name,
      description: a.description || a.guidance || '',
      icon: AGENT_ICON[id] ?? 'icon-user-group',
      color: ITEM_COLOR,
      keywords: splitKeywords(id, a.builtin ? '内置 builtin' : undefined)
    })
  }
  out.sort((a, b) => (a.label ?? a.id).localeCompare(b.label ?? b.id))
  return out
}

async function cached(
  cache: Map<string, CacheEntry>,
  inflight: Map<string, Promise<MentionItem[]>>,
  key: string,
  load: () => Promise<MentionItem[]>
): Promise<MentionItem[]> {
  const hit = cache.get(key)
  if (hit && Date.now() - hit.ts < CACHE_TTL) return hit.items
  const pending = inflight.get(key)
  if (pending) return pending
  const task = (async (): Promise<MentionItem[]> => {
    try {
      const items = await load()
      cache.set(key, { ts: Date.now(), items })
      return items
    } catch {
      // 后端不可用:按空列表处理,弹层显示空态,不打断输入
      return []
    } finally {
      inflight.delete(key)
    }
  })()
  inflight.set(key, task)
  return task
}

/** 已启用技能 → 提及条目 */
export function fetchMentionSkills(): Promise<MentionItem[]> {
  return cached(skillCache, skillInflight, 'enabled', async () => {
    const raw = (await invoke('list_skills', { enabledOnly: true })) as SkillSummaryLike[]
    return toSkillItems(Array.isArray(raw) ? raw : [])
  })
}

/** 全部智能体 → 提及条目 */
export function fetchMentionAgents(): Promise<MentionItem[]> {
  return cached(agentCache, agentInflight, 'all', async () => {
    const raw = (await invoke('list_agents')) as AgentCardLike[]
    return toAgentItems(Array.isArray(raw) ? raw : [])
  })
}

/** 清空缓存(技能/智能体被增删改后调用) */
export function clearMentionSourceCache(): void {
  skillCache.clear()
  agentCache.clear()
}
