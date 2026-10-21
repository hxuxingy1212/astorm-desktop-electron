<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import MentionPopover from './MentionPopover.vue'
import {
  commitChip,
  createElementChip,
  createOfficeRefChip,
  createQuoteChip,
  getCaretToken,
  renderTextToEditor,
  serializeEditor
} from './chip'
import { hasProvider, queryMentions } from './registry'
import { rankItems } from './match'
import { bumpUsage, getUsage } from './usage'
import { TRIGGER_BY_TYPE } from './types'
import type { CollectedChipRefs, MentionGroup, MentionItem } from './types'

/**
 * 提及编辑器:contenteditable 富文本,把 '/'、'$'、'@' 引用的条目渲染为
 * 彩色 chip(真实 DOM,可整体选中/删除)。弹层固定贴输入栏上方,分组滚动列表。
 *
 * 对外接口:
 *   v-model 序列化纯文本(chip → token,如 '/goal'),供父组件判断能否发送
 *   @submit(text) 按 Enter(弹层关闭时)提交的纯文本
 *   @focus / @blur 转发,父组件用于聚焦态
 */
const props = defineProps<{
  /** 占位文案(空内容时显示) */
  placeholder?: string
  /** 流式输出期间禁用编辑 */
  disabled?: boolean
  /** 当前会话工作目录(绝对路径,@ 文件引用的数据源;缺省时弹层提示先选目录) */
  sessionDir?: string
  /**
   * 选中条目时是否插入 chip（缺省 true）。返回 false 表示由父组件接管该条目
   * （如技能改由输入框上方胶囊承载），编辑器只清除已输入的 token 文本，不插 chip。
   */
  shouldCommit?: (item: MentionItem) => boolean
}>()

const model = defineModel<string>({ default: '' })
const emit = defineEmits<{
  (e: 'submit', text: string): void
  (e: 'focus'): void
  (e: 'blur'): void
  /** 选中某个条目（无论是否插入 chip 都会发出，供父组件接管技能等） */
  (e: 'pick', item: MentionItem): void
  /** 编辑器内容变化（供父组件对账元素 chip 增删等） */
  (e: 'change'): void
}>()

const { t } = useI18n()

const editorRef = ref<HTMLDivElement | null>(null)
const isComposing = ref(false)

// ---------- 弹层状态 ----------
const trigger = ref<'' | '/' | '$' | '@'>('')
const query = ref('')
const groups = ref<MentionGroup[]>([])
const activeIndex = ref(0)
const anchor = ref<{ left: number; bottom: number; width: number; dialogH: number } | null>(null)
const popupOpen = computed(() => trigger.value !== '' && anchor.value !== null)

const flatItems = computed(() => groups.value.flatMap((g) => g.items))
const totalItems = computed(() => flatItems.value.length)

/** 弹层单次展示上限:大工作区过滤命中可能上千条,截断避免无用 DOM 与滚动开销 */
const MAX_RESULTS = 50

/** 底部提示行:仅 '@' 文件引用需要(输入以搜索) */
const hint = computed(() => (trigger.value === '@' ? t('mentions.fileSearchHint') : ''))

/** 空态文案:'@' 且无工作目录时提示先选目录,其余沿用「无匹配结果」 */
const emptyText = computed(() =>
  trigger.value === '@' && !props.sessionDir?.trim() ? t('mentions.noWorkspace') : t('mentions.empty')
)

function itemLabel(i: MentionItem): string {
  return i.labelKey ? t(i.labelKey) : (i.label ?? i.id)
}

function itemDesc(i: MentionItem): string {
  return i.descriptionKey ? t(i.descriptionKey) : (i.description ?? '')
}

/** token 反查使用频率(排序加权用) */
function usageOf(item: MentionItem): number {
  return getUsage(TRIGGER_BY_TYPE[item.type] + item.id)
}

