<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { useRoute, useRouter } from 'vue-router'
import type { Session } from '@/types'
import { useSessionsStore } from '@/stores/sessions'
import type { SessionStatusKey } from '@/stores/sessions'
import { useSettingsStore } from '@/stores/settings'
import { useUiStore } from '@/stores/ui'
import { relativeTime } from '@/utils/time'
import { useNow } from '@/composables/useNow'
import { useHoverMarquee } from '@/composables/useHoverMarquee'
import SettingsMenu from '@/components/settings/SettingsMenu.vue'
import FileExplorerPanel from '@/components/sidebar/FileExplorerPanel.vue'
import SessionRowHoverCard from '@/components/sidebar/SessionRowHoverCard.vue'
import { useFilesStore } from '@/stores/files'
import { hasSidebarGlass, hasTrafficLights } from '@/utils/platform'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { localizedSessionTitle } from '@/utils/sessionTitle'

const props = defineProps<{ collapsed: boolean }>()

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const sessions = useSessionsStore()
/** 共享 60s ticker：任务行 / 子会话行的相对时间自己走（不再停在进入侧栏那一刻） */
const nowTick = useNow()
const settings = useSettingsStore()
const ui = useUiStore()
const files = useFilesStore()

// 设置视图:路由进入 /settings 时侧栏显示设置菜单
const isSettingsRoute = computed(() => route.path.startsWith('/settings'))

// macOS 品牌行折叠按钮:与顶栏按钮同逻辑(展开/折叠侧边栏)
function onToggleSidebar(): void {
  settings.setSidebarCollapsed(!props.collapsed)
}

// ---------- 任务 / 项目:切换按钮切换,同一时刻只显示一组 ----------
/** 当前激活的分组:由顶部分段切换按钮控制 */
const activeSectionKey = ref<'tasks' | 'projects'>('tasks')

/**
 * 根会话（顶层行）：无父，或父已不在列表（父被删 → 子当根显示，绝不隐藏）。
 *
 * 子会话由后端 `sub_session` 工具创建（一次委派可能并发多个），**不占左栏行**：
 * 入口统一收在对话区右上角规划胶囊的展开面板里（`components/chat/ChildSessionsList.vue`），
 * 左栏只列根会话（计数 / 空态同口径，子会话不重复计数）。
 *
 * 定时任务会话（`sessionType === 'scheduled'`）同样**不占左栏行**：它们由 cron 触发产生，
 * 数量随触发次数增长，入口统一在「定时任务」页的任务卡片下（按任务归集，见 `sessionsOfTask`）。
 */
const rootTasks = computed(() =>
  sessions.tasks
    .filter((s) => isRootSession(s) && s.sessionType !== 'scheduled')
    // 待办（从未执行，= 看板 todo 列）不占左栏行：入口统一在看板；
    // 当前正打开的除外 —— 刚创建待执行 / 从看板点开时行还在（带灰点），切走即收进看板
    .filter((s) => s.id === sessions.activeId || sessions.boardColumnOf(s.id) !== 'todo')
    .sort(byRecencyDesc)
)
const rootProjects = computed(() => sessions.projects.filter(isRootSession).sort(byRecencyDesc))

/** 某会话是否为子会话（pid 指向仍在列表中的某会话的 backendId） */
function isRootSession(s: Session): boolean {
  if (!s.pid) return true
  return !sessions.sessions.some((p) => p.backendId === s.pid)
}

/** 会话最近活动时间（缺省回退创建时间）——列表统一按它**倒序**：近的在上、远的在下 */
function sessionRecency(s: Session): number {
  return s.updatedAt ?? s.createdAt ?? 0
}
function byRecencyDesc(a: Session, b: Session): number {
  return sessionRecency(b) - sessionRecency(a)
}

/**
 * 分组内容:
 * - 任务：纯平铺列表(每条任务一行,不按日期/工作区分组;后端默认工作区已精确到秒,目录各不相同)
 * - 项目：仍按工作区目录分组(项目节点 → 会话子节点 的树)
 */
interface Section {
  key: 'tasks' | 'projects'
  labelKey: string
  iconKey: string
  list: Session[]
  tree: SessionTreeNode[]
}

const sections = computed<Section[]>(() => [
  // 计数 / 空态 / 分组都只算根会话（子会话挂在父行悬浮列表里，不重复计数）
  { key: 'tasks', labelKey: 'sidebar.tasks', iconKey: 'icon-clipboard-list', list: rootTasks.value, tree: [] },
  {
    key: 'projects',
    labelKey: 'sidebar.projects',
    iconKey: 'icon-folder',
    list: rootProjects.value,
    tree: buildGroupTree(rootProjects.value, defaultWorkspaceTitle.value)
  }
])

const activeSection = computed(() => sections.value.find((g) => g.key === activeSectionKey.value) ?? sections.value[0])

const activeTree = computed(() => activeSection.value.tree ?? [])

/** 当前分组列表是否为空(仅空态启用 flex 撑满居中,不影响有内容时的分组布局) */
const sidebarListEmpty = computed(
  () => (activeSectionKey.value === 'tasks' ? !rootTasks.value.length : !rootProjects.value.length)
)

// ---------- 分段滑块:测量激活按钮位置,推动白色拇指平滑滑移 ----------
const segEl = ref<HTMLElement | null>(null)
const animateThumb = ref(false)
let segResizeObserver: ResizeObserver | undefined
const thumbStyle = reactive({ left: 0, width: 0, opacity: 0 })

function measureThumb(): void {
  const btn = segEl.value?.querySelector<HTMLElement>('.side-toggle-btn.active')
  if (!btn) {
    thumbStyle.opacity = 0
    return
  }
  // btn 与 thumb 的定位参照系都是 .side-toggle-seg(position:relative),
  // 直接用 btn.offsetLeft 即可对齐(勿再减 seg.offsetLeft,会偏出胶囊)
  thumbStyle.left = btn.offsetLeft
  thumbStyle.width = btn.offsetWidth
  thumbStyle.opacity = 1
}

watch(activeSectionKey, () => {
  animateThumb.value = true
  void nextTick(measureThumb)
})

// 会话计数异步加载、语言变化、侧栏重新展开都会改变按钮宽度。
// 观察实际按钮和容器，避免只在挂载/窗口 resize 时量到初始的「0」。
watch(segEl, (el) => {
  segResizeObserver?.disconnect()
  animateThumb.value = false
  if (!el) {
    thumbStyle.opacity = 0
    return
  }
  measureThumb()
  segResizeObserver = new ResizeObserver(measureThumb)
  segResizeObserver.observe(el)
  for (const btn of el.querySelectorAll('.side-toggle-btn')) segResizeObserver.observe(btn)
}, { flush: 'post' })

// ---------- 置顶分组:置顶(按 pinnedAt 倒序) / 其他 ----------
const pinnedTasks = computed(() =>
  rootTasks.value
    .filter((s) => sessions.pins[s.id])
    .sort((a, b) => (sessions.pins[b.id] ?? 0) - (sessions.pins[a.id] ?? 0))
)
const otherTasks = computed(() => rootTasks.value.filter((s) => !sessions.pins[s.id]))

const pinnedDirs = computed(() =>
  activeTree.value
    .filter((n) => n.workspace && sessions.pinnedProjects[n.workspace])
    .sort((a, b) => (sessions.pinnedProjects[b.workspace ?? ''] ?? 0) - (sessions.pinnedProjects[a.workspace ?? ''] ?? 0))
)
const otherDirs = computed(() => activeTree.value.filter((n) => !n.workspace || !sessions.pinnedProjects[n.workspace]))

