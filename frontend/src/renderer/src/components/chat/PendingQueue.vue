<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore, type PendingQueueItem } from '@/stores/sessions'

/**
 * 前端消息队列(会话正在生成时发送的消息先入列,收到 done 后按列表序自动发送第一条)。
 * 位于输入面板的**第一层**(与新建页 FileBranchBar 同层级,共享整体圆角边框与深色底):
 * 纵向列表 + 上下拖拽排序;子项为 pick-btn 风格芯片(透明底 / 8px 圆角 / hover 灰底)。
 */
const props = defineProps<{ items: PendingQueueItem[] }>()

const emit = defineEmits<{ (e: 'edit', payload: { id: string; text: string }): void }>()

const { t } = useI18n()
const sessions = useSessionsStore()

const dragIndex = ref<number | null>(null)

/** 纯图片消息(无文本)在队列里的展示文案:文件名列表 */
function queuedImageLabel(item: PendingQueueItem): string {
  const names = (item.opts.attachments ?? []).map((a) => a.name).filter(Boolean)
  return names.length ? names.join('、') : t('chat.attachmentsOnly')
}

function onDragStart(index: number): void {
  dragIndex.value = index
}
function onDragOver(e: DragEvent): void {
  e.preventDefault()
}
function onDrop(index: number): void {
  if (dragIndex.value === null) return
  sessions.movePending(sessions.activeId, dragIndex.value, index)
  dragIndex.value = null
}
</script>

<template>
  <!-- 顶层条:不设独立边框/背景,背景由面板第一层(chat-input-top)提供;无标题文字 -->
  <div class="pending-queue" :aria-label="t('chat.queueTitle')">
    <div class="pq-list">
      <div
        v-for="(item, i) in props.items"
        :key="item.id"
        class="pq-chip"
        :class="{ dragging: dragIndex === i }"
        draggable="true"
        @dragstart="onDragStart(i)"
        @dragend="dragIndex = null"
        @dragover="onDragOver"
        @drop="onDrop(i)"
      >
        <icon-drag-dot-vertical class="pq-drag" :size="13" />
        <icon-image v-if="item.opts.attachments?.length" class="pq-image-icon" :size="13" />
        <span class="pq-text" :title="item.text">{{ item.text || queuedImageLabel(item) }}</span>
        <button type="button" class="pq-btn" @click="sessions.sendImmediate(sessions.activeId, item.id)">
          {{ t('chat.queueImmediate') }}
        </button>
        <button type="button" class="pq-btn" @click="emit('edit', { id: item.id, text: item.text })">
          {{ t('chat.queueEdit') }}
        </button>
        <button type="button" class="pq-btn" @click="sessions.cancelPending(sessions.activeId, item.id)">
          {{ t('chat.queueCancel') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pending-queue {
  padding: 6px 10px 4px;
}

/* 纵向列表:上下拖拽排序;最多显示 5 条(超 5 条滚动),scrollbar 样式沿用全局 6px 细滚动条;
   padding-right 预留滚动条宽度(6px),出现滚动条时内容不偏移 */
.pq-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 136px;
  overflow-y: auto;
  padding-right: 6px;
}

/* 子项:pick-btn 风格芯片(透明底 / 8px 圆角 / hover 灰底);
   flex-shrink:0 防止超出 max-height 时被压缩(保持整行,滚动条触发) */
.pq-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  height: 24px;
  padding: 0 10px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-primary);
  cursor: grab;
  transition: background 0.15s;
}

.pq-chip:hover {
  background: color-mix(in srgb, var(--bg-3) 60%, var(--bg-2));
}

.pq-chip.dragging {
  opacity: 0.55;
}

.pq-chip:active {
  cursor: grabbing;
}

.pq-drag {
  flex-shrink: 0;
  color: var(--text-tertiary);
  cursor: grab;
}

/* 带图片附件的队列项:文件名前加图片图标提示 */
.pq-image-icon {
  flex-shrink: 0;
  color: var(--text-tertiary);
}

.pq-text {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 行内操作按钮:常态带灰底(bg-3)可辨识,悬浮只做亮度变化(无颜色变化) */
.pq-btn {
  height: 18px;
  padding: 0 6px;
  border: none;
  border-radius: 6px;
  font-size: var(--font-size-xs);
  line-height: 18px;
  color: var(--text-secondary);
  background: var(--bg-3);
  cursor: pointer;
  flex-shrink: 0;
  transition: filter 0.15s;
}

.pq-btn:hover {
  filter: brightness(0.8);
}

[data-theme='dark'] .pq-btn:hover {
  filter: brightness(1.3);
}
</style>