/** 组内按匹配分 + 使用频率排序过滤;@ 文件列表额外文件夹优先 */
function filterGroups(all: MentionGroup[], q: string): MentionGroup[] {
  let budget = MAX_RESULTS
  const out: MentionGroup[] = []
  for (const g of all) {
    if (budget <= 0) break
    const ranked = rankItems(g.items, {
      query: q,
      textsOf: (i) => [i.id, itemLabel(i), itemDesc(i), ...(i.keywords ?? [])],
      usageOf,
      foldersFirst: trigger.value === '@',
      isFolder: (i) => i.type === 'file' && i.id.endsWith('/')
    })
    const items = ranked.slice(0, budget)
    if (!items.length) continue
    budget -= items.length
    out.push({ ...g, items })
  }
  return out
}

// ---------- 触发 / 关闭 ----------

/** 请求序号:文件列表走异步 IPC,快速输入时丢弃过期结果,避免旧响应覆盖新查询 */
let refreshSeq = 0

async function refresh(): Promise<void> {
  if (!trigger.value) return
  const seq = ++refreshSeq
  const q = query.value.trim()
  const all = await queryMentions(trigger.value, q, { query: q, sessionDir: props.sessionDir })
  if (seq !== refreshSeq) return
  groups.value = filterGroups(all, q)
  activeIndex.value = 0
}

function closePopup(): void {
  refreshSeq++
  trigger.value = ''
  query.value = ''
  groups.value = []
  anchor.value = null
}

/** 弹层贴在输入栏上方:bottom 距离 = 视口高度 - 输入栏顶(再加 8px 间距);
    同时记录对话框高度,弹层列表最大高度 = 2.5 × 对话框高度 */
function reposition(): void {
  const el = editorRef.value
  if (!el) return
  const r = el.getBoundingClientRect()
  const dialog = el.closest<HTMLElement>('.chat-input-pill') ?? el
  anchor.value = {
    left: r.left,
    width: r.width,
    bottom: window.innerHeight - r.top + 8,
    dialogH: dialog.getBoundingClientRect().height
  }
}

/** 每次输入/光标移动后检测光标前的 token,决定弹层开/关与过滤词(组合期不检测) */
function updatePopup(): void {
  if (isComposing.value) return
  const el = editorRef.value
  if (!el) return
  const token = getCaretToken(el)
  if (!token || !hasProvider(token.trigger)) {
    closePopup()
    return
  }
  const changed = token.trigger !== trigger.value || token.query !== query.value
  trigger.value = token.trigger
  query.value = token.query
  reposition()
  if (changed) void refresh()
}

// ---------- 选中 / 提交 chip ----------

function moveActive(delta: number): void {
  const n = totalItems.value
  if (!n) return
  activeIndex.value = (activeIndex.value + delta + n) % n
}

function commitActive(): void {
  const item = flatItems.value[activeIndex.value]
  if (item) doCommit(item)
}

function onHover(index: number): void {
  activeIndex.value = index
}

function onPick(item: MentionItem): void {
  doCommit(item)
}

function doCommit(item: MentionItem): void {
  // 骨架提示条目(以 __ 开头)不可选中
  if (item.id.startsWith('__')) return
  emit('pick', item)
  const insertChip = props.shouldCommit ? props.shouldCommit(item) : true
  commitChip(item, itemLabel(item), insertChip)
  bumpUsage(TRIGGER_BY_TYPE[item.type] + item.id)
  syncModel()
  closePopup()
}

// ---------- 序列化 ----------

function syncModel(): void {
  if (!editorRef.value) return
  model.value = serializeEditor(editorRef.value)
}

// ---------- 编辑器事件 ----------

function onEditorInput(): void {
  if (props.disabled) return
  syncModel()
  updatePopup()
  emit('change')
}