/** 「置顶 / 其他」分组(展开为默认,折叠态仅内存) */
const groupCollapsed = reactive<Record<string, boolean>>({})
function toggleGroup(key: string): void {
  groupCollapsed[key] = !groupCollapsed[key]
}

/** 项目视图渲染分组:置顶在前、其他在后(目录内会话子树保持原序) */
const projectGroups = computed(() => [
  { key: 'pinned', labelKey: 'sidebar.pinned', nodes: pinnedDirs.value },
  { key: 'others', labelKey: 'sidebar.others', nodes: otherDirs.value }
])

function sessionPinned(id: string | undefined): boolean {
  return !!id && !!sessions.pins[id]
}

function projectPinned(workspace?: string): boolean {
  return !!workspace && !!sessions.pinnedProjects[workspace]
}

/** 点击任务行:激活会话并跳转 */
function openSession(s: Session): void {
  sessions.setActive(s.id)
  router.push(`/chat/${s.id}`)
}

const {
  active: marqueeActive,
  enter: onTitleEnter,
  leave: onTitleLeave
} = useHoverMarquee()

/** 任务行高亮:与路由会话 id 一致 */
function taskSelected(id: string): boolean {
  return route.params.id === id
}

/**
 * 任务树状态图标（判定链在 store：`sessions.statusKeyOf`，任务看板 / 子会话列表共用同一份）。
 *
 * 优先级 流式转圈 > 等你处理(待答表单 / 待审批,橙点) > 进程中断(灰点) >
 * 异常终态(用户打断 / 执行失败,红点) > 正常结束(绿点) > 无标记(灰点=待办,与看板 todo 列同款)；
 * 悬停说明由 taskStatusTitle 给具体文案。
 * 红点有两种来源(样式同色,说明按 `taskFlags.end` 区分「已取消 / 执行失败」):
 * 用户主动打断 → `cancelled`,执行失败 → `error`。二者都算「没正常退出」,
 * 但语义不同,所以图标各留一个枚举值,别合并成 error。
 */
function taskStatusIcon(id: string | undefined): SessionStatusKey {
  return sessions.statusKeyOf(id)
}

/** 状态图标悬停说明（判定链同样在 store：能区分「等待你的回答 / 等待你的审批」）。
 *  无标记（待办）也给出文案 —— 看板 todo 列同名。 */
function taskStatusTitle(id: string | undefined): string {
  const key = sessions.statusLabelKeyOf(id)
  return key ? t(`session.status.${key}`) : t('board.column.todo')
}

/**
 * 会话类型徽标 —— 侧栏**不再列定时任务会话**（`rootTasks` 已按 `sessionType` 过滤），
 * 普通 / 项目会话由分组自身表达，子会话不占行，故暂无需要挂徽标的类型。
 */

// ---------- 项目工作区文件浏览模式:左侧栏整体切换为「搜索框 + 文件树 + 返回任务」 ----------
/** true = 正在浏览某项目的工作区文件(侧栏任务列表/切换条被文件浏览器替代) */
const filesMode = ref(false)
/** 从项目节点进入文件浏览:加载该工作区目录的整棵文件树。
 *  若当前不在该项目的会话聊天页(新建页 / 定时页 / 其他会话),跳转到该项目
 *  最近更新的会话聊天页——中心区域显示会话聊天内容,文件预览页签在右侧打开 */
async function viewProjectFiles(node: SessionTreeNode): Promise<void> {
  // 与 viewTaskFiles / openSessionFolder 同口径:相对工作区(默认时间戳目录)须归入
  // `<用户目录>/workspace/` 下解析(见 utils/workspace.resolveSessionWorkspaceDir),
  // 否则后端按相对路径拼出的目录不存在(读目录 ENOENT)
  const dir = await resolveSessionWorkspaceDir(node.workspace)
  if (!dir) return
  filesMode.value = true
  files.active = true
  void files.loadTree(dir)
  const projectSessions = (node.children ?? [])
    .map((c) => c.session)
    .filter((s): s is Session => !!s)
  const currentIsProjectChat =
    typeof route.params.id === 'string' && projectSessions.some((s) => s.id === route.params.id)
  if (!currentIsProjectChat && projectSessions.length) {
    const target = [...projectSessions].sort(
      (a, b) => (b.updatedAt ?? b.createdAt) - (a.updatedAt ?? a.createdAt)
    )[0]
    sessions.setActive(target.id)
    router.push(`/chat/${target.id}`)
  }
}
/** 返回任务:退出文件浏览并复位文件状态 */
function exitFiles(): void {
  filesMode.value = false
  files.reset()
}

/**
 * 从任务行进入文件浏览:解析该任务工作区目录(与 sessions.openSessionFolder 同口径:
 * 绝对路径直接用;相对路径(默认日期空间)归入 `<用户目录>/workspace/` 下解析,
 * 见 utils/workspace.resolveSessionWorkspaceDir),
 * 侧栏整块切换为文件树;若当前不在该任务聊天页则跳转过去 */
async function viewTaskFiles(s: Session): Promise<void> {
  const dir = await resolveSessionWorkspaceDir(s.workspace)
  if (!dir) {
    Message.info(t('chatMore.openDirNoPath'))
    return
  }
  filesMode.value = true
  files.active = true
  void files.loadTree(dir)
  if (typeof route.params.id !== 'string' || route.params.id !== s.id) {
    sessions.setActive(s.id)
    router.push(`/chat/${s.id}`)
  }
}

/** 分组内会话树节点(参考 astorm-desktop NavSider:项目节点 → 会话子节点) */
interface SessionTreeNode {
  key: string
  title: string
  type: 'project' | 'session'
  /** 项目节点的工作区完整路径(操作:添加任务 / 删除分组) */
  workspace?: string
  /** 项目节点的会话计数 */
  count?: number
  /** 会话节点关联的会话 */
  session?: Session
  children?: SessionTreeNode[]
}

/** 工作区显示名:绝对/相对路径取末段目录名(如 C:\a\myapp → myapp);默认工作区占位原样 */
function workspaceLabel(ws: string, defaultTitle: string): string {
  if (ws === defaultTitle || !ws) return ws
  const base = ws.trim().replace(/[\\/]+$/, '')
  const last = base.split(/[\\/]/).pop()
  return last || ws
}

/** 按 workspace 分组构建树:项目节点(workspace) → 会话子节点。
 *  **先按最近活动倒序**：组内会话近的在上；组（项目）本身也随首个（最新）会话排序，近的项目在上 */
function buildGroupTree(list: Session[], defaultTitle: string): SessionTreeNode[] {
  const ordered = [...list].sort(byRecencyDesc)
  const groups = new Map<string, Session[]>()
  for (const s of ordered) {
    const ws = s.workspace || defaultTitle
    if (!groups.has(ws)) groups.set(ws, [])
    groups.get(ws)!.push(s)
  }
  return [...groups.entries()].map(([ws, items]) => ({
    key: `ws:${ws}`,
    // 分组 key 用完整工作区路径(不同路径不合并),展示名只留目录名
    title: workspaceLabel(ws, defaultTitle),
    workspace: ws,
    type: 'project',
    count: items.length,
    children: items.map((s) => ({
      key: `s:${s.id}`,
      title: localizedSessionTitle(s),
      type: 'session',
      session: s
    }))
  }))
}

