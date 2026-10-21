<script setup lang="ts">
/**
 * 页签栏（从 PanelContainer 抽出，供应用顶栏 StatusBar 承载）。
 *
 * 只负责「页签的展示与切换」：页签列表、+ 添加、全部关闭。
 * 面板内容仍由 PanelContainer 渲染，两者通过 tabs store 共享状态，互不依赖。
 *
 * 为何抽到顶栏：右侧面板展开时，把页签栏提升到窗口顶部（与窗口控制同一行），
 * 面板内部只留内容，视觉更干净，也便于「面板最大化」时整条页签栏横跨窗口顶部。
 */
import { nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useTabsStore } from '@/stores/tabs'
import { usePluginsStore } from '@/stores/plugins'
import { useSessionsStore } from '@/stores/sessions'
import { getPanelDef, PANEL_REGISTRY } from '@/panels'
import { getFileIconUrl } from '@/utils/fileIcons'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import { useHoverMarquee } from '@/composables/useHoverMarquee'

/** 文件图标 URL(与文件树同源:手写 SVG) */
function fileIconUrl(fileName: string): string {
  return getFileIconUrl(fileName)
}

const { t } = useI18n()
const tabs = useTabsStore()
const plugins = usePluginsStore()
const sessions = useSessionsStore()

/** 页签标题溢出时的悬浮跑马灯（与侧栏任务行共用同一实现） */
const {
  active: marqueeActive,
  enter: onTabTitleEnter,
  leave: onTabTitleLeave,
  stop: stopTabMarquee
} = useHoverMarquee()

// 添加工具下拉的展开状态
const addVisible = ref(false)

/**
 * 「添加页签」弹层的展开方向：
 * - `bl`：左缘对齐触发按钮 → 卡片向右展开（默认，符合「在添加按钮右侧」）；
 * - `br`：右缘对齐 → 卡片向左展开（右侧空间不足时兜底）。
 * Arco 的 autoFitPosition 对 bl/br 只做**纵向**自适应，不会横向翻转，故这里自行按剩余宽度选边。
 */
const addBtnRef = ref<HTMLElement | null>(null)
const addPosition = ref<'bl' | 'br'>('bl')
/** 卡片最小宽（.tab-picker min-width 168）+ 弹层内边距，用作展开方向的判定阈值 */
const ADD_MENU_WIDTH = 200

/** 展开前按「按钮左缘到视口右缘」的剩余空间决定向左还是向右 */
function updateAddPosition(): void {
  const rect = addBtnRef.value?.getBoundingClientRect()
  if (!rect) return
  addPosition.value = window.innerWidth - rect.left >= ADD_MENU_WIDTH ? 'bl' : 'br'
}

/** 选中工具:打开/激活页签并收起下拉 */
function onPick(toolId: string): void {
  tabs.openTab(toolId)
  addVisible.value = false
}