function onKeyup(e: KeyboardEvent): void {
  // 方向键/首尾键移动光标后 token 可能失效,重新检测(其余字符走 input 事件)
  if (e.key === 'ArrowLeft' || e.key === 'ArrowRight' || e.key === 'Home' || e.key === 'End') updatePopup()
}

function onKeydown(e: KeyboardEvent): void {
  if (props.disabled) return
  if (isComposing.value || e.keyCode === 229) return

  if (popupOpen.value) {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      moveActive(1)
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      moveActive(-1)
      return
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      commitActive()
      return
    }
    if (e.key === 'Escape') {
      e.preventDefault()
      closePopup()
      return
    }
    return // 其余按键保持默认,由 input 事件重新检测
  }

  if (e.key === 'Enter' && !e.shiftKey) {
    // Enter 提交(空内容不提交,停止生成仍走父组件按钮)
    e.preventDefault()
    const el = editorRef.value
    if (!el) return
    const text = serializeEditor(el)
    if (!text) return
    model.value = text
    emit('submit', text)
    el.innerHTML = ''
    model.value = ''
  }
}

function onCompositionStart(): void {
  isComposing.value = true
}

function onCompositionEnd(): void {
  isComposing.value = false
  updatePopup()
}

/** 粘贴一律转纯文本,避免富文本污染消息内容 */
function onPaste(e: ClipboardEvent): void {
  if (props.disabled) return
  e.preventDefault()
  const text = e.clipboardData?.getData('text/plain') ?? ''
  if (!text) return
  document.execCommand('insertText', false, text)
  updatePopup()
}

function onFocus(): void {
  emit('focus')
}

function onBlur(): void {
  emit('blur')
}

// ---------- 全局监听(弹层打开期间)----------

function onDocMouseDown(e: MouseEvent): void {
  const target = e.target as Node | null
  if (!target) return
  if (editorRef.value?.contains(target)) return
  if ((target as Element).closest?.('.mention-popover')) return
  closePopup()
}

watch(popupOpen, (v) => {
  if (v) {
    document.addEventListener('mousedown', onDocMouseDown, true)
    window.addEventListener('scroll', reposition, true)
    window.addEventListener('resize', reposition)
  } else {
    document.removeEventListener('mousedown', onDocMouseDown, true)
    window.removeEventListener('scroll', reposition, true)
    window.removeEventListener('resize', reposition)
  }
})

// 外部清空(如点击发送按钮)时同步清空编辑器 DOM
watch(model, (v) => {
  if (!v && editorRef.value && editorRef.value.innerHTML) editorRef.value.innerHTML = ''
})

// 会话切换/工作目录解析完成后,若弹层正开在 '@' 上则重新拉取文件列表
watch(
  () => props.sessionDir,
  () => {
    if (trigger.value === '@') void refresh()
  }
)

/**
 * 外部回显(编辑消息队列条目/插件注入):把纯文本写入编辑器并**重建 chip**,
 * 聚焦并把光标置于末尾。`/`/`$` 经注册表、`@` 经启发式还原为 chip,再次发送按 token 序列化。
 */
function setContent(text: string): void {
  const el = editorRef.value
  if (!el) return
  renderTextToEditor(el, text, itemLabel)
  model.value = serializeEditor(el)
  el.focus()
  const range = document.createRange()
  range.selectNodeContents(el)
  range.collapse(false)
  const sel = window.getSelection()
  sel?.removeAllRanges()
  sel?.addRange(range)
}

/** 编辑器中元素 chip 的 key 列表(供父组件与 store 对账增删) */
function elementChipKeys(): number[] {
  const el = editorRef.value
  if (!el) return []
  return [...el.querySelectorAll<HTMLElement>('.mention-chip[data-type="element"]')]
    .map((n) => Number(n.dataset.elementKey))
    .filter((n) => Number.isFinite(n))
}