const defaultWorkspaceTitle = computed(() => t('sidebar.defaultWorkspace'))

// 各分组树的展开状态:默认展开全部项目节点,新增项目节点自动补入
const expandedByGroup = reactive<Record<string, string[]>>({ tasks: [], projects: [] })
watch(
  () => sections.value.map((g) => g.tree.map((n) => n.key)),
  (treeKeys) => {
    treeKeys.forEach((keys, i) => {
      const groupKey = sections.value[i].key
      const missing = keys.filter((k) => !expandedByGroup[groupKey].includes(k))
      if (missing.length) expandedByGroup[groupKey] = [...expandedByGroup[groupKey], ...missing]
    })
  },
  { immediate: true }
)

/** 目录行展开态(项目区两级列表):展开集存 expandedByGroup */
function dirExpanded(node: SessionTreeNode): boolean {
  return expandedByGroup[activeSectionKey.value].includes(node.key)
}

/** 点击目录行:切换展开/收起(文件夹图标开/关随之切换) */
function toggleProjectDir(node: SessionTreeNode): void {
  const idx = expandedByGroup[activeSectionKey.value].indexOf(node.key)
  if (idx >= 0) expandedByGroup[activeSectionKey.value].splice(idx, 1)
  else expandedByGroup[activeSectionKey.value].push(node.key)
}

/** 点击会话子项:激活并跳转聊天页 */
function openProjectSession(node: SessionTreeNode | undefined): void {
  if (node?.session) openSession(node.session)
}

// 刷新按钮:占位(后续接后端后改为重新拉取列表);点击不做旋转动画
function onRefresh(): void {
  /* 占位 */
}

/** 进入设置前的路由(返回时回到该页,而非固定跳第一个对话) */
const returnRoute = ref('')

/** 底部按钮:设置视图 → 返回上一页;文件浏览 → 退出文件模式;其它 → 进设置 */
function onFooterAction(): void {
  if (isSettingsRoute.value) {
    router.push(returnRoute.value || '/newChat')
    return
  }
  if (filesMode.value) {
    exitFiles()
    return
  }
  returnRoute.value = route.fullPath
  router.push('/settings/system')
}

function onScheduled(): void {
  router.push('/scheduled')
}

// ---------- 项目节点(目录)操作:全部走右键菜单(行内不再放悬浮按钮) ----------

/**
 * 项目目录行「＋」：跳到新建会话页并预填目录。
 *
 * **不传分支**（早先是从子会话里挑第一个带 branch 的，取到谁取决于子会话顺序、且那份是
 * 建会话时的快照，切过分支即过期）。分支已不进会话记录，也不需要预填——表单里的分支是
 * 对所选目录**实时探测**的只读展示，系统提示词每轮自己取当前分支。
 */
function addTaskFor(node: SessionTreeNode): void {
  router.push({
    path: '/newChat',
    query: { directory: node.workspace ?? '' }
  })
}

/** 删除目录分组:逐会话真实删除(后端 sqlite + jsonl),本地同步移除 */
function deleteProjectGroup(node: SessionTreeNode): void {
  const list = (node.children ?? [])
    .map((c) => c.session)
    .filter((s): s is Session => !!s)
  void macConfirm({
    title: t('sidebar.deleteProjectTitle'),
    content: t('sidebar.deleteProjectContent', { name: node.title, count: list.length }),
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      await Promise.all(list.map((s) => sessions.deleteSession(s.id)))
    }
  })
}

// ---------- 行右键菜单:任务行(重命名/归档/打开目录 占位,删除生效)与项目行(查看文件/删除)共用同一浮层 ----------

/** 右键菜单目标:会话行(任务行 / 项目下会话子项)或项目目录行 */
type CtxMenuTarget = { kind: 'session'; session: Session } | { kind: 'project'; node: SessionTreeNode }

const ctxMenu = reactive<{ visible: boolean; x: number; y: number; target: CtxMenuTarget | null }>({
  visible: false,
  x: 0,
  y: 0,
  target: null
})

/** 浮层 DOM:↑↓ 在条目间移动焦点用 */
const ctxMenuEl = ref<HTMLElement | null>(null)

const CTX_MENU_WIDTH = 168
/** 浮层高度估算(贴边钳制用,沿用原有口径):条目 32 + 分隔线 1 + 上下内边距 8 */
const CTX_MENU_HEIGHT_SESSION = 6 * 32 + 1 + 8
const CTX_MENU_HEIGHT_PROJECT = 4 * 32 + 1 + 8

/** 菜单当前会话目标(模板按此渲染会话条目) */
const ctxMenuSession = computed(() => (ctxMenu.target?.kind === 'session' ? ctxMenu.target.session : null))
/** 菜单当前项目目标(模板按此渲染项目条目) */
const ctxMenuProject = computed(() => (ctxMenu.target?.kind === 'project' ? ctxMenu.target.node : null))

/** 在鼠标处打开浮层,贴边钳制,避免菜单越出视口 */
function openCtxMenu(e: MouseEvent, target: CtxMenuTarget): void {
  ctxMenu.target = target
  const menuHeight = target.kind === 'project' ? CTX_MENU_HEIGHT_PROJECT : CTX_MENU_HEIGHT_SESSION
  ctxMenu.x = Math.max(4, Math.min(e.clientX, window.innerWidth - CTX_MENU_WIDTH - 4))
  ctxMenu.y = Math.max(4, Math.min(e.clientY, window.innerHeight - menuHeight - 4))
  ctxMenu.visible = true
}

function onSessionContext(e: MouseEvent, session?: Session): void {
  if (!session) return
  openCtxMenu(e, { kind: 'session', session })
}

/** 项目目录行右键:与任务行右键同一套浮层 / 定位 / 关闭逻辑 */
function onProjectContext(e: MouseEvent, node: SessionTreeNode): void {
  openCtxMenu(e, { kind: 'project', node })
}

function hideCtxMenu(): void {
  ctxMenu.visible = false
  ctxMenu.target = null
}

function onSessionMenuAction(
  action: 'files' | 'pin' | 'reveal' | 'rename' | 'archive' | 'delete'
): void {
  const target = ctxMenu.target
  hideCtxMenu()
  if (target?.kind !== 'session') return
  const session = target.session
  if (action === 'files') {
    void viewTaskFiles(session)
    return
  }
  if (action === 'pin') {
    sessions.togglePinSession(session.id)
    return
  }
  if (action === 'reveal') {
    // 从文件管理器打开会话目录(绝对路径直接用;相对接默认会话空间基目录)
    void sessions.openSessionFolder(session)
    return
  }
  if (action === 'rename' || action === 'archive') {
    // 占位:后续接入重命名/归档
    Message.info(t('common.comingSoon'))
    return
  }
  void macConfirm({
    title: t('sidebar.deleteConfirmTitle'),
    content: t('sidebar.deleteConfirmContent', { name: localizedSessionTitle(session) }),
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      if (!(await sessions.deleteSession(session.id))) return
      // 删除后跳新建页,避免落到一个暂无历史消息的会话空态页
      router.push('/newChat')
    }
  })
}

/**
 * 项目行右键菜单动作:查看文件(原行内按钮) / 删除(原 ⋯ 菜单唯一可用项,确认弹窗不变)。
 * 原 `…` 下拉只有「删除」一条,故不另留占位分支。
 */
