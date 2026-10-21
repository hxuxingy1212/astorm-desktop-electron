<script setup lang="ts">
/**
 * 面板内容容器：只渲染**激活页签的内容**（文件预览 / 浏览器 / 工具 / 子会话…）
 * 与「新建页签」空态落地页。
 *
 * 页签栏（▾ / 页签列表 / + / 🗑）已上提到应用顶栏（`PanelTabBar.vue`），
 * 本组件不再包含页签切换 UI —— 面板内部只有内容，视觉更干净。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useTabsStore } from '@/stores/tabs'
import { useSessionsStore } from '@/stores/sessions'
import { getPanelDef, PANEL_REGISTRY } from '@/panels'
import FilePreviewPanel from '@/panels/FilePreviewPanel.vue'
import BrowserPanel from '@/panels/BrowserPanel.vue'
import TerminalPanel from '@/panels/TerminalPanel.vue'
import PluginSitePanel from '@/panels/PluginSitePanel.vue'
import SessionPanel from '@/panels/SessionPanel.vue'
import PendingSubSessionPanel from '@/panels/PendingSubSessionPanel.vue'
import SourceResultsPanel from '@/panels/SourceResultsPanel.vue'
import MemoryResultsPanel from '@/panels/MemoryResultsPanel.vue'
import MemoryConceptPanel from '@/panels/MemoryConceptPanel.vue'
import MemoryRawPanel from '@/panels/MemoryRawPanel.vue'

const { t } = useI18n()
const tabs = useTabsStore()
const sessions = useSessionsStore()

/** 激活文件页签 → FilePreviewPanel;网页页签 → BrowserPanel;插件页签 → PluginSitePanel;工具页签 → 面板组件 */
const activePanel = computed(() => {
  const tab = tabs.activeTab
  if (!tab) return null
  // 未开始的子智能体委派：只展示任务文本（用户消息），子会话创建后原地升级为实时会话页签
  if (tab.session?.pendingTask) {
    return { type: 'pending' as const, id: tab.id, task: tab.session.pendingTask }
  }
  // 子会话页签：渲染该会话的实时对话（无输入框 / 无规划胶囊，审批与表单照常）
  if (tab.session) return { type: 'session' as const, id: tab.id, sessionId: tab.session.sessionId }
  if (tab.file) return { type: 'file' as const, id: tab.id }
  if (tab.web) return { type: 'web' as const, id: tab.id }
  if (tab.sources) return { type: 'sources' as const, id: tab.id, items: tab.sources.items }
  if (tab.memoryResults)
    return { type: 'memoryResults' as const, id: tab.id, items: tab.memoryResults.items }
  if (tab.memory) return { type: 'memory' as const, id: tab.id, conceptId: tab.memory.conceptId }
  if (tab.memoryRaw) return { type: 'memoryRaw' as const, id: tab.id, stem: tab.memoryRaw.stem }
  if (tab.pluginId) return { type: 'plugin' as const, pluginId: tab.pluginId }
  const def = tab.toolId ? getPanelDef(tab.toolId) : null
  return def ? { type: 'tool' as const, component: def.component, toolId: tab.toolId } : null
})

/** 当前激活的**工具**页签 id(非工具页签为空) */
const activeToolId = computed(() =>
  activePanel.value?.type === 'tool' ? activePanel.value.toolId : ''
)
/**
 * 所有「打开过终端」的会话 id：当前会话用 live 页签，其它会话用内存快照。
 * 终端要**按会话常驻挂载**(非激活只隐藏)：
 * xterm 缓冲与后端 PTY 都是运行期状态，一旦卸载/切走就丢历史、重开 shell，切换瞬间还会闪空。
 * 因此每个这类会话各渲染一个 TerminalPanel 常驻实例，切换会话/页签只显隐、不重建。
 */
const terminalSessionIds = computed(() => {
  const ids = new Set<string>()
  if (tabs.currentSessionId && tabs.tabs.some((tab) => tab.toolId === 'terminal')) {
    ids.add(tabs.currentSessionId)
  }
  for (const [sid, snap] of Object.entries(tabs.perSession)) {
    if (snap.tabs.some((tab) => tab.toolId === 'terminal')) ids.add(sid)
  }
  return [...ids]
})
</script>

