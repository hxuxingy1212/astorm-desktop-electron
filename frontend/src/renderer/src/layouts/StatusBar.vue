<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { useSessionsStore } from '@/stores/sessions'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { useBreakpoint } from '@/composables/useBreakpoint'
import { useSidebarCollapsed } from '@/composables/useSidebarCollapsed'
import { usePanelLayout } from '@/composables/usePanelLayout'
import { applyFullscreen } from '@/composables/useFullscreen'
import { hasTrafficLights, showWindowControls } from '@/utils/platform'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import BranchMenu from '@/components/git/BranchMenu.vue'
import GitGraphCard from '@/components/git/GitGraphCard.vue'
import PanelTabBar from '@/components/panels/PanelTabBar.vue'
// 面板最大化/还原按钮使用 Arco 官方图标（局部引入以覆盖全局手写字形）
import { IconFullscreen, IconFullscreenExit } from '@arco-design/web-vue/es/icon'

const { t } = useI18n()
const settings = useSettingsStore()
const tabs = useTabsStore()
const sessions = useSessionsStore()
const route = useRoute()
const router = useRouter()
const { bp } = useBreakpoint()
const api = window.api

// 顶部工具图标区(原右侧纵向工具条):仅聊天视图可见,点击打开/激活页签

// 更多操作菜单:受控展开,点击外部收起
const moreVisible = ref(false)
const moreBtnRef = ref<HTMLButtonElement | null>(null)

function onDocMouseDown(e: MouseEvent): void {
  const target = e.target as Element | null
  if (!target) return
  // 按钮自身 / 弹层内部(弹层 div 带 .chat-more-dropdown 类)不收起
  if (moreBtnRef.value?.contains(target)) return
  if (target.closest('.chat-more-dropdown')) return
  moreVisible.value = false
}

watch(moreVisible, (m) => {
  if (m) document.addEventListener('mousedown', onDocMouseDown, true)
  else document.removeEventListener('mousedown', onDocMouseDown, true)
})
onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocMouseDown, true)
})

// 顶部对话标题栏:仅聊天视图显示(裸路径 /chat 也属于聊天视图,见 PanelToolbar 同款注释)
const activeSession = computed(() => sessions.sessions.find((s) => s.id === sessions.activeId))
const isChatView = computed(() => route.path.startsWith('/chat'))

// ── 任务看板(中心区域切换):进看板 / 再点一次回到进来之前的那个视图 ──
const isBoardView = computed(() => route.path.startsWith('/board'))

// ── 邮箱(全视口界面):进邮箱 / 再点一次回到进来之前的那个视图 ──
const isMailView = computed(() => route.path.startsWith('/mail'))
const returnFromMail = ref('')
function onToggleMail(): void {
  if (isMailView.value) {
    router.push(returnFromMail.value || '/newChat')
    return
  }
  returnFromMail.value = route.fullPath
  router.push('/mail')
}
/** 看板与邮箱都要求整幅宽度:进入时收起两侧，离开时恢复进入前状态。 */
const isFullWidthView = computed(() => isBoardView.value || isMailView.value)

/**
 * 待处理任务数（与任务看板「待处理」列同口径）：非子会话中落 `pending` 列的会话数。
 * 用于顶栏看板按钮的角标——有需要介入（待审批 / 待答表单 / 中断 / 失败等）的任务时提示用户。
 */
const pendingCount = computed(
  () =>
    sessions.sessions.filter(
      (s) => (s.sessionType ?? 'normal') !== 'child' && sessions.boardColumnOf(s.id) === 'pending'
    ).length
)
/** 进看板前的路由(退出时回这里;空则回新建对话页)。看板是全局视图,故记在模块级 ref */
const returnFromBoard = ref('')
function onToggleBoard(): void {
  if (isBoardView.value) {
    router.push(returnFromBoard.value || '/newChat')
    return
  }
  returnFromBoard.value = route.fullPath
  router.push('/board')
}

/**
 * 进看板时把两侧都收起来（左：侧栏；右：页签面板）—— 看板要的是整幅宽度。
 * 监听**路由**而不是按钮点击：直接跳 `/board`（深链、其它入口）也一致。
 * 离开时恢复进入前的状态：侧栏要写回**原始值**（可能是 null = 跟随断点自动折叠），
 * 写死 false 会让窄窗口退出看板后强行展开。
 */
