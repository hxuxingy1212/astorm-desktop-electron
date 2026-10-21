<script setup lang="ts">
import AppSidebar from './AppSidebar.vue'
import { hasSidebarGlass } from '@/utils/platform'
import StatusBar from './StatusBar.vue'
import SearchModal from '@/components/search/SearchModal.vue'
import { computed } from 'vue'
import { useSidebarCollapsed } from '@/composables/useSidebarCollapsed'
import { useResizeDrag } from '@/composables/useResizeDrag'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH } from '@/stores/settings'

// 侧栏折叠:启动默认展开(历史折叠记忆在 loadPersistedSettings 丢弃),运行期只认显式值
const sidebarCollapsed = useSidebarCollapsed()
const settings = useSettingsStore()
const tabs = useTabsStore()

// 面板最大化(右侧面板占满内容区):隐藏左侧栏,复现图3「只留页签 + 内容」的全屏观感。
// 与侧栏折叠同路由:折叠为 0 宽;还原时回到用户原偏好(不改写 sidebarCollapsed)。
const panelMaximized = computed(() => settings.panelMaximized && !tabs.collapsed)
const sidebarHidden = computed(() => sidebarCollapsed.value || panelMaximized.value)

// 侧栏宽度拖拽手柄:锚点位于中心区左缘(空隙区域,见 .center-resize-handle)。
// 用 settings.sidebarWidth 读写,与 AppSidebar 折叠态一致。
const { onPointerDown, onPointerMove, onPointerUp } = useResizeDrag({
  min: SIDEBAR_MIN_WIDTH,
  max: SIDEBAR_MAX_WIDTH,
  get: () => settings.sidebarWidth,
  set: (w) => settings.setSidebarWidth(w)
})
</script>

<template>
  <!-- 本机玻璃窗口底层保持透明，右侧内容保持不透明。 -->
  <div class="app-shell relative flex h-full w-full overflow-hidden bg-[var(--bg-1)]" :class="{ 'app-shell-glass': hasSidebarGlass }">
    <AppSidebar :collapsed="sidebarHidden" />

    <!-- 侧栏宽度拖拽手柄:锚点位于侧栏与中心区交界处,不随侧栏 overflow 裁剪;
         面板最大化时侧栏隐藏,手柄一并隐藏 -->
    <div
      v-if="!sidebarHidden"
      class="center-resize-handle"
      :style="{ '--sidebar-actual': `${settings.sidebarWidth}px` }"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
    />

    <!-- 右侧整列:状态栏(工具栏)+ 路由视图(会话/新建/设置)。
         平铺面板，与侧栏无缝相接 -->
    <div
      class="app-right-card flex min-w-0 flex-1 flex-col overflow-hidden bg-[var(--panel-bg)]"
    >
      <StatusBar />
      <!-- 内容区:路由视图。KeepAlive 保活聊天视图:设置往返不卸载/不重渲染,消息与滚动位置保留 -->
      <div class="flex min-h-0 flex-1">
        <router-view v-slot="{ Component }">
          <KeepAlive>
            <component :is="Component" />
          </KeepAlive>
        </router-view>
      </div>
    </div>

    <SearchModal />
  </div>
</template>

<style scoped>
.app-shell-glass {
  background-color: transparent;
}

@media (prefers-reduced-transparency: reduce) {
  .app-shell-glass {
    background-color: var(--bg-1);
  }
}

/* 透明拖拽热区跨过交界处，无可见分隔条。 */
.center-resize-handle {
  position: absolute;
  left: calc(var(--sidebar-actual, 0px) - 6px);
  top: 0;
  bottom: 0;
  width: 12px;
  cursor: col-resize;
  z-index: 20;
}

</style>
