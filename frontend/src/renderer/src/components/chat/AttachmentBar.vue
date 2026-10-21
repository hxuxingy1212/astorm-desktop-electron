<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { MessageAttachment } from '@/types'
import { getFileIconUrl } from '@/utils/fileIcons'
import FilePreviewModal from '@/components/common/FilePreviewModal.vue'

/**
 * 发送框上方的附件条：每项一个胶囊（图 / 文件名 + 体积 + 移除）。
 *
 * 两类附件形态不同（见 `main/attachmentFiles.ts`）：
 * - **图片**（`mime` 为 `image/*`）：显示缩略图，经 `astorm-attachment://` 协议直读本地
 *   staging 副本（不受 8MB base64 上限约束），失败时回退文件名占位；
 * - **其他文件**：显示按扩展名的文件图标（`utils/fileIcons`，与左侧文件树同一套手写 SVG）。
 *   这类文件没有 staging 副本，`path` 即用户原始路径（后端只记录，由 agent 自行读取）。
 *
 * 点击胶囊 → 统一文件预览弹窗（`FilePreviewModal`）：`path` 恒为本地绝对路径
 * （图片 = staging 副本，其他 = 用户原始路径），面板直读渲染；文件已被移动 / 删除时
 * 弹窗内呈现读取失败态。移除按钮不触发预览。
 *
 * 视觉语言与 ElementAttachment（网页元素胶囊）一致：浅色胶囊 + 细边框。
 */
const props = withDefaults(
  defineProps<{
    items: MessageAttachment[]
    /**
     * 是否可移除(消息气泡内展示时传 false)。
     *
     * ⚠️ **必须显式给 `default: true`**：Vue 对「Boolean 类型且无 default」的 prop 有
     * 「缺省即 false」的转换规则（runtime-core `resolvePropValue`: `isAbsent && !hasDefault`
     * → `value = false`）。父组件不传时拿到的是 `false` 而不是 `undefined`，移除按钮会静默消失。
     */
    removable?: boolean
  }>(),
  { removable: true }
)

const emit = defineEmits<{
  (e: 'remove', index: number): void
}>()

const { t } = useI18n()

/** 缩略图加载失败(文件被移动/删除)→ 占位图标,不显示破图 */
const failed = ref<Set<string>>(new Set())

/** 是否图片附件(非图片不取字节,只画文件图标) */
function isImage(item: MessageAttachment): boolean {
  return !!item.mime?.startsWith('image/')
}

function thumbUrl(path: string): string {
  return window.api?.attachment?.toUrl?.(path) ?? ''
}

function markFailed(path: string): void {
  failed.value.add(path)
  failed.value = new Set(failed.value)
}

function sizeText(size?: number): string {
  if (!size) return ''
  if (size >= 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`
  if (size >= 1024) return `${Math.round(size / 1024)} KB`
  return `${size} B`
}

/** 点击胶囊 → 统一文件预览弹窗（全项目同一套，见 components/common/FilePreviewModal） */
const previewFile = ref<MessageAttachment | null>(null)
</script>

<template>
  <div v-if="props.items.length" class="attach-root">
    <div class="attach-bar">
      <div
        v-for="(item, i) in props.items"
        :key="item.path"
        class="attach-chip"
        role="button"
        :title="item.path"
        @click="previewFile = item"
      >
        <img
          v-if="isImage(item) && thumbUrl(item.path) && !failed.has(item.path)"
          class="attach-chip-thumb"
          :src="thumbUrl(item.path)"
          :alt="item.name"
          @error="markFailed(item.path)"
        />
        <span v-else-if="isImage(item)" class="attach-chip-thumb attach-chip-fallback">
          <icon-image :size="14" />
        </span>
        <!-- 非图片:按扩展名的文件图标(与左侧文件树同一套手写 SVG) -->
        <span v-else class="attach-chip-thumb attach-chip-fallback">
          <img class="attach-chip-glyph" :src="getFileIconUrl(item.name)" width="16" height="16" alt="" />
        </span>
        <span class="attach-chip-name">{{ item.name }}</span>
        <span v-if="sizeText(item.size)" class="attach-chip-size">{{ sizeText(item.size) }}</span>
        <button
          v-if="props.removable"
          type="button"
          class="attach-chip-remove"
          :title="t('chat.removeAttachment')"
          @click.stop="emit('remove', i)"
        >
          <icon-close :size="12" />
        </button>
      </div>
    </div>

    <!-- 附件预览：全项目统一的文件预览弹窗（与邮件附件 / 交付物预览同款） -->
    <FilePreviewModal
      :open="!!previewFile"
      :name="previewFile?.name ?? ''"
      :path="previewFile?.path ?? ''"
      @close="previewFile = null"
    />
  </div>
</template>

<style scoped>
/* 根容器：承接父级 flex 的 order 等透传属性（单根）；内部为附件条 + 预览弹窗 */
.attach-root {
  width: 100%;
}

/* 附件条:横向排列,放不下时换行(与上方文件/分支条同层,共享浅灰底) */
.attach-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 12px 0;
  max-width: 100%;
}

/* 胶囊:缩略图/文件图标 + 文件名 + 体积 + 移除；可点击 → 预览弹窗 */
.attach-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 4px 0 4px;
  max-width: 260px;
  border: 0.5px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-1);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  cursor: pointer;
  transition:
    background-color 0.15s,
    border-color 0.15s;
}

.attach-chip:hover {
  background: var(--bg-2);
  border-color: var(--border-strong);
}

.attach-chip-thumb {
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  border-radius: 5px;
  object-fit: cover;
  background: var(--bg-3);
}

.attach-chip-fallback {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
}

/* 文件图标:手写 SVG 16px(设计尺寸,缩小会糊),在外壳 26px 槽位内居中 */
.attach-chip-glyph {
  display: block;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

.attach-chip-name {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 130px;
}

.attach-chip-size {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.attach-chip-remove {
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

.attach-chip-remove:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}
</style>
