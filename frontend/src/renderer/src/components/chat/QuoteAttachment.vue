<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useQuotePickStore } from '@/stores/quotePick'
import QuotePill, { type QuoteCard } from './QuotePill.vue'

/**
 * 发送框上方的「对话引用」胶囊:图标 + N 条对话引用 + 移除,悬停弹出引用卡片
 * (正文摘录 + 来源消息)。数据来自 quotePick store(对话正文「添加到当前任务」推入)。
 */
const quotePick = useQuotePickStore()
const { t } = useI18n()

const cards = computed<QuoteCard[]>(() =>
  quotePick.items.map((it) => ({
    text: it.text,
    meta:
      it.role === 'user'
        ? `${t('chat.quoteFromUser')}${it.seq != null ? ` · #${it.seq}` : ''}`
        : `${t('chat.quoteFromAssistant')}${it.seq != null ? ` · #${it.seq}` : ''}`
  }))
)
</script>

<template>
  <!-- 外层用真实元素(而非直接把 v-if 的组件当根):与 ElementAttachment 一致,
       避免「根为带 v-if 的组件」在增删瞬间触发 Vue 补丁崩溃(getNextHostNode null subTree) -->
  <div v-if="quotePick.count" class="quote-attach-wrap">
    <QuotePill :cards="cards" removable @remove="quotePick.remove()" />
  </div>
</template>

<style scoped>
.quote-attach-wrap {
  max-width: 100%;
}
</style>