let sidesBeforeFull: { sidebar: boolean | null; tabs: boolean } | null = null
watch(
  isFullWidthView,
  (onFull) => {
    if (onFull) {
      sidesBeforeFull = { sidebar: settings.sidebarCollapsed, tabs: tabs.collapsed }
      settings.setSidebarCollapsed(true)
      tabs.collapsed = true
      return
    }
    if (!sidesBeforeFull) return
    settings.sidebarCollapsed = sidesBeforeFull.sidebar
    tabs.collapsed = sidesBeforeFull.tabs
    sidesBeforeFull = null
  },
  { immediate: true }
)
const displayTitle = computed(() =>
  localizedSessionTitle(activeSession.value).slice(0, 20)
)
// 窄窗口(md,<960px)下隐藏目录/分支徽章、收窄标题,避免标题簇把工具按钮挤出
const showMetaChips = computed(() => bp.value !== 'md')

// ── git 分支 chip(见 workspaceName 之后的探测块;弹层/图谱在其下方) ──

/** 顶栏「更多」菜单：只保留有真实实现的会话操作 */
const moreActions = computed(() => {
  const s = activeSession.value
  const pinned = !!s && !!sessions.pins[s.id]
  return [
    {
      key: 'pin',
      title: pinned ? t('chatMore.unpin') : t('chatMore.pin'),
      icon: pinned ? 'icon-pushpin-off' : 'icon-pushpin'
    },
    { key: 'reveal', title: t('chatMore.reveal'), icon: 'icon-folder' },
    { key: 'copyPath', title: t('chatMore.copyPath'), icon: 'icon-copy' },
    { key: 'copyId', title: t('chatMore.copyId'), icon: 'icon-idcard' }
  ]
})

async function copyValue(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    Message.success(t('common.copied'))
  } catch {
    Message.error(t('common.copyFailed'))
  }
}

/** 删除当前会话：确认后走后端真实删除，并回到新建页 */
function deleteActiveSession(): void {
  const s = activeSession.value
  if (!s) return
  void macConfirm({
    title: t('sidebar.deleteConfirmTitle'),
    content: t('sidebar.deleteConfirmContent', { name: localizedSessionTitle(s) || t('common.unnamed') }),
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      if (!(await sessions.deleteSession(s.id))) return
      router.push('/newChat')
    }
  })
}

/** 更多操作：置顶 / 打开目录 / 复制路径 / 复制会话 ID / 删除会话 */
async function onMoreAction(
  key: string | number | Record<string, unknown> | undefined
): Promise<void> {
  const s = activeSession.value
  if (!s) return
  switch (String(key)) {
    case 'pin':
      sessions.togglePinSession(s.id)
      return
    case 'reveal':
      void sessions.openSessionFolder(s)
      return
    case 'copyPath': {
      const dir = await resolveSessionWorkspaceDir(s.workspace || workspaceDisplay.value)
      if (!dir) {
        Message.info(t('chatMore.openDirNoPath'))
        return
      }
      await copyValue(dir)
      return
    }
    case 'copyId':
      await copyValue(s.backendId || s.id)
      return
    case 'delete':
      deleteActiveSession()
  }
}

/** 目录 chip：点击在系统文件管理器中打开该会话的工作区目录（不显示悬浮说明） */
function openWorkspaceFolder(): void {
  if (activeSession.value) void sessions.openSessionFolder(activeSession.value)
}

// ── 会话信息卡片:数据来自 store.sessionInfo(后端每次持久化推送 session_info 事件实时刷新) ──
const sessionInfo = computed(() => {
  const a = activeSession.value
  if (!a) return undefined
  return sessions.sessionInfo[a.id]
})
/**
 * 会话目录 chip 的显示值:后端权威工作区(与「会话信息」卡片的「工作区」同源)优先,
 * 本地工作区兜底。本地新建会话的 workspace 是当天日期占位(createSession → todayKey),
 * 与后端按创建时刻命名的工作区目录(精确到秒)不同,故以外层记录为准 ——
 * 卡片显示什么,chip 就显示什么。
 */
const workspaceDisplay = computed(
  () => sessionInfo.value?.workspace || activeSession.value?.workspace || ''
)

/**
 * 路径最后一级目录名(展示用;完整路径在 hover tooltip 与「复制路径」里保留)。
 * 兼容 Windows 反斜杠与结尾分隔符;取不到时原样返回。
 */
function lastDirName(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, '')
  if (!trimmed) return path
  const idx = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  const name = idx >= 0 ? trimmed.slice(idx + 1) : trimmed
  return name || trimmed
}

/** 目录 chip / 信息卡「工作区」行展示的最后一级目录名(如 `astorm-desktop-eletron`) */
const workspaceName = computed(() => (workspaceDisplay.value ? lastDirName(workspaceDisplay.value) : ''))