/** 编辑器中对话引用 chip 的 key 列表(供父组件与 store 对账增删) */
function quoteChipKeys(): number[] {
  const el = editorRef.value
  if (!el) return []
  return [...el.querySelectorAll<HTMLElement>('.mention-chip[data-type="quote"]')]
    .map((n) => Number(n.dataset.quoteKey))
    .filter((n) => Number.isFinite(n))
}

/** 编辑器中 office 路径 chip 的 key 列表(供父组件与 store 对账增删) */
function officeChipKeys(): number[] {
  const el = editorRef.value
  if (!el) return []
  return [...el.querySelectorAll<HTMLElement>('.mention-chip[data-type="office"]')]
    .map((n) => Number(n.dataset.officeKey))
    .filter((n) => Number.isFinite(n))
}

/**
 * 增删同步顶层「office 路径」chip:全量重建(与 elementPick 同一手法,保证数量/顺序严格一致),
 * chip 追加在末尾、自成一段,不打断用户正文。由父组件在 store 变化时调用。
 */
function syncOfficeChips(items: { key: number; token: string; label: string }[]): void {
  const el = editorRef.value
  if (!el) return
  el.querySelectorAll<HTMLElement>('.mention-chip[data-type="office"]').forEach((n) => n.remove())
  items.forEach((it, i) => {
    const label = `${i + 1}. ${it.label}`
    el.appendChild(createOfficeRefChip(it.token, label, it.key))
  })
  model.value = serializeEditor(el)
}

/** 全量重建对话引用 chip(与 elementPick 同一手法):先清空,再按 store 顺序补回 */
function syncQuoteChips(items: { key: number; label: string }[]): void {
  const el = editorRef.value
  if (!el) return
  el.querySelectorAll<HTMLElement>('.mention-chip[data-type="quote"]').forEach((n) => n.remove())
  items.forEach((it) => el.appendChild(createQuoteChip(it.label, it.key)))
  model.value = serializeEditor(el)
}

/**
 * 增删同步顶层「网页元素」chip:补上缺失的(追加到末尾、自成一段)、删掉多余的;
 * 不打断已有文本。由父组件在 store 变化时调用。
 */
function syncElementChips(items: { key: number; token: string; label: string }[]): void {
  const el = editorRef.value
  if (!el) return
  // 全量重建:先清掉编辑器里所有元素 chip,再按 store 顺序逐条补回。
  // 采用「重建」而非「增量对账」——增量依赖 chip 的 key 唯一,一旦 key 重复/ DOM 漂移
  // 就会出现「胶囊 N 个、输入框却只有 M 个」。全量重建保证二者数量与顺序**严格一致**。
  // 元素 chip 始终追加在末尾(见 createElementChip 的使用约定),重建不打断用户正文。
  el.querySelectorAll<HTMLElement>('.mention-chip[data-type="element"]').forEach((n) => n.remove())
  items.forEach((it, i) => {
    const label = `${i + 1}. ${it.label}`
    el.appendChild(createElementChip(it.token, label, it.key))
  })
  model.value = serializeEditor(el)
}

/**
 * 收集编辑器内 chip 的机器可读引用(文件/智能体)与元素围栏原文:
 * 内容正文保留「原文」,这些结构作为请求元数据下发,供后端生成/重建「引用说明」。
 */
function collectMentions(): CollectedChipRefs {
  const el = editorRef.value
  const out: CollectedChipRefs = { mentions: [], elements: [], officeRefs: [] }
  if (!el) return out
  el.querySelectorAll<HTMLElement>('.mention-chip').forEach((n) => {
    const type = n.dataset.type
    const label = n.querySelector('.mention-chip-label')?.textContent ?? ''
    if (type === 'file') {
      out.mentions.push({ kind: 'file', id: n.dataset.id ?? '', token: n.dataset.token ?? '', label })
    } else if (type === 'agent') {
      const bare = (n.dataset.id ?? '').replace(/^agent\//, '')
      out.mentions.push({ kind: 'agent', id: bare, token: n.dataset.token ?? '', label })
    } else if (type === 'element' && n.dataset.fence) {
      out.elements.push(n.dataset.fence)
    } else if (type === 'office' && n.dataset.fence) {
      out.officeRefs.push(n.dataset.fence)
    }
  })
  return out
}

defineExpose({
  setContent,
  elementChipKeys,
  syncElementChips,
  quoteChipKeys,
  syncQuoteChips,
  officeChipKeys,
  syncOfficeChips,
  collectMentions
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocMouseDown, true)
  window.removeEventListener('scroll', reposition, true)
  window.removeEventListener('resize', reposition)
})
</script>

