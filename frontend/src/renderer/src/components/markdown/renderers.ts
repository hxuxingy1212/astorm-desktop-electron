/**
 * markdown 节点渲染器（对话正文 / 文件预览 / 步骤与思考卡片 / 记忆面板共用的**唯一**一份）。
 *
 * - `link`：本地文件 → 文件引用 chip；文字是编号的 http(s) 链接 → 网页引用角标；
 *   其余 → MarkdownLink（外链确认弹窗 + 站内浏览器页签）；
 * - `inlineCode`：内容是文件路径的行内代码 → 文件引用 chip；其余保持库内置行内代码外观。
 *
 * 判定与路径解析见 `utils/fileRefs`（相对路径按 `FILE_REF_BASE` 注入的基准目录解析）；
 * 两种引用的书写约定在系统提示里（`astorm-core/src/agent/prompt.rs` 的「行为准则」）。
 * 新增调用方一律引用这里的 `markdownNodeRenderers`，不要各写一份——否则「哪些地方的文件/网页
 * 引用可点」会各说各话。
 */
import FileRefCode from '@/components/markdown/FileRefCode.vue'
import LinkRenderer from '@/components/markdown/LinkRenderer.vue'

export const markdownNodeRenderers = {
  link: LinkRenderer,
  inlineCode: FileRefCode
}