// ── git 分支:目录解析与「代码审查」页签同源(resolveSessionWorkspaceDir),再探测真实分支 ──
//
// repoDir 非空 = 该目录确实是 git 仓库(且已有提交) → 分支胶囊才显示;同时作为分支菜单与
// Git 图谱的入参。branchState 一次拉齐(当前/本地/远程/未提交数),菜单与图谱共用同一份。
const repoDir = ref('')
const branchState = ref<{
  current: string
  local: string[]
  remotes: string[]
  uncommitted: number
}>({ current: '', local: [], remotes: [], uncommitted: 0 })

/** 胶囊上显示的分支名(窄窗口隐藏徽章) */
const branchDisplay = computed(() => (showMetaChips.value ? branchState.value.current : ''))

/** 分支菜单展开态 */
const branchMenuVisible = ref(false)

/** Git 图谱展开态(只有点过菜单里的「Git 图谱」才为 true) */
const graphVisible = ref(false)

/**
 * 重新探测分支信息。
 *
 * 目录一律走 `resolveSessionWorkspaceDir`(相对工作区会补成 `<用户目录>/workspace/<名>`)——
 * 原来只对**绝对路径**探测,相对路径的会话拿不到分支。探测前先清空,避免切换会话瞬间
 * 显示上一个目录的分支。
 */
async function refreshBranch(): Promise<void> {
  repoDir.value = ''
  branchState.value = { current: '', local: [], remotes: [], uncommitted: 0 }
  const dir = await resolveSessionWorkspaceDir(workspaceDisplay.value)
  if (!dir) return
  try {
    const s = await window.api.git.branchState(dir)
    // current 为空 = 非仓库 / 尚无提交(rev-parse 失败)→ 视为不是仓库
    if (!s.current) return
    repoDir.value = dir
    branchState.value = s
  } catch {
    /* 探测失败按「不是仓库」处理:胶囊隐藏,不打扰用户 */
  }
}

watch(() => workspaceDisplay.value, () => void refreshBranch(), { immediate: true })
// 窄窗口切回宽窗口时徽章重新出现,补一次探测(此前可能没跑过)
watch(showMetaChips, (show) => {
  if (show) void refreshBranch()
})

/**
 * 切换 / 新建分支成功后：重探分支（胶囊与新建会话表单都以实时 HEAD 为准）。
 *
 * **不写会话记录**：分支已不进会话记录，系统提示词环境块的 `- git 分支：X` 每个用户消息
 * 自己探测工作区，切完分支下一条消息就是新值。
 * 图谱开着的话，它会跟着 `branchState.current` 变化自己重载。
 */
async function onBranchChanged(_branch: string): Promise<void> {
  await refreshBranch()
}

/** 从分支菜单进入 Git 图谱:先收起菜单(否则两层浮层叠着),再打开卡片 */
function onOpenGraph(): void {
  branchMenuVisible.value = false
  graphVisible.value = true
}

// 窗口最大化状态
const maximized = ref(false)
let offMaximized: (() => void) | null = null

// 全屏状态:共享 ref + html[data-fullscreen],驱动顶部工具栏与侧栏放大及对话列上限放宽
const fullscreen = ref(false)
let offFullscreen: (() => void) | null = null

function syncFullscreen(f: boolean): void {
  fullscreen.value = f
  applyFullscreen(f)
}

onMounted(() => {
  window.api.window.isMaximized().then((m) => {
    maximized.value = m
  })
  offMaximized = window.api.window.onMaximizedChange((m) => {
    maximized.value = m
  })
  // 可选链:兼容旧 preload 热更新过渡期(HMR 后 preload 尚未重建)
  window.api.window.isFullscreen?.().then((f) => {
    syncFullscreen(f)
  })
  offFullscreen = window.api.window.onFullscreenChange?.((f) => {
    syncFullscreen(f)
  }) ?? null
})
onBeforeUnmount(() => {
  offMaximized?.()
  offFullscreen?.()
})

// 侧栏折叠:与 AppLayout/Workspace 同一规则(启动默认展开/用户偏好优先/断点自适应),点按即可在任何宽度展开
const effectiveSidebarCollapsed = useSidebarCollapsed()
function onToggleSidebar(): void {
  settings.setSidebarCollapsed(!effectiveSidebarCollapsed.value)
}

/**
 * 新建会话(侧栏收起时在展开按钮右侧提供的入口)。
 * 继承当前会话的上下文：当前是**项目任务** → 预填其目录（分支由新建表单对目录实时探测）；
 * **普通任务** → 不带目录，直接进新建会话页。
 */
function onNewSession(): void {
  const directory = activeSession.value?.kind === 'project' ? workspaceDisplay.value : ''
  if (directory) router.push({ path: '/newChat', query: { directory } })
  else router.push('/newChat')
}

