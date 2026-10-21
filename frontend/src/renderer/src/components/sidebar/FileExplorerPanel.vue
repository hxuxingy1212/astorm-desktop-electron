<script setup lang="ts">
import { computed, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useFilesStore } from '@/stores/files'
import { useTabsStore } from '@/stores/tabs'
import { invoke } from '@/services/backend'
import type { FileNode } from '@/types'
import FileTreeNode from './FileTreeNode.vue'

const { t } = useI18n()
const files = useFilesStore()
const tabs = useTabsStore()

/** 展开路径集合:provide 给递归 FileTreeNode(Set 引用替换式更新) */
const expandedPaths = ref(files.expanded)
provide('expandedPaths', expandedPaths)
watch(
  () => files.expanded,
  (s) => {
    expandedPaths.value = s
  }
)

/** 已打开的预览文件(全路径):树中对应行高亮(参考工程 activeFile) */
const openFilePaths = computed(() => {
  const set = new Set<string>()
  // 统一正斜杠比较(root 来自 Windows 反斜杠绝对路径)
  const root = files.root.replace(/[\\/]+$/, '').replace(/\\/g, '/')
  for (const t of tabs.tabs) {
    if (t.file) {
      const fp = String(t.file.filePath).replace(/\\/g, '/')
      if (fp.startsWith(root + '/')) set.add(fp.slice(root.length + 1))
    }
  }
  return set
})
/** 相对节点路径 → 是否已在预览页签打开 */
function isOpenFile(rel: string): boolean {
  return openFilePaths.value.has(rel)
}
provide('isOpenFile', isOpenFile)

/** 搜索词:200ms 防抖 → 过滤树(命中节点与含命中后代的文件夹都保留并强制展开) */
const query = ref('')
let debounceTimer: ReturnType<typeof setTimeout> | null = null
watch(query, () => {
  if (debounceTimer) clearTimeout(debounceTimer)
  debounceTimer = setTimeout(() => {
    const q = query.value.trim().toLowerCase()
    // 命中文件夹时(含子命中)把对应分支展开——作为搜索的副作用集中在此处理,
    // 展示 computed 保持纯函数(不修改 store 状态)
    files.query = q
    if (q) for (const n of files.tree) expandBranchIfMatch(n, '', q)
  }, 200)
})

/** 命中(自身或后代)的文件夹分支加入展开集合(纯副作用,仅搜索时调用) */
function expandBranchIfMatch(node: FileNode, prefix: string, q: string): void {
  const path = prefix ? `${prefix}/${node.name}` : node.name
  const selfMatch = node.name.toLowerCase().includes(q)
  const childMatch = node.children?.some((c) => {
    let hit = c.name.toLowerCase().includes(q)
    if (!hit && c.type === 'folder') hit = folderDescendantHit(c, q)
    return hit
  })
  if (node.type === 'folder' && (selfMatch || childMatch)) {
    const next = new Set(files.expanded)
    next.add(path)
    files.expanded = next
  }
  if (node.children) for (const c of node.children) expandBranchIfMatch(c, path, q)
}

/** 后代是否命中(递归) */
function folderDescendantHit(node: FileNode, q: string): boolean {
  if (node.name.toLowerCase().includes(q)) return true
  return (node.children ?? []).some((c) => folderDescendantHit(c, q))
}

/** 递归过滤:命中自身或后代命中 → 保留(展示层纯函数,不再改展开态) */
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

/** 点击文件:打开右侧文件预览页签(标题 = 文件名;文件路径与列出的根目录拼接为全路径) */
function openFile(payload: { path: string; name: string; type: FileNode['type'] }): void {
  if (payload.type !== 'file') return
  const full = files.root.replace(/[\\/]+$/, '') + '/' + payload.path
  tabs.openFileTab({ path: full, name: payload.name })
}

/** 根目录名(参考工程:树外层包一个以工作区目录名为 name 的 folder 节点) */
const rootName = computed(() => {
  const r = files.root
  if (!r) return ''
  const parts = r.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? r
})

watch(
  () => rootName.value,
  (name) => {
    if (name && !files.expanded.has(name)) {
      const next = new Set(files.expanded)
      next.add(name)
      files.expanded = next
    }
  },
  { immediate: true }
)

// ── 根目录行操作:仓库 Wiki(预留) / 资源管理器打开 / 查看变更 / 刷新 ──

