import type { Component } from 'vue'
import SystemSettings from './pages/SystemSettings.vue'
import SessionSettings from './pages/SessionSettings.vue'
import ModelSettings from './pages/ModelSettings.vue'
import AboutSettings from './pages/AboutSettings.vue'
import SkillsSettings from './pages/SkillsSettings.vue'
import AgentSettings from './pages/AgentSettings.vue'
import ToolsSettings from './pages/ToolsSettings.vue'
import PluginsSettings from './pages/PluginsSettings.vue'
import SecuritySettings from './pages/SecuritySettings.vue'
import AuditSettings from './pages/AuditSettings.vue'
import AutomationSettings from './pages/AutomationSettings.vue'
import MemorySettings from './pages/MemorySettings.vue'
import MailSettings from './pages/MailSettings.vue'
import InstructionSettings from './pages/InstructionSettings.vue'
import UsageStatsSettings from './pages/UsageStatsSettings.vue'
import ConnectionSettings from './pages/ConnectionSettings.vue'
import MemoryView from '@/views/MemoryView.vue'
import { isWebShell } from '@/utils/platform'

export interface SettingDef {
  key: string
  titleKey: string
  icon: string
  group: SettingGroup
  component: Component
  /** 浏览器部署(web-bridge)下隐藏:桌面独有能力(如插件从本地目录安装/打开插件目录)在容器里不可用 */
  webHidden?: boolean
}

/**
 * 设置菜单分组(对应侧栏设置菜单的分组标题)。
 *
 * 四组按"配置维度"划分：
 * - `basic`：基础设置——注重参数设置（系统 / 模型 / 记忆设置 / 安全【含本地环境】 / 浏览器设置）
 * - `capability`：能力拓展（工具 / 技能 / 插件 / 智能体 / 全局约定）
 * - `data`：数据分析——注重数据的查看（记忆内容 / 审计日志）
 * - `other`：其他说明（关于）
 */
export type SettingGroup = 'basic' | 'capability' | 'data' | 'other'

/** 分组展示顺序与标题键 */
export const SETTING_GROUPS: { key: SettingGroup; titleKey: string }[] = [
  { key: 'basic', titleKey: 'settings.groupBasic' },
  { key: 'capability', titleKey: 'settings.groupCapability' },
  { key: 'data', titleKey: 'settings.groupData' },
  { key: 'other', titleKey: 'settings.groupOther' }
]

/**
 * 设置菜单注册表:侧栏设置菜单与右侧设置页面共用。
 * 新增设置项 = 新建页面组件 + 在此注册一行。
 */
export const SETTING_REGISTRY: SettingDef[] = [
  // 基础设置：注重参数设置
  { key: 'system', titleKey: 'settings.title', icon: 'icon-settings', group: 'basic', component: SystemSettings },
  { key: 'session', titleKey: 'settings.menuSession', icon: 'icon-message', group: 'basic', component: SessionSettings },
  { key: 'models', titleKey: 'settings.menuModels', icon: 'icon-box', group: 'basic', component: ModelSettings },
  { key: 'memory', titleKey: 'settings.menuMemory', icon: 'icon-memory', group: 'basic', component: MemorySettings },
  { key: 'security', titleKey: 'settings.menuSecurity', icon: 'icon-lock', group: 'basic', component: SecuritySettings },
  { key: 'mail', titleKey: 'settings.menuMail', icon: 'icon-mail', group: 'basic', component: MailSettings },
  { key: 'automation', titleKey: 'settings.menuAutomation', icon: 'icon-globe', group: 'basic', component: AutomationSettings },
  // 能力拓展：工具 / 技能 / 插件 / 智能体 / 全局约定
  { key: 'tools', titleKey: 'settings.menuTools', icon: 'icon-tool', group: 'capability', component: ToolsSettings },
  { key: 'skills', titleKey: 'settings.menuSkills', icon: 'icon-sparkles', group: 'capability', component: SkillsSettings },
  // 插件设置入口暂不对用户展示（菜单在 SettingsMenu 中按 key 过滤；页面/组件/i18n 全保留）
  { key: 'plugins', titleKey: 'settings.menuPlugins', icon: 'icon-puzzle', group: 'capability', component: PluginsSettings, webHidden: true },
  { key: 'agents', titleKey: 'settings.menuAgents', icon: 'icon-user-group', group: 'capability', component: AgentSettings },
  { key: 'instructions', titleKey: 'settings.menuInstructions', icon: 'icon-file-text', group: 'capability', component: InstructionSettings },
  // 数据分析：注重数据的查看
  { key: 'usage', titleKey: 'settings.menuUsage', icon: 'icon-bar-chart', group: 'data', component: UsageStatsSettings },
  { key: 'memoryContent', titleKey: 'settings.menuMemoryContent', icon: 'icon-memory', group: 'data', component: MemoryView },
  { key: 'audit', titleKey: 'settings.menuAudit', icon: 'icon-file-text', group: 'data', component: AuditSettings },
  // 其他说明
  { key: 'about', titleKey: 'settings.menuAbout', icon: 'icon-info', group: 'other', component: AboutSettings },
  // 连接方式（本机 / 云端）：壳子级设置，浏览器部署没有「本机」可选 → web 隐藏
  { key: 'connection', titleKey: 'settings.menuConnection', icon: 'icon-link', group: 'other', component: ConnectionSettings, webHidden: true }
]

/** 当前部署下可见的设置项(浏览器部署过滤掉桌面独有项:菜单与路由回退共用同一份口径) */
export function visibleSettingDefs(): SettingDef[] {
  return SETTING_REGISTRY.filter((d) => !(d.webHidden && isWebShell))
}

/** 该键在当前部署下被隐藏(浏览器部署的桌面独有项):直达访问时应回落地址栏 */
export function isHiddenSetting(key: string): boolean {
  return isWebShell && SETTING_REGISTRY.some((d) => d.webHidden && d.key === key)
}

export function getSettingDef(key: string): SettingDef {
  const visible = visibleSettingDefs()
  return visible.find((d) => d.key === key) ?? visible.find((d) => d.key === 'system')!
}