// 右侧面板布局:与正文 Workspace 共用同一套宽度，令顶栏页签栏与分割线严格对齐
const { panelWidth } = usePanelLayout()
/** 面板展开(聊天视图内):顶栏据此划出与正文一致的分割线，并把页签栏左对齐到面板左缘 */
const panelVisible = computed(() => isChatView.value && !tabs.collapsed)
/** 页签栏(▾/页签/+ /🗑)可见:面板展开且有页签 */
const tabbarVisible = computed(() => panelVisible.value && tabs.tabs.length > 0)
/**
 * 顶栏右侧「面板段」宽度 = 正文面板列宽度；左侧留白自动等于聊天区宽度，
 * 于是页签栏左缘与正文分割线在一条竖线上（左中右结构）。
 * 面板最大化时面板占满，无需分段。
 */
const headerPanelWidth = computed(() =>
  panelVisible.value && !settings.panelMaximized ? panelWidth.value : null
)
/**
 * 面板最大化时：对话区隐藏、面板占满，顶栏不再分段——页签栏直接铺满剩余宽度（左对齐）。
 * 故面板段改为 flex-1（撑满），否则它会塌成只有窗口控制按钮的宽度、页签栏被挤没。
 */
const headerSegGrow = computed(() => panelVisible.value && settings.panelMaximized)

/**
 * 面板最大化(占满内容区):仅聊天视图 + 面板展开时可用。
 * 关闭面板 / 离开聊天视图时自动复位(见下方 watch)。
 */
const panelMaximized = computed(() => settings.panelMaximized)
function onTogglePanelMaximize(): void {
  settings.togglePanelMaximized()
}
// 面板关闭 / 离开聊天视图 → 退出最大化(避免留下「最大化却无面板」的状态)
watch([() => tabs.collapsed, isChatView], ([collapsed, chat]) => {
  if (collapsed || !chat) settings.setPanelMaximized(false)
})

</script>