/** git 变更过滤模式:开启后只展示有变更的文件与包含变更文件的目录 */
const changesMode = ref(false)
/** 相对路径 → git 状态字(A/M/D/R/C…) */
const changeStatus = ref<Map<string, string>>(new Map())
const changesLoading = ref(false)
const changesError = ref('')
/** 当前工作区是否为 git 仓库(空状态据此显示/隐藏「创建 Git 仓库」按钮) */
const isGitRepo = ref(false)

/** 拉取当前工作区 git 状态,归一化路径后写入 changeStatus */
async function loadChanges(): Promise<void> {
  changeStatus.value = new Map()
  changesError.value = ''
  if (!changesMode.value) return
  changesLoading.value = true
  try {
    const st = (await invoke('git_status', { dir: files.root })) as {
      staged?: Array<{ path: string; status: string }>
      unstaged?: Array<{ path: string; status: string }>
    }
    isGitRepo.value = true
    const map = new Map<string, string>()
    const collect = (list?: Array<{ path: string; status: string }>): void => {
      for (const c of list ?? []) {
        const p = c.path.replace(/\\/g, '/')
        if (p && !map.has(p)) map.set(p, c.status || 'M')
      }
    }
    collect(st.staged)
    collect(st.unstaged)
    // git status 路径相对仓库根;若项目工作区是仓库子目录,去掉首段(工作区目录名)再匹配一次
    if (rootName.value) {
      const strip: Map<string, string> = new Map()
      for (const [p, s] of map) {
        const seg = p.split('/')
        if (seg.length > 1 && seg[0] === rootName.value) strip.set(seg.slice(1).join('/'), s)
      }
      for (const [p, s] of strip) if (!map.has(p)) map.set(p, s)
    }
    changeStatus.value = map
    // 变更模式下自动展开所有变更文件的祖先目录,徽标(M/A…)立即可见
    if (map.size) {
      const next = new Set(files.expanded)
      for (const rel of map.keys()) {
        let p = rel
        let idx: number
        while ((idx = p.lastIndexOf('/')) > 0) {
          p = p.slice(0, idx)
          next.add(p)
        }
      }
      files.expanded = next
    }
    // 无变更不是错误:清空错误文案,由模板按「空状态」分支展示
    changesError.value = ''
  } catch (err) {
    changeStatus.value = new Map()
    isGitRepo.value = false
    // 目录非 git 仓库(git_status 报错)不当作严重错误展示,交由空状态引导「创建 Git 仓库」
    changesError.value = ''
  } finally {
    changesLoading.value = false
  }
}

/** 查看变更开关:开启拉取 git 状态并过滤树,关闭还原完整树 */
function toggleChanges(): void {
  changesMode.value = !changesMode.value
  if (changesMode.value) void loadChanges()
  else changeStatus.value = new Map()
}

/** 创建 Git 仓库:初始化后重新拉取变更(空状态按钮,仅目录非 git 仓库时显示) */
async function onCreateRepo(): Promise<void> {
  try {
    await invoke('git_init', { dir: files.root })
    isGitRepo.value = true
    void loadChanges()
    void refreshTree()
  } catch (err) {
    changesError.value = err instanceof Error ? err.message : String(err)
  }
}

/** 刷新:强制重载文件树;变更模式下顺带刷新 git 状态与忽略列表 */
function refreshTree(): void {
  if (!files.root) return
  void files.loadTree(files.root, true)
  void loadIgnored()
  if (changesMode.value) void loadChanges()
}

/** 从系统资源管理器打开工作区目录 */
function openInExplorer(): void {
  if (files.root) void window.api.fs.openFolder(files.root)
}

/** 变更过滤:只保留发生变更的文件与包含变更文件的目录(相对路径与树一致) */
function filterChangedNodes(nodes: FileNode[], prefix: string): FileNode[] {
  const result: FileNode[] = []
  for (const node of nodes) {
    const rel = prefix ? `${prefix}/${node.name}` : node.name
    if (node.type === 'folder' && node.children?.length) {
      const kept = filterChangedNodes(node.children, rel)
      if (kept.length) result.push({ ...node, children: kept })
    } else if (changeStatus.value.has(rel)) {
      result.push(node)
    }
  }
  return result
}

/** 提供给 FileTreeNode:相对路径 → git 状态字(仅变更模式有效)。
 *  未跟踪目录(git 只报 `?? dir/`)时,目录下所有文件同属新增,向下传播 '?' */
function gitStatusOf(rel: string): string {
  if (!changesMode.value) return ''
  const direct = changeStatus.value.get(rel)
  if (direct) return direct
  let prefix = rel
  let idx: number
  while ((idx = prefix.lastIndexOf('/')) > 0) {
    prefix = prefix.slice(0, idx)
    if (changeStatus.value.get(prefix) === '?') return '?'
  }
  return ''
}
provide('gitStatusOf', gitStatusOf)

