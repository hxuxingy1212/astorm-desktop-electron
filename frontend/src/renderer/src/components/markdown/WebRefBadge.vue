<script setup lang="ts">
/**
 * 正文里的**网页引用角标**：小圆角编号（悬浮显示域名，点击在**应用内浏览器页签**打开来源）。
 *
 * 由 `LinkRenderer` 在「链接文字是编号 + 目标是 http(s)」时选用（`[1](url)` / `[[1]](url)` 都认），
 * 编号即链接文字，与正文末尾的来源清单一一对应。点击**不再弹外链确认框**：它只开站内浏览器页签
 * （不交给系统外部程序），比确认框里的「外部打开」更安全，且来源核对就该一键直达。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useTabsStore } from '@/stores/tabs'

const props = defineProps<{
  /** 角标编号（链接文字） */
  index: string
  /** 来源地址（已过 harden 校验的 http(s) URL） */
  url: string
}>()

const { t } = useI18n()
const tabs = useTabsStore()

/** 悬浮提示：域名 + 完整地址（域名在前，一眼看出是哪家来源） */
const tip = computed(() => {
  let host = props.url
  try {
    host = new URL(props.url).host
  } catch {
    /* 解析不了就原样显示 */
  }
  return `${t('chat.webRef.source', { host })}\n${props.url}`
})

function open(): void {
  const res = tabs.applyTabCommand({ action: 'open', kind: 'browser', url: props.url, new_tab: true })
  if (!res.ok) window.open(props.url, '_blank')
}
</script>

<template>
  <a class="wr-badge" :href="url" :title="tip" @click.prevent="open">{{ index }}</a>
</template>

<style scoped>
/* 网页来源编号：紧凑的中性圆角标签，与正文基线对齐，不抬高行框。 */
.wr-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  min-width: 20px;
  height: 20px;
  margin: 0 2px;
  padding: 0 5px;
  vertical-align: baseline;
  border: none;
  border-radius: var(--radius-full);
  background: var(--fill-2);
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  font-weight: var(--font-weight-regular);
  line-height: var(--line-height-ui);
  text-decoration: none;
  white-space: nowrap;
  cursor: pointer;
  transition: background-color var(--transition-fast), color var(--transition-fast);
}

.wr-badge:hover,
.wr-badge:focus-visible {
  background: var(--fill-3);
  color: var(--text-primary);
  text-decoration: none;
}

.wr-badge:focus-visible {
  outline: 2px solid var(--accent-text);
  outline-offset: 1px;
}

@media (prefers-reduced-motion: reduce) {
  .wr-badge {
    transition: none;
  }
}
</style>