<template>
  <!-- macOS 桌面壳:系统红黄绿悬浮于窗口左上角;侧栏折叠为 0 宽时本行紧贴窗口左缘,需左移内容让位(全屏时系统自动隐藏红黄绿)。
       浏览器部署没有红黄绿(hasTrafficLights=false),不偏移 -->
  <!-- z-40:本行是一个层叠上下文,内部的顶栏浮层(更多菜单/分支菜单)必须**高于**
       对话区内的悬浮元素(任务规划胶囊 .plan-bar 为 z-index 30) -->
  <header
    class="app-region-drag relative z-40 flex h-11 shrink-0 items-center"
    :class="{
      'mac-lights-offset':
        hasTrafficLights && !fullscreen && (effectiveSidebarCollapsed || panelMaximized)
    }"
  >
    <!-- 侧栏折叠按钮:macOS 桌面壳下侧栏展开时移到品牌行(红黄绿旁),此处仅折叠态仍展示以恢复侧栏;
         无红黄绿(Windows/Linux 桌面壳、浏览器部署)则常驻此处;
         面板最大化时侧栏是被临时隐藏(非折叠),按钮无意义,故不显示 -->
    <div
      v-if="!hasTrafficLights || (effectiveSidebarCollapsed && !panelMaximized)"
      class="app-region-no-drag flex items-center gap-1 px-1"
    >
      <a-tooltip :content="t('statusbar.toggleSidebar')">
        <!-- 参考 astorm-desktop TopBar 的侧栏伸缩按钮;与页签伸缩按钮共用同一套图标风格 -->
        <button type="button" class="topbar-btn" @click="onToggleSidebar">
          <svg
            width="16"
            height="16"
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
      <!-- 新建会话:仅侧栏收起时出现在展开按钮右侧(展开时左栏已有「新建」入口)。
           项目任务预填当前会话目录，普通任务不带目录 -->
      <a-tooltip
        v-if="effectiveSidebarCollapsed && !panelMaximized"
        :content="t('statusbar.newSession')"
      >
        <button type="button" class="topbar-btn" @click="onNewSession">
          <icon-plus :size="16" />
        </button>
      </a-tooltip>
    </div>

    <!-- 对话标题栏:标题 + 会话信息 + 所在目录 + git 分支 + 更多操作(仅聊天视图;面板最大化时隐藏) -->
    <div
      v-if="isChatView && activeSession && !panelMaximized"
      class="app-region-no-drag flex min-w-0 items-center gap-2 px-2 text-xs"
    >
      <!-- 标题:限 20 字,溢出隐藏,悬浮显示完整;md 断点下收窄给工具栏让位 -->
      <a-tooltip :content="localizedSessionTitle(activeSession)">
        <span
          class="chat-title-in-bar min-w-0"
          :class="bp === 'md' ? 'max-w-[140px]' : 'max-w-[240px]'"
        >{{ displayTitle }}</span>
      </a-tooltip>
      <!-- 所在工作目录:点击在系统文件管理器中打开对应目录；不显示悬浮说明 -->
      <button
        v-if="showMetaChips && workspaceDisplay"
        type="button"
        class="chat-meta-chip chat-meta-chip-button"
        @click.stop="openWorkspaceFolder"
      >
        <icon-folder :size="13" />
        <span class="chat-meta-chip-text">{{ workspaceName }}</span>
      </button>
      <!-- git 分支:点开分支菜单(搜索 / 切换 / 新建并检出 / 查看 Git 图谱)。
           仅当目录确实是 git 仓库(且已有提交)时才可点——否则没有分支可切 -->
      <a-dropdown
        v-if="repoDir && branchDisplay"
        v-model:popup-visible="branchMenuVisible"
        trigger="click"
        position="bl"
        :popup-max-height="false"
        class="chat-branch-dropdown"
      >
        <button type="button" class="chat-meta-chip chat-branch-chip">
          <icon-branch :size="13" />
          <span class="chat-meta-chip-text">{{ branchDisplay }}</span>
          <icon-down :size="11" class="chat-meta-chip-arrow" />
        </button>
        <template #content>
          <BranchMenu
            :dir="repoDir"
            :open="branchMenuVisible"
            @close="branchMenuVisible = false"
            @changed="onBranchChanged"
            @open-graph="onOpenGraph"
          />
        </template>
      </a-dropdown>
      <!-- 三个点:更多操作(仅 UI);popup-max-height 关闭,避免菜单项与滚动条重叠 -->
      <a-dropdown
        v-model:popup-visible="moreVisible"
        trigger="click"
        position="bl"
        :popup-max-height="false"
        class="chat-more-dropdown"
        @select="onMoreAction"
      >
        <button ref="moreBtnRef" type="button" class="topbar-btn" :aria-label="t('statusbar.moreActions')">
          <icon-more :size="16" />
        </button>
        <template #content>
          <a-doption v-for="act in moreActions" :key="act.key" :value="act.key">
            <template #icon>
              <component :is="act.icon" :size="14" />
            </template>
            {{ act.title }}
          </a-doption>
          <a-doption value="delete" class="more-delete">
            <template #icon>
              <icon-delete :size="14" />
            </template>
            {{ t('chatMore.deleteSession') }}
          </a-doption>
        </template>
      </a-dropdown>
    </div>

    <!-- 拖拽空间:占据「聊天区」宽度;面板展开时右侧由固定宽度的面板段补足，
         于是本 spacer 的右缘 = 正文分割线位置。
         **面板最大化**时对话区隐藏、面板段自己 flex-1 撑满，本 spacer 收为 0 -->
    <div v-if="!headerSegGrow" class="min-w-0 flex-1" />

    <!-- 邮箱入口:点击进全视口邮箱界面;常显,落在任务看板图标左侧(整组靠右) -->
    <div class="app-region-no-drag flex shrink-0 items-center px-1">
      <a-tooltip
        :content="isMailView ? t('statusbar.mailBack') : t('statusbar.mail')"
        position="bottom"
      >
        <button
          type="button"
          class="topbar-btn"
          :class="{ active: isMailView }"
          @click="onToggleMail()"
        >
          <icon-mail :size="16" />
        </button>
      </a-tooltip>
    </div>

    <!-- 任务看板:属于中间对话区(非页签区)的全局入口，落在聊天区右缘、分割线左侧;
         任何路由都可用(看板是全局视图)，再点一次回到进来前的视图。
         **面板最大化**(页签全屏)时对话区隐藏 → 该入口一并隐藏，保持全屏顶栏干净 -->
    <div v-if="!panelMaximized" class="app-region-no-drag flex shrink-0 items-center px-1">
      <a-tooltip
        :content="isBoardView ? t('statusbar.boardBack') : t('statusbar.board')"
        position="bottom"
      >
        <button
          type="button"
          class="topbar-btn"
          :class="{ active: isBoardView }"
          @click="onToggleBoard()"
        >
          <icon-kanban :size="16" />
          <!-- 待处理任务数角标（有待处理才显示） -->
          <span v-if="pendingCount" class="topbar-badge">
            {{ pendingCount > 99 ? '99+' : pendingCount }}
          </span>
        </button>
      </a-tooltip>
    </div>

    <!-- 与正文分割线连续的竖线:直接钉在顶栏上(不依赖面板段盒子),
         right = 面板宽 → 与正文 PanelSplitter 的 1px 分割线同一 x;
         top/bottom 撑满顶栏高 → 与正文分割线无缝相接。 -->
    <span
      v-if="headerPanelWidth !== null"
      class="header-seg-divider"
      :style="{ right: `${headerPanelWidth}px` }"
    />

    <!-- 顶栏「面板段」:宽度 = 正文面板列宽度，左缘与正文分割线对齐。
         含页签栏(左对齐) + 窗口控制(最右)。
         self-stretch:撑满标题栏高(44/全屏 52),让窗口控制条(.win-controls)能贴满高度 -->
    <div
      class="app-region-no-drag relative flex min-w-0 items-center self-stretch"
      :class="{
        'header-panel-seg': headerPanelWidth !== null,
        'flex-1': headerSegGrow,
        'shrink-0': !headerSegGrow
      }"
      :style="headerPanelWidth !== null ? { width: `${headerPanelWidth}px` } : undefined"
    >

      <!-- 页签栏(右侧面板展开且有页签时上提到顶栏):▾ 下拉 / 页签 / + / 🗑。
           左对齐、可压缩;窄窗口下由内部页签列表横向滚动 -->
      <PanelTabBar v-if="tabbarVisible" class="min-w-0 flex-1" />
      <div v-else class="min-w-0 flex-1" />

      <!-- 窗口控制分隔线:仅带自绘窗口按钮(Windows/Linux 桌面壳)时保留 -->
      <span v-if="showWindowControls" class="mx-1 h-[18px] w-px shrink-0 bg-[var(--panel-border)]" />

      <!-- 窗口控制:页签伸缩(所有平台);最小化/最大化/关闭仅桌面壳的非 macOS 平台显示
           (macOS 为系统原生红黄绿;浏览器部署由浏览器自身提供窗口按钮,不画无效果的按钮) -->
      <!-- 会话面板(左侧会话列表)图标不在此显示:面板最大化时侧栏隐藏，该按钮无落点 -->
      <!-- self-stretch:容器撑满标题栏高,让窗口控制条(.win-controls)能贴满 44px 高 -->
      <div class="flex shrink-0 items-center gap-0.5 px-1 self-stretch">
      <!-- 面板最大化(占满内容区,隐藏侧栏与对话区):仅面板展开时可用;再点还原。
           不做常驻选中高亮(全屏态按钮保持普通样式) -->
      <a-tooltip
        v-if="panelVisible"
        :content="panelMaximized ? t('statusbar.restorePanel') : t('statusbar.maximizePanel')"
        position="bottom"
      >
        <button type="button" class="topbar-btn" @click="onTogglePanelMaximize()">
          <icon-fullscreen
            v-if="!panelMaximized"
            :size="16"
            :stroke-width="4"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
          <icon-fullscreen-exit
            v-else
            :size="16"
            :stroke-width="4"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </button>
      </a-tooltip>
      <a-tooltip :content="tabs.collapsed ? t('statusbar.expandTabs') : t('statusbar.collapseTabs')">
        <!-- 页签属于会话:仅聊天视图可伸缩,其他页面(首页/设置)点击无效 -->
        <button
          type="button"
          class="topbar-btn"
          :disabled="!isChatView"
          @click="tabs.toggleCollapsed()"
        >
          <!-- 固定图标:窗口 + 右侧面板,与侧栏伸缩按钮同一套图标风格 -->
          <svg
            width="16"
            height="16"
            viewBox="0 0 48 48"
            fill="none"
            stroke="currentColor"
            stroke-width="4"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <rect x="6" y="10" width="36" height="28" rx="5" />
            <line x1="30" y1="10" x2="30" y2="38" />
          </svg>
        </button>
      </a-tooltip>
      <!-- Windows/Linux 窗口控制:原生细线图标连排贴边(参考 Win11 标题栏);macOS 为系统原生红黄绿 -->
      <div v-if="showWindowControls" class="win-controls app-region-no-drag">
        <button
          type="button"
          class="win-ctrl"
          :aria-label="t('statusbar.minimize')"
          @click="api.window.minimize()"
        >
          <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M.5 5.5h9" /></svg>
        </button>
        <button
          type="button"
          class="win-ctrl"
          :aria-label="maximized ? t('statusbar.restore') : t('statusbar.maximize')"
          @click="api.window.toggleMaximize()"
        >
          <!-- 还原图标:后置方框的可见轮廓(被前置方框遮住其余部分)+ 前置方框,与 Windows 原生字形同构 -->
          <svg v-if="maximized" viewBox="0 0 10 10" aria-hidden="true">
            <path d="M8 .5H.5V8h2M8 .5v2" />
            <rect x="2.5" y="2.5" width="7" height="7" />
          </svg>
          <svg v-else viewBox="0 0 10 10" aria-hidden="true">
            <rect x=".5" y=".5" width="9" height="9" />
          </svg>
        </button>
        <button
          type="button"
          class="win-ctrl win-ctrl-close"
          :aria-label="t('statusbar.close')"
          @click="api.window.close()"
        >
          <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M.5 .5l9 9M9.5 .5l-9 9" /></svg>
        </button>
      </div>
      </div>
      <!-- /窗口控制 -->
    </div>
    <!-- /顶栏面板段 -->

    <!-- Git 图谱浮层(Teleport 到 body,故放在此处只为挂在同一个根下)。
         无遮罩,自己处理 Esc / 点外部关闭;只画当前分支的历史 -->
    <GitGraphCard
      v-if="graphVisible && repoDir && branchState.current"
      :dir="repoDir"
      :branch="branchState.current"
      :local="branchState.local"
      :remotes="branchState.remotes"
      @close="graphVisible = false"
    />
  </header>
