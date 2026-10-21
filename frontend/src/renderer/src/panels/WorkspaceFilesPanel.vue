<script setup lang="ts">
/**
 * 工作区文件页签：浏览当前会话工作区的文件树。
 *
 * 与左侧栏的文件模式共用**渲染层**（`FileTreeNode` + `utils/fileIcons` 手写图标 +
 * `fileExplorer.*` 文案），但**状态各持一份**——左侧文件树的状态在 Pinia 单例
 * `stores/files.ts`（`root/tree/expanded/query`）里，两处同时挂载会互相抢状态，
 * 故本面板自带一套精简状态：树 + 搜索 + 刷新/在访达打开 + 点击预览。
 *
 * 刻意不做（属左栏文件模式的职责）：git 变更徽章、忽略淡显、「查看变更」开关、
 * 创建 Git 仓库。`FileTreeNode` 的 `gitStatusOf` / `isIgnoredPath` inject 自带默认值
 * （`''` / `false`），这里**不 provide** 即天然关闭这两个特性，无需改动该组件。
 */
import { computed, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import { useTabsStore } from '@/stores/tabs'
import { invoke } from '@/services/backend'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { isWebShell } from '@/utils/platform'
import type { FileNode } from '@/types'
import FileTreeNode from '@/components/sidebar/FileTreeNode.vue'

const { t } = useI18n()
const sessions = useSessionsStore()
const tabs = useTabsStore()

const activeSession = computed(
  () => sessions.sessions.find((s) => s.id === sessions.activeId) ?? null
)

/** 工作区绝对路径（空 = 该会话没有工作区目录） */
const dir = ref('')
/** 整棵文件树（后端已递归；展开态存于节点之外的集合里） */
const tree = ref<FileNode[]>([])
const loading = ref(false)
const error = ref('')
/** 展开态：provide 给递归 FileTreeNode（Set 引用替换式更新） */
const expanded = ref<Set<string>>(new Set())
/** 搜索词（防抖后驱动过滤） */
const query = ref('')
/** 首次目录解析是否已完成（异步解析期间不显示「没有工作区目录」空态，避免闪一下） */
const resolvedOnce = ref(false)

provide('expandedPaths', expanded)

/** 目录名（末段路径），根行展示用 */
const rootName = computed(() => {
  const r = dir.value
  if (!r) return ''
  const parts = r.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? r
})

/** napi 对象字段为 Rust 名 camelCase(nodeType)，前端契约统一为 type；递归归一化 */
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

/** 加载目录文件树。切换目录时重置展开态与搜索词（刷新同一目录则保留） */
async function load(next: string): Promise<void> {
  const nextDir = next.trim().replace(/[\\/]+$/, '')
  if (!nextDir) {
    dir.value = ''
    tree.value = []
    error.value = ''
    return
  }
  if (nextDir !== dir.value) {
    expanded.value = new Set()
    query.value = ''
  }
  dir.value = nextDir
  loading.value = true
  error.value = ''
  try {
    const list = (await invoke('list_directory', { dir: nextDir })) as FileNode[]
    const nodes = Array.isArray(list) ? list : []
    tree.value = nodes.map((n) => normalizeNode(n as unknown as Record<string, unknown>))
    // 顶层目录默认展开（深目录按需展开），与左栏同口径
    const next2 = new Set(expanded.value)
    for (const n of nodes) if (n.type === 'folder') next2.add(n.name)
    expanded.value = next2
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err)
    tree.value = []
  } finally {
    loading.value = false
  }
}

/** 激活会话切换 → 重新解析工作区目录并加载（同目录不重复 IPC） */
watch(
  () => activeSession.value?.workspace,
  async () => {
    try {
      const resolved = await resolveSessionWorkspaceDir(activeSession.value?.workspace)
      if (resolved === dir.value) return
      await load(resolved)
    } finally {
      // 目录解析是异步的，首帧 dir 仍为空；不置位的话会先闪一下「没有工作区目录」
      resolvedOnce.value = true
    }
  },
  { immediate: true }
)

/** 刷新：强制重载当前目录（保留展开态与搜索词） */
function refresh(): void {
  const cur = dir.value
  if (!cur) return
  tree.value = []
  void load(cur)
}

// ── 已打开的预览文件（相对路径集合）：树中对应行高亮，与左栏同口径 ──
const openFilePaths = computed(() => {
  const set = new Set<string>()
  if (!dir.value) return set
  // 统一正斜杠比较（Windows 绝对路径为反斜杠）
  const root = dir.value.replace(/\\/g, '/')
  for (const tab of tabs.tabs) {
    if (!tab.file) continue
    const fp = String(tab.file.filePath).replace(/\\/g, '/')
    if (fp.startsWith(root + '/')) set.add(fp.slice(root.length + 1))
  }
  return set
})
provide('isOpenFile', (rel: string) => openFilePaths.value.has(rel))

// ── 搜索：200ms 防抖 → 过滤树（命中节点与含命中后代的文件夹都保留并强制展开） ──
let debounceTimer: ReturnType<typeof setTimeout> | null = null
watch(query, () => {
  if (debounceTimer) clearTimeout(debounceTimer)
  debounceTimer = setTimeout(() => {
    const q = query.value.trim().toLowerCase()
    if (q) for (const n of tree.value) expandBranchIfMatch(n, '', q)
  }, 200)
})

/** 命中（自身或后代）的文件夹分支加入展开集合（纯副作用，仅搜索时调用） */
function expandBranchIfMatch(node: FileNode, prefix: string, q: string): void {
  const path = prefix ? `${prefix}/${node.name}` : node.name
  const selfMatch = node.name.toLowerCase().includes(q)
  const childMatch = node.children?.some((c) => {
    let hit = c.name.toLowerCase().includes(q)
    if (!hit && c.type === 'folder') hit = folderDescendantHit(c, q)
    return hit
  })
  if (node.type === 'folder' && (selfMatch || childMatch)) {
    const next = new Set(expanded.value)
    next.add(path)
    expanded.value = next
  }
  if (node.children) for (const c of node.children) expandBranchIfMatch(c, path, q)
}

