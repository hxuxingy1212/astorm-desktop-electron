<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { PickedElementCard } from '@/types'

/**
 * 网页元素胶囊(发送框上方 / 消息卡片共用):图标 + 「N 个网页元素」+ 悬停弹出卡片。
 * 悬停/点击都触发弹层(消息侧无移除按钮时也可点开查看)。
 * 纯展示:卡片数据由调用方给出(发送框来自 elementPick store;消息侧由 ```element 围栏重建)。
 */
const props = withDefaults(
  defineProps<{
    cards: PickedElementCard[]
    /** 显示移除按钮(发送框用;消息侧不需要) */
    removable?: boolean
    /** 计数文案里的数量(缺省 = cards.length) */
    count?: number
    /** 计数文案整体覆写(缺省 = 浏览器「N 个网页元素」;office 胶囊传自己的) */
    countLabel?: string
    /** 移除按钮 title 覆写(缺省 = 浏览器文案) */
    removeTitle?: string
  }>(),
  { removable: false, count: undefined }
)

const emit = defineEmits<{ (e: 'remove'): void }>()

const { t } = useI18n()

const countText = computed(() => props.count ?? props.cards.length)

function labelOf(key: string): string {
  // inspect.* 的键与 cardModel 的 row.key 一一对应；消息侧降级卡片用 elementBlock.* 的 key，
  // 故两套都试，都缺则回退 key 本身。
  const inspect = `panel.browser.inspect.${key}`
  const ti = t(inspect)
  if (ti !== inspect) return ti
  const block = `panel.browser.elementBlock.${key}`
  const tb = t(block)
  if (tb !== block) return tb
  return key
}
</script>

<template>
  <span v-if="cards.length" class="element-attach">
    <a-popover
      :trigger="['hover', 'click']"
      position="tl"
      content-class="element-popover"
      :arrow-style="{ display: 'none' }"
    >
      <span class="element-pill-body">
        <icon-pick-element :size="14" />
        <span class="element-pill-text">
          {{ props.countLabel ?? t('panel.browser.elementCount', { count: countText }) }}
        </span>
      </span>
      <template #content>
        <div class="element-cards">
          <div v-for="(card, ci) in cards" :key="ci" class="element-card" :class="ci > 0 && 'element-card-next'">
            <div class="element-card-head">
              <span class="element-card-name">{{ card.name }}</span>
              <span class="element-card-size">{{ card.size }}</span>
            </div>
            <template v-for="(row, ri) in card.rows" :key="ri">
              <div v-if="row.sep" class="element-card-sep" />
              <div class="element-card-row">
                <span class="element-card-label">{{ labelOf(row.key) }}</span>
                <span class="element-card-value" :class="row.path && 'is-path'">
                  <i v-if="row.swatch" class="element-swatch" :style="{ background: row.swatch }" />
                  <span class="element-card-text">{{ row.value }}</span>
                </span>
              </div>
            </template>
          </div>
        </div>
      </template>
    </a-popover>

    <button
      v-if="removable"
      type="button"
      class="element-remove"
      :title="props.removeTitle ?? t('panel.browser.elementRemove')"
      @click="emit('remove')"
    >
      <icon-close :size="12" />
    </button>
  </span>
</template>

<style scoped>
/* 胶囊:浅色圆角 + 细边框,内部左侧为悬停/点击触发区,右侧(可选)为移除按钮 */
.element-attach {
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

.element-pill-body {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 100%;
  padding: 0 6px 0 10px;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  cursor: pointer;
}

.element-pill-text {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.element-remove {
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

.element-remove:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}
</style>

<style>
/* 弹层内容渲染在 body 层(Arco Trigger 的 portal),需非 scoped 命中。
   Arco 的 DOM 是 两层:content-class(.element-popover = .arco-popover-popup-content)
   包着 .arco-popover-content。默认两层各有一套白底/内边距/圆角/阴影,叠起来像套了两层壳。
   这里把**外层**壳(chrome)抹掉,只让内层 .arco-popover-content 呈现一张卡片 —— 视觉上单层。 */
.element-popover.element-popover {
  padding: 0;
  border: none;
  border-radius: 0;
  background: transparent;
  box-shadow: none;
}

.element-popover .arco-popover-content {
  padding: 0;
  border-radius: 10px;
  overflow: hidden;
  background: var(--bg-1);
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.08),
    0 10px 28px rgba(0, 0, 0, 0.16);
}

.element-popover .element-cards {
  max-height: min(360px, 55vh);
  overflow-y: auto;
  padding: 10px 12px;
  scrollbar-gutter: stable;
}

.element-popover .element-card {
  min-width: 250px;
  max-width: 420px;
}

.element-popover .element-card-next {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--border-light);
}

.element-popover .element-card-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  margin-bottom: 4px;
}

.element-popover .element-card-name {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.element-popover .element-card-size {
  margin-left: auto;
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.element-popover .element-card-row {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  font-size: var(--font-size-2xs);
  line-height: 16px;
}

.element-popover .element-card-label {
  flex: 0 0 66px;
  color: var(--text-tertiary);
}

.element-popover .element-card-value {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  color: var(--text-primary);
}

.element-popover .element-card-text {
  font-family: var(--font-mono);
  word-break: break-all;
}

.element-popover .element-card-value.is-path .element-card-text {
  color: var(--primary);
}

.element-popover .element-swatch {
  width: 10px;
  height: 10px;
  flex-shrink: 0;
  border-radius: 2px;
  border: 1px solid var(--border-strong);
}

.element-popover .element-card-sep {
  height: 1px;
  margin: 5px 0;
  background: var(--border-light);
}
</style>
