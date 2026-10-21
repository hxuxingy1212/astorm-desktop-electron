import type { FileNode } from '@/types'
import { invoke } from '@/services/backend'

/**
 * `@` 文件提及的数据源:把 `list_directory` 的递归树(只有节点名、没有路径)
 * 扁平化为「相对路径 + 名字 + 父目录」列表,供弹层过滤展示。
 *
 * 约定:
 * - 跳过以 `.` 开头的文件/目录(与提及列表展示口径一致;node_modules/target/dist/.git
 *   等重目录后端已在 list_directory 内整支跳过)
 * - 排序与展示均以相对路径为准(大小写不敏感),弹层里即「同级文件按名、跨级按路径」
 * - 结果按目录缓存 10s(并合并并发请求):大仓首次 `@` 只付一次全量 IPC 开销
 */

export interface WorkspaceEntry {
  /** 相对工作区根的路径(文件夹不带尾斜杠) */
  relPath: string
  /** 节点名 */
  name: string
  /** 父目录相对路径(根级为空串) */
  dir: string
  type: 'file' | 'folder'
}

const CACHE_TTL = 10_000
const cache = new Map<string, { ts: number; entries: WorkspaceEntry[] }>()
const inflight = new Map<string, Promise<WorkspaceEntry[]>>()

/** napi 对象字段为 Rust 名 camelCase(nodeType),前端契约统一为 type */
function normalizeNode(node: Record<string, unknown>): FileNode {
  const rawType = node.type ?? node.nodeType ?? 'file'
  return {
    name: typeof node.name === 'string' ? node.name : '',
    type: rawType === 'folder' ? 'folder' : 'file',
    children: Array.isArray(node.children)
      ? node.children.map((c) => normalizeNode(c as Record<string, unknown>))
      : undefined
  }
}

/** 递归扁平化:prefix 为当前节点的父目录相对路径 */
function flatten(nodes: FileNode[], prefix: string, out: WorkspaceEntry[]): void {
  for (const node of nodes) {
    if (!node.name || node.name.startsWith('.')) continue
    const relPath = prefix ? `${prefix}/${node.name}` : node.name
    out.push({ relPath, name: node.name, dir: prefix, type: node.type })
    if (node.type === 'folder' && node.children?.length) flatten(node.children, relPath, out)
  }
}

/** 列出工作区条目(带缓存;传入目录为空或不可读时返回空数组,不抛错) */
export async function listWorkspaceEntries(dir: string): Promise<WorkspaceEntry[]> {
  const root = dir.trim().replace(/[\\/]+$/, '')
  if (!root) return []

  const hit = cache.get(root)
  if (hit && Date.now() - hit.ts < CACHE_TTL) return hit.entries

  const pending = inflight.get(root)
  if (pending) return pending

  const task = (async (): Promise<WorkspaceEntry[]> => {
    try {
      const list = (await invoke('list_directory', { dir: root })) as Record<string, unknown>[]
      const out: WorkspaceEntry[] = []
      flatten((Array.isArray(list) ? list : []).map(normalizeNode), '', out)
      out.sort((a, b) => a.relPath.toLowerCase().localeCompare(b.relPath.toLowerCase()))
      cache.set(root, { ts: Date.now(), entries: out })
      return out
    } catch {
      // 目录不存在 / 无权限:按空结果处理(弹层显示无匹配,不打断输入)
      return []
    } finally {
      inflight.delete(root)
    }
  })()

  inflight.set(root, task)
  return task
}

/** 清空缓存(目录省略时全清):工作区文件变更后需要立刻刷新时调用 */
export function clearWorkspaceEntriesCache(dir?: string): void {
  if (dir) cache.delete(dir.trim().replace(/[\\/]+$/, ''))
  else cache.clear()
}
