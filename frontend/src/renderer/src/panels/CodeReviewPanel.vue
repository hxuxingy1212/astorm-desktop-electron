<script setup lang="ts">
/**
 * 代码审查页签：会话工作区目录的 git 变更查看器（只读）。
 *
 * 数据流：`git_diff_summary`（文件列表 + 状态字 + 增删计数）→ 展开某文件时按需
 * `git_file_diff`（统一 diff 全文 + 新侧总行数）→ `CodeDiffView` 渲染。
 *
 * 范围两档：**未暂存**（工作树 vs 索引，含未跟踪文件）/ **已暂存**（索引 vs HEAD）。
 * 不做暂存 / 提交 / 推送等写操作——它只是「看」。
 *
 * 目录解析走 `resolveSessionWorkspaceDir`（与终端、文件树同源），随激活会话切换重载。
 */
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore } from '@/stores/sessions'
import { useGitStore } from '@/stores/git'
import { useTabsStore } from '@/stores/tabs'
import { invoke } from '@/services/backend'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import CodeDiffView from './CodeDiffView.vue'

interface DiffEntry {
  path: string
  status: string
  additions: number
  deletions: number
}
interface FileDiff {
  diff: string
  newLines: number
}

const { t } = useI18n()
const sessions = useSessionsStore()
const git = useGitStore()
const tabs = useTabsStore()
const rootRef = ref<HTMLElement | null>(null)

const activeSession = computed(
  () => sessions.sessions.find((s) => s.id === sessions.activeId) ?? null
)

const dir = ref('')
const staged = ref(false)
const entries = ref<DiffEntry[]>([])
const loading = ref(false)
/** 非仓库用哨兵值 'notRepo'，其余为原始错误文本 */
const error = ref('')
const expanded = ref<Record<string, boolean>>({})
const diffs = ref<Record<string, FileDiff>>({})
const loadingPath = ref<Record<string, boolean>>({})
/** 正在初始化 Git 仓库（空状态「创建 Git 仓库」按钮防重复点） */
const creatingRepo = ref(false)

/** 状态字 → 徽章配色类（非变更类状态统一走默认色） */
function statusClass(status: string): string {
  const s = (status || 'M').toUpperCase()
  if (s === 'A') return 'st-a'
  if (s === 'D') return 'st-d'
  if (s === '?' || s === 'U') return 'st-u'
  if (s === 'R' || s === 'C') return 'st-r'
  return 'st-m'
}

function statusLabel(status: string): string {
  const s = (status || 'M').toUpperCase()
  return s === '?' ? 'U' : s
}

function baseName(path: string): string {
  return path.split('/').pop() ?? path
}

function dirName(path: string): string {
  const i = path.lastIndexOf('/')
  return i < 0 ? '' : path.slice(0, i + 1)
}

async function reload(): Promise<void> {
  loading.value = true
  // 目录 / 范围变化都要丢弃旧的展开与缓存,避免展示过期 diff
  expanded.value = {}
  diffs.value = {}
  loadingPath.value = {}
  error.value = ''
  entries.value = []
  try {
    dir.value = await resolveSessionWorkspaceDir(activeSession.value?.workspace)
    if (!dir.value) return
    // 先经主进程探测是否 git 仓库：非仓库直接进空态，**不去调 napi `git_diff_summary`**
    // ——`git diff` 在工作区外会向 stderr 打出整段 `--no-index` usage，经 IPC 抛回渲染层
    // 既刷屏又无意义。`git:is-repo` 走 `rev-parse --is-inside-work-tree`，空仓库（无提交）
    // 也判为仓库——否则刚点「创建 Git 仓库」后会因 unborn HEAD 被误判而非仓库、按钮反复出现。
    const isRepo = await window.api.git.isRepo(dir.value)
    if (!isRepo) {
      error.value = 'notRepo'
      return
    }
    entries.value = (await invoke('git_diff_summary', {
      dir: dir.value,
      staged: staged.value
    })) as DiffEntry[]
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    // 非仓库:git 的报错文案不一,统一给可读提示(与文件树「查看变更」同口径)
    error.value = /not a git repository|不是.{0,4}git.{0,4}仓库|尚无仓库/i.test(msg) ? 'notRepo' : msg
  } finally {
    loading.value = false
  }
}

