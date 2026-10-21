<script setup lang="ts">
/**
 * 「未开始的子智能体委派」页签：子会话尚未创建，只展示本次委派的任务（用户消息），
 * 无输入框、无规划胶囊。子会话创建后由 tabs store 原地升级为实时会话页签。
 */
import MentionText from '@/components/mentions/MentionText.vue'

defineProps<{ task: string }>()
</script>

<template>
  <div class="pending-subs">
    <div class="pending-bubble">
      <MentionText :text="task" />
    </div>
  </div>
</template>

<style scoped>
.pending-subs {
  display: flex;
  height: 100%;
  flex-direction: column;
  overflow-y: auto;
  padding: 5px 16px 16px;
}

/* 用户消息气泡（与对话区用户消息同款：右对齐、浅灰底、右下角小圆角） */
.pending-bubble {
  align-self: flex-end;
  max-width: 80%;
  padding: 10px 14px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  border-bottom-right-radius: 4px;
  background-color: var(--bg-2);
  color: var(--text-primary);
  font-size: var(--font-size-md);
  line-height: 1.6;
  word-break: break-word;
  white-space: pre-wrap;
}
</style>
