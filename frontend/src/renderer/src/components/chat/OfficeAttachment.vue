<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useOfficePickStore } from '@/stores/officePick'
import { officeRefChipLabel } from '@/utils/officeRef'
import type { PickedElementCard, PickedElementCardRow } from '@/types'
import ElementPill from './ElementPill.vue'

/**
 * 发送框上方的「文档元素」附件：office 预览拾取的胶囊(图标 + N 个文档元素 + 移除)，
 * 悬停弹出卡片。外观与浏览器网页元素胶囊完全同款([`ElementPill`])，字段为
 * office 契约键(文件/路径/类型/文本)；行标签在构造时本地化，`ElementPill` 的
 * labelOf 查不到 browser 键时原样回退为已本地化文本。
 */
const officePick = useOfficePickStore()
const { t } = useI18n()

const cards = computed<PickedElementCard[]>(() =>
  officePick.items.map((it) => {
    const d = it.data
    const add = (key: string, value: string | number | undefined): PickedElementCardRow | undefined => {
      if (value === undefined || value === null || `${value}`.trim() === '') return undefined
      return { key: t(`panel.filePreview.refRows.${key}`), value: `${value}`, path: key === 'file' || key === 'path' }
    }
    const rows: PickedElementCardRow[] = [
      add('file', d.file),
      add('path', d.path),
      add('type', d.type),
      add('name', d.name),
      add('text', d.text)
    ].filter((r): r is PickedElementCardRow => r !== undefined)
    return { name: officeRefChipLabel(d), size: d.kind.toUpperCase(), rows }
  })
)
</script>

<template>
  <div v-if="officePick.count" class="element-attach-wrap">
    <ElementPill
      :cards="cards"
      removable
      :count-label="t('panel.filePreview.attachCount', { count: officePick.count })"
      :remove-title="t('panel.filePreview.attachRemove')"
      @remove="officePick.remove()"
    />
  </div>
</template>

<style scoped>
.element-attach-wrap {
  max-width: 100%;
}
</style>