/** 后代是否命中（递归） */
function folderDescendantHit(node: FileNode, q: string): boolean {
  if (node.name.toLowerCase().includes(q)) return true
  return (node.children ?? []).some((c) => folderDescendantHit(c, q))
}

/** 递归过滤：命中自身或后代命中 → 保留（展示层纯函数，不改展开态） */
function filterTree(nodes: FileNode[], q: string): FileNode[] {
  if (!q) return nodes
  const result: FileNode[] = []
  for (const node of nodes) {
    const matchSelf = node.name.toLowerCase().includes(q)
    if (node.type === 'folder' && node.children?.length) {
      const children = filterTree(node.children, q)
      if (matchSelf || children.length > 0) {
        result.push({ ...node, children: children.length ? children : node.children })
      }
    } else if (matchSelf) {
      result.push(node)
    }
  }
  return result
}

/** 展示节点列表（搜索过滤后） */
const displayNodes = computed<FileNode[]>(() =>
  filterTree(tree.value, query.value.trim().toLowerCase())
)

/** 点击文件：打开右侧文件预览页签（路径 = 工作区目录 + 相对路径） */
function openFile(payload: { path: string; name: string; type: FileNode['type'] }): void {
  if (payload.type !== 'file') return
  const full = dir.value.replace(/[\\/]+$/, '') + '/' + payload.path
  tabs.openFileTab({ path: full, name: payload.name })
}

/** 从系统资源管理器打开工作区目录 */
function openInExplorer(): void {
  if (dir.value) void window.api.fs.openFolder(dir.value)
}
</script>

<template>
  <div class="wf-panel">
    <!-- 搜索框（与左栏文件树同一套样式与文案） -->
    <div class="wf-search">
      <icon-search :size="15" class="wf-search-icon" />
      <input
        v-model="query"
        type="text"
        class="wf-search-input"
        :placeholder="t('fileExplorer.searchPlaceholder')"
        spellcheck="false"
      />
    </div>

    <div class="wf-tree">
      <div v-if="loading" class="wf-hint">{{ t('common.loading') }}</div>
      <div v-else-if="error" class="wf-hint wf-error">
        {{ t('fileExplorer.loadError') }}: {{ error }}
      </div>
      <!-- 无工作区目录：目录解析完成前不判空（异步解析期间首帧 dir 必为空，否则会闪一下空态） -->
      <div v-else-if="!dir" class="wf-hint">
        {{ resolvedOnce ? t('panel.workspaceFiles.noWorkspace') : t('common.loading') }}
      </div>
      <div v-else class="wf-tree-inner">
        <!-- 根目录行：固定文本（无图标/折叠箭头），右侧操作按钮右对齐（同左栏写法） -->
        <div class="wf-root" :title="dir">
          <span class="wf-root-name">{{ rootName }}</span>
          <span class="wf-root-actions">
            <!-- 系统资源管理器打开：浏览器部署无此能力，隐藏按钮 -->
            <button
              v-if="!isWebShell"
              type="button"
              class="wf-act-btn"
              :title="t('fileExplorer.openInExplorer')"
              @click.stop="openInExplorer"
            >
              <icon-folder :size="13" />
            </button>
            <button
              type="button"
              class="wf-act-btn"
              :title="t('fileExplorer.refresh')"
              @click.stop="refresh"
            >
              <icon-refresh :size="13" />
            </button>
          </span>
        </div>

        <!-- 子节点（根目录固定展开） -->
        <div v-if="displayNodes.length" class="wf-list">
          <FileTreeNode
            v-for="child in displayNodes"
            :key="child.name"
            :node="child"
            :depth="1"
            :path="child.name"
            :child-base-path="child.name"
            @open="openFile"
          />
        </div>
        <div v-else class="wf-hint">{{ t('fileExplorer.empty') }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wf-panel {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  gap: 4px;
  /* 与顶栏间距 5px（其余方向保持 8px） */
  padding: 5px 8px 8px;
}

/* 搜索框：图标 + 输入，聚焦时边框高亮（同左栏 .fe-search） */
.wf-search {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  height: 32px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  background-color: var(--bg-2);
  color: var(--text-secondary);
  transition: border-color 0.15s ease;
}

.wf-search:focus-within {
  border-color: var(--border-strong);
}

.wf-search-icon {
  flex-shrink: 0;
  color: var(--text-tertiary);
}

.wf-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
}

.wf-search-input::placeholder {
  color: var(--text-tertiary);
}

/* 树容器：滚动条走全局那套（4px 细条 / 空闲隐藏 / 滚动时出现，同左栏） */
.wf-tree {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.wf-tree-inner {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.wf-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.wf-hint {
  padding: 8px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  text-align: center;
}

.wf-error {
  color: var(--danger);
  word-break: break-all;
}

/* 根行：与 FileTreeNode 的 .ft-row 同一套紧凑尺寸（14px 文字 / 26px 行高）。
   注：FileTreeNode 的 .ft-row/.ft-name 是组件内 scoped 样式，这里按同款补齐 */
.wf-root {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  min-height: 26px;
  padding: 1px 6px 1px 4px;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  transition: background-color var(--transition-fast);
}

.wf-root:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

.wf-root-name {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-md);
  font-weight: 500;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.wf-root-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  margin-left: 6px;
  flex-shrink: 0;
}

.wf-act-btn {
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-tertiary);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.wf-act-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
</style>