/**
 * 空状态的「创建 Git 仓库」：在当前会话工作区 `git init`，成功后重载变更列表。
 * 失败（git 不可用 / 无权限）写入 error 由内容区展示。
 */
async function createRepo(): Promise<void> {
  if (!dir.value || creatingRepo.value) return
  creatingRepo.value = true
  try {
    await invoke('git_init', { dir: dir.value })
    await reload()
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    creatingRepo.value = false
  }
}

async function toggle(entry: DiffEntry): Promise<void> {
  const p = entry.path
  if (expanded.value[p]) {
    expanded.value = { ...expanded.value, [p]: false }
    return
  }
  expanded.value = { ...expanded.value, [p]: true }
  if (diffs.value[p] !== undefined || loadingPath.value[p]) return
  loadingPath.value = { ...loadingPath.value, [p]: true }
  try {
    const r = (await invoke('git_file_diff', {
      dir: dir.value,
      path: p,
      staged: staged.value
    })) as FileDiff
    diffs.value = { ...diffs.value, [p]: r }
  } catch {
    // 单文件失败不打断整页:降级为空 diff(由 CodeDiffView 显示「无差异」)
    diffs.value = { ...diffs.value, [p]: { diff: '', newLines: 0 } }
  } finally {
    loadingPath.value = { ...loadingPath.value, [p]: false }
  }
}

function onScopeChange(v: unknown): void {
  staged.value = v === 'staged'
}