</template>

<style scoped>
/* 工具栏不画分割线：与下方内容区靠留白 + 背景层次区分即可（面板本身已有圆角与投影）。
   此前这里有一条 1px var(--panel-border)，在深色/浅色下都显得像多出来的一条横线。 */

/* macOS:系统红黄绿按钮悬浮在窗口左上角;侧栏折叠为 0 宽时本行紧贴窗口左缘,
   预留按钮簇宽度(~52px)+间距,避免内容被遮挡(全屏时系统自动隐藏按钮,此时不偏移)。
   留白 72px + 簇自身 px-1(4px) = 76px,与展开态品牌行 pl-[76px] 的按钮 x 完全一致 */
.mac-lights-offset {
  padding-left: 72px;
}

/* 顶栏「面板段」:宽度 = 正文面板列宽度，与正文面板同速过渡，避免拖拽/开关时错位 */
.header-panel-seg {
  transition: width 0.5s ease;
}

/* 顶栏右缘分割线:钉在 header 上,right = 面板宽(与正文 PanelSplitter 的分割线同一 x),
   top/bottom 撑满顶栏高(与正文分割线无缝相接)。不依赖面板段盒子,避免 items-center 导致的高度差 */
.header-seg-divider {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 1px;
  background-color: var(--panel-border);
}

