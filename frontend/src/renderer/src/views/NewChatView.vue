<script setup lang="ts">
/**
 * 新建会话页：只做页面外壳（欢迎标题 + 居中布局 + 背景），表单本身用
 * `components/chat/NewSessionForm.vue`。两个入口都落到这里：
 * - 普通新建（侧栏「新建对话」）：发送 = 立即执行；
 * - 看板「新建待办」（`?todo=1`）：发送 = 添加待办（状态由入口决定，表单不显示切换）。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import NewSessionForm from '@/components/chat/NewSessionForm.vue'

const { t } = useI18n()
const route = useRoute()

/**
 * 侧栏项目节点「添加任务」跳转时携带目录 → 预填给表单。
 *
 * 只带目录：分支不进会话记录，表单里的分支是对所选目录的**实时探测**（只读展示），
 * 系统提示词每个用户消息自己取工作区当前分支。
 */
const initialDirectory = computed(() =>
  typeof route.query.directory === 'string' ? route.query.directory : ''
)

/** 看板「新建待办」跳转（`?todo=1`）：提交档位默认「添加待办」 */
const defaultExecute = computed(() => route.query.todo !== '1')
</script>

<template>
  <!-- w-full:撑满内容区(flex 行内 router-view 不自动伸展),否则右侧页签工具栏会被挤到中间 -->
  <div
    class="flex h-full w-full flex-col items-center overflow-hidden px-10"
    style="background-color: var(--panel-bg)"
  >
    <!-- 欢迎标题 -->
    <div class="flex flex-col items-center gap-1 pb-12 pt-[98px]">
      <h1 class="text-2xl font-bold" style="color: var(--text-primary)">{{ t('newChat.title') }}</h1>
      <p class="text-base" style="color: var(--text-tertiary)">{{ t('newChat.subtitle') }}</p>
    </div>

    <!-- 表单:文件+Git 栏在胶囊内部顶部,胶囊铺满容器宽度,边框保持一致 -->
    <div class="w-full max-w-3xl">
      <NewSessionForm
        :initial-directory="initialDirectory"
        :default-execute="defaultExecute"
        navigate-on-create
      />
    </div>
  </div>
</template>