/** 路径归一化（反斜杠 → 正斜杠、去 `./`），与 artifacts 事件里的路径同口径 */
function normPath(p: string): string {
  return p.replace(/\\/g, '/').replace(/^\.\//, '')
}

/**
 * 应用「交付物卡片跳转」的聚焦请求：展开目标文件并滚到可视区。
 * diff 仍按需懒加载（经 {@link toggle}）；请求只消费一次（消费后清空 store）。
 */
async function applyFocus(): Promise<void> {
  const focus = tabs.reviewFocusPath
  if (!focus || loading.value || !entries.value.length) return
  const target =
    entries.value.find((e) => normPath(e.path) === normPath(focus)) ??
    entries.value.find((e) => normPath(e.path).endsWith(`/${normPath(focus)}`))
  tabs.clearReviewFocus()
  if (!target) return
  if (!expanded.value[target.path]) await toggle(target)
  await nextTick()
  rootRef.value
    ?.querySelector<HTMLElement>(`[data-cr-path="${CSS.escape(target.path)}"]`)
    ?.scrollIntoView({ block: 'nearest' })
}

// 激活会话的工作区变化 → 重载（页签按会话隔离,切会话即换目录）
watch(() => activeSession.value?.workspace, () => void reload())
watch(staged, () => void reload())
// 顶栏切了分支 → 本页签的 diff 不再是同一基线,必须跟着重载
watch(() => git.rev, () => void reload())
// 交付物卡片跳转：聚焦请求到达即展开对应文件（面板已挂载时）
watch(() => tabs.reviewFocusSeq, () => void applyFocus())
onMounted(async () => {
  await reload()
  await applyFocus()
})
</script>

<template>
  <div ref="rootRef" class="cr-root">
    <!-- 工具条:范围切换(紧凑弹出按钮,同分支选择器)+ 计数 + 刷新 -->
    <div class="cr-bar">
      <div class="cr-scope">
        <a-select
          :model-value="staged ? 'staged' : 'unstaged'"
          size="small"
          popup-container="body"
          @change="onScopeChange"
        >
          <a-option value="unstaged">{{ t('panel.codeReview.unstaged') }}</a-option>
          <a-option value="staged">{{ t('panel.codeReview.staged') }}</a-option>
        </a-select>
      </div>
      <span v-if="entries.length" class="cr-count">
        {{ t('panel.codeReview.filesCount', { n: entries.length }) }}
      </span>
      <span class="cr-spacer" />
      <button type="button" class="cr-btn" :disabled="loading" @click="reload">
        <icon-refresh :size="13" :class="{ 'cr-spin': loading }" />
        {{ t('common.refresh') }}
      </button>
    </div>

    <!-- 内容:分支顺序要紧——loading 在空态之前（重载会先清空 entries） -->
    <div class="cr-body">
      <div v-if="loading" class="cr-empty">
        <span class="cr-empty-text">{{ t('common.loading') }}</span>
      </div>
      <div v-else-if="error && error !== 'notRepo'" class="cr-empty">
        <span class="cr-empty-text cr-err">
          {{ t('panel.codeReview.loadFailed') }}：{{ error }}
        </span>
      </div>
      <!-- 非 Git 仓库：引导创建仓库（复用文件树空状态的文案与按钮） -->
      <div v-else-if="error === 'notRepo' && dir" class="cr-empty">
        <icon-git-branch :size="26" class="cr-empty-icon" />
        <span class="cr-empty-title">{{ t('fileExplorer.gitNoChanges') }}</span>
        <span class="cr-empty-text">{{ t('fileExplorer.gitNoChangesDesc') }}</span>
        <button type="button" class="cr-cta" :disabled="creatingRepo" @click="createRepo">
          {{ t('fileExplorer.gitCreateRepo') }}
        </button>
      </div>
      <div v-else-if="!dir || !entries.length" class="cr-empty">
        <icon-git-branch :size="26" class="cr-empty-icon" />
        <span class="cr-empty-text">
          <template v-if="!dir">{{ t('panel.codeReview.noWorkspace') }}</template>
          <template v-else>{{ t('panel.codeReview.noChanges') }}</template>
        </span>
      </div>

      <div v-else class="cr-list">
        <div v-for="e in entries" :key="e.path" class="cr-file" :data-cr-path="e.path">
          <button
            type="button"
            class="cr-file-head"
            :class="{ open: expanded[e.path] }"
            @click="toggle(e)"
          >
            <span class="cr-status" :class="statusClass(e.status)">{{ statusLabel(e.status) }}</span>
            <span class="cr-name" :title="e.path">{{ baseName(e.path) }}</span>
            <span class="cr-dir" :title="e.path">{{ dirName(e.path) }}</span>
            <span class="cr-stat">
              <span v-if="e.additions" class="cr-add">+{{ e.additions }}</span>
              <span v-if="e.deletions" class="cr-del">-{{ e.deletions }}</span>
            </span>
            <icon-right :size="12" class="cr-chev" :class="{ open: expanded[e.path] }" />
          </button>
          <div v-if="expanded[e.path]" class="cr-diff">
            <div v-if="loadingPath[e.path]" class="cr-empty">
              <span class="cr-empty-text">{{ t('common.loading') }}</span>
            </div>
            <CodeDiffView
              v-else-if="diffs[e.path]"
              :diff="diffs[e.path].diff"
              :new-lines="diffs[e.path].newLines"
              :path="e.path"
            />
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* macOS 风格：无边框弹出按钮（半径 8、悬停 --bg-3）、发丝分隔线、系统色徽章 */
.cr-root {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
}

/* ---------- 工具条 ---------- */
.cr-bar {
  display: flex;
  flex: none;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  border-bottom: 1px solid var(--border-light);
}

/* 范围选择：固定宽度 + 透明无边框（同 FileBranchBar 的分支选择器） */
.cr-scope {
  flex: none;
  width: 104px;
}

.cr-scope :deep(.arco-select-view) {
  border: none !important;
  box-shadow: none !important;
  background: transparent !important;
  height: 24px !important;
  border-radius: 8px !important;
  width: 100% !important;
  min-width: 0 !important;
  padding: 0 8px !important;
}

.cr-scope :deep(.arco-select-view:hover),
.cr-scope :deep(.arco-select-view:focus-within),
.cr-scope :deep(.arco-select-view.arco-select-view-focus) {
  background: var(--bg-3) !important;
  border-color: transparent !important;
}

.cr-scope :deep(.arco-select-view-value) {
  font-size: var(--font-size-xs);
  color: var(--text-primary);
}

.cr-scope :deep(.arco-select-view-suffix) {
  line-height: 0;
  padding-left: 0;
  color: var(--text-tertiary);
}

.cr-count {
  flex: none;
  white-space: nowrap;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.cr-spacer {
  flex: 1;
  min-width: 0;
}

/* 刷新：紧凑无边框按钮；nowrap + flex:none 防止中文标题被折成两行 */
.cr-btn {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 5px;
  height: 24px;
  padding: 0 9px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-xs);
  white-space: nowrap;
  transition: background-color 0.15s, color 0.15s;
}

.cr-btn:hover:not(:disabled) {
  background: var(--bg-3);
  color: var(--text-primary);
}

.cr-btn:disabled {
  opacity: 0.45;
  cursor: default;
}

.cr-spin {
  animation: cr-rot 0.9s linear infinite;
}

@keyframes cr-rot {
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 内容区 ---------- */
.cr-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  scrollbar-gutter: stable;
}

/* 空态：图标 + 说明居中（macOS 空白页观感） */
.cr-empty {
  display: flex;
  height: 100%;
  min-height: 160px;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 24px;
}

.cr-empty-icon {
  color: var(--text-tertiary);
  opacity: 0.35;
}

.cr-empty-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
  text-align: center;
}

