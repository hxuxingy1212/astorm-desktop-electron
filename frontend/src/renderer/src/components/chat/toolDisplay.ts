/**
 * 工具调用的「展示派生」唯一实现:类型名/图标、行摘要、过程归类。
 *
 * 关键:字段名必须与后端各工具的 **input schema** 对齐——
 * - `file_operation` 的动作字段是 `operation`(不是 `action`);
 * - `chat_with_user` 的提问文本即必填 `description`(不再另设 tip);
 * - `file_operation` 的 glob 用 `glob_pattern`、grep 用 `pattern`;
 * - `search` 的子类型在 `tool` 字段。
 * 供 `ChatStepList` 与 `ChatTurnGroup`(过程摘要)共用,避免两处各写一套漂移。
 */

import { fileGlyphUrl } from '@/utils/fileIcons'
import { i18n } from '@/i18n'

/** 一轮过程摘要的类别(与 i18n chat.actions.* 对应) */
export type ToolActionKind =
  | 'thinking'
  | 'read'
  | 'readImage'
  | 'write'
  | 'search'
  | 'edit'
  | 'commands'
  | 'code'
  | 'webSearch'
  | 'webFetch'
  | 'subagents'
  | 'plan'
  | 'questions'
  | 'tools'

function strOf(params: unknown, key: string): string {
  const v = (params as Record<string, unknown> | null | undefined)?.[key]
  return typeof v === 'string' ? v : ''
}

/** 取参数里的首个非空字段(字符串或数组,数组按逗号连接) */
function firstOf(params: unknown, keys: string[], sep = ', '): string {
  const p = (params ?? {}) as Record<string, unknown>
  for (const k of keys) {
    const v = p[k]
    if (typeof v === 'string' && v.trim()) return v.trim()
    if (Array.isArray(v) && v.length) return v.map((x) => String(x)).join(sep)
  }
  return ''
}

/** 兜底:参数里第一个非空字符串值 */
function firstAnyString(params: unknown): string {
  const p = (params ?? {}) as Record<string, unknown>
  for (const v of Object.values(p)) if (typeof v === 'string' && v.trim()) return v.trim()
  return ''
}

/**
 * 动作字段值:绝大多数工具用 `action`，`file_operation` 用 `operation`。
 * （这是与 schema 对齐的关键修正点。）
 */
export function actionOf(name: string, params: unknown): string {
  return strOf(params, name === 'file_operation' ? 'operation' : 'action')
}

/** `search` 工具的 5 个子类型 */
export type SearchSubTool = 'websearch' | 'webfetch' | 'github' | 'arxiv' | 'history'

function searchSub(params: unknown): SearchSubTool {
  const v = strOf(params, 'action')
  return (['websearch', 'webfetch', 'github', 'arxiv', 'history'] as const).includes(
    v as SearchSubTool
  )
    ? (v as SearchSubTool)
    : 'websearch'
}

/* ── `office` 工具(astorm-office 五 CLI：统一入口 + 四格式)的展示派生 ────
   参数面只有 `{ bin, args }`:bin 选格式(docx/xlsx/pptx/pdf)或 office(统一入口),
   args 是 CLI 子命令与参数。故展示拆成三段——图标/类型名按 bin 取格式,
   摘要取 args 命令行,说明按子命令给固定说法。 */

/** 目标格式(归一别名后的四种；`bin=office` 时未知 → 通用 Office 图标/类型名) */
export type OfficeFormat = 'docx' | 'xlsx' | 'pptx' | 'pdf'

/** `bin` → 格式(word/excel/ppt 与 docx/xlsx/pptx 等价;office/未知 → null) */
function officeFormat(params: unknown): OfficeFormat | null {
  switch (strOf(params, 'bin').trim().toLowerCase()) {
    case 'docx':
    case 'word':
      return 'docx'
    case 'xlsx':
    case 'excel':
      return 'xlsx'
    case 'pptx':
    case 'ppt':
      return 'pptx'
    case 'pdf':
      return 'pdf'
    default:
      return null
  }
}

/** 格式 → 文件树同款品牌字形(assets/file-icons/) */
const OFFICE_GLYPH: Record<OfficeFormat, string> = {
  docx: 'word',
  xlsx: 'excel',
  pptx: 'powerpoint',
  pdf: 'pdf'
}

/** 格式 → 类型名 i18n 键 */
const OFFICE_LABEL_KEY: Record<OfficeFormat, string> = {
  docx: 'chat.step.officeDocx',
  xlsx: 'chat.step.officeXlsx',
  pptx: 'chat.step.officePptx',
  pdf: 'chat.step.officePdf'
}