function onProjectContextAction(action: 'files' | 'addTask' | 'pin' | 'delete'): void {
  const target = ctxMenu.target
  const node = target?.kind === 'project' ? target.node : null
  hideCtxMenu()
  if (!node) return
  switch (action) {
    case 'files':
      void viewProjectFiles(node)
      return
    case 'addTask':
      addTaskFor(node)
      return
    case 'pin':
      sessions.togglePinProject(node.workspace ?? '')
      return
    default:
      deleteProjectGroup(node)
  }
}

function onCtxMenuGlobalHide(): void {
  hideCtxMenu()
}

/** 键盘可达性:Esc 关闭(与点击外部同义);↑↓ 在菜单条目间移动焦点(条目本身是 button,回车原生触发) */
function onCtxMenuKeydown(e: KeyboardEvent): void {
  if (!ctxMenu.visible) return
  if (e.key === 'Escape') {
    hideCtxMenu()
    return
  }
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
  const items = Array.from(ctxMenuEl.value?.querySelectorAll<HTMLElement>('.ctx-item') ?? [])
  if (!items.length) return
  e.preventDefault()
  const idx = items.indexOf(document.activeElement as HTMLElement)
  const next = e.key === 'ArrowDown' ? (idx + 1) % items.length : idx <= 0 ? items.length - 1 : idx - 1
  items[next].focus()
}

onMounted(() => {
  // 点击空白 / 滚动 / 右键他处 / Esc 均关闭菜单。
  // contextmenu/scroll 用捕获阶段:先于元素自身的处理执行,
  // 打开菜单的右键不会被自身事件冒泡回来立刻关闭。
  window.addEventListener('click', onCtxMenuGlobalHide)
  window.addEventListener('contextmenu', onCtxMenuGlobalHide, true)
  window.addEventListener('scroll', onCtxMenuGlobalHide, true)
  window.addEventListener('blur', onCtxMenuGlobalHide)
  window.addEventListener('keydown', onCtxMenuKeydown)
})

onUnmounted(() => {
  window.removeEventListener('click', onCtxMenuGlobalHide)
  window.removeEventListener('contextmenu', onCtxMenuGlobalHide, true)
  window.removeEventListener('scroll', onCtxMenuGlobalHide, true)
  window.removeEventListener('blur', onCtxMenuGlobalHide)
  window.removeEventListener('keydown', onCtxMenuKeydown)
  segResizeObserver?.disconnect()
})
</script>

<template>
  <aside
    class="nav-sider relative flex h-full shrink-0 flex-col overflow-hidden"
    :class="{
      'collapsed w-0': collapsed,
      'native-glass': hasSidebarGlass
    }"
    :style="{ width: collapsed ? '0px' : `${settings.sidebarWidth}px` }"
  >
    <!-- 品牌区:UI 图标 + 名称。macOS 桌面壳下窗口左上角为系统红黄绿,品牌行不再显示 logo/名称,
         仅保留「折叠/展开侧边栏」按钮紧贴红黄绿,整行为拖拽区可移动窗口;
         品牌行高度与顶栏一致(h-11=44px),折叠/展开两种状态按钮位置不变,红黄绿无需移动。
         浏览器部署没有红黄绿(hasTrafficLights=false)→ 走普通品牌行,折叠按钮留在顶栏 -->
    <div
      class="sider-brand flex shrink-0 items-center"
      :class="
        collapsed
          ? 'justify-center px-0'
          : hasTrafficLights
            ? 'h-11 gap-0 pl-[76px] pr-3 app-region-drag'
            : 'h-12 gap-2.5 px-3'
      "
    >
      <template v-if="hasTrafficLights">
        <a-tooltip :content="t('statusbar.toggleSidebar')">
          <button type="button" class="mac-sidebar-toggle app-region-no-drag" @click="onToggleSidebar">
            <svg
              width="18"
              height="18"
              viewBox="0 0 48 48"
              fill="none"
              stroke="currentColor"
              stroke-width="4"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <rect x="6" y="10" width="36" height="28" rx="5" />
              <line x1="18" y1="10" x2="18" y2="38" />
            </svg>
          </button>
        </a-tooltip>
      </template>
      <template v-else>
        <img src="@/assets/logo.png" alt="logo" class="h-7 w-7 shrink-0 rounded-[6px] object-cover" />
        <span v-if="!collapsed" class="brand-title truncate text-base font-semibold text-[var(--text-primary)]">
          {{ t('common.appName') }}
        </span>
      </template>
    </div>

    <!-- 顶部返回按钮(logo 下方):文件树浏览 / 设置视图时显示,点击返回上一页或退出文件树 -->
    <div v-if="collapsed && (isSettingsRoute || filesMode)" class="sider-back">
      <a-tooltip :content="isSettingsRoute ? t('settings.back') : t('fileExplorer.backToTasks')" position="right">
        <button type="button" class="footer-btn collapsed" @click="onFooterAction">
          <icon-left :size="18" />
        </button>
      </a-tooltip>
    </div>
    <div v-else-if="(isSettingsRoute || filesMode) && !collapsed" class="sider-back">
      <button type="button" class="footer-btn back-btn" @click="onFooterAction">
        <icon-left :size="18" />
        <span class="btn-label">{{ isSettingsRoute ? t('settings.back') : t('fileExplorer.backToTasks') }}</span>
      </button>
    </div>

    <!-- 设置菜单视图 -->
    <template v-if="isSettingsRoute">
      <SettingsMenu :collapsed="collapsed" class="settings-view" />
    </template>

    <!-- 会话视图 -->
    <template v-else>
      <!-- 文件浏览模式:顶级视图(同任务/设置同级),整块替换工具栏 + 切换条 + 列表 -->
      <template v-if="filesMode && !collapsed">
        <FileExplorerPanel class="fe-panel flex-1 min-h-0" />
      </template>

      <template v-else>
      <!-- 工具栏:新建任务 / 全局搜索 / 定时任务 / 交付物·资料库 -->
      <div class="sider-toolbar">
        <a-tooltip :content="t('sidebar.newTask')" position="right" :disabled="!collapsed">
          <button class="toolbar-btn new-chat-btn" :class="collapsed && 'collapsed'" @click="router.push('/newChat')">
            <icon-plus :size="18" />
            <span v-if="!collapsed" class="btn-label">{{ t('sidebar.newTask') }}</span>
          </button>
        </a-tooltip>
        <a-tooltip :content="t('sidebar.search')" position="right" :disabled="!collapsed">
          <button class="toolbar-btn" :class="collapsed && 'collapsed'" @click="ui.openSearch()">
            <icon-search :size="18" />
            <span v-if="!collapsed" class="btn-label">{{ t('sidebar.search') }}</span>
          </button>
        </a-tooltip>
        <a-tooltip :content="t('sidebar.scheduled')" position="right" :disabled="!collapsed">
          <button class="toolbar-btn" :class="collapsed && 'collapsed'" @click="onScheduled">
            <icon-schedule :size="18" />
            <span v-if="!collapsed" class="btn-label">{{ t('sidebar.scheduled') }}</span>
          </button>
        </a-tooltip>
        <a-tooltip :content="t('sidebar.knowledge')" position="right" :disabled="!collapsed">
          <button class="toolbar-btn" :class="collapsed && 'collapsed'" @click="router.push('/knowledge')">
            <icon-library :size="18" />
            <span v-if="!collapsed" class="btn-label">{{ t('sidebar.knowledge') }}</span>
          </button>
        </a-tooltip>
      </div>

      <!-- 会话列表:任务 / 项目 用切换按钮切换,下方只显示当前分组 -->
      <div v-if="!collapsed" class="sider-content" :class="{ 'list-empty': sidebarListEmpty }">
        <!-- 分组切换:任务 / 项目 + 右侧刷新 -->
        <div class="side-toggle">
          <div ref="segEl" class="side-toggle-seg">
            <div class="side-toggle-thumb" :class="{ 'is-animated': animateThumb }" :style="{ left: `${thumbStyle.left}px`, width: `${thumbStyle.width}px`, opacity: thumbStyle.opacity }" />
            <button
              v-for="g in sections"
              :key="g.key"
              type="button"
              class="side-toggle-btn"
              :class="activeSection.key === g.key && 'active'"
              @click="activeSectionKey = g.key as 'tasks' | 'projects'"
            >
              <span class="side-toggle-icon">
                <component :is="g.iconKey" :size="14" />
              </span>
              <span class="side-toggle-label">{{ t(g.labelKey) }}</span>
              <span class="side-toggle-count">{{ g.list.length }}</span>
            </button>
          </div>
          <button class="drop-refresh" :title="t('sidebar.refresh')" @click="onRefresh">
            <icon-refresh :size="16" />
          </button>
        </div>

        <!-- 列表区:任务 = 纯平铺任务列表(一条一行,非文件树);项目 = 工作区树 -->
        <div class="drop-body" :class="{ 'list-empty': sidebarListEmpty }">
          <!-- 任务:平铺列表,按「置顶 / 其他」分组(组题可点击折叠) -->
          <div v-if="activeSectionKey === 'tasks'" class="task-list" :class="{ 'is-empty': !rootTasks.length }">
              <template v-if="rootTasks.length">
              <!-- 置顶组(为空也显示标签) -->
              <div class="pin-group">
                <div class="pin-group-header" @click="toggleGroup('tasks-pinned')">
                  <span class="pin-group-label">{{ t('sidebar.pinned') }}</span>
                  <span class="pin-group-count">{{ pinnedTasks.length }}</span>
                </div>
                <template v-if="!groupCollapsed['tasks-pinned']">
