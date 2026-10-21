<script setup lang="ts">
/**
 * 「相关记忆」页签：列出本轮的**记忆检索**（`memory` action=search）结果卡片。
 * 点击卡片 → 记忆文档预览页签（MemoryConceptPanel）。
 */
import { useI18n } from 'vue-i18n'
import type { MemorySource } from '@/types'
import { useTabsStore } from '@/stores/tabs'

const props = defineProps<{ items: MemorySource[] }>()

const { t } = useI18n()
const tabs = useTabsStore()

function openCard(item: MemorySource): void {
  tabs.openMemoryConceptTab(item.conceptId, item.title)
}
</script>

<template>
  <div class="mr-root">
    <div v-if="!props.items.length" class="mr-empty">{{ t('panel.memoryResults.empty') }}</div>
    <div v-else class="mr-list">
      <button
        v-for="(item, i) in props.items"
        :key="item.conceptId"
        type="button"
        class="mr-card"
        :title="item.conceptId"
        @click="openCard(item)"
      >
        <div class="mr-head">
          <span class="mr-source">
            <icon-memory :size="15" />
            <span class="mr-source-text">
              {{ [t('panel.memoryConcept.title'), item.observedAt].filter(Boolean).join(' · ') }}
            </span>
          </span>
          <span class="mr-index">{{ i + 1 }}</span>
        </div>
        <div class="mr-title">{{ item.title }}</div>
        <div v-if="item.snippet" class="mr-snippet">{{ item.snippet }}</div>
      </button>
    </div>
  </div>
</template>

<style scoped>
.mr-root {
  height: 100%;
  overflow-y: auto;
  padding: 5px 4px 16px;
}
.mr-empty {
  padding: 24px 12px;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}
.mr-list {
  display: flex;
  flex-direction: column;
}
.mr-card {
  display: block;
  width: 100%;
  padding: 12px 10px;
  border: none;
  border-bottom: 1px solid var(--border-light);
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color 0.15s;
}
.mr-card:hover {
  background: var(--bg-2);
}
.mr-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.mr-source {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}
.mr-source-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mr-index {
  flex: none;
  margin-left: auto;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: var(--bg-3);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
  line-height: 20px;
  text-align: center;
}
.mr-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  line-height: 1.45;
  color: var(--text-primary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.mr-snippet {
  margin-top: 5px;
  font-size: var(--font-size-code);
  line-height: 1.5;
  color: var(--text-tertiary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
</style>
