<script setup lang="ts">
/**
 * 思考链卡片（主色标题 + 灰底正文，默认收起），供「会话轨迹」页签使用
 * （对话区的思考已统一走过程收纳块的新渲染，不再使用本组件）。
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Markdown } from 'vue-stream-markdown'
import { markdownNodeRenderers } from '@/components/markdown/renderers'

const props = withDefaults(
  defineProps<{
    content: string
    /** 流式输出中：标题显示 Thinking…，正文按 streaming 渲染 */
    streaming?: boolean
    isDark?: boolean
  }>(),
  { streaming: false, isDark: false }
)

const { t } = useI18n()


// 默认收起（折叠时不挂载正文的 markdown）
const manual = ref<boolean | null>(null)
const open = computed(() => manual.value ?? false)
</script>

<template>
  <div class="think-card">
    <button type="button" class="think-header" @click="manual = !open">
      <!-- 全局统一思考图标(与对话区过程行同款 icon-think) -->
      <icon-think :size="14" class="think-icon" />
      <span class="think-title">{{
        props.streaming ? `${t('chat.step.think')}…` : t('chat.step.think')
      }}</span>
      <span class="think-arrow">{{ open ? '▼' : '▶' }}</span>
    </button>
    <div v-if="open" class="think-body">
      <Markdown
        :content="props.content"
        :is-dark="props.isDark"
        :mode="props.streaming ? 'streaming' : 'static'"
        :enable-animate="false"
        :node-renderers="markdownNodeRenderers"
      />
    </div>
  </div>
</template>

<style scoped>
/* 思考链(think):蓝色标题 + 灰底正文,**默认收起** */
.think-card {
  margin: 2px 0 8px;
}

.think-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 0;
  border: none;
  background: transparent;
  color: var(--accent-text);
  font-size: var(--font-size-sm);
  font-weight: 500;
  cursor: pointer;
  user-select: none;
}

.think-icon {
  flex-shrink: 0;
}

.think-title {
  line-height: 1;
}

.think-arrow {
  margin-left: auto;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.think-body {
  margin-top: 4px;
  padding: 8px 12px;
  border-radius: var(--radius-sm);
  background: var(--bg-1);
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}
</style>