<SessionRowHoverCard
                    v-for="s in pinnedTasks"
                    :key="s.id"
                    :session="s"
                    :disabled="ctxMenu.visible"
                  >
                    <div
                      class="task-item"
                    :class="taskSelected(s.id) && 'active'"
                    @click="openSession(s)"
                    @contextmenu.prevent="onSessionContext($event, s)"
                    @mouseenter="onTitleEnter($event, s.id)"
                    @mouseleave="onTitleLeave(s.id)"
                  >
                    <!-- 前方状态位:流式转圈 / 等你处理橙点 / 异常红点(打断·失败) / 正常结束绿点(与项目子项一致) -->
                    <span class="task-status-slot" :title="taskStatusTitle(s.id)">
                      <icon-loading v-if="taskStatusIcon(s.id) === 'running'" class="proj-stream-spin" :size="15" />
                      <span v-else-if="taskStatusIcon(s.id) === 'form'" class="task-flag task-flag-dot task-flag-form" />
                      <span v-else-if="taskStatusIcon(s.id) === 'interrupted'" class="task-flag task-flag-dot task-flag-interrupted" />
                      <span v-else-if="taskStatusIcon(s.id) === 'ignored'" class="task-flag task-flag-dot task-flag-ignored" />
                      <span v-else-if="taskStatusIcon(s.id) === 'cancelled'" class="task-flag task-flag-dot task-flag-cancelled" />
                      <span v-else-if="taskStatusIcon(s.id) === 'error'" class="task-flag task-flag-dot task-flag-error" />
                      <span v-else-if="taskStatusIcon(s.id) === 'done'" class="task-flag task-flag-dot task-flag-done" />
                      <span v-else class="task-flag task-flag-dot task-flag-todo" />
                    </span>
                    <span
                      class="task-title marquee-title"
                      :class="{ 'is-overflowing': marqueeActive[s.id] }"
                    >{{ localizedSessionTitle(s) || t('common.unnamed') }}</span>
                    <span class="task-time">{{ relativeTime(s.updatedAt ?? s.createdAt, nowTick) }}</span>
                    </div>
                  </SessionRowHoverCard>
                </template>
              </div>
              <!-- 其他组(为空也显示标签) -->
              <div class="pin-group">
                <div class="pin-group-header" @click="toggleGroup('tasks-others')">
                  <span class="pin-group-label">{{ t('sidebar.others') }}</span>
                  <span class="pin-group-count">{{ otherTasks.length }}</span>
                </div>
                <template v-if="!groupCollapsed['tasks-others']">
<SessionRowHoverCard
                    v-for="s in otherTasks"
                    :key="s.id"
                    :session="s"
                    :disabled="ctxMenu.visible"
                  >
                    <div
                      class="task-item"
                    :class="taskSelected(s.id) && 'active'"
                    @click="openSession(s)"
                    @contextmenu.prevent="onSessionContext($event, s)"
                    @mouseenter="onTitleEnter($event, s.id)"
                    @mouseleave="onTitleLeave(s.id)"
                  >
                    <span class="task-status-slot" :title="taskStatusTitle(s.id)">
                      <icon-loading v-if="taskStatusIcon(s.id) === 'running'" class="proj-stream-spin" :size="15" />
                      <span v-else-if="taskStatusIcon(s.id) === 'form'" class="task-flag task-flag-dot task-flag-form" />
                      <span v-else-if="taskStatusIcon(s.id) === 'interrupted'" class="task-flag task-flag-dot task-flag-interrupted" />
                      <span v-else-if="taskStatusIcon(s.id) === 'ignored'" class="task-flag task-flag-dot task-flag-ignored" />
                      <span v-else-if="taskStatusIcon(s.id) === 'cancelled'" class="task-flag task-flag-dot task-flag-cancelled" />
                      <span v-else-if="taskStatusIcon(s.id) === 'error'" class="task-flag task-flag-dot task-flag-error" />
                      <span v-else-if="taskStatusIcon(s.id) === 'done'" class="task-flag task-flag-dot task-flag-done" />
                      <span v-else class="task-flag task-flag-dot task-flag-todo" />
                    </span>
                    <span
                      class="task-title marquee-title"
                      :class="{ 'is-overflowing': marqueeActive[s.id] }"
                    >{{ localizedSessionTitle(s) || t('common.unnamed') }}</span>
                    <span class="task-time">{{ relativeTime(s.updatedAt ?? s.createdAt, nowTick) }}</span>
                    </div>
                  </SessionRowHoverCard>
                </template>
              </div>
              </template>
            <div v-if="!rootTasks.length" class="side-empty">
              <icon-check-circle :size="30" class="side-empty-icon" />
              <span class="side-empty-title">{{ t('sidebar.emptyTasks') }}</span>
              <span class="side-empty-desc">{{ t('sidebar.emptyTasksDesc') }}</span>
            </div>
          </div>

          <!-- 项目:两级列表(目录行 = 文件夹开/关图标 + 名称 + 操作;会话子项无图标),按「置顶 / 其他」分组 -->
          <div v-else-if="activeSectionKey === 'projects'" class="project-list" :class="{ 'is-empty': !rootProjects.length }">
            <template v-if="rootProjects.length">
            <template v-for="g in projectGroups" :key="g.key">
              <div class="pin-group">
                <div class="pin-group-header" @click="toggleGroup('proj-' + g.key)">
                  <span class="pin-group-label">{{ t(g.labelKey) }}</span>
                  <span class="pin-group-count">{{ g.nodes.length }}</span>
                </div>
                <template v-if="!groupCollapsed['proj-' + g.key]">
                  <template v-for="node in g.nodes" :key="node.key">
                    <div
                      class="proj-dir"
                      :class="dirExpanded(node) && 'expanded'"
                      @click="toggleProjectDir(node)"
                      @contextmenu.prevent="onProjectContext($event, node)"
                    >
                      <span class="proj-folder" :class="dirExpanded(node) ? 'i-lucide-folder-open' : 'i-lucide-folder'" />
                      <span class="proj-dir-name">{{ node.title }}</span>
                      <!-- 目录行只保留「添加任务」；置顶 / 查看文件 / 删除走右键菜单 -->
                      <span class="tree-project-actions" @click.stop>
                        <a-tooltip :content="t('sidebar.addTask')">
                          <button type="button" class="project-act-btn" @click="addTaskFor(node)">
                            <icon-plus :size="18" />
                          </button>
                        </a-tooltip>
                      </span>
                    </div>
                    <!-- 会话子项:展开时显示,无图标 -->
                    <template v-if="dirExpanded(node)">
