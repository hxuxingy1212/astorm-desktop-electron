import type { MentionContext, MentionGroup, MentionItem, MentionProvider, MentionTrigger } from './types'
import { BUILTIN_COMMANDS } from './config'
import { listWorkspaceEntries } from './fileSource'
import { fetchMentionAgents, fetchMentionSkills } from './source'
import { getFileIconUrl, getFolderIconUrl } from '@/utils/fileIcons'

/**
 * 提及注册表:静态条目(TS 配置内置 + register* API 运行时追加)+ 动态 Provider。
 *
 * 触发符约定:
 *   '/' → commandMap(快捷命令)
 *   '$' → skillMap + agentMap(技能 / 子智能体)
 *   '@' → fileMap(文件引用;默认由内置 Provider 列会话工作目录,注册条目可覆盖同名 token)
 */
const commandMap = new Map<string, MentionItem>()
const skillMap = new Map<string, MentionItem>()
const agentMap = new Map<string, MentionItem>()
const fileMap = new Map<string, MentionItem>()
const providers = new Map<MentionTrigger, MentionProvider>()

function mapOf(type: MentionItem['type']): Map<string, MentionItem> {
  switch (type) {
    case 'command':
      return commandMap
    case 'skill':
      return skillMap
    case 'agent':
      return agentMap
    case 'file':
      return fileMap
  }
}

function seed(item: MentionItem): void {
  mapOf(item.type).set(item.id, item)
}

// ---------- 注册 API ----------

/** 注册任意类型条目(插件/动态场景) */
export function registerMention(item: MentionItem): void {
  seed(item)
}

export function registerCommand(def: Omit<MentionItem, 'type'>): void {
  seed({ ...def, type: 'command' })
}

export function registerSkill(def: Omit<MentionItem, 'type'>): void {
  seed({ ...def, type: 'skill' })
}

export function registerAgent(def: Omit<MentionItem, 'type'>): void {
  seed({ ...def, type: 'agent' })
}

export function unregisterMention(id: string): void {
  for (const map of [commandMap, skillMap, agentMap, fileMap]) map.delete(id)
}

/** 注册动态数据源 Provider(如 @ 文件引用,数据来自当前会话目录) */
export function registerProvider(provider: MentionProvider): void {
  providers.set(provider.trigger, provider)
}

export function hasProvider(trigger: string): boolean {
  return providers.has(trigger as MentionTrigger)
}

// ---------- 查询与解析 ----------

export function queryMentions(
  trigger: MentionTrigger,
  query: string,
  ctx: MentionContext
): Promise<MentionGroup[]> {
  const provider = providers.get(trigger)
  if (!provider) return Promise.resolve([])
  return Promise.resolve(provider.fetch(query, ctx))
}

/** 依据 token(如 '/goal'、'$code-review')反查条目,消息侧渲染用 */
export function resolveToken(token: string): MentionItem | null {
  const trigger = token[0] as MentionTrigger
  const id = token.slice(1)
  if (trigger === '/') return commandMap.get(id) ?? null
  if (trigger === '$') return skillMap.get(id) ?? agentMap.get(id) ?? null
  if (trigger === '@') return fileMap.get(id) ?? null
  return null
}

// ---------- 内置 Provider ----------

/**
 * '/' 快捷命令:单一分组(不做分类);命令名用原始英文(`/init`),描述按界面语言呈现。
 */
const CommandProvider: MentionProvider = {
  trigger: '/',
  fetch() {
    const items = [...commandMap.values()]
    if (!items.length) return []
    return [{ id: 'commands', labelKey: 'mentions.groups.commands', items }]
  }
}

/**
 * '$' 技能 / 子智能体:从后端 list_skills / list_agents 动态拉取,按 type 分成两组。
 * 拉取后 seed 进注册表,供消息侧 token 反查(技能 `$name`、智能体 `$agent/id`)。
 */
const SkillProvider: MentionProvider = {
  trigger: '$',
  async fetch() {
    const [skills, agents] = await Promise.all([fetchMentionSkills(), fetchMentionAgents()])
    for (const item of skills) seed(item)
    for (const item of agents) seed(item)
    const groups: MentionGroup[] = []
    if (skills.length) groups.push({ id: 'skills:all', labelKey: 'mentions.groups.skills', items: skills })
    if (agents.length) groups.push({ id: 'agents:all', labelKey: 'mentions.groups.agents', items: agents })
    return groups
  }
}

/** '@' 文件引用:列出会话工作目录下的文件/文件夹(工作目录缺省时不返回条目,弹层提示先选目录) */
const FileProvider: MentionProvider = {
  trigger: '@',
  async fetch(_query, ctx) {
    const dir = ctx.sessionDir?.trim()
    if (!dir) return []
    const entries = await listWorkspaceEntries(dir)
    if (!entries.length) return []
    const items: MentionItem[] = entries.map((e) => {
      const isFolder = e.type === 'folder'
      return {
        type: 'file',
        // 文件夹 id 带尾斜杠:与同名文件区分,token 形如 '@backend/crates/'
        id: isFolder ? `${e.relPath}/` : e.relPath,
        label: isFolder ? `${e.name}/` : e.name,
        // 副标题 = 父目录(根级文件为空,不显示灰字)
        description: e.dir ? `${e.dir}/` : '',
        icon: isFolder ? 'icon-folder' : 'icon-file',
        iconUrl: isFolder ? getFolderIconUrl() : getFileIconUrl(e.name),
        color: 'var(--accent-text)',
        keywords: [e.relPath]
      }
    })
    // 不带分组标题:文件列表本身就是弹层主体(对齐设计稿,唯一分组不显示「文件」头)
    return [{ id: 'files:workspace', items }]
  }
}

// 模块加载即注册内置命令与全部 Provider(技能/智能体由 SkillProvider 运行时拉取)
for (const item of BUILTIN_COMMANDS) seed(item)
registerProvider(CommandProvider)
registerProvider(SkillProvider)
registerProvider(FileProvider)

/**
 * 启动预取技能/智能体并 seed 注册表:保证页面刷新后,历史消息里的 `$` token
 * 无需先打开 `$` 弹层也能反查到条目并渲染为 chip(仅做一次)。
 */
let hydratePromise: Promise<void> | null = null
export function hydrateMentions(): Promise<void> {
  if (!hydratePromise) {
    hydratePromise = Promise.all([fetchMentionSkills(), fetchMentionAgents()]).then(([skills, agents]) => {
      for (const item of skills) seed(item)
      for (const item of agents) seed(item)
    })
  }
  return hydratePromise
}