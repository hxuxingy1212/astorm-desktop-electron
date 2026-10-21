<script setup lang="ts">
/**
 * 单条聊天消息：用户气泡（附件 + 提及）/ 助手**正文**（含结尾正文与等待提示）。
 *
 * 思考链与工具卡片**只走过程收纳块的新渲染**（ChatTurnGroup/ChatStepList）；
 * 本组件是块外落点，`parts` 只认 `content`（`reasoning`/`tools` 传入不再渲染——
 * 老的单条卡片路径已删除）。尾部动作行（时间/复制/fork/轮次）也随之只服务正文。
 *
 * **独立成组件是为了渲染隔离**：对消息字段的所有深层读取都发生在本组件内，
 * 因此流式追加一个 token 只触发「那一条」的重新渲染；ChatPanel 只读 `msg.id`/`msg.role`
 * （列表 key 与用户/助手分支），不再随正文增长整表重渲染。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import MarkdownBody from '@/components/common/MarkdownBody.vue'
import type { ChatMessage, MessageAttachment } from '@/types'
import { stripArtifactMarkers } from '@/utils/artifacts'
import { PART_FULL, type MessagePart } from './turnGrouping'
import { getFileIconUrl } from '@/utils/fileIcons'
import MentionText from '../mentions/MentionText.vue'
import { formatCount } from '@/utils/number'
import { formatDateTime } from '@/utils/time'

const props = withDefaults(
  defineProps<{
    msg: ChatMessage
    isDark: boolean
    /** 要渲染的部分（缺省全部）；见文件头注释 */
    parts?: MessagePart[]
    /** 隐藏右下角时间戳（收纳块内的步骤用：块头已有该轮总耗时，逐步再显示太吵） */
    hideTime?: boolean
    /** 用户消息是否可撤回（摘要边界之前的消息不可撤回；缺省 false = 隐藏动作） */
    canUndo?: boolean
    /** 助手消息是否提供 fork 按钮（内嵌只读页签等场景置 false） */
    canFork?: boolean
    /** 本轮是第几次用户提问（跨会话累计，1 起；仅结尾无工具助手消息底部展示） */
    turnNo?: number
    /** 本轮步数（= 本轮 LLM 调用次数；一次响应多个工具调用算 1 步） */
    turnSteps?: number
  }>(),
  { parts: () => PART_FULL, hideTime: false, canUndo: false, canFork: false }
)

const emit = defineEmits<{ (e: 'undo'): void; (e: 'fork'): void }>()

const { t } = useI18n()

/** 各部分的开关：只做「有没有」和 parts 的交集判断，不读正文内容做判定 */
const showContentPart = computed(() => props.parts.includes('content'))
const showContent = computed(() => showContentPart.value && !!props.msg.content)

/** 助手正文动作行(时间 + 复制 + fork):仅「无工具调用的最终助手消息」展示;收纳块内步骤不展示 */
const showAssistantActions = computed(
  () =>
    props.msg.role === 'assistant' &&
    showContentPart.value &&
    !!props.msg.content &&
    !props.msg.streaming &&
    !props.hideTime &&
    !props.msg.summary &&
    !props.msg.tools?.length
)

/**
 * 展示用正文：剥离交付物 / 变更机器标记块（`<!--astorm:artifacts ... -->`）。
 * 后端落盘前已剥离，但流式期间正文是原始增量；这里兜底避免标记块闪现（含 `includes` 预检，成本可忽略）。
 */
const displayContent = computed(() => stripArtifactMarkers(props.msg.content ?? ''))

/**
 * 该消息在当前 parts 下是否有可见内容：没有就整条不渲染（否则收纳块内会留一个空行 + 间距）。
 * 流式占位（正文还没到）仍要渲染，等待提示得有地方显示。
 */
const visible = computed(
  () =>
    props.msg.role === 'user' ||
    showContent.value ||
    (showContentPart.value && !!props.msg.streaming)
)