<SessionRowHoverCard
                        v-for="child in node.children ?? []"
                        :key="child.key"
                        :session="child.session"
                        :disabled="ctxMenu.visible"
                      >
                        <div
                          class="proj-session"
                        :class="taskSelected(child.session?.id ?? '') && 'active'"
                        @click="openProjectSession(child)"
                        @contextmenu.prevent="onSessionContext($event, child.session)"
                        @mouseenter="onTitleEnter($event, child.key)"
                        @mouseleave="onTitleLeave(child.key)"
                      >
                        <!-- 前方状态位:流式转圈 / 等你处理 / 上次中断 / 异常 / 正常结束绿标,空闲占位保持对齐 -->
                        <span class="proj-stream-slot" :title="taskStatusTitle(child.session?.id)">
                          <icon-loading v-if="child.session && taskStatusIcon(child.session.id) === 'running'" class="proj-stream-spin" :size="15" />
                          <span v-else-if="child.session && taskStatusIcon(child.session.id) === 'form'" class="task-flag task-flag-dot task-flag-form" />
                          <span v-else-if="child.session && taskStatusIcon(child.session.id) === 'interrupted'" class="task-flag task-flag-dot task-flag-interrupted" />
                          <span v-else-if="child.session && taskStatusIcon(child.session.id) === 'ignored'" class="task-flag task-flag-dot task-flag-ignored" />
                          <span v-else-if="child.session && taskStatusIcon(child.session.id) === 'cancelled'" class="task-flag task-flag-dot task-flag-cancelled" />
                          <span v-else-if="child.session && taskStatusIcon(child.session.id) === 'error'" class="task-flag task-flag-dot task-flag-error" />
                          <span v-else-if="child.session && taskStatusIcon(child.session.id) === 'done'" class="task-flag task-flag-dot task-flag-done" />
                          <span v-else-if="child.session" class="task-flag task-flag-dot task-flag-todo" />
                        </span>
                        <span
                          class="tree-session-title marquee-title"
                          :class="{ 'is-overflowing': marqueeActive[child.key] }"
                        >{{ child.title }}</span>
                        <span class="session-time">{{ relativeTime(child.session?.updatedAt ?? child.session?.createdAt ?? 0, nowTick) }}</span>
                        </div>
                      </SessionRowHoverCard>
                    </template>
                  </template>
                </template>
              </div>
            </template>
            </template>
            <div v-if="!rootProjects.length" class="side-empty">
              <icon-folder :size="30" class="side-empty-icon" />
              <span class="side-empty-title">{{ t('sidebar.emptyProjects') }}</span>
              <span class="side-empty-desc">{{ t('sidebar.emptyProjectsDesc') }}</span>
            </div>
          </div>
        </div>
      </div>
      </template>
    </template>

    <!-- 底部:设置入口(设置视图 / 文件树模式时返回按钮已在顶部 logo 下方,此处不再重复) -->
    <div v-if="!isSettingsRoute && !filesMode" class="sider-footer">
      <a-tooltip :content="t('sidebar.settings')" position="right" :disabled="!collapsed">
        <button
          class="footer-btn"
          :class="collapsed && 'collapsed'"
          @click="onFooterAction"
        >
          <icon-settings :size="18" />
          <span v-if="!collapsed" class="btn-label">{{ t('sidebar.settings') }}</span>
        </button>
      </a-tooltip>
    </div>
  </aside>

  <!-- 行右键菜单:Teleport 到 body,避免被侧栏裁剪。
       任务行(含项目下会话子项)与项目目录行共用这一套浮层结构与样式 -->
  <Teleport to="body">
    <div
      v-if="ctxMenu.visible"
      ref="ctxMenuEl"
      class="session-ctx-menu"
      :style="{ left: `${ctxMenu.x}px`, top: `${ctxMenu.y}px` }"
      @contextmenu.prevent
    >
      <!-- 任务行:查看文件 / 置顶 / 重命名 / 归档 / 打开目录 / 删除 -->
      <template v-if="ctxMenuSession">
        <button type="button" class="ctx-item" @click="onSessionMenuAction('files')">
          <icon-mind-mapping :size="15" />
          <span class="ctx-label">{{ t('sidebar.viewFiles') }}</span>
        </button>
        <button type="button" class="ctx-item" @click="onSessionMenuAction('pin')">
          <component
            :is="sessionPinned(ctxMenuSession.id) ? 'icon-pushpin-off' : 'icon-pushpin'"
            :size="15"
          />
          <span class="ctx-label">
            {{ sessionPinned(ctxMenuSession.id) ? t('sidebar.unpin') : t('sidebar.pin') }}
          </span>
        </button>
        <button type="button" class="ctx-item" @click="onSessionMenuAction('rename')">
          <icon-edit :size="15" />
          <span class="ctx-label">{{ t('sidebar.renameSession') }}</span>
        </button>
        <button type="button" class="ctx-item" @click="onSessionMenuAction('archive')">
          <icon-folder-add :size="15" />
          <span class="ctx-label">{{ t('sidebar.archiveSession') }}</span>
        </button>
        <button type="button" class="ctx-item" @click="onSessionMenuAction('reveal')">
          <icon-folder :size="15" />
          <span class="ctx-label">{{ t('chatMore.reveal') }}</span>
        </button>
        <div class="ctx-divider" />
        <button type="button" class="ctx-item ctx-danger" @click="onSessionMenuAction('delete')">
          <icon-delete :size="15" />
          <span class="ctx-label">{{ t('sidebar.deleteSession') }}</span>
        </button>
      </template>
      <!-- 项目目录行:行内只留「添加任务」；菜单里补齐 新建→查看→置顶，删除独立在分隔线之后 -->
      <template v-else-if="ctxMenuProject">
        <button type="button" class="ctx-item" @click="onProjectContextAction('addTask')">
          <icon-plus :size="15" />
          <span class="ctx-label">{{ t('sidebar.addTask') }}</span>
        </button>
        <button type="button" class="ctx-item" @click="onProjectContextAction('files')">
          <icon-mind-mapping :size="15" />
          <span class="ctx-label">{{ t('sidebar.viewFiles') }}</span>
        </button>
        <button type="button" class="ctx-item" @click="onProjectContextAction('pin')">
          <component
            :is="projectPinned(ctxMenuProject.workspace) ? 'icon-pushpin-off' : 'icon-pushpin'"
            :size="15"
          />
          <span class="ctx-label">
            {{ projectPinned(ctxMenuProject.workspace) ? t('sidebar.unpin') : t('sidebar.pin') }}
          </span>
        </button>
        <div class="ctx-divider" />
        <button type="button" class="ctx-item ctx-danger" @click="onProjectContextAction('delete')">
          <icon-delete :size="15" />
          <span class="ctx-label">{{ t('sidebar.deleteSession') }}</span>
        </button>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.nav-sider {
  display: flex;
  flex-direction: column;
  height: 100%;
  flex-shrink: 0;
  overflow: hidden;
  user-select: none;
  background-color: var(--bg-1);
  /* 宽度直接落到目标值：逐帧动画会反复重排看板多列与邮箱三栏，
     拖拽时还会被每次 pointermove 重启，产生追赶与卡顿。 */
}