/** 插件页签标题;工具页签标题;文件页签标题 = 文件名;网页页签标题 = 站点 title;自定义 label 优先(终端) */
function titleOf(tab: {
  pluginId?: string
  toolId?: string
  label?: string
  file?: { fileName?: string }
  web?: { title?: string; url?: string }
  session?: { sessionId?: string; label?: string }
  sources?: { items: unknown[] }
  memoryResults?: { items: unknown[] }
  memory?: { conceptId: string; label?: string }
  memoryRaw?: { stem: string; label?: string }
}): string {
  // 子会话页签：标题实时取会话列表（后端随后生成标题，页签随之更新），兜底存下的 label
  if (tab.session) {
    const live = tab.session.sessionId
      ? sessions.sessions.find((s) => s.id === tab.session!.sessionId)
      : undefined
    return localizedSessionTitle(live) || tab.session.label || tab.label || t('chat.childSessions')
  }
  if (tab.sources) return tab.label || t('panel.sources.title')
  if (tab.memoryResults) return tab.label || t('panel.memoryResults.title')
  if (tab.memory) return tab.memory.label || tab.label || t('panel.memoryConcept.title')
  if (tab.memoryRaw) return tab.memoryRaw.label || tab.label || t('panel.memoryRaw.title')
  if (tab.pluginId) return plugins.byId.get(tab.pluginId)?.manifest.name ?? tab.label ?? tab.pluginId
  if (tab.file?.fileName) return tab.file.fileName
  if (tab.web) {
    // 浏览器页签：优先页面标题（同原生页签）；跨域读不到标题（回退成 URL）时显示站点域名
    const wt = tab.web.title?.trim() ?? ''
    if (wt && !/^https?:\/\//i.test(wt)) return wt
    const src = tab.web.url || wt
    try {
      return new URL(src).hostname.replace(/^www\./, '')
    } catch {
      return wt || tab.label || t('panel.browser.title')
    }
  }
  if (tab.label) return tab.label
  return tab.toolId ? t(getPanelDef(tab.toolId).titleKey) : t('panel.newTab')
}

function isActive(toolId: string): boolean {
  return tabs.activeTab?.toolId === toolId
}

/** 页签滚动容器(跟随激活项时用) */
const tabListEl = ref<HTMLElement | null>(null)

/** 滚轮纵向位移 → 横向滚动的放大系数(1 太钝,滚很久才挪一个页签) */
const WHEEL_SCROLL_FACTOR = 4

/**
 * 页签栏横向滚动:滚动条隐藏,鼠标悬浮时滚轮上下滚动转为横向(仅当内容溢出可滚时)。
 *
 * 触摸板一次滑动会同时给出 deltaX / deltaY,若只看 deltaY(或把两者相加)就会与
 * 用户的实际横向滑动方向打架 → 页签来回弹。故只取**绝对值较大的那个轴**作为主方向,
 * 纵向位移按系数放大,横向位移原样使用(触摸板横向滑动本就是这个方向)。
 */
function onTabbarWheel(e: WheelEvent): void {
  const el = e.currentTarget as HTMLElement
  if (el.scrollWidth <= el.clientWidth) return
  const dominant = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY * WHEEL_SCROLL_FACTOR
  if (dominant === 0) return
  el.scrollLeft += dominant
  e.preventDefault()
}

/** 激活页签两侧留白(px):避免选中页签紧贴可视区边缘、看不出它被自动带进来了 */
const TAB_EDGE_PAD = 8

/**
 * 让**当前选中的页签**进入可视区:新开页签、点击切换、键盘/其它入口切换都跟随。
 *
 * 只改本容器的 scrollLeft,不用 `scrollIntoView`——后者会连带滚动祖先(整条页签栏在顶栏里,
 * 祖先被滚动会导致整页跳位);也不用 offsetLeft(本应用里多处滚动容器的 offsetParent 链不可靠),
 * 一律用两个 rect 的差值换算,与定位上下文无关。
 */
function ensureActiveTabVisible(): void {
  const el = tabListEl.value
  if (!el) return
  const active = el.querySelector<HTMLElement>('.tabbar-tab-active')
  if (!active) return
  const elRect = el.getBoundingClientRect()
  const actRect = active.getBoundingClientRect()
  const left = actRect.left - elRect.left + el.scrollLeft - TAB_EDGE_PAD
  const right = actRect.right - elRect.left + el.scrollLeft + TAB_EDGE_PAD
  if (left < el.scrollLeft) {
    el.scrollLeft = left
  } else if (right > el.scrollLeft + el.clientWidth) {
    el.scrollLeft = right - el.clientWidth
  }
}

// 激活项变化(含新开页签、关闭后自动激活相邻页签)或页签数量变化 → 等 DOM 落定后跟随。
// flush: 'post' 保证在渲染后测量;容器隐藏(clientWidth=0)时函数自身安全返回。
watch(
  () => [tabs.activeId, tabs.tabs.length] as const,
  () => void nextTick(ensureActiveTabVisible),
  { flush: 'post' }
)

// 页签增删后旧的标题节点可能已卸载，停掉跑马灯避免 RAF 指向脱离文档的元素
watch(
  () => tabs.tabs.map((tab) => tab.id).join('|'),
  () => stopTabMarquee()
)

</script>

<template>
  <!-- 无页签时整条隐藏(顶栏不占位;空态落地页由面板内部渲染) -->
  <div v-if="tabs.tabs.length > 0" class="tabbar-bar flex min-w-0 items-center gap-1">
    <!-- 页签列表:超宽时横向滚动(无滚动条,悬浮滚轮控制);单页签限宽,文字溢出省略;悬浮显示删除按钮 -->
    <div
      ref="tabListEl"
      class="tabbar-list flex min-w-0 items-center gap-1 overflow-x-auto"
      @wheel="onTabbarWheel"
    >
      <div
        v-for="tab in tabs.tabs"
        :key="tab.id"
        class="tabbar-tab cursor-pointer"
        :class="tab.id === tabs.activeId && 'tabbar-tab-active'"
        @mouseenter="onTabTitleEnter($event, tab.id)"
        @mouseleave="onTabTitleLeave(tab.id)"
        @click="
          tab.file || tab.web || tab.session || tab.sources || tab.memoryResults || tab.memory || tab.memoryRaw
            ? tabs.activateTab(tab.id)
            : tab.pluginId
              ? tabs.openPluginTab(tab.pluginId)
              : tabs.openTab(tab.toolId!)
        "
      >
        <img v-if="tab.file" class="tab-glyph" :src="fileIconUrl(tab.file.fileName)" width="16" height="16" alt="" />
        <icon-globe v-else-if="tab.web" :size="16" class="shrink-0" />
        <icon-puzzle v-else-if="tab.pluginId" :size="16" class="shrink-0" />
        <icon-user-group v-else-if="tab.session" :size="16" class="shrink-0" />
        <icon-link v-else-if="tab.sources" :size="16" class="shrink-0" />
        <icon-memory v-else-if="tab.memoryResults || tab.memory || tab.memoryRaw" :size="16" class="shrink-0" />
        <component v-else :is="getPanelDef(tab.toolId!).icon" :size="16" class="shrink-0" />
        <span
          class="marquee-title min-w-0 flex-1 truncate"
          :class="{ 'is-overflowing': marqueeActive[tab.id] }"
        >{{ titleOf(tab) }}</span>
        <button
          type="button"
          class="tabbar-tab-close"
          :aria-label="t('panel.closeTab')"
          @click.stop="tabs.closeTab(tab.id)"
        >
          <icon-close :size="12" />
        </button>
      </div>
    </div>

    <!-- 添加页签:紧邻页签右侧,下拉选择工具(同样关闭弹层限高) -->
    <!-- 添加页签:弹层默认在按钮**右侧**展开(左缘对齐按钮);右侧空间不足时翻到左侧(见 updateAddPosition) -->
    <a-dropdown v-model:popup-visible="addVisible" trigger="hover" :position="addPosition" :popup-max-height="false" class="tab-dropdown">
      <!-- 悬浮即弹出卡片，故不再挂 tooltip（正文与弹层会重叠） -->
      <button
        ref="addBtnRef"
        type="button"
        class="tabbar-btn tabbar-btn-round"
        :aria-label="t('panel.addTab')"
        @mouseenter="updateAddPosition"
      >
        <icon-plus :size="16" />
      </button>
      <template #content>
        <div class="tab-picker">
          <div
            v-for="def in PANEL_REGISTRY"
            :key="def.toolId"
            class="tab-picker-item"
            :class="isActive(def.toolId) && 'tab-picker-item-active'"
            @click="onPick(def.toolId)"
          >
            <component :is="def.icon" :size="16" class="shrink-0" />
            <span class="min-w-0 flex-1 truncate">{{ t(def.titleKey) }}</span>
          </div>
        </div>
      </template>
    </a-dropdown>

    <!-- 占位:把「全部删除」推到最右 -->
    <div class="min-w-0 flex-1" />

    <!-- 全部删除:居右对齐 -->
    <a-tooltip :content="t('panel.closeAll')">
      <button type="button" class="tabbar-btn tabbar-btn-round" :aria-label="t('panel.closeAll')" @click="tabs.closeAll()">
        <icon-close :size="16" />
      </button>
    </a-tooltip>
  </div>
</template>

<style scoped>
/* 页签栏与窗口控制之间保留间距。 */
.tabbar-bar {
  padding-left: 6px;
}

/* 页签栏按钮:与页签同高,圆形无边框(transparent 占位),风格协调 */
.tabbar-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  flex-shrink: 0;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 50%;
  background-color: transparent;
  /* 与顶栏其它按钮统一：主文字色（此前为次要灰，导致同一行图标深浅不一） */
  color: var(--text-primary);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.tabbar-btn:hover {
  background-color: var(--bg-hover);
}

/* 全部删除按钮:圆形无边框,仅悬浮时显示背景 */
.tabbar-btn-round {
  border: none;
  border-radius: 50%;
}

/* 页签:圆角,限宽,文字溢出省略;非选中态透明边框(仅占位,不显示线) */
.tabbar-tab {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  min-width: 0;
  max-width: 160px;
  flex-shrink: 0;
  padding: 0 8px;
  border: 1px solid transparent;
  border-radius: 10px;
  background-color: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  transition:
    background-color 0.15s ease,
    color 0.15s ease,
    border-color 0.15s ease;
}

.tabbar-tab:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

/* 页签标题:加粗加大 */
.tabbar-tab .truncate {
  font-size: var(--font-size-sm);
  font-weight: 600;
}

/* 标题溢出时悬浮用 scrollLeft 跑马灯（useHoverMarquee）；左侧 12px 渐隐 */
.tabbar-tab:hover .marquee-title.is-overflowing {
  text-overflow: clip;
  -webkit-mask-image: linear-gradient(to right, transparent 0, #000 12px);
  mask-image: linear-gradient(to right, transparent 0, #000 12px);
}

/* 当前激活页签:仅淡色背景高亮(不画边框);不随主题色 */
.tabbar-tab-active,
.tabbar-tab-active:hover {
  background-color: var(--bg-hover);
  border-color: transparent;
  color: var(--text-primary);
}

/* 页签删除按钮:默认透明占位,悬浮页签时显示;
   preflight 关闭,需重置 button 默认样式(灰底/凸起边框/padding) */
.tabbar-tab-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
  padding: 0;
  border: none;
  border-radius: 50%;
  background-color: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  opacity: 0;
  transition:
    opacity 0.15s ease,
    background-color 0.15s ease,
    color 0.15s ease;
}

.tabbar-tab:hover .tabbar-tab-close {
  opacity: 1;
}

.tabbar-tab-close:hover {
  background-color: var(--bg-active);
  color: var(--text-primary);
}

/* 下拉列表:与页签同款样式(圆角边框卡片,激活态与页签选中态一致)。
   底色/边框/圆角由外层弹层盒子统一提供,内部不留 padding */
.tab-picker {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 168px;
}

.tab-picker-item {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  min-width: 0;
  flex-shrink: 0;
  padding: 0 8px;
  border: none;
  border-radius: 6px;
  background-color: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.tab-picker-item:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

/* 激活项:与页签选中态一致(中性灰黑) */
.tab-picker-item-active,
.tab-picker-item-active:hover {
  background-color: var(--bg-active);
  color: var(--text-primary);
}

/* 页签列表横向滚动时隐藏滚动条,滚动由鼠标悬浮滚轮控制(见 onTabbarWheel)。
   与看板横向列同理:横向槽占高度、scrollbar-gutter 管不到,一显示就压掉页签高度,故整体不显示 */
.tabbar-list {
  scrollbar-width: none;
}

.tabbar-list::-webkit-scrollbar {
  display: none;
}

/* 文件页签图标:与文件树同款手写 SVG(14px 与页签字号对齐;实色自带明暗可读性,无需滤镜) */
.tab-glyph {
  display: block;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

</style>

<style>
/* 非 scoped:Arco 弹层 teleport 到 body,scoped 选择器覆盖不到。
   外层盒子保留边框(与全局分割线同色)、底色统一为面板色,仅留少量内边距 */
.tab-dropdown .arco-dropdown {
  padding: 4px;
  border: 1px solid var(--panel-border);
  background-color: var(--panel-bg);
}
</style>
