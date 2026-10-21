<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { visibleSettingDefs, SETTING_GROUPS } from './registry'

defineProps<{ collapsed: boolean }>()

const { t } = useI18n()
const route = useRoute()
const router = useRouter()

// 当前选中项由路由 /settings/:key 驱动
const activeKey = computed(() =>
  typeof route.params.key === 'string' && route.params.key ? route.params.key : 'system'
)

// 插件暂不展示设置菜单入口；注册表与页面保留。
const menuDefs = computed(() => visibleSettingDefs().filter((d) => d.key !== 'plugins'))
const defMap = computed(() => new Map(menuDefs.value.map((d) => [d.key, d])))

// 按分组聚合菜单项(保持注册表内相对顺序);hidden / webHidden 项已在 menuDefs 中过滤
const grouped = computed(() =>
  SETTING_GROUPS.map((g) => ({
    ...g,
    items: menuDefs.value.filter((d) => d.group === g.key)
  }))
)
</script>

<template>
  <div class="settings-nav">
    <template v-for="group in grouped" :key="group.key">
      <!-- 分组标题:浅灰小字,无边框 -->
      <div v-if="group.items.length" class="settings-nav-group">
        <span v-if="!collapsed" class="settings-nav-group-title">{{ t(group.titleKey) }}</span>
        <a-tooltip
          v-for="key in group.items"
          :key="key.key"
          :content="t(defMap.get(key.key)!.titleKey)"
          position="right"
          :disabled="!collapsed"
        >
          <button
            class="settings-nav-btn"
            :class="[activeKey === key.key && 'active', collapsed && 'collapsed']"
            @click="router.push(`/settings/${key.key}`)"
          >
            <component :is="defMap.get(key.key)!.icon" :size="18" />
            <span v-if="!collapsed" class="btn-label">{{ t(defMap.get(key.key)!.titleKey) }}</span>
          </button>
        </a-tooltip>
      </div>
    </template>
  </div>
</template>

<style scoped>
.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 8px 8px;
}

.settings-nav-group {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* 分组标题:浅灰小字,参考截图 */
.settings-nav-group-title {
  padding: 0 8px;
  margin-bottom: 2px;
  font-size: var(--font-size-xs);
  line-height: var(--line-height-ui);
  color: var(--text-tertiary);
  user-select: none;
}

.settings-nav-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: var(--nav-item-height);
  /* 菜单项内容相对分组标题再留一段左空隙（参考侧栏任务行状态位造成的缩进） */
  padding: 0 8px 0 14px;
  border: none;
  background: transparent;
  border-radius: var(--radius-md);
  color: var(--text-primary);
  cursor: pointer;
  font-size: var(--font-size-md);
  line-height: var(--line-height-ui);
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.settings-nav-btn:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

.settings-nav-btn.active {
  /* 只保留背景高亮,字体保持原色(--primary 为失效变量,会导致暗色下变黑) */
  background-color: var(--bg-active);
}

.settings-nav-btn.collapsed {
  justify-content: center;
  padding: 0;
}

.btn-label {
  font-size: var(--font-size-md);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
