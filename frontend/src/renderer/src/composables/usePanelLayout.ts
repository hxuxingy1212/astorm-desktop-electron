import { computed } from 'vue'
import type { ComputedRef } from 'vue'
import { useBreakpoint } from '@/composables/useBreakpoint'
import { useSidebarCollapsed } from '@/composables/useSidebarCollapsed'
import { useSettingsStore, CHAT_MIN_WIDTH, PANEL_COMPACT_WIDTH } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { widthModeOfTab, type PanelWidthMode } from '@/panels'

/**
 * 右侧面板列布局：把「可用宽度 / 面板实际宽度 / 聊天区宽度」收敛到一处，
 * 供 **Workspace（正文分栏）** 与 **StatusBar（顶栏按面板宽度对齐页签栏）**
 * 共用同一套数值，保证顶栏与正文的纵向分割线严格对齐（左中右结构）。
 */
/** 页签栏固定控件最小宽度（▾ / + / 🗑）：低于它控件会被裁掉 */
const TABBAR_MIN = 116
/** 侧栏与内容区之间的固定间隙（与 AppLayout .app-right-card 的 margin-left 一致） */
const CENTER_GAP = 12
/** 分栏拖拽条宽度（PanelSplitter 的 w-1.5 = 6px） */
export const SPLITTER_WIDTH = 6

export interface PanelLayout {
  /** 面板可见（未收起；调用方按聊天视图再叠加判断） */
  tabsOpen: ComputedRef<boolean>
  /** 侧栏实际占宽（折叠 = 0） */
  sidebarWidth: ComputedRef<number>
  /** 右侧内容区可用宽度（视口 − 侧栏 − 间隙） */
  available: ComputedRef<number>
  /** 面板列实际宽度（钳制后，含拖拽值） */
  panelWidth: ComputedRef<number>
  /** 面板列宽度下限（拖拽范围，随激活页签的窄档变化） */
  panelMin: ComputedRef<number>
  /** 面板列宽度上限（两档统一：可用宽 − 聊天区最小宽；宽/窄只决定默认宽度,不限制拖拽） */
  panelMax: ComputedRef<number>
  /** 激活页签的宽度档位：宽=仅浏览器与 office 预览,窄=其余全部（决定激活时的默认宽度） */
  widthMode: ComputedRef<PanelWidthMode>
  /** 聊天区实际宽度（可用 − 分割条 − 面板列） */
  chatWidth: ComputedRef<number>
}

/**
 * 计算右侧面板列布局。与 `Workspace.vue` 原有公式保持一致：
 * 面板宽度由用户拖拽（settings.panelWidth）驱动，钳制在 [panelMin, panelMax]。
 */
export function usePanelLayout(): PanelLayout {
  const tabs = useTabsStore()
  const settings = useSettingsStore()
  const { width } = useBreakpoint()
  const sidebarCollapsed = useSidebarCollapsed()

  const tabsOpen = computed(() => !tabs.collapsed)

  // 宽度档位:无页签(落地页)按窄档;有页签看激活项(浏览器 / office 文件 = 宽)
  const widthMode = computed<PanelWidthMode>(() =>
    tabs.tabs.length === 0 ? 'narrow' : widthModeOfTab(tabs.activeTab)
  )

  const sidebarWidth = computed(() => (sidebarCollapsed.value ? 0 : settings.sidebarWidth))

  const available = computed(() => width.value - sidebarWidth.value - CENTER_GAP)
  const splitArea = computed(() => Math.max(0, available.value - SPLITTER_WIDTH))

  const chatMinWidth = computed(() =>
    Math.min(CHAT_MIN_WIDTH, Math.max(0, splitArea.value - TABBAR_MIN))
  )

  const SPLIT_MIN_FRAC = 0.28
  const panelMin = computed(() =>
    Math.min(
      Math.max(280, Math.round(splitArea.value * SPLIT_MIN_FRAC)),
      Math.max(TABBAR_MIN, splitArea.value - chatMinWidth.value)
    )
  )
  /** 拖拽上限（两档统一）= 可用宽 − 聊天区最小宽：页签可拿满、聊天区退让到下限 480。
   *  宽/窄档只决定**激活时的默认宽度**，不限制用户拖拽（自由从最小到最大）。 */
  const panelMax = computed(() => Math.max(panelMin.value, splitArea.value - chatMinWidth.value))
  /** 拖拽下限：窄档允许收得比常规下限更小（资料/树这类列表用不着宽幅；宽档保持常规下限） */
  const panelMinForTab = computed(() =>
    widthMode.value === 'narrow' ? Math.min(panelMin.value, PANEL_COMPACT_WIDTH) : panelMin.value
  )
  const panelWidth = computed(() =>
    Math.min(panelMax.value, Math.max(panelMinForTab.value, settings.panelWidth))
  )
  const chatWidth = computed(() =>
    Math.max(0, splitArea.value - (tabsOpen.value ? panelWidth.value : 0))
  )

  return {
    tabsOpen,
    sidebarWidth,
    available,
    panelWidth,
    panelMin: panelMinForTab,
    panelMax,
    widthMode,
    chatWidth
  }
}