/* 参考 astorm-desktop TopBar 的按钮:36px 方形圆角 6px,悬停/按下带明暗两套反馈 */
.topbar-btn {
  position: relative;
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

/* 待处理任务数角标：右上角小胶囊（橙色=需介入，与「等你处理」同色系） */
.topbar-badge {
  position: absolute;
  top: 1px;
  right: 1px;
  min-width: 15px;
  height: 15px;
  padding: 0 4px;
  border-radius: 8px;
  background-color: var(--warning);
  color: #fff;
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  font-weight: 700;
  line-height: 15px;
  text-align: center;
  font-variant-numeric: tabular-nums;
  pointer-events: none;
}

.topbar-btn:hover {
  background-color: rgba(0, 0, 0, 0.08);
}

/* 禁用态(非聊天视图的页签伸缩):点击无效,弱化视觉 */
.topbar-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.topbar-btn:disabled:hover {
  background-color: transparent;
}

.topbar-btn:disabled:active {
  background-color: transparent;
}

.topbar-btn:active {
  background-color: rgba(0, 0, 0, 0.14);
}

[data-theme='dark'] .topbar-btn:hover {
  background-color: rgba(255, 255, 255, 0.1);
}

/* 当前就在看板视图:按钮常驻底色(与 hover 同档,表明这是一个可开关的视图入口) */
.topbar-btn.active {
  background-color: rgba(0, 0, 0, 0.08);
}

[data-theme='dark'] .topbar-btn.active {
  background-color: rgba(255, 255, 255, 0.1);
}

[data-theme='dark'] .topbar-btn:active {
  background-color: rgba(255, 255, 255, 0.16);
}

/* ===== Windows/Linux 窗口控制条:原生风格(细线图标 + 贴边连排 + 方角) ===== */
/* 旧实现复用 .topbar-btn(36px 圆角胶囊、彼此留缝、lucide 粗图标),与原生窗口观感差异大 */
.win-controls {
  align-self: stretch;
  display: flex;
  margin-right: -4px; /* 抵消容器 px-1,贴齐窗口右缘 */
}

.win-ctrl {
  width: 46px;
  border: none;
  border-radius: 0;
  background-color: transparent;
  color: var(--text-primary);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
}

/* 10×10 细线字形(1px 描边);选择器带 .win-ctrl 以压过全屏模式的 header svg 放大规则 */
.win-controls .win-ctrl svg {
  width: 10px;
  height: 10px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1;
}

.win-ctrl:hover {
  background-color: rgba(0, 0, 0, 0.06);
}

.win-ctrl:active {
  background-color: rgba(0, 0, 0, 0.1);
}

[data-theme='dark'] .win-ctrl:hover {
  background-color: rgba(255, 255, 255, 0.08);
}

[data-theme='dark'] .win-ctrl:active {
  background-color: rgba(255, 255, 255, 0.12);
}

/* 关闭悬停:Windows 原生红色语义 */
.win-ctrl-close:hover,
.win-ctrl-close:active {
  background-color: #c42b1c;
  color: #fff;
}

/* 顶部对话标题栏 */
.chat-title-in-bar {
  font-size: var(--font-size-sm);
  font-weight: 700;
  color: var(--text-title);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 目录 / 分支徽章:参考图样式——浅灰全圆角胶囊(无边框),图标 + 文字 + (分支箭头) */
.chat-meta-chip {
  flex-shrink: 0;
  max-width: 180px;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  background: var(--bg-2);
  padding: 4px 10px;
  border-radius: 999px;
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.chat-meta-chip svg {
  flex-shrink: 0;
}

/* 目录 chip 可点击：保持胶囊外观，补上按钮重置与悬浮/聚焦反馈 */
.chat-meta-chip-button {
  border: 0;
  font-family: inherit;
  line-height: inherit;
  cursor: pointer;
  transition: background-color var(--transition-fast);
}

.chat-meta-chip-button:hover {
  background: var(--bg-3);
}

.chat-meta-chip-button:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 2px;
}

/* 徽章内文字:独立省略,保证图标不被压缩 */
.chat-meta-chip-text {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 分支下拉箭头:次要色,与文字间距小 */
.chat-meta-chip-arrow {
  flex-shrink: 0;
  color: var(--text-tertiary);
}

/* 分支胶囊是 <button>(可点开菜单):清掉 UA 默认字体/边框与手型,底色沿用 .chat-meta-chip */
.chat-branch-chip {
  font: inherit;
  border: none;
  cursor: pointer;
  transition: background-color 0.15s;
}

.chat-branch-chip:hover {
  background: var(--bg-3);
}
</style>

<style>
/* 非 scoped:更多菜单弹层 teleport 到 body;对齐全局 .session-ctx-menu / .ctx-item 的菜单观感 */
.chat-more-dropdown .arco-dropdown {
  min-width: 168px;
  width: max-content;
  padding: 4px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-2);
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
}

.chat-more-dropdown .arco-dropdown-list {
  padding: 0;
}

.chat-more-dropdown .arco-dropdown-option {
  height: var(--menu-item-height);
  line-height: var(--line-height-ui);
  padding: 0 10px;
  border-radius: 6px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

.chat-more-dropdown .arco-dropdown-option:hover,
.chat-more-dropdown .arco-dropdown-option.arco-dropdown-option-active {
  background: var(--bg-3);
  color: var(--text-primary);
}

.chat-more-dropdown .arco-dropdown-option-icon {
  margin-right: 8px;
  font-size: 14px;
}

[data-theme='dark'] .chat-more-dropdown .arco-dropdown-option:hover,
[data-theme='dark'] .chat-more-dropdown .arco-dropdown-option.arco-dropdown-option-active {
  background: rgba(255, 255, 255, 0.1);
}

/* 删除项：独立的危险色 + 内缩分隔线（与侧栏右键菜单一致） */
.chat-more-dropdown .arco-dropdown-option.more-delete {
  position: relative;
  margin-top: 5px;
  color: var(--danger);
}

.chat-more-dropdown .arco-dropdown-option.more-delete::before {
  content: '';
  position: absolute;
  top: -3px;
  left: 6px;
  right: 6px;
  height: 1px;
  background: var(--border-base);
}

.chat-more-dropdown .arco-dropdown-option.more-delete:hover {
  background: color-mix(in srgb, var(--danger) 10%, var(--bg-2));
  color: var(--danger);
}

/* 非 scoped:分支菜单弹层(teleport 到 body)。面板自带内边距,这里去掉 Arco 默认的
   弹层 padding 并补上边框/圆角/阴影,与其它浮层观感一致 */
.chat-branch-dropdown .arco-dropdown {
  padding: 0;
  border: 1px solid var(--border-base);
  border-radius: 12px;
  background-color: var(--panel-bg);
  box-shadow: 0 10px 32px rgba(0, 0, 0, 0.18), 0 1px 2px rgba(0, 0, 0, 0.06);
}
</style>
