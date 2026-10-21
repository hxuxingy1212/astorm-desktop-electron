<script setup lang="ts">
import { computed, watchEffect } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getSettingDef, isHiddenSetting } from './registry'

const route = useRoute()
const router = useRouter()

// 当前设置项由路由 /settings/:key 驱动
const routeKey = computed(() =>
  typeof route.params.key === 'string' && route.params.key ? route.params.key : 'system'
)
const def = computed(() => getSettingDef(routeKey.value))

// 直达被隐藏项(浏览器部署下的插件页)→ 地址栏回落到系统设置,避免侧栏「无高亮项」的悬空态
watchEffect(() => {
  if (isHiddenSetting(routeKey.value)) void router.replace(`/settings/${def.value.key}`)
})
</script>

<template>
  <div class="flex h-full min-h-0 min-w-0 flex-1 flex-col">
    <!-- 具体设置页面:内容通栏铺满,两侧留白由各页面自身的 padding 控制 -->
    <div class="settings-scroll min-h-0 min-w-0 flex-1 overflow-y-auto">
      <component :is="def.component" />
    </div>
  </div>
</template>

<style scoped>
/* 预占滚动条位置:内容增删导致滚动条出现/消失时,页面不左右晃动 */
.settings-scroll {
  scrollbar-gutter: stable;
  /* 设置页一律不横向溢出:个别宽内容(热力图等)自行内部滚动 */
  overflow-x: clip;
}
</style>