/* macOS 原生 vibrancy 会在全屏动画里换材质并透出桌面，侧栏颜色无法保持；
   固定使用窗口模式的等效实色，保证窗口/全屏/动画快照三态完全一致。 */
.nav-sider.native-glass {
  background-color: var(--sidebar-solid-bg);
}

@media (prefers-reduced-transparency: reduce) {
  .nav-sider.native-glass {
    background-color: var(--sidebar-solid-bg);
  }
}

/* 工具栏 */
.sider-toolbar {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 4px 8px;
}

/* macOS 品牌行折叠按钮:与顶栏 topbar-btn 同风格的 36px 圆角悬停块(紧贴系统红黄绿) */
.mac-sidebar-toggle {
  width: 36px;
  height: 36px;
  border: none;
  border-radius: 6px;
  background-color: transparent;
  color: var(--text-primary);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background-color 0.2s ease;
}

.mac-sidebar-toggle:hover {
  background-color: rgba(0, 0, 0, 0.08);
}

.mac-sidebar-toggle:active {
  background-color: rgba(0, 0, 0, 0.14);
}

[data-theme='dark'] .mac-sidebar-toggle:hover {
  background-color: rgba(255, 255, 255, 0.1);
}

[data-theme='dark'] .mac-sidebar-toggle:active {
  background-color: rgba(255, 255, 255, 0.16);
}

/* 顶部返回按钮容器(logo 下方):文件树/设置视图显示,横向 padding 对齐品牌区 */
.sider-back {
  padding: 4px 8px;
}

.sider-back .footer-btn {
  width: 100%;
}

/* 返回按钮文字:与设置入口一致 */
.sider-back .back-btn .btn-label {
  font-size: var(--font-size-md);
}

.toolbar-btn,
.footer-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: var(--nav-item-height);
  padding: 0 8px;
  border: none;
  background: transparent;
  border-radius: var(--radius-md);
  /* 图标/文字颜色与顶部工具栏统一 */
  color: var(--text-primary);
  cursor: pointer;
  font-size: var(--font-size-md);
  line-height: var(--line-height-ui);
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.toolbar-btn:hover,
.footer-btn:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

.toolbar-btn.active,
.footer-btn.active {
  /* 只保留背景高亮,字体保持原色(--primary 为失效变量,会导致暗色下变黑) */
  background-color: var(--bg-active);
}

.new-chat-btn {
  height: var(--nav-item-height);
  color: var(--text-primary);
}

.toolbar-btn.collapsed,
.footer-btn.collapsed {
  justify-content: center;
  padding: 0;
}

.btn-label {
  font-size: var(--font-size-md);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 会话列表:整列 flex;工具栏/切换条固定,只有下方列表滚动 */
.sider-content {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  /* 与顶部品牌行的间距收紧到 5px */
  padding: 5px 8px;
}

/* 任务 / 项目 切换条:分段按钮 + 右侧刷新(固定不滚动) */
.side-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
  flex-shrink: 0;
}

/* 刷新按钮:靠右对齐(seg 为 fit-content,不铺满) */
.side-toggle .drop-refresh {
  margin-left: auto;
}

.side-toggle-seg {
  position: relative;
  display: flex;
  gap: 2px;
  width: fit-content;
  padding: 2px;
  background-color: var(--bg-3);
  border-radius: var(--radius-full);
}

/* 初次定位直接贴合按钮，只在用户切换分组后启用滑移动画。 */
.side-toggle-thumb {
  position: absolute;
  top: 2px;
  bottom: 2px;
  z-index: 0;
  border-radius: var(--radius-full);
  background-color: var(--bg-base);
  box-shadow: var(--shadow-sm);
}

.side-toggle-thumb.is-animated {
  transition:
    left var(--transition-fast),
    width var(--transition-fast);
}

@media (prefers-reduced-motion: reduce) {
  .side-toggle-thumb.is-animated {
    transition: none;
  }
}

.side-toggle-btn {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  height: 24px;
  padding: 0 10px;
  border: none;
  border-radius: var(--radius-full);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition:
    color var(--transition-fast);
}

.side-toggle-btn:hover {
  color: var(--text-primary);
}

.side-toggle-btn.active {
  color: var(--text-primary);
  font-weight: 500;
}

.side-toggle-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-secondary);
}

.side-toggle-btn.active .side-toggle-icon {
  color: var(--text-primary);
}

.side-toggle-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.side-toggle-count {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  background-color: var(--bg-3);
  padding: 0 6px;
  border-radius: 10px;
  line-height: 16px;
}