/**
 * `office` 子命令 → 执行说明 i18n 键(`chat.step.officeCmd.*`)。
 * 五 CLI 命令面一致(unpack/repack/view/edit/render/validate/dump/batch/schema/help/
 * templates/convert 等),PDF 另有 pages/meta/form/extract/qa/palette/design 等子命令,
 * 统一入口另有 formats/batch(见内置技能分册);表外的子命令不给说明(摘要里已有原命令行)。
 */
const OFFICE_CMD_KEY: Record<string, string> = {
  view: 'view',
  unpack: 'unpack',
  repack: 'repack',
  render: 'render',
  validate: 'validate',
  convert: 'convert',
  extract: 'extract',
  dump: 'dump',
  batch: 'batch',
  query: 'query',
  merge: 'merge',
  import: 'import',
  raw: 'raw',
  'raw-set': 'rawSet',
  templates: 'templates',
  schema: 'schema',
  help: 'help',
  meta: 'meta',
  pages: 'pages',
  form: 'form',
  qa: 'qa',
  env: 'env',
  formats: 'formats',
  palette: 'palette',
  design: 'design',
  check: 'check',
  code: 'code',
  content: 'content'
}

/** 统一入口的格式别名（`astorm-office <格式> <命令...>`；说明取格式后面的子命令） */
const OFFICE_FORMAT_ALIASES = new Set([
  'word', 'docx', 'doc', 'docs', 'excel', 'xlsx', 'xls', 'sheet',
  'ppt', 'pptx', 'slides', 'presentation', 'pdf', 'astorm-office', 'office', 'all'
])

/** 格式别名 → 归一格式（非格式别名 → null） */
function officeFormatAlias(token: string): OfficeFormat | null {
  switch (token) {
    case 'word':
    case 'docx':
    case 'doc':
    case 'docs':
    case 'json2docx':
      return 'docx'
    case 'excel':
    case 'xlsx':
    case 'xls':
    case 'sheet':
    case 'json2xlsx':
      return 'xlsx'
    case 'ppt':
    case 'pptx':
    case 'slides':
    case 'presentation':
    case 'json2pptx':
      return 'pptx'
    case 'pdf':
    case 'json2pdf':
      return 'pdf'
    default:
      return null
  }
}

/** `office` 工具参数里的 args（统一小写，便于比较） */
function officeArgsOf(params: unknown): string[] {
  const raw = (params as Record<string, unknown> | null | undefined)?.['args']
  return Array.isArray(raw) ? raw.map((x) => String(x).trim().toLowerCase()) : []
}

/**
 * 定位真正执行的 CLI 子命令下标：跳过全局旗标与统一入口的格式前缀
 * （与后端 `office.rs::command_pos` 同口径；旗标与格式别名可任意先后）。
 */
function officeCommandIndex(args: string[]): number {
  let i = 0
  let skippedFormat = false
  while (i < args.length) {
    const a = args[i]
    if (a === '-f' || a === '--format' || a === '-o' || a === '--output') {
      i += 2
      continue
    }
    if (
      a === '--json' ||
      a === '--quiet' ||
      a === '--verbose' ||
      a.startsWith('--format=') ||
      a.startsWith('--output=')
    ) {
      i += 1
      continue
    }
    if (!skippedFormat && OFFICE_FORMAT_ALIASES.has(a)) {
      skippedFormat = true
      i += 1
      continue
    }
    break
  }
  return i
}

/**
 * 统一入口（`bin=office`）的格式推断：显式 `-f/--format` > 格式别名 > 文件扩展名。
 * `batch` / `formats` 这类跨格式命令返回 null（过程行用批处理/通用关键词）。
 */
function officeFormatFromArgs(params: unknown): OfficeFormat | null {
  const args = officeArgsOf(params)
  const pos = officeCommandIndex(args)
  const cmd = args[pos] ?? ''
  if (cmd === 'batch' || cmd === 'formats') return null
  // 子命令前的 token：-f 的值或格式别名（如 word --json view …）
  for (const token of args.slice(0, pos)) {
    const fmt = officeFormatAlias(token) ?? officeFormatAlias(token.replace(/^--format=/, ''))
    if (fmt) return fmt
  }
  // 子命令本身也可能是格式别名（不会：commandIndex 已跳过），再看路径扩展名
  for (const a of args) {
    if (/\.(docx?|docm|rtf|odt)$/.test(a)) return 'docx'
    if (/\.(xlsx?|xlsm|csv|ods)$/.test(a)) return 'xlsx'
    if (/\.(pptx?|pptm|ppsx|odp)$/.test(a)) return 'pptx'
    if (/\.pdf$/.test(a)) return 'pdf'
  }
  return null
}

