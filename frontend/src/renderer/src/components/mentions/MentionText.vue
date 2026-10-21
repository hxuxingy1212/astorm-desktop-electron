<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { parseMentionSegments } from './parse'
import type { ParsedSegment } from './parse'
import { elementCardFromBlock, findElementBlocks } from '@/utils/elementBlock'
import ElementPill from '@/components/chat/ElementPill.vue'
import type { MentionItem } from './types'
import type { PickedElementCard } from '@/types'

/**
 * 消息侧渲染:把用户消息里的提及 token 还原为与输入框同款配色的标签。
 *   - `/goal`、`$code-review`、`$agent/xxx`:仅在注册表能反查到条目时渲染(否则是正文路径/命令名)
 *   - `@src/a.ts`:启发式文件引用,无需注册表即可还原(刷新后历史消息仍成立)
 *
 * 内置浏览器拾取的元素以 ```element 围栏块随消息落库:先切出围栏(否则 XPath 里的 '/html'
 * 会被提及正则误判成 token),再按「连续围栏块合并」渲染为与发送框一致的**元素胶囊**
 * (悬停/点击弹出卡片),不再内联展开字段。
 */
const props = defineProps<{ text: string }>()

const { t } = useI18n()

/** 片段:提及片段必然带条目;文本片段来自 parse;元素片段携带若干 ```element 正文 */
type Segment = ParsedSegment | { type: 'element'; blocks: string[] }

const segments = computed<Segment[]>(() => {
  const src = props.text ?? ''
  const blocks = findElementBlocks(src)
  if (!blocks.length) return parseMentionSegments(src)

  const out: Segment[] = []
  let last = 0
  let pending: string[] = []
  // 连续元素块(之间只有空白)合并为一枚胶囊,计数与发送框一致
  const flush = (): void => {
    if (pending.length) {
      out.push({ type: 'element', blocks: pending })
      pending = []
    }
  }
  for (const block of blocks) {
    const between = src.slice(last, block.start)
    if (between.trim()) {
      flush()
      out.push(...parseMentionSegments(between))
    }
    pending.push(block.body)
    last = block.end
  }
  flush()
  const tail = src.slice(last)
  if (tail.trim()) out.push(...parseMentionSegments(tail))
  return out
})

function itemLabel(i: MentionItem): string {
  return i.labelKey ? t(i.labelKey) : (i.label ?? i.id)
}

/** 元素块正文 → 悬停卡片(降级:结构化数据只剩文本,按 i18n 反查 key 重建) */
function elementCards(blocks: string[]): PickedElementCard[] {
  return blocks.map((body) => elementCardFromBlock(body, t))
}
</script>

<template>
  <span class="mention-text">
    <template v-for="(seg, i) in segments" :key="i">
      <span v-if="seg.type === 'text'" class="mention-text-plain">{{ seg.text }}</span>
      <span v-else-if="seg.type === 'element'" class="mention-element">
        <ElementPill :cards="elementCards(seg.blocks)" />
      </span>
      <span
        v-else
        class="mention-text-chip"
        :class="{ 'mention-text-chip-file': seg.item.type === 'file' }"
        :style="{ '--chip-color': seg.item.color }"
      >
        <img v-if="seg.item.iconUrl" class="mention-text-chip-icon" :src="seg.item.iconUrl" width="12" height="12" alt="" />
        <span v-else-if="seg.item.icon" class="mention-text-chip-icon">
          <component :is="seg.item.icon" :size="11" />
        </span>
        {{ itemLabel(seg.item) }}
      </span>
    </template>
  </span>
</template>

<style scoped>
/* 保留换行/空白,与用户气泡的 pre-wrap 保持一致 */
.mention-text {
  white-space: pre-wrap;
  word-break: break-word;
}

/* 消息侧提及 chip:与输入框统一为「图标 + 彩色文字」的轻量内联样式(无胶囊底/边框) */
/* 与输入框统一:inline + inline-block 图标,基线随文字,不撑高行高 */
.mention-text-chip {
  display: inline;
  vertical-align: baseline;
  margin: 0 1px;
  padding: 0 3px;
  border-radius: 4px;
  background: transparent;
  color: var(--chip-color);
  line-height: inherit;
  white-space: nowrap;
}

.mention-text-chip:hover {
  background: color-mix(in srgb, var(--chip-color) 12%, transparent);
}

.mention-text-chip-icon {
  display: inline-block;
  width: 1em;
  height: 1em;
  vertical-align: -0.125em;
  margin-right: 0.28em;
  color: var(--chip-color);
}

/* 文件引用图标(手写 SVG 字形) */
.mention-text-chip-file img.mention-text-chip-icon {
  width: 1em;
  height: 1em;
}

/* 元素胶囊:与发送框上方一致(悬停/点击弹出卡片) */
.mention-element {
  display: inline-block;
  vertical-align: middle;
  margin: 2px 0;
}
</style>
