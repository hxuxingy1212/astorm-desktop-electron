import { defineStore } from 'pinia'
import type { FileNode } from '@/types'
import { invoke } from '@/services/backend'
import { clearWorkspaceEntriesCache } from '@/components/mentions/fileSource'

/**
 * 项目工作区文件浏览状态:
 * 左侧栏进入「文件」模式时按项目工作区目录加载整棵文件树(list_directory 递归),
 * 文件预览页签经 read_file_content 惰性读内容(1MB 上限 / 二进制守卫由后端处理)。
 */
/** napi 对象字段为 Rust 名 camelCase(nodeType),前端契约统一为 type;递归归一化 */
function normalizeNode(node: Record<string, unknown>): FileNode {
  const name = typeof node.name === 'string' ? node.name : ''
  const rawType = node.type ?? node.nodeType ?? 'file'
  const children = Array.isArray(node.children) ? node.children.map((c) => normalizeNode(c as Record<string, unknown>)) : undefined
  return {
    name,
    type: rawType === 'folder' ? 'folder' : 'file',
    children
  }
}

export const useFilesStore = defineStore('files', {
  state: () => ({
    /** 当前浏览的项目工作区目录(absolute) */
    root: '',
    /** 左侧栏是否处于文件树浏览模式(进入/退出文件树时切换;供其它模块判断,如打开页签不收缩侧栏) */
    active: false,
    /** 整棵文件树(后端已递归;展开/折叠为前端局部状态,存于节点不可变对象外) */
    tree: [] as FileNode[],
    loading: false,
    error: '',
    /** 已展开的目录路径集合(参考工程在树节点上就地改 expanded,这里用路径集合保持纯状态) */
    expanded: new Set<string>(),
    /** 上次搜索词(触发过滤) */
    query: ''
  }),
  actions: {
    /** 加载指定目录文件树(幂等:同目录不重复加载;新目录重置展开态)。
     *  force 强制重新加载(刷新按钮):同一目录下保留已展开路径与搜索词,
     *  只更新树内容;切换目录时才重置展开态 */
    async loadTree(root: string, force = false): Promise<void> {
      const dir = root.trim().replace(/[\\/]+$/, '')
      if (!dir) {
        this.error = 'dir-empty'
        return
      }
      const sameDir = this.root === dir
      if (!force && sameDir && this.tree.length) return
      if (!sameDir) {
        this.expanded = new Set()
        this.query = ''
      }
      // 强制刷新或切换目录时,同步失效 `@` 文件提及缓存,避免弹层列出过期文件
      if (force || !sameDir) clearWorkspaceEntriesCache(dir)
      this.root = dir
      this.loading = true
      this.error = ''
      try {
        const list = (await invoke('list_directory', { dir })) as FileNode[]
        const nodes = Array.isArray(list) ? list : []
        this.tree = nodes.map((n) => normalizeNode(n as unknown as Record<string, unknown>))
        // 顶层目录默认展开(深目录按需展开)
        for (const n of nodes) if (n.type === 'folder') this.expanded.add(n.name)
      } catch (err) {
        this.error = err instanceof Error ? err.message : String(err)
        this.tree = []
      } finally {
        this.loading = false
      }
    },
    /** 读取文件内容(>1MB/二进制由后端守卫返回空串;错误记录给页签展示) */
    async readContent(path: string): Promise<{ content: string | null; error?: string }> {
      try {
        const content = (await invoke('read_file_content', { path })) as string
        return { content }
      } catch (err) {
        return { content: null, error: err instanceof Error ? err.message : String(err) }
      }
    },
    reset(): void {
      this.root = ''
      this.active = false
      this.tree = []
      this.error = ''
      this.expanded = new Set()
      this.query = ''
    }
  }
})