/** `office` 调用的执行说明(子命令 → 固定说法;`edit … get` 是只读,单独一条) */
function officeCommandText(params: unknown): string {
  const args = officeArgsOf(params)
  const i = officeCommandIndex(args)
  const first = args[i] ?? ''
  // `edit <输入> <路径> get`：get 在路径之后
  const key =
    first === 'edit'
      ? args.slice(i + 2).includes('get')
        ? 'editGet'
        : 'edit'
      : (OFFICE_CMD_KEY[first] ?? '')
  return key ? i18n.global.t(`chat.step.officeCmd.${key}`) : ''
}

/**
 * 行「类型名 + 图标」:返回 i18n key(由调用方 `t(...)` 解析)与图标名。
 * 与各工具 schema 的动作字段一一对应。
 *
 * `iconUrl`(可选):品牌彩色图标(文件树同款字形),与 `icon` 二者取一——
 * 目前只有 `office` 用它(按 `bin` 取 Word/Excel/PPT/PDF 图标),其余行走 `icon` 组件名。
 */
export function toolMeta(name: string, params: unknown): {
  icon: string
  iconUrl?: string
  labelKey: string
} {
  const action = actionOf(name, params)
  switch (name) {
    case 'bash':
      return { icon: 'icon-shell', labelKey: 'chat.step.bash' }
    case 'office': {
      // 格式优先取 bin；bin=office（统一入口）再从 args 的格式别名/文件扩展名推断
      const fmt = officeFormat(params) ?? officeFormatFromArgs(params)
      if (fmt) {
        return {
          icon: 'icon-file-text',
          iconUrl: fileGlyphUrl(OFFICE_GLYPH[fmt]),
          labelKey: OFFICE_LABEL_KEY[fmt]
        }
      }
      // 跨格式批处理单独给关键词；其余统一入口仍显示「Office 文档」
      const args = officeArgsOf(params)
      if (args[officeCommandIndex(args)] === 'batch') {
        return { icon: 'icon-layers', labelKey: 'chat.step.officeBatch' }
      }
      return { icon: 'icon-file-text', labelKey: 'chat.step.office' }
    }
    case 'file_operation':
      switch (action) {
        case 'write':
        case 'append':
          return { icon: 'icon-edit', labelKey: 'chat.step.write' }
        case 'edit':
          return { icon: 'icon-edit', labelKey: 'chat.step.edit' }
        case 'list':
          return { icon: 'icon-folder', labelKey: 'chat.step.list' }
        case 'glob':
          return { icon: 'icon-search', labelKey: 'chat.step.glob' }
        case 'grep':
          return { icon: 'icon-search', labelKey: 'chat.step.grep' }
        default:
          return { icon: 'icon-file-text', labelKey: 'chat.step.read' }
      }
    case 'read_images':
      return { icon: 'icon-media', labelKey: 'chat.step.readImage' }
    case 'browser':
      return { icon: 'icon-globe', labelKey: 'chat.step.browser' }
    case 'search':
      switch (searchSub(params)) {
        case 'webfetch':
          return { icon: 'icon-globe', labelKey: 'chat.step.webFetch' }
        case 'github':
          return { icon: 'icon-github', labelKey: 'chat.step.github' }
        case 'arxiv':
          return { icon: 'icon-book', labelKey: 'chat.step.arxiv' }
        case 'history':
          return { icon: 'icon-history', labelKey: 'chat.step.history' }
        default:
          return { icon: 'icon-search', labelKey: 'chat.step.search' }
      }
    case 'sub_session':
      return { icon: 'icon-robot', labelKey: 'chat.step.subSession' }
    case 'skills':
      return { icon: 'icon-sparkles', labelKey: 'chat.step.skills' }
    case 'tabs':
      return { icon: 'icon-deck', labelKey: 'chat.step.tabs' }
    case 'schedule_task':
      return { icon: 'icon-schedule', labelKey: 'chat.step.schedule' }
    case 'task_list':
      return { icon: 'icon-clipboard-list', labelKey: 'chat.step.taskList' }
    case 'chat_with_user':
      return { icon: 'icon-message', labelKey: 'chat.step.ask' }
    case 'memory':
      return { icon: 'icon-memory', labelKey: 'chat.step.memory' }
    case 'submit_deliverables':
      return { icon: 'icon-box', labelKey: 'chat.step.submitDeliverables' }
    case 'tool_load':
      return { icon: 'icon-launch', labelKey: 'chat.step.toolLoad' }
    default:
      return { icon: 'icon-tool', labelKey: 'chat.step.tool' }
  }
}

/** `submit_deliverables` 参数清单计数（交付物 / 修改文件 / 相关记忆 id） */
export interface DeliveryStat {
  deliverables: number
  changedFiles: number
  memoryIds: number
}

