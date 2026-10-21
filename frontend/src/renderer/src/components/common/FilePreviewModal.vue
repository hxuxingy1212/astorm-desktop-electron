<script setup lang="ts">
/**
 * 通用文件预览弹窗（全项目统一的弹窗预览组件）。
 *
 * 形态参考邮件附件的点击预览：自绘遮罩层（毛玻璃）+ 标题栏 + 关闭按钮，
 * 内容复用文件预览页签面板（`FilePreviewPanel` 直连模式）——图片 / Office / 文本 /
 * Markdown 等类型由面板内部分流渲染，弹窗本身不关心文件类型。
 * 用法：`<FilePreviewModal v-model:open="open" :name="name" :path="absPath" />`
 */
import FilePreviewPanel from '../../panels/FilePreviewPanel.vue'

const props = defineProps<{
  /** 是否打开（v-model:open） */
  open: boolean
  /** 标题栏显示的文件名 */
  name: string
  /** 文件绝对路径（FilePreviewPanel 直连模式） */
  path: string
}>()

const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
  (e: 'close'): void
}>()

function close(): void {
  emit('update:open', false)
  emit('close')
}
</script>

<template>
  <div v-if="open" class="fpm-mask" @click.self="close">
    <div class="fpm-layer">
      <div class="fpm-head">
        <span class="fpm-title" :title="props.name">{{ props.name }}</span>
        <button type="button" class="fpm-close" @click="close">
          <icon-close :size="16" />
        </button>
      </div>
      <div class="fpm-body">
        <!-- :key 按路径重挂载:连续预览不同文件时面板内部状态不串 -->
        <FilePreviewPanel :key="props.path" :file-path="props.path" :file-name="props.name" />
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 遮罩：毛玻璃压暗（点击空白处关闭）；弹层占视口 3/4 居中 */
.fpm-mask {
  position: fixed;
  inset: 0;
  z-index: 60;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.28);
  backdrop-filter: blur(8px) saturate(1.1);
}
.fpm-layer {
  display: flex;
  flex-direction: column;
  width: 75vw;
  height: 75vh;
  background: var(--bg-base);
  border: 0.5px solid var(--border-strong);
  border-radius: 14px;
  overflow: hidden;
  box-shadow:
    0 24px 70px rgba(0, 0, 0, 0.35),
    0 0 0 0.5px rgba(0, 0, 0, 0.08);
}
.fpm-head {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  height: 44px;
  flex-shrink: 0;
  padding: 0 10px 0 16px;
  border-bottom: 1px solid var(--panel-border);
}
/* 标题绝对居中:不受右侧关闭按钮宽度影响;长名两端各让出按钮宽度防重叠 */
.fpm-title {
  position: absolute;
  left: 50%;
  transform: translateX(-50%);
  max-width: calc(100% - 110px);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}
.fpm-close {
  /* 标题绝对居中后脱离文档流:关闭按钮自身推到右侧原位 */
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  flex-shrink: 0;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background-color 0.12s ease;
}
.fpm-close:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.fpm-body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
@media (prefers-reduced-transparency: reduce) {
  .fpm-mask {
    backdrop-filter: none;
  }
}
</style>
