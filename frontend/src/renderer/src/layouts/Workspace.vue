<script setup lang="ts">
import { computed, watch } from 'vue'
import ChatPanel from '@/components/chat/ChatPanel.vue'
import PanelContainer from '@/components/panels/PanelContainer.vue'
import PanelSplitter from '@/components/panels/PanelSplitter.vue'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore, PANEL_COMPACT_WIDTH } from '@/stores/settings'
import { usePanelLayout } from '@/composables/usePanelLayout'
import { useFilesStore } from '@/stores/files'

const tabs = useTabsStore()
const settings = useSettingsStore()

// 面板布局数值（与 StatusBar 共用同一套，保证顶栏分割线与正文严格对齐）
const { tabsOpen, panelWidth, panelMin, panelMax, widthMode } = usePanelLayout()

// 面板最大化:右侧面板占满内容区,隐藏对话区与分隔线(侧栏由 AppLayout 隐藏)。
// 仅在面板可见时生效,避免「最大化却无内容」。
const panelMaximized = computed(() => settings.panelMaximized && tabsOpen.value)

// 页签面板打开 / **激活页签(或其宽度档位)变化** 时,按两档标准重设**默认宽度**:
// 窄档(资料/记忆/终端/普通文件等,见 panels::widthModeOfTab) → 标准窄宽 480;
// 宽档(仅浏览器与 office/PDF 预览) → 拉满到可用上限。此后拖拽完全自由(最小↔最大)。
watch(
  () => (tabsOpen.value ? `${tabs.activeId}|${widthMode.value}` : ''),
  (key) => {
    if (!tabsOpen.value || !key) return
    const files = useFilesStore()
    if (!files.active) useSettingsStore().setSidebarCollapsed(true)
    // 等侧栏收缩/宽度重算(下一帧)后再取上限，避免用收缩前的旧宽
    requestAnimationFrame(() => {
      const target =
        widthMode.value === 'narrow'
          ? Math.min(PANEL_COMPACT_WIDTH, panelMax.value)
          : panelMax.value
      if (target) settings.setPanelWidth(target)
    })
  },
  { immediate: true }
)
</script>

<template>
  <div class="flex min-h-0 flex-1 overflow-hidden">
    <!-- 对话框:常态占满,页签打开时收缩;**面板最大化时隐藏**(让面板独占内容区) -->
    <ChatPanel v-if="!panelMaximized" class="flex-1" />

    <!-- 拖拽分栏(页签区三态:次要/平分/主要,范围由 panelMin/panelMax 动态钳制)。
         最大化时隐藏(面板已占满,无可拖) -->
    <PanelSplitter v-if="tabsOpen && !panelMaximized" :min="panelMin" :max="panelMax" />

    <!-- 多页签面板:页签数为 0 时宽度为 0 并隐藏(分隔线由 Splitter 统一提供)。
         打开时 min-w-[116px] 兜底页签栏控件不被裁;关闭时 min-w-0,让对话区占满右侧。
         **最大化时宽度 100%(占满内容区)** -->
    <div
      class="h-full shrink overflow-hidden transition-[width,opacity] duration-500"
      :class="
        panelMaximized
          ? 'opacity-100 w-full min-w-0'
          : tabsOpen
            ? 'opacity-100 min-w-[116px]'
            : 'opacity-0 min-w-0'
      "
      :style="{ width: panelMaximized ? '100%' : tabsOpen ? `${panelWidth}px` : '0px' }"
    >
      <PanelContainer class="h-full" />
    </div>
  </div>
</template>