/* 刷新按钮:悬停显示,点击旋转 */
.drop-refresh {
  width: 28px;
  height: 28px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  cursor: pointer;
  opacity: 0;
  transition:
    opacity var(--transition-fast),
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.side-toggle:hover .drop-refresh {
  opacity: 1;
}

.drop-refresh:hover {
  color: var(--text-primary);
  background-color: var(--bg-active);
}

/* 列表区:固定下方,自身滚动(切换条保持不动);
   负右外边距抵消父级右内边距,让滚动条贴侧栏右缘、内容仍内缩 8px */
.drop-body {
  padding-left: 6px;
  margin-right: -8px;
  padding-right: 8px;
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

/* 空列表时才启用 flex 撑满,空状态垂直居中(不影响有内容时的分组布局) */
.drop-body.list-empty {
  display: flex;
  flex-direction: column;
}

/* 任务区:纯平铺列表(每条任务一行,非文件树) */
.task-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* 空列表:撑满列表区,空状态内容垂直居中 */
.task-list.is-empty,
.project-list.is-empty {
  flex: 1;
  justify-content: center;
}

.task-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  height: 28px;
  border-radius: var(--radius-md);
  color: var(--text-primary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.task-item:hover {
  background-color: var(--bg-hover);
}

.task-item.active,
.task-item.active:hover {
  background-color: var(--bg-active);
}

.task-title {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: var(--font-size-md);
  line-height: var(--line-height-ui);
  font-weight: 500;
  color: var(--text-primary);
}

.task-item.active .task-title {
  /* 保持与普通任务一致的文字色(不用蓝色高亮),仅靠背景与字重区分当前会话 */
  color: var(--text-primary);
  font-weight: 600;
}

.task-time {
  flex-shrink: 0;
  margin-left: auto;
  white-space: nowrap;
  font-size: var(--font-size-xs);
  line-height: var(--line-height-ui);
  color: var(--text-tertiary);
}

/* 项目区:两级列表(目录行 = 文件夹开/关图标;会话子项无图标) */
.project-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* 目录行:文件夹图标 + 名称 + 计数 + 操作;点击切换展开/收起 */
.proj-dir {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 8px 0 4px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: background-color var(--transition-fast);
}

.proj-dir:hover {
  background-color: var(--bg-hover);
}

/* 文件夹图标(开/关):lucide,颜色随主题(--text-secondary,与侧栏其他图标一致) */
.proj-folder {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
  color: var(--text-secondary);
}

.proj-dir-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-md);
  line-height: var(--line-height-ui);
  font-weight: 400;
  color: var(--text-primary);
}

/* 会话子项:无文件图标;前方状态位(流式转圈/空闲占位)保持文字与目录行左对齐 */
.proj-session {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 8px 0 4px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: background-color var(--transition-fast);
}

.proj-session:hover {
  background-color: var(--bg-hover);
}

.proj-session.active,
.proj-session.active:hover {
  background-color: var(--bg-active);
}

/* 前方状态位:16px 占位(与目录行文件夹图标同宽),保证文字左对齐列一致 */
.proj-stream-slot {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

/* 任务行前方状态位:16px(与项目子项一致,占位保持文字左对齐) */
.task-status-slot {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

/* 流式转圈图标 */
.proj-stream-spin {
  color: var(--accent-text);
  animation: sidebar-spin 0.9s linear infinite;
}

/* 任务状态图标:已完成绿点 / 等你处理橙点 / 异常(打断·失败)红点 / 进程中断灰点 */
.task-flag {
  color: var(--text-tertiary);
}

/* 圆点状态位:完成=绿色实心点,等待=橙色实心点,异常=红色实心点,中断=灰色实心点 */
.task-flag-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.task-flag-done {
  color: var(--success);
  background-color: var(--success);
}

/* 待审批 / 待填写表单:橙色实心点(「等你处理」,比红点弱一档而不是报错)
   悬停说明由 taskStatusTitle 区分「等待你的审批 / 等待你的回答」 */
.task-flag-form {
  color: var(--warning);
  background-color: var(--warning);
}

/* 异常终态红点:用户主动打断(cancelled)与执行失败(error)同色,
   悬停说明由 taskStatusTitle 按 taskFlags.end 区分（见 taskStatusIcon 注释） */
.task-flag-error,
.task-flag-cancelled {
  color: var(--danger);
  background-color: var(--danger);
}

/* 上次运行被中断（进程退出打断）：安静的中性灰点，与「执行报错」区分 */
.task-flag-interrupted,
/* 提问被忽略（用户放弃回答，仍可「继续填写表单」）：同为灰点，说明文案区分 */
.task-flag-ignored,
/* 待办（从未执行，= 看板 todo 列灰点）：仅当前正打开的待办 / 项目树子项会出现 */
.task-flag-todo {
  color: var(--text-tertiary);
  background-color: var(--text-tertiary);
}

@keyframes sidebar-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 「置顶 / 其他」分组 ---------- */
.pin-group {
  display: flex;
  flex-direction: column;
}

.pin-group-header {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 8px;
  border-radius: var(--radius-sm, 6px);
  cursor: pointer;
  user-select: none;
  transition: background-color var(--transition-fast);
}

.pin-group-header:hover {
  background-color: var(--bg-hover);
}

.pin-group-label {
  font-size: var(--font-size-md);
  font-weight: 400;
  color: var(--text-tertiary);
}

.pin-group-count {
  min-width: 18px;
  padding: 0 6px;
  border-radius: 10px;
  background-color: var(--bg-3);
  font-size: var(--font-size-2xs);
  line-height: 18px;
  text-align: center;
  color: var(--text-tertiary);
}

/* 目录操作按钮组:绝对定位右对齐,脱离文档流——常态不占位(目录文字占满整个宽度),
   悬停目录行时浮现在右侧,整个按钮区域以一块深色底遮挡目录文字尾部 */
.tree-project-actions {
  position: absolute;
  right: 2px;
  top: 50%;
  transform: translateY(-50%);
  display: inline-flex;
  align-items: center;
  gap: 0;
  padding: 0 2px;
  opacity: 0;
  pointer-events: none;
  border-radius: 8px;
  background-color: var(--bg-2);
  transition: opacity 0.15s ease;
}

.proj-dir:hover .tree-project-actions,
.tree-project-actions:focus-within {
  opacity: 1;
  pointer-events: auto;
}

/* 目录行唯一保留的行内按钮（添加任务）：24px 命中区，图标 18 */
.project-act-btn {
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-primary);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background-color 0.15s ease, color 0.15s ease;
}

.project-act-btn:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}

/* 图标线宽统一走全局标准 UI_ICON_STROKE_WIDTH（AppIcon 默认），不要逐处覆盖。 */

[data-theme='dark'] .ctx-item:hover {
  background-color: rgba(255, 255, 255, 0.10);
}

[data-theme='dark'] .project-act-btn:hover {
  background: rgba(255, 255, 255, 0.12);
}

.tree-session-title {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: var(--font-size-md);
  line-height: var(--line-height-ui);
  font-weight: 500;
  color: var(--text-primary);
}

/* ---------- 标题跑马灯：溢出时悬浮用 scrollLeft 滚动（RAF 在 onTitleEnter），日期列不动 ---------- */
/* 滚动时只淡出左侧移出的部分，右侧保留完整可读；移出后 class 移除，省略号恢复 */
.task-item:hover .marquee-title.is-overflowing,
.proj-session:hover .marquee-title.is-overflowing {
  text-overflow: clip;
  -webkit-mask-image: linear-gradient(to right, transparent 0, #000 12px);
  mask-image: linear-gradient(to right, transparent 0, #000 12px);
}

.session-time {
  flex-shrink: 0;
  margin-left: auto;
  white-space: nowrap;
  font-size: var(--font-size-xs);
  line-height: var(--line-height-ui);
  color: var(--text-tertiary);
}

/* 任务/项目空状态:图标 + 标题 + 描述,由父级(.is-empty)垂直居中 */
.side-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 12px;
  color: var(--text-tertiary);
  text-align: center;
}

.side-empty-icon {
  color: var(--text-tertiary);
  margin-bottom: 2px;
}

.side-empty-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.side-empty-desc {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  line-height: 1.5;
}

/* 设置视图:占满剩余空间,内容多时可滚动,底部按钮固定在最底部 */
.settings-view {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

/* 底部:始终吸附在侧边栏最底部 */
.sider-footer {
  margin-top: auto;
  flex-shrink: 0;
  padding: 8px;
}

/* 行右键菜单(任务行 / 项目目录行共用;Teleport 到 body,此样式仍随组件作用域生效) */
.session-ctx-menu {
  position: fixed;
  z-index: 3000;
  min-width: 168px;
  padding: 4px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-2);
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
  user-select: none;
}

.ctx-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: var(--menu-item-height);
  line-height: var(--line-height-ui);
  padding: 0 10px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

/* 悬停底色不能用 --bg-hover:它与菜单底色 --bg-2 同值(浅色 #f2f3f5 / 深色 #2e2e2e),
   等于没有反馈(用户实测「查看文件」悬停无变化)。改用 --bg-3 —— 与项目行按钮组、
   任务行操作按钮的悬停底色同一 token,观感一致;深色下另给一层提亮(见下)。 */
.ctx-item:hover {
  background-color: var(--bg-3);
}

.ctx-label {
  line-height: 1;
}

.ctx-divider {
  height: 1px;
  margin: 4px 6px;
  background-color: var(--border-base);
}

.ctx-danger {
  color: var(--danger);
}

.ctx-danger:hover {
  background-color: color-mix(in srgb, var(--danger) 10%, var(--bg-2));
}
</style>