// ── git 忽略文件:显示在树上但淡色区分(git 项目;非仓库/出错时为空集合不影响) ──
/** 忽略路径集合(相对路径;目录不含尾部斜杠) */
const ignoredPaths = ref<Set<string>>(new Set())

/** 拉取 git 忽略列表(与 status 相同路径归一化) */
async function loadIgnored(): Promise<void> {
  ignoredPaths.value = new Set()
  if (!files.root) return
  try {
    const list = (await invoke('git_ignored', { dir: files.root })) as string[]
    const norm = new Set<string>()
    for (const p of list ?? []) {
      const rel = p.replace(/\\/g, '/')
      norm.add(rel)
      // 工作区是仓库子目录时去掉首段再匹配一次
      if (rootName.value) {
        const seg = rel.split('/')
        if (seg.length > 1 && seg[0] === rootName.value) norm.add(seg.slice(1).join('/'))
      }
    }
    ignoredPaths.value = norm
  } catch {
    ignoredPaths.value = new Set()
  }
}

/** 相对路径(或某级祖先)是否被 git 忽略 */
function isIgnoredPath(rel: string): boolean {
  if (!rel) return false
  if (ignoredPaths.value.has(rel)) return true
  const idx = rel.lastIndexOf('/')
  return idx > 0 ? isIgnoredPath(rel.slice(0, idx)) : false
}
provide('isIgnoredPath', isIgnoredPath)

// 目录加载/刷新后拉取忽略列表(树可用时执行)
watch(
  rootName,
  () => {
    if (files.root) void loadIgnored()
  },
  { immediate: true }
)

/** 展示节点列表:搜索过滤后叠加 git 变更过滤。
 *  变更模式下:有变更只展示变更文件树;无变更返回空(不显示完整文件树) */
const displayNodes = computed<FileNode[]>(() => {
  let nodes = files.tree
  const q = files.query
  if (changesMode.value) {
    // 仅当有变更才展示变更子树;无变更不显示任何文件节点
    if (!changeStatus.value.size) return []
    nodes = filterChangedNodes(nodes, '')
  }
  if (q) nodes = filterTree(nodes, q)
  return nodes
})

</script>

<template>
  <div class="flex h-full min-h-0 flex-col gap-1 px-2 py-2">
    <!-- 头部:搜索框(返回任务按钮在侧栏底部,与「设置→返回对话」同位) -->
    <div class="fe-top">
      <div class="fe-search">
        <icon-search :size="15" class="fe-search-icon" />
        <input
          v-model="query"
          type="text"
          class="fe-search-input"
          :placeholder="t('fileExplorer.searchPlaceholder')"
          spellcheck="false"
        />
      </div>
    </div>

    <!-- 树:顶层为工作区根目录行(固定、无图标,右侧操作按钮组),其下为文件树 -->
    <div class="fe-tree">
      <div v-if="files.loading" class="fe-hint">{{ t('common.loading') }}</div>
      <div v-else-if="files.error" class="fe-hint fe-error">{{ t('fileExplorer.loadError') }}: {{ files.error }}</div>
      <div v-else-if="rootName" class="fe-tree-inner">
        <!-- 根目录行:固定文本(无图标/折叠箭头)+ 仓库 Wiki(预留) / 资源管理器打开 / 查看变更 / 刷新,按钮右对齐 -->
        <div class="ft-row fe-root" style="padding-left: 4px">
          <span class="ft-file-info">
            <span class="ft-name">{{ rootName }}</span>
          </span>
          <span class="fe-root-actions">
            <button type="button" class="fe-act-btn" :title="t('fileExplorer.repoWiki')" @click.stop>
              <icon-book :size="13" />
            </button>
            <button type="button" class="fe-act-btn" :title="t('fileExplorer.openInExplorer')" @click.stop="openInExplorer">
              <icon-folder :size="13" />
            </button>
            <button
              type="button"
              class="fe-act-btn"
              :class="changesMode && 'active'"
              :title="t('fileExplorer.viewChanges')"
              @click.stop="toggleChanges"
            >
              <icon-loading v-if="changesLoading" :size="13" class="fe-spin" />
              <icon-gitlab v-else :size="13" />
            </button>
            <button type="button" class="fe-act-btn" :title="t('fileExplorer.refresh')" @click.stop="refreshTree">
              <icon-refresh :size="13" />
            </button>
          </span>
        </div>

        <!-- 变更模式提示:git 出错 → 红色错误;无变更 → 空状态(参考图:图标+标题+描述+按钮) -->
        <div v-if="changesMode && changesError" class="fe-hint fe-error">{{ changesError }}</div>
        <div v-else-if="changesMode && !changeStatus.size" class="fe-hint fe-empty-hint">
          <icon-git-branch :size="30" class="fe-empty-icon" />
          <span class="fe-empty-title">{{ t('fileExplorer.gitNoChanges') }}</span>
          <span class="fe-empty-desc">{{ t('fileExplorer.gitNoChangesDesc') }}</span>
          <!-- 仅当目录尚未初始化 git 时显示「创建 Git 仓库」按钮;已初始化则不显示 -->
          <button v-if="!isGitRepo" type="button" class="fe-init-repo-btn" @click="onCreateRepo">
            {{ t('fileExplorer.gitCreateRepo') }}
          </button>
        </div>

        <!-- 子节点(根目录固定,始终展开);变更模式下无变更则不渲染树 -->
        <div v-if="displayNodes.length" class="fe-list">
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
        <div v-else-if="!changesMode" class="fe-hint">{{ t('fileExplorer.empty') }}</div>
      </div>
      <div v-else class="fe-hint">{{ t('fileExplorer.empty') }}</div>
    </div>
  </div>