<template>
  <div
    ref="editorRef"
    class="mention-editor"
    :class="disabled && 'mention-editor-disabled'"
    :contenteditable="disabled ? 'false' : 'true'"
    :data-placeholder="placeholder ?? ''"
    spellcheck="false"
    @input="onEditorInput"
    @keydown="onKeydown"
    @keyup="onKeyup"
    @compositionstart="onCompositionStart"
    @compositionend="onCompositionEnd"
    @paste="onPaste"
    @focus="onFocus"
    @blur="onBlur"
  ></div>

  <MentionPopover
    :open="popupOpen"
    :groups="groups"
    :active-index="activeIndex"
    :anchor="anchor"
    :query="query"
    :hint="hint"
    :empty-text="emptyText"
    @select="onPick"
    @hover="onHover"
  />
</template>

<style scoped>
/* 编辑器:与旧 textarea 视觉一致(同字号/行高/内边距),最低高度容纳 2 行文字,
   内容超出后自动增高,过高时内部滚动 */
.mention-editor {
  box-sizing: border-box;
  min-height: 62px;
  max-height: 40vh;
  overflow-y: auto;
  padding: 10px 20px 4px;
  font-size: var(--font-size-title);
  line-height: 1.6;
  color: var(--text-primary);
  overflow-wrap: break-word;
  word-break: break-word;
  outline: none;
  cursor: text;
  user-select: text;
}

.mention-editor:empty::before {
  content: attr(data-placeholder);
  color: var(--text-tertiary);
  pointer-events: none;
}

.mention-editor-disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>

<style>
/* 输入框内 chip(原生 DOM 构造,scoped 选择器覆盖不到,放全局):
   统一为「图标 + 标签文字」的轻量内联样式(与文件引用一致):无胶囊底/边框,
   字号行高继承正文(不撑高光标行),悬浮淡底提示可整体删除。图标与文字取 --chip-color。 */
/* 用纯 inline(而非 inline-flex):chip 的基线=标签文字基线、行高随正文,
   图标用 inline-block + 负 vertical-align 落在文本行内 —— 不会撑高行高/光标。 */
.mention-chip {
  display: inline;
  vertical-align: baseline;
  margin: 0 1px;
  padding: 0 3px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--chip-color);
  font-size: inherit;
  line-height: inherit;
  white-space: nowrap;
  transition: background-color 0.12s ease;
}

.mention-chip:hover {
  background: color-mix(in srgb, var(--chip-color) 12%, transparent);
}

/* 文件/文件夹图标(手写 SVG 字形,自带彩色) */
.mention-chip-glyph {
  display: inline-block;
  width: 1em;
  height: 1em;
  vertical-align: -0.125em;
  margin-right: 0.28em;
}

/* 命令/智能体的内联 SVG 图标 */
.mention-chip-icon {
  display: inline-block;
  width: 1em;
  height: 1em;
  vertical-align: -0.125em;
  margin-right: 0.28em;
  color: var(--chip-color);
}

.mention-chip-dot {
  display: inline-block;
  width: 0.5em;
  height: 0.5em;
  border-radius: 50%;
  background: var(--chip-color);
  vertical-align: -0.05em;
  margin-right: 0.3em;
}

.mention-chip-label {
  user-select: none;
  color: var(--chip-color);
}
</style>