/** 附件绝对路径 → astorm-attachment:// URL(主进程协议只允许 staging 与会话附件目录) */
function attachmentUrl(path: string): string {
  return window.api?.attachment?.toUrl?.(path) ?? ''
}

/** 是否图片附件：非图片没有本地副本（后端只记原始路径），只能画扩展名图标、不做预览 */
function isImageAttachment(att: MessageAttachment): boolean {
  return !!att.mime?.startsWith('image/')
}

function sizeText(size?: number): string {
  if (!size) return ''
  if (size >= 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`
  if (size >= 1024) return `${Math.round(size / 1024)} KB`
  return `${size} B`
}

/** 复制消息正文(提及 token 原样保留) */
async function copyContent(): Promise<void> {
  try {
    await navigator.clipboard.writeText(props.msg.content ?? '')
    Message.success(t('common.copied'))
  } catch {
    Message.error(t('common.copyFailed'))
  }
}

/** 撤回本条用户消息(不可撤回时按钮已禁用,这里再兜底) */
function onUndo(): void {
  if (props.canUndo) emit('undo')
}

</script>

<template>
  <div
    v-if="visible"
    class="msg-row"
    :class="msg.role"
    :data-rail-anchor="msg.role === 'user' ? msg.id : undefined"
    :data-message-id="msg.id"
    :data-message-seq="msg.seq"
    :data-message-role="msg.role"
  >
    <!-- 用户消息:右对齐气泡(图片附件缩略图 + 提及 token 渲染为标签) -->
    <div v-if="msg.role === 'user'" class="msg-bubble user-bubble">
      <!-- 图片附件:缩略图 + 文件名,点击经 Arco Image 预览放大(文件缺失时显示占位) -->
      <div v-if="msg.attachments?.length" class="user-attachments">
        <div v-for="att in msg.attachments" :key="att.path" class="user-attachment">
          <!-- 图片：缩略图（点击放大）。其他文件：按扩展名的图标 + 名称（无本地副本、不预览） -->
          <a-image
            v-if="isImageAttachment(att)"
            :src="attachmentUrl(att.path)"
            :width="132"
            :height="96"
            fit="cover"
            :alt="att.name"
            class="user-attachment-img"
          />
          <div v-else class="user-attachment-file" :title="att.path">
            <img class="user-attachment-glyph" :src="getFileIconUrl(att.name)" width="16" height="16" alt="" />
            <span class="user-attachment-file-name">{{ att.name }}</span>
            <span v-if="sizeText(att.size)" class="user-attachment-file-size">{{ sizeText(att.size) }}</span>
          </div>
          <!-- 图片的名称在图下；文件卡里已带名称与体积，不重复 -->
          <span v-if="isImageAttachment(att)" class="user-attachment-name" :title="att.path">{{ att.name }}</span>
        </div>
      </div>
      <div v-if="msg.content" class="msg-content"><MentionText :text="msg.content" /></div>
    </div>

    <!-- 用户消息动作行:时间 + 撤回(可撤回到此消息之前) + 复制 -->
    <div v-if="msg.role === 'user'" class="user-actions">
      <span class="user-time">{{ formatDateTime(msg.createdAt) }}</span>
      <button
        type="button"
        class="msg-action"
        :class="{ 'msg-action-disabled': !canUndo }"
        :disabled="!canUndo"
        :title="canUndo ? t('chat.rewind') : t('chat.rewindDisabled')"
        @click="onUndo"
      >
        <icon-undo :size="14" />
      </button>
      <button type="button" class="msg-action" :title="t('common.copy')" @click="copyContent">
        <icon-copy :size="14" />
      </button>
    </div>

    <!-- 助手消息:思考链 / 工具卡片 / markdown 正文 + 流式光标 + 时间戳
         致命错误与工具失败提示统一显示在底部聊天框上方(不占内容区) -->
    <div v-else class="msg-wrapper">
      <div class="msg-bubble assistant-bubble">
        <!-- 正文(思考链 / 工具卡片只走过程收纳块,见文件头注释;渲染器与文件预览共用 MarkdownBody) -->
        <div v-if="showContent" class="msg-content">
          <MarkdownBody
            :content="displayContent"
            :is-dark="isDark"
            :mode="msg.streaming ? 'streaming' : 'static'"
          />
        </div>
      </div>
      <!-- 助手正文动作行:轮次/步数 + 用量 + 日期 + 复制/fork(仅无工具调用的最终助手消息;整行右对齐) -->
      <div v-if="showAssistantActions" class="assistant-actions">
        <span v-if="turnNo" class="msg-stat">
          <icon-gauge :size="13" />
          {{ t('chat.roundSteps', { r: turnNo, s: turnSteps ?? 0 }) }}
        </span>
        <span v-if="msg.usage?.total" class="msg-stat">
          <icon-data :size="13" />
          {{ t('chat.usageLabel') }} {{ formatCount(msg.usage.total) }} token
        </span>
        <span class="msg-time-text">{{ formatDateTime(msg.completedAt ?? msg.createdAt) }}</span>
        <button type="button" class="msg-action" :title="t('common.copy')" @click="copyContent">
          <icon-copy :size="14" />
        </button>
        <button
          v-if="canFork"
          type="button"
          class="msg-action"
          :title="t('chat.fork')"
          @click="emit('fork')"
        >
          <icon-branch :size="14" />
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.msg-row {
  display: flex;
  flex-direction: column;
}

.msg-row.user {
  align-items: flex-end;
}

.msg-row.assistant {
  align-items: flex-start;
}

/* 气泡 */
.msg-bubble {
  padding: 10px 14px;
  border-radius: var(--radius-md);
  line-height: var(--line-height-body);
  font-size: var(--font-size-md);
  word-break: break-word;
}

.user-bubble {
  white-space: pre-wrap;
  background-color: var(--bg-2);
  color: var(--text-primary);
  border: none;
  border-bottom-right-radius: 4px;
}

/* 用户消息动作行:时间 + 撤回 / 复制(右对齐,与气泡同级) */
.user-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
  height: 18px;
}

.user-time {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

.msg-action {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
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

.msg-action:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.msg-action-disabled {
  cursor: not-allowed;
  opacity: 0.4;
}

.msg-action-disabled:hover {
  background: transparent;
  color: var(--text-tertiary);
}

/* 用户消息附件（可换行）：图片是缩略图卡片（名称在下方）；其他文件是「图标 + 名称 + 体积」横条 */
.user-attachments {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 6px;
}

/* 图片卡宽度固定 132（与 a-image 同宽）；文件卡按内容自适应但同样受上限约束 */
.user-attachment {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-width: 132px;
}

.user-attachment-img {
  border-radius: 8px;
  overflow: hidden;
  border: 1px solid var(--border-base);
}

/* 非图片附件:横向卡片(文件图标 + 名称 + 体积)。没有本地副本,点击/悬浮不做预览,只有完整路径 tooltip */
.user-attachment-file {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 132px;
  height: 32px;
  padding: 0 8px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background-color: var(--bg-1);
}

.user-attachment-glyph {
  display: block;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

.user-attachment-file-name {
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.user-attachment-file-size {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.user-attachment-name {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.assistant-bubble {
  background: transparent;
  padding: 0;
  border-radius: 0;
  color: var(--text-primary);
  line-height: 1.7;
}

.msg-wrapper {
  position: relative;
  width: 100%;
}

/* 助手正文动作行:轮次/步数 + 用量 + 日期 + 复制 / fork(整行右对齐,与正文同宽) */
.assistant-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 2px;
  height: 18px;
  width: 100%;
}

/* 轮次/步数、用量:次要灰字 + 图标 */
.msg-stat {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

.msg-time-text {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}
</style>