.cr-empty-text {
  font-size: var(--font-size-code);
  color: var(--text-tertiary);
  text-align: center;
  overflow-wrap: anywhere;
}

/* 「创建 Git 仓库」：浅灰圆角按钮（与文件树空状态同款） */
.cr-cta {
  margin-top: 6px;
  height: 30px;
  padding: 0 14px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background-color: var(--bg-2);
  color: var(--text-primary);
  font-family: inherit;
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition: background-color 0.15s ease, color 0.15s ease;
}

.cr-cta:hover:not(:disabled) {
  background-color: var(--bg-hover);
}

.cr-cta:disabled {
  opacity: 0.45;
  cursor: default;
}

.cr-err {
  color: rgb(var(--danger-6));
}

/* ---------- 文件列表：整行发丝分隔 + 悬停填充 ---------- */
.cr-list {
  display: flex;
  flex-direction: column;
}

.cr-file + .cr-file .cr-file-head {
  border-top: 1px solid var(--border-light);
}

/* 文件行。
   ⚠️ 必须显式重置按钮默认样式：Chromium 的 UA 样式给 <button> 上 `background: buttonface`
   （亮色）+ `color: buttontext` + Arial 字体。项目**没有全局 button 重置**，不重置的话
   暗色主题下整行会变成亮底，而文件名用的是暗色主题的 --text-primary（近白）→ 白底白字看不见；
   悬停时又因下面那条 `:hover` 把底改暗，文字突然可读——表现为「只有鼠标那一行清楚」。
   （同项目 BrowserPanel 的 .bw-btn / FileBranchBar 的 .pick-btn 都显式重置了。） */
.cr-file-head {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 8px;
  height: 30px;
  padding: 0 10px;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  transition: background-color 0.12s;
}

.cr-file-head:hover,
.cr-file-head.open {
  background: var(--bg-hover);
}

/* 状态字徽章：小圆角方块，macOS 系统色 */
.cr-status {
  flex: none;
  width: 15px;
  height: 15px;
  line-height: 15px;
  text-align: center;
  border-radius: 4px;
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  font-weight: 700;
}

.st-a {
  color: #34c759;
  background: rgba(52, 199, 89, 0.16);
}

.st-m {
  color: #ff9500;
  background: rgba(255, 149, 0, 0.16);
}

.st-d {
  color: #ff3b30;
  background: rgba(255, 59, 48, 0.15);
}

.st-u {
  color: var(--text-tertiary);
  background: var(--bg-2);
}

.st-r {
  color: #af52de;
  background: rgba(175, 82, 222, 0.16);
}

/* 文件名主色、目录弱化；两者都可截断，右侧统计与箭头恒不被挤压 */
.cr-name {
  flex: 0 1 auto;
  max-width: 42%;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-code);
  color: var(--text-primary);
}

.cr-dir {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.cr-stat {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 6px;
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
}

.cr-add {
  color: #34c759;
}

.cr-del {
  color: #ff3b30;
}

.cr-chev {
  flex: none;
  color: var(--text-tertiary);
  transition: transform 0.15s;
}

.cr-chev.open {
  transform: rotate(90deg);
}

/* 展开区：与上一行留一条发丝线，轻微下沉底色 */
.cr-diff {
  border-top: 1px solid var(--border-light);
  background: var(--bg-1);
}
</style>

