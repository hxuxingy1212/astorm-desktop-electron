<script setup lang="ts">
/**
 * 共享 Markdown 正文渲染（对话正文 / 文件预览共用的**唯一**一处实现）。
 *
 * 直接包装 vue-stream-markdown 的 `Markdown`：节点渲染器走共享的 `markdownNodeRenderers`
 * （本地文件引用 = 可点击 chip，其余链接 = 外链确认弹窗 + 站内浏览器页签），关掉逐块入场动画，
 * 默认静态模式（流式正文由调用方传 `mode="streaming"`）。
 * 字号在这里钉死（14px，与对话气泡正文同值）——外层容器只管「可滚动 + 内边距」，
 * 代码块 / 表格 / 列表 / 行高全部来自库主题，两处调用方因此不可能再出现两套排版。
 */
import { Markdown } from 'vue-stream-markdown'
import { markdownNodeRenderers } from '@/components/markdown/renderers'

/** 代码块保留复制；关闭折叠、下载与全屏操作。 */
const markdownControls = {
  code: { collapse: false, copy: true, download: false, fullscreen: false }
}

/** 代码正文不显示行号，保持标题、语法高亮与复制操作。 */
const markdownCodeOptions = {
  lineNumbers: false
}

withDefaults(
  defineProps<{
    content: string
    isDark: boolean
    /** 流式输出中传 'streaming'（带光标），静态文件/历史内容保持 'static' */
    mode?: 'static' | 'streaming'
  }>(),
  { mode: 'static' }
)

/** 节点渲染器：模块级常量，避免每次渲染新建对象（同聊天侧原有写法） */
const nodeRenderers = markdownNodeRenderers
</script>

<template>
  <Markdown
    class="markdown-body"
    :content="content"
    :is-dark="isDark"
    :mode="mode"
    :enable-animate="false"
    :node-renderers="nodeRenderers"
    :controls="markdownControls"
    :code-options="markdownCodeOptions"
  />
</template>

<style scoped>
/* markdown 根元素不带 font-size（继承自容器）：这里统一到字体体系的「正文档」
   = --font-size-md（14px）+ --line-height-body（1.5，按本地 Codex 视觉校准），
   让文件预览这类自有容器无需再抄一份排版 */
.markdown-body {
  color: var(--text-body);
  font-size: var(--font-size-md);
  line-height: var(--line-height-body);
}

/* 代码块、工具条和正文统一使用对话区与用户消息的背景面。 */
.markdown-body :deep([data-stream-markdown='code-block']),
.markdown-body :deep([data-stream-markdown='code-block-content']),
.markdown-body :deep([data-stream-markdown='code-block-content'] pre),
.markdown-body :deep([data-stream-markdown='code-block-content'] .shiki) {
  background: var(--bg-2) !important;
}

/* 行内代码与文件引用共用同一颗药丸：inline-flex 保证文字/图标垂直居中。 */
.markdown-body :deep([data-stream-markdown='inline-code']) {
  display: inline-flex;
  align-items: center;
  box-sizing: border-box;
  max-width: 100%;
  min-width: 0;
  padding: 0 5px;
  vertical-align: baseline;
  border: 0.5px solid var(--border-base);
  border-radius: var(--radius-sm);
  background: var(--bg-2) !important;
  font-family: var(--font-mono);
  font-size: 0.875em !important;
  line-height: 1.5;
  overflow-wrap: anywhere;
}

/* 链接与文件引用同一套柔和链接色；参考实现不加下划线 */
.markdown-body :deep([data-stream-markdown='link']) {
  color: var(--link-text) !important;
  text-decoration: none !important;
}

.markdown-body :deep([data-stream-markdown='link']:hover),
.markdown-body :deep([data-stream-markdown='link']:focus-visible) {
  color: color-mix(in srgb, var(--link-text) 88%, var(--text-primary)) !important;
  text-decoration: none !important;
}

/* 代码块不要外框：靠底色与圆角分层，头部只留图标 + 类型名。 */
.markdown-body :deep([data-stream-markdown='code-block']) {
  border: none !important;
}

.markdown-body :deep([data-stream-markdown='code-block-header']) {
  border-bottom: 0 !important;
  background: var(--bg-2) !important;
}

/* 头部左侧：代码图标与类型名同一条水平线（SVG 默认 inline 会带基线空隙）。 */
.markdown-body :deep([data-stream-markdown='language-title']) {
  align-items: center !important;
  line-height: 1 !important;
}

.markdown-body :deep([data-stream-markdown='language-title'] svg) {
  display: block;
  flex-shrink: 0;
}

.markdown-body :deep([data-stream-markdown='language-name']) {
  line-height: 1 !important;
}

/* 标题栏与第一行代码靠近一些，保留左右和底部留白。 */
.markdown-body :deep([data-stream-markdown='code-block-content'] pre) {
  padding-top: 0 !important;
}
</style>
