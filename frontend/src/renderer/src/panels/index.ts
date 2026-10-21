import type { Component } from 'vue'
import type { PanelTab } from '@/types'
import BrowserPanel from './BrowserPanel.vue'
import TerminalPanel from './TerminalPanel.vue'
import CodeReviewPanel from './CodeReviewPanel.vue'
import WorkspaceFilesPanel from './WorkspaceFilesPanel.vue'
import SessionTracePanel from './SessionTracePanel.vue'

export interface PanelDef {
  /** 工具 id,同时是页签唯一标识 */
  toolId: string
  /** 面板标题的 i18n key */
  titleKey: string
  /** 面板描述的 i18n key(新建页签落地页用) */
  descKey: string
  /** Arco 图标名(已全局注册) */
  icon: string
  component: Component
}

/**
 * 页签宽度标准(两档,见 `usePanelLayout`)：
 * - **宽**(`wide`)：浏览器页签、office/PDF 文件预览、以及**交付物**打开的文件预览
 *   （交付物=Agent 产出的成品，见 `FileTabState.origin`）——这些内容需要大幅面；
 * - **窄**(`narrow`)：其余全部(相关资料/相关记忆/终端/代码审查/工作区文件/会话轨迹/
 *   插件页/子会话/普通文本与图片预览等)，标准宽度 = `PANEL_COMPACT_WIDTH`。
 *
 * **只决定激活页签时的默认宽度**（避免「先开浏览器(宽)再切资料页」一直沿用宽幅）；
 * 用户拖拽不受档位限制，两档都可从最小自由拖到最大。
 */
export type PanelWidthMode = 'wide' | 'narrow'

/** office/PDF 预览类文件(与 FilePreviewPanel 的 office 分支同口径) */
const WIDE_FILE_RE = /\.(docx?|docm|xlsx?|xlsm|csv|pptx?|pptm|ppsx?|pps|pdf)$/i

/** 页签宽度档位:浏览器 / office 预览 = 宽,其余统一窄(无页签的落地页 = 窄) */
export function widthModeOfTab(tab: PanelTab | null | undefined): PanelWidthMode {
  if (!tab) return 'narrow'
  if (tab.toolId === 'browser') return 'wide'
  if (tab.file) {
    // 交付物预览始终宽档:成品多为文档/表格/网页/长文本,窄档看不全(用户要求)。
    // 其余文件仍按扩展名——office/PDF 宽,普通文本与图片窄。
    if (tab.file.origin === 'deliverable') return 'wide'
    return WIDE_FILE_RE.test(tab.file.fileName ?? '') ? 'wide' : 'narrow'
  }
  return 'narrow'
}

/**
 * 工具/面板注册表:状态栏工具栏与页签容器共用。
 * 新增工具 = 新增一个面板组件 + 在此注册一行。
 */
export const PANEL_REGISTRY: PanelDef[] = [
  { toolId: 'browser', titleKey: 'panel.browser.title', descKey: 'panel.browser.desc', icon: 'icon-globe', component: BrowserPanel },
  { toolId: 'terminal', titleKey: 'panel.terminal.title', descKey: 'panel.terminal.desc', icon: 'icon-code-square', component: TerminalPanel },
  { toolId: 'code-review', titleKey: 'panel.codeReview.title', descKey: 'panel.codeReview.desc', icon: 'icon-git-branch', component: CodeReviewPanel },
  { toolId: 'workspace-files', titleKey: 'panel.workspaceFiles.title', descKey: 'panel.workspaceFiles.desc', icon: 'icon-mind-mapping', component: WorkspaceFilesPanel },
  { toolId: 'session-trace', titleKey: 'panel.sessionTrace.title', descKey: 'panel.sessionTrace.desc', icon: 'icon-history', component: SessionTracePanel }
]

export function getPanelDef(toolId: string): PanelDef {
  const def = PANEL_REGISTRY.find((p) => p.toolId === toolId)
  if (!def) throw new Error(`Unknown panel tool: ${toolId}`)
  return def
}
