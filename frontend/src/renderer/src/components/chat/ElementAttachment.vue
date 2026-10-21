<script setup lang="ts">
import { computed } from 'vue'
import { useElementPickStore } from '@/stores/elementPick'
import { elementChipLabel } from '@/utils/elementBlock'
import type { PickedElementCard } from '@/types'
import ElementPill from './ElementPill.vue'

/**
 * 发送框上方的「网页元素」附件:macOS 风格胶囊(图标 + N 个网页元素 + 移除),悬停弹出卡片。
 * 胶囊本体与消息卡片共用 [`ElementPill`];卡片数据来自主进程拾取时的 cardModel——
 * 与页面内高亮卡片同一份数据,只是换了浅色外壳,保证两处内容逐字一致。
 */
const elementPick = useElementPickStore()

const cards = computed<PickedElementCard[]>(() =>
  elementPick.items.map((it) => ({
    name: it.data.card?.name || elementChipLabel(it.data),
    size: it.data.card?.size ?? '',
    rows: it.data.card?.rows ?? []
  }))
)
</script>

<template>
  <div v-if="elementPick.count" class="element-attach-wrap">
    <ElementPill :cards="cards" removable @remove="elementPick.remove()" />
  </div>
</template>

<style scoped>
.element-attach-wrap {
  max-width: 100%;
}
</style>
