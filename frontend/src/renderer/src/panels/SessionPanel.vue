<script setup lang="ts">
/**
 * 子会话页签面板：渲染某个子会话（后端 `sub_session` 创建）的**实时对话**。
 *
 * 直接内嵌对话页组件（`ChatPanel` 的内嵌形态），因此与对话页同款：
 * 流式正文 / 思考链 / 工具卡 / 轮次收纳 / 状态行 / 错误条都一致，
 * **审批卡与提问表单照常可用**（决议提交到子会话自身）。
 * 区别只有两点：没有输入框、没有右上角规划胶囊（见 ChatPanel 的 `embedded`）。
 *
 * 子会话的生命周期由后端持有：它跑完即结束，本面板只是它的一个视图；
 * 关闭页签不影响子会话运行（仍在后台跑，父会话继续等它）。
 */
import ChatPanel from '@/components/chat/ChatPanel.vue'

defineProps<{ sessionId: string }>()
</script>

<template>
  <div class="session-panel">
    <ChatPanel :session-id="sessionId" embedded />
  </div>
</template>

<style scoped>
.session-panel {
  height: 100%;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
</style>
