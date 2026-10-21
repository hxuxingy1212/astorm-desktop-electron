<script lang="ts">
/** 引用卡片数据(正文摘录 + 来源说明) */
export interface QuoteCard {
  /** 引用正文(卡片里显示,长文本由 CSS 截断) */
  text: string
  /** 来源说明(如「助手消息 · #430」) */
  meta: string
}
</script>

<script setup lang="ts">
import { useI18n } from 'vue-i18n'

/**
 * 对话引用胶囊(发送框上方 / 消息卡片共用):图标 + 「N 条对话引用」+ 悬停弹出引用卡片。
 * 纯展示:卡片数据由调用方给出(发送框来自 quotePick store;消息侧可另行重建)。
 */
const props = withDefaults(
  defineProps<{
    cards: QuoteCard[]
    /** 显示移除按钮(发送框用;消息侧不需要) */
    removable?: boolean
  }>(),
  { removable: false }
)

const emit = defineEmits<{ (e: 'remove'): void }>()

const { t } = useI18n()
</script>

<template>
  <span v-if="props.cards.length" class="quote-attach">
    <a-popover
      :trigger="['hover', 'click']"
      position="tl"
      content-class="quote-popover"
      :arrow-style="{ display: 'none' }"
    >
      <span class="quote-pill-body">
        <svg
          class="quote-pill-icon"
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path
            d="M3 21c3 0 7-1 7-8V5c0-1.25-.756-2.017-2-2H4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2 1 0 1 0 1 1v1c0 1-1 2-2 2s-1 .008-1 1.031V20c0 1 0 1 1 1z"
          />
          <path
            d="M15 21c3 0 7-1 7-8V5c0-1.25-.757-2.017-2-2h-4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2h.75c0 2.25.25 4-2.75 4v3c0 1 0 1 1 1z"
          />
        </svg>
        <span class="quote-pill-text">{{ t('chat.quoteCount', { count: props.cards.length }) }}</span>
      </span>
      <template #content>
        <div class="quote-cards">
          <div
            v-for="(card, ci) in props.cards"
            :key="ci"
            class="quote-card"
            :class="ci > 0 && 'quote-card-next'"
          >
            <div class="quote-card-text">{{ card.text }}</div>
            <div class="quote-card-meta">{{ card.meta }}</div>
          </div>
        </div>
      </template>
    </a-popover>

    <button
      v-if="removable"
      type="button"
      class="quote-remove"
      :title="t('chat.quoteRemove')"
      @click="emit('remove')"
    >
      <icon-close :size="12" />
    </button>
  </span>
</template>

<style scoped>
/* 胶囊:浅色圆角 + 细边框,与附件 / 技能 / 网页元素胶囊同高(32px) */
.quote-attach {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  height: 32px;
  padding-right: 3px;
  border: 0.5px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-1);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  max-width: 100%;
  vertical-align: middle;
}

.quote-pill-body {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 100%;
  padding: 0 6px 0 10px;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  cursor: pointer;
}

.quote-pill-icon {
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.quote-pill-text {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.quote-remove {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  padding: 0;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition:
    background-color 0.15s,
    color 0.15s;
}

.quote-remove:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}
</style>

<style>
/* 弹层渲染在 body 层(Arco portal),需非 scoped 命中。
   Arco 是两层(content-class 包 .arco-popover-content),抹掉外层壳,只留单层卡片 */
.quote-popover.quote-popover {
  padding: 0;
  border: none;
  border-radius: 0;
  background: transparent;
  box-shadow: none;
}

.quote-popover .arco-popover-content {
  padding: 0;
  border-radius: 10px;
  overflow: hidden;
  background: var(--bg-1);
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.08),
    0 10px 28px rgba(0, 0, 0, 0.16);
}

.quote-popover .quote-cards {
  max-height: min(360px, 55vh);
  overflow-y: auto;
  padding: 10px 12px;
  scrollbar-gutter: stable;
}

.quote-popover .quote-card {
  min-width: 240px;
  max-width: 420px;
}

.quote-popover .quote-card-next {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--border-light);
}

.quote-popover .quote-card-text {
  font-size: var(--font-size-xs);
  line-height: 18px;
  color: var(--text-primary);
  /* 长文本截断到约 6 行 */
  display: -webkit-box;
  -webkit-line-clamp: 6;
  -webkit-box-orient: vertical;
  overflow: hidden;
  white-space: pre-wrap;
  word-break: break-word;
}

.quote-popover .quote-card-meta {
  margin-top: 6px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
</style>
