/**
 * 提及使用频率:localStorage 持久化每个 token 的选择次数,供排序加权
 * (高频/最近使用的条目在弹层里更靠前)。读写全部走内存缓存,批量落盘。
 */

const STORAGE_KEY = 'astorm.mention.usage'

interface UsageEntry {
  count: number
  last: number
}

type UsageMap = Record<string, UsageEntry>

let cache: UsageMap | null = null

function load(): UsageMap {
  if (cache) return cache
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    cache = raw ? (JSON.parse(raw) as UsageMap) : {}
  } catch {
    cache = {}
  }
  return cache
}

function persist(): void {
  if (!cache) return
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cache))
  } catch {
    // 存储不可用(隐私模式等):保持内存态,不影响输入
  }
}

/** 记录一次选择(token 形如 '/goal'、'$code-review'、'$agent/x'、'@src/a.ts') */
export function bumpUsage(token: string): void {
  if (!token) return
  const map = load()
  const prev = map[token]
  map[token] = { count: (prev?.count ?? 0) + 1, last: Date.now() }
  persist()
}

/** 使用次数(排序加权用) */
export function getUsage(token: string): number {
  return load()[token]?.count ?? 0
}

/** 清空使用记录 */
export function clearUsage(): void {
  cache = {}
  persist()
}
