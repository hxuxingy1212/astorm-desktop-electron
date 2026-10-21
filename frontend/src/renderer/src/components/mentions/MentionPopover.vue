<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { highlightSegments } from './match'
import type { MentionGroup, MentionItem } from './types'

const props = defineProps<{
  open: boolean
  groups: MentionGroup[]
  /** 全列表扁平索引(跨分组) */
  activeIndex: number
  /** 弹层锚点:贴输入栏上方,宽度与输入栏一致;dialogH 用于计算列表最大高度 */
  anchor: { left: number; bottom: number; width: number; dialogH: number } | null
  /** 当前过滤词(用于命中高亮) */
  query?: string
  /** 底部提示行(如「输入内容以搜索文件或文件夹」);为空则不渲染 */
  hint?: string
  /** 空态文案(缺省「无匹配结果」) */
  emptyText?: string
}>()

const emit = defineEmits<{
  (e: 'select', item: MentionItem): void
  (e: 'hover', index: number): void
}>()

const { t } = useI18n()

const listRef = ref<HTMLElement | null>(null)
/** 列表可滚动且未到底:底部渐隐一行,提示还有更多结果 */
const faded = ref(false)

/** 每个分组内条目的扁平起始索引 */
const offsets = computed(() => {
  let acc = 0
  return props.groups.map((g) => {
    const start = acc
    acc += g.items.length
    return start
  })
})

const total = computed(() => props.groups.reduce((n, g) => n + g.items.length, 0))

function groupLabel(g: MentionGroup): string {
  return g.labelKey ? t(g.labelKey) : (g.label ?? g.id)
}

/** 分组头:仅有标题的分组才渲染(如 '@' 单组文件列表不带「文件」标题) */
function hasGroupLabel(g: MentionGroup): boolean {
  return Boolean(g.labelKey || g.label)
}

function itemLabel(i: MentionItem): string {
  return i.labelKey ? t(i.labelKey) : (i.label ?? i.id)
}

function itemDesc(i: MentionItem): string {
  return i.descriptionKey ? t(i.descriptionKey) : (i.description ?? '')
}

/** 按当前过滤词切分命中片段(供 <mark> 高亮) */
function highlight(text: string): { text: string; hit: boolean }[] {
  return highlightSegments(text, props.query ?? '')
}

function updateFade(): void {
  const el = listRef.value
  if (!el) return
  const scrollable = el.scrollHeight - el.clientHeight > 1
  const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 2
  faded.value = scrollable && !atBottom
}

// 键盘高亮项变化时滚动到可见区域
watch(
  () => props.activeIndex,
  () => {
    nextTick(() => {
      listRef.value?.querySelectorAll<HTMLElement>('.mention-item')[props.activeIndex]?.scrollIntoView({
        block: 'nearest'
      })
      updateFade()
    })
  }
)

