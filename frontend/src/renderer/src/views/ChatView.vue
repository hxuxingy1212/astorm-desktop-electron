<script setup lang="ts">
import { watch } from 'vue'
import { useRoute } from 'vue-router'
import { useSessionsStore } from '@/stores/sessions'
import Workspace from '@/layouts/Workspace.vue'

// 供 AppLayout 的 KeepAlive include 匹配(设置往返时保住聊天 DOM 与滚动位置)
defineOptions({ name: 'ChatView' })

const route = useRoute()
const sessions = useSessionsStore()

// 进入 /chat/:id 时切换当前会话(侧栏点击会话也会跳转到这里)
watch(
  () => route.params.id,
  (id) => {
    if (typeof id === 'string' && id && sessions.sessions.some((s) => s.id === id)) {
      sessions.setActive(id)
    }
  },
  { immediate: true }
)
</script>

<template>
  <Workspace />
</template>