<template>
  <div class="flex h-full min-w-0 flex-col">
    <template v-if="tabs.tabs.length > 0">
      <!-- 面板内容:文件预览 / 插件 / 工具页签为**单实例**(激活才渲染);
            浏览器页签**常驻挂载**(非激活只隐藏),使每个页签的 iframe/OOPIF 存活,
            供「一会话一浏览器实例」同时管理多个页签(AI 可直接驱动任意页签)。 -->
      <div class="relative min-h-0 flex-1 overflow-auto">
        <BrowserPanel
          v-for="tab in tabs.browserTabs"
          v-show="tab.id === tabs.activeId"
          :key="tab.id"
          :tab-id="tab.id"
          class="absolute inset-0"
        />
        <!-- 终端:按会话常驻挂载（非激活会话/页签只隐藏），保留各自的 xterm 缓冲与 PTY；
             切换会话或页签都不重建，历史保留、不闪空 -->
        <TerminalPanel
          v-for="sid in terminalSessionIds"
          :key="sid"
          v-show="sid === sessions.activeId && activeToolId === 'terminal'"
          :session-id="sid"
          class="absolute inset-0"
        />
        <SessionPanel
          v-if="activePanel?.type === 'session'"
          :session-id="activePanel.sessionId"
        />
        <PendingSubSessionPanel
          v-else-if="activePanel?.type === 'pending'"
          :task="activePanel.task"
        />
        <SourceResultsPanel
          v-else-if="activePanel?.type === 'sources'"
          :items="activePanel.items"
        />
        <MemoryResultsPanel
          v-else-if="activePanel?.type === 'memoryResults'"
          :items="activePanel.items"
        />
        <MemoryConceptPanel
          v-else-if="activePanel?.type === 'memory'"
          :concept-id="activePanel.conceptId"
        />
        <MemoryRawPanel
          v-else-if="activePanel?.type === 'memoryRaw'"
          :stem="activePanel.stem"
        />
        <FilePreviewPanel v-else-if="activePanel?.type === 'file'" :tab-id="activePanel.id" />
        <PluginSitePanel v-else-if="activePanel?.type === 'plugin'" :plugin-id="activePanel.pluginId" />
        <!-- 其它工具页签:激活才渲染（终端已在上方常驻挂载，这里排除避免重复实例） -->
        <component
          v-else-if="activePanel?.type === 'tool' && activeToolId !== 'terminal'"
          :is="activePanel.component"
        />
      </div>
    </template>

      <!-- 无页签:新建页签落地页,展示可选择的工具;网格随面板宽度自适应(auto-fit),宽面板铺满 -->
      <div v-else class="tab-new flex h-full flex-col items-center justify-center gap-5 overflow-auto p-8">
        <div class="flex items-center gap-1.5 text-sm font-semibold text-[var(--text-primary)]">
          <icon-plus :size="16" class="tab-new-icon" />
          {{ t('panel.newTab') }}
        </div>
        <div class="grid w-full grid-cols-[repeat(auto-fit,minmax(160px,1fr))] gap-3">
        <button
          v-for="def in PANEL_REGISTRY"
          :key="def.toolId"
          type="button"
          class="tab-new-card"
          @click="tabs.openTab(def.toolId)"
        >
          <component :is="def.icon" :size="22" class="tab-new-icon shrink-0" />
          <span class="text-xs font-medium text-[var(--text-primary)]">{{ t(def.titleKey) }}</span>
          <span class="text-[11px] leading-snug text-[var(--text-tertiary)]">{{ t(def.descKey) }}</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 新建页签落地页卡片:无边框（按需求去掉，改以浅填充区分层次） */
.tab-new-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-height: 96px;
  padding: 12px 8px;
  border: none;
  border-radius: 10px;
  background-color: var(--bg-2);
  cursor: pointer;
  transition: background-color 0.15s ease;
}

.tab-new-card:hover {
  background-color: var(--bg-3);
}

/* 落地页图标:正常文字色(暗色模式为白色),不跟随主题色 */
.tab-new-icon {
  color: var(--text-primary);
}
</style>