// 列表内容/开关变化后重算底部渐隐
watch(
  () => [props.open, props.groups, props.anchor?.width, props.anchor?.dialogH],
  () => nextTick(updateFade)
)
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open && anchor"
      class="mention-popover"
      :style="{ left: `${anchor.left}px`, bottom: `${anchor.bottom}px`, width: `${anchor.width}px` }"
    >
      <div
        ref="listRef"
        class="mention-popover-list"
        :class="{ 'mention-popover-list-faded': faded }"
        :style="{ maxHeight: `${anchor.dialogH * 2.5}px` }"
        @scroll="updateFade"
      >
        <template v-for="(g, gi) in groups" :key="g.id">
          <div v-if="g.items.length && hasGroupLabel(g)" class="mention-group-label">{{ groupLabel(g) }}</div>
          <div
            v-for="(item, ii) in g.items"
            :key="`${g.id}:${item.id}`"
            class="mention-item"
            :class="{ 'mention-item-active': offsets[gi] + ii === activeIndex }"
            @mousemove="emit('hover', offsets[gi] + ii)"
            @mousedown.prevent
            @click="emit('select', item)"
          >
            <!-- 彩色文件图标(手写 SVG);无 iconUrl 时回退线框图标 -->
            <span v-if="item.iconUrl" class="mention-item-icon">
              <img class="mention-item-glyph" :src="item.iconUrl" width="16" height="16" alt="" />
            </span>
            <span v-else class="mention-item-icon" :style="{ color: item.color }">
              <component :is="item.icon || 'icon-file'" :size="15" />
            </span>
            <span class="mention-item-main">
              <span class="mention-item-label">
                <span
                  v-for="(p, pi) in highlight(itemLabel(item))"
                  :key="`l${pi}`"
                  :class="{ 'mention-hit': p.hit }"
                >{{ p.text }}</span>
              </span>
              <span v-if="itemDesc(item)" class="mention-item-desc">
                <span
                  v-for="(p, pi) in highlight(itemDesc(item))"
                  :key="`d${pi}`"
                  :class="{ 'mention-hit': p.hit }"
                >{{ p.text }}</span>
              </span>
            </span>
          </div>
        </template>
        <div v-if="!total" class="mention-empty">{{ emptyText || t('mentions.empty') }}</div>
      </div>

      <!-- 底部提示行:固定在弹层底部,不随列表滚动 -->
      <div v-if="hint" class="mention-hint">
        <icon-info-circle :size="14" />
        <span>{{ hint }}</span>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
/* 弹层:teleport 到 body,固定定位贴输入栏上方;宽度跟随输入栏。
   面板(边框/底色/阴影)在外层,列表只负责滚动,提示行固定在底部 */
.mention-popover {
  position: fixed;
  z-index: 1000;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background-color: color-mix(in srgb, var(--bg-2) 96%, transparent);
  backdrop-filter: blur(12px);
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.14);
}

.mention-popover-list {
  /* 最大高度 = 2.5 × 对话框高度(由父组件内联传入),超出内部滚动;滚动条走全局那套 */
  overflow-y: auto;
  padding: 6px;
}

/* 未滚到底:底部一行渐隐,暗示还有更多结果 */
.mention-popover-list-faded {
  mask-image: linear-gradient(to bottom, #000 calc(100% - 24px), transparent);
  -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 24px), transparent);
}

.mention-group-label {
  padding: 6px 10px 4px;
  font-size: var(--font-size-2xs);
  line-height: 16px;
  color: var(--text-tertiary);
}

.mention-item {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 38px;
  padding: 6px 8px;
  border-radius: 8px;
  cursor: pointer;
  font-size: var(--font-size-xs);
}

.mention-item:hover {
  background-color: var(--bg-hover);
}

.mention-item-active {
  background-color: var(--bg-active);
}

/* 图标槽:16px 彩色图标水平居中 */
.mention-item-icon {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
}

/* 与文件树同款的手写 SVG 图标(实色自带明暗可读性,无需滤镜) */
.mention-item-glyph {
  display: block;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

/* 标题 + 描述:左右一行,标题字号大、颜色深,描述字号小、颜色淡 */
.mention-item-main {
  display: flex;
  align-items: baseline;
  gap: 8px;
  flex: 1;
  min-width: 0;
}

.mention-item-label {
  flex-shrink: 0;
  font-size: var(--font-size-md);
  font-weight: 600;
  line-height: 20px;
  color: var(--text-primary);
}

.mention-item-desc {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-xs);
  line-height: 20px;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 命中片段:主色 + 加粗(列表标题与描述共用) */
.mention-hit {
  color: var(--accent-text);
  font-weight: 600;
}

.mention-empty {
  padding: 10px 12px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  text-align: center;
}

/* 提示行:与列表以细线分隔,图标+文案左对齐 */
.mention-hint {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  padding: 9px 14px 11px;
  border-top: 1px solid var(--border-light);
  font-size: var(--font-size-sm);
  line-height: 18px;
  color: var(--text-secondary);
}
</style>