/** 取参数里数组字段的长度（缺省 / 非数组计 0） */
function countOf(params: unknown, key: string): number {
  const v = (params as Record<string, unknown> | null | undefined)?.[key]
  return Array.isArray(v) ? v.length : 0
}

/**
 * `submit_deliverables` 的清单计数（其他工具返回 `null`）。
 * 「相关资料」不在工具参数里（网页资料是 search 工具的检索结果，本轮汇总由
 * `collectTurnSources` 统计），关键词行由调用方补充该段。
 */
export function deliveryStatOf(name: string, params: unknown): DeliveryStat | null {
  if (name !== 'submit_deliverables') return null
  return {
    deliverables: countOf(params, 'deliverables'),
    changedFiles: countOf(params, 'changed_files'),
    memoryIds: countOf(params, 'memory_ids')
  }
}

/**
 * 行摘要(title):按各工具 schema 显式取字段,尽量取最有信息量的一项。
 * 与 `toolMeta` 同源,字段名严格对齐后端 schema。
 */
export function toolTitle(name: string, params: unknown): string {
  const action = actionOf(name, params)
  switch (name) {
    case 'bash':
      return firstOf(params, ['command'])
    case 'file_operation':
      switch (action) {
        case 'grep':
          return firstOf(params, ['pattern'])
        case 'glob':
          return firstOf(params, ['glob_pattern'])
        default:
          return firstOf(params, ['path'])
      }
    case 'read_images':
      return [firstOf(params, ['images']), firstOf(params, ['detail'])].filter(Boolean).join(' · ')
    case 'chat_with_user':
      // 提问文本即 description（同一字段），不再单列摘要，避免重复
      return ''
    case 'sub_session':
      return firstOf(params, ['task', 'agent_type'])
    case 'skills':
      return [action, firstOf(params, ['skills', 'query', 'path'])].filter(Boolean).join(' · ')
    case 'search':
      return firstOf(params, ['query', 'url'])
    case 'schedule_task':
      return firstOf(params, ['name', 'query']) || action
    case 'task_list':
      return firstOf(params, ['title', 'task_id']) || action
    case 'tabs': {
      const idx = (params as Record<string, unknown> | null)?.['index']
      return (
        firstOf(params, ['url', 'path', 'id']) || (idx != null ? String(idx) : '') || action
      )
    }
    case 'memory':
      return firstOf(params, ['query', 'concept_id']) || action
    case 'office':
      // 摘要 = CLI 子命令行(与 bash 行展示命令同口径);args 缺失时退到格式名
      return firstOf(params, ['args'], ' ') || strOf(params, 'bin')
    case 'tool_load':
      // 摘要 = 本次加载 / 移除的工具名列表（status 无 tools → 动作名）
      return firstOf(params, ['tools']) || action
    case 'browser':
      return action
    default:
      return (
        firstOf(params, ['path', 'url', 'query', 'command', 'task', 'name', 'title', 'action']) ||
        firstAnyString(params)
      )
  }
}

/**
 * 调用说明:工具 schema 的 `description`(模型填写的本次调用目的),过程行灰色小字展示。
 *
 * `schedule_task` 的 `description` 兼是**任务描述**功能字段(add/update 写入任务、用于
 * 列表展示与搜索),不是调用说明 → 过程行不渲染,避免与任务名重复。
 *
 * `office` 的 `description` 是可选(模型可不填) → 缺省回落到按子命令的固定说法,
 * 保证过程行永远有一句「这次在干什么」。
 */
const DESCRIPTION_SUPPRESSED = new Set(['schedule_task'])

export function toolDescription(name: string, params: unknown): string {
  if (DESCRIPTION_SUPPRESSED.has(name)) return ''
  const filled = strOf(params, 'description')
  if (name === 'office') return filled || officeCommandText(params)
  return filled
}

/** 过程摘要类别(供 ChatTurnGroup 的前 3 类统计) */
export function toolKind(name: string, params: unknown): ToolActionKind {
  const action = actionOf(name, params)
  switch (name) {
    case 'bash':
      return 'commands'
    case 'file_operation':
      if (action === 'grep' || action === 'glob') return 'search'
      if (action === 'write' || action === 'append') return 'write'
      if (action === 'edit') return 'edit'
      return 'read'
    case 'read_images':
      return 'readImage'
    case 'browser':
      return 'webFetch'
    case 'search':
      switch (searchSub(params)) {
        case 'websearch':
          return 'webSearch'
        case 'webfetch':
          return 'webFetch'
        default:
          return 'tools'
      }
    case 'sub_session':
      return 'subagents'
    case 'task_list':
      return 'plan'
    case 'chat_with_user':
      return 'questions'
    default:
      return 'tools'
  }
}