</template>

<style scoped>
.fe-top {
  display: flex;
  align-items: center;
  gap: 6px;
}

/* 搜索框:图标 + 输入,聚焦时边框高亮 */
.fe-search {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
  height: 32px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  background-color: var(--bg-2);
  color: var(--text-secondary);
  transition: border-color 0.15s ease;
}

.fe-search:focus-within {
  border-color: var(--border-strong);
}

.fe-search-icon {
  flex-shrink: 0;
  color: var(--text-tertiary);
}

.fe-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
}

.fe-search-input::placeholder {
  color: var(--text-tertiary);
}

/* 树容器:滚动条走全局那套(4px 细条 / 空闲隐藏 / 滚动时出现,见 global.css 与
   services/scrollbars.ts)。原先这里整体隐藏(怕常驻实心条贴住行尾的 git 变更徽章),
   现在那条缝由 scrollbar-gutter 常驻预留、条本身只在滚动时出现,故不再特判。 */
.fe-tree {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.fe-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.fe-hint {
  padding: 8px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  text-align: center;
}

.fe-error {
  color: var(--danger);
  word-break: break-all;
}

/* 空状态提示:撑满剩余高度,水平+垂直中心居中(参考 git 空状态) */
.fe-empty-hint {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  flex: 1;
  min-height: 0;
  width: 100%;
  padding: 28px 12px;
  color: var(--text-tertiary);
  text-align: center;
}

.fe-empty-icon {
  color: var(--text-tertiary);
  margin-bottom: 2px;
}

.fe-empty-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.fe-empty-desc {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  text-align: center;
  line-height: 1.5;
}

/* 创建 Git 仓库按钮:浅灰圆角按钮,贴近空状态 */
.fe-init-repo-btn {
  margin-top: 8px;
  height: 30px;
  padding: 0 14px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background-color: var(--bg-2);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.fe-init-repo-btn:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

/* 根目录行:固定,无前置图标;右侧操作按钮组(仓库 Wiki / 资源管理器 / 查看变更 / 刷新)。
   注:FileTreeNode 的 .ft-row/.ft-name 是组件内 scoped 样式,这里按同款补齐(否则根行不参与 flex、颜色回退默认) */
.fe-tree-inner {
  display: flex;
  flex-direction: column;
  gap: 1px;
  /* 撑满 .fe-tree 可视高度,让空状态 flex:1 能垂直居中 */
  min-height: 100%;
}

/* 根行:与 FileTreeNode 的 .ft-row 保持同一套紧凑尺寸(14px 文字 / 26px 行高) */
.fe-root {
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

.fe-root:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

.fe-root .ft-file-info {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  display: flex;
  align-items: center;
}

.fe-root .ft-name {
  font-size: var(--font-size-md);
  font-weight: 500;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.fe-root-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  margin-left: 6px;
  flex-shrink: 0;
}

.fe-act-btn {
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

.fe-act-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

/* 查看变更激活态:仅背景高亮,文字保持主题主色(不做蓝色强调) */
.fe-act-btn.active {
  background: var(--bg-active);
  color: var(--text-primary);
}

.fe-spin {
  animation: fe-spin 0.9s linear infinite;
}

@keyframes fe-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
</style>