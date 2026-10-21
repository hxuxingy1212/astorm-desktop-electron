<script setup lang="ts">
/**
 * 轻量富文本编辑器（邮件正文用）。
 *
 * 基于 contenteditable + document.execCommand（Electron/Chromium 下稳定）实现，
 * 不引入额外依赖。对外只暴露 v-model:html（HTML 字符串）；工具条按钮覆盖
 * 撤销/重做、字体/字号、粗斜下划线删除线、字体色/高亮色、列表/缩进、行高、
 * 对齐、引用、行内代码——即参考图的邮件工具条。
 *
 * 工具条图标用内联 SVG（应用图标集未收录这些字形，且它们高度专用），
 * 统一 48 网格 / stroke currentColor / 圆角，与本应用手写图标同一造型语言。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{ html: string; placeholder?: string; minHeight?: number }>()
const emit = defineEmits<{ (e: 'update:html', v: string): void }>()

const { t } = useI18n()

const root = ref<HTMLElement | null>(null)
const focused = ref(false)
/** 对齐下拉展开态 */
const alignOpen = ref(false)
const ddEl = ref<HTMLElement | null>(null)

/** 格式刷：记录一份「格式快照」，下一次选区释放时套用 */
interface FormatPainter {
  bold: boolean
  italic: boolean
  underline: boolean
  strike: boolean
  foreColor: string
  fontName: string
  fontSize: string
}
const painterActive = ref(false)
let painterSnapshot: FormatPainter | null = null

/** 颜色按钮显示的下划线色条（取最近一次选择，默认红/黄，与参考图一致） */
const textColor = ref('#f53f3f')
const highlightColor = ref('#ffd534')

/** 插入图片时的加载态 */
const imageLoading = ref(false)

/** 点空白处收起对齐下拉 */
function onDocPointerDown(e: PointerEvent): void {
  if (alignOpen.value && ddEl.value && !ddEl.value.contains(e.target as Node)) {
    alignOpen.value = false
  }
}
onMounted(() => document.addEventListener('pointerdown', onDocPointerDown, true))
onBeforeUnmount(() => document.removeEventListener('pointerdown', onDocPointerDown, true))

/**
 * 字体候选：label 为可读字体名（与参考图一致），value 是写进正文的 CSS font-family 栈。
 * 中文栈把 Windows / macOS 常见字形都列上，跨平台尽量命中；选项本身用各自字体预览。
 */
const FONT_FAMILIES = [
  { label: '宋体', value: 'SimSun, "Songti SC", "STSong", serif' },
  { label: '黑体', value: '"Microsoft YaHei", "PingFang SC", "Heiti SC", sans-serif' },
  { label: '楷体', value: 'KaiTi, "Kaiti SC", "STKaiti", serif' },
  { label: '幼圆', value: 'YouYuan, "Yuanti SC", "STYuanti", sans-serif' },
  { label: 'Arial', value: 'Arial, Helvetica, sans-serif' },
  { label: 'Arial Black', value: '"Arial Black", "Arial Bold", sans-serif' },
  { label: 'Times New Roman', value: '"Times New Roman", Times, serif' },
  { label: 'Verdana', value: 'Verdana, Geneva, sans-serif' }
]
const FONT_SIZES = [12, 14, 16, 18, 20, 24, 28, 32]
const LINE_HEIGHTS = ['1.0', '1.15', '1.5', '1.75', '2.0']

/** 对齐选项：execCommand + 菜单项图标（内联 SVG，48 网格语义） */
const ALIGNS = [
  {
    cmd: 'justifyLeft',
    label: 'mail.alignLeft',
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="4" y1="6" x2="20" y2="6"/><line x1="4" y1="12" x2="14" y2="12"/><line x1="4" y1="18" x2="18" y2="18"/></svg>'
  },
  {
    cmd: 'justifyCenter',
    label: 'mail.alignCenter',
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="4" y1="6" x2="20" y2="6"/><line x1="7" y1="12" x2="17" y2="12"/><line x1="5" y1="18" x2="19" y2="18"/></svg>'
  },
  {
    cmd: 'justifyRight',
    label: 'mail.alignRight',
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="4" y1="6" x2="20" y2="6"/><line x1="10" y1="12" x2="20" y2="12"/><line x1="6" y1="18" x2="20" y2="18"/></svg>'
  },
  {
    cmd: 'justifyFull',
    label: 'mail.alignJustify',
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="4" y1="6" x2="20" y2="6"/><line x1="4" y1="12" x2="20" y2="12"/><line x1="4" y1="18" x2="20" y2="18"/></svg>'
  }
]

/** 初始化回填 DOM 时抑制一次 input 事件，避免光标丢失 */
let suppress = false

function syncFromProps(): void {
  const el = root.value
  if (!el) return
  if (el.innerHTML !== props.html) {
    suppress = true
    el.innerHTML = props.html || ''
    suppress = false
  }
}

onMounted(syncFromProps)
watch(
  () => props.html,
  () => {
    if (!focused.value) syncFromProps()
  }
)

function emitChange(): void {
  if (suppress) return
  emit('update:html', root.value?.innerHTML ?? '')
}

function focusEditor(): void {
  root.value?.focus()
}

/** 执行命令并刷新内容；value 传第三参（如 foreColor / formatBlock） */
function cmd(command: string, value?: string): void {
  focusEditor()
  document.execCommand(command, false, value)
  emitChange()
}

function setFontFamily(e: Event): void {
  const v = (e.target as HTMLSelectElement).value
  // 用 span[style] 包裹（而非 execCommand('fontName') 产出的 <font>）：
  // 多字体栈能原样写进 font-family，且发出去的 HTML 更干净
  if (v) wrapSelection(`font-family:${v}`)
}

function setFontSize(e: Event): void {
  const px = Number((e.target as HTMLSelectElement).value)
  if (px) wrapSelection(`font-size:${px}px`)
  ;(e.target as HTMLSelectElement).selectedIndex = 0
}

function setLineHeight(e: Event): void {
  const v = (e.target as HTMLSelectElement).value
  if (v) wrapSelection(`line-height:${v}`)
  ;(e.target as HTMLSelectElement).selectedIndex = 0
}

/** 用当前选区文本包一个带内联样式的 span */
function wrapSelection(style: string): void {
  const sel = window.getSelection()
  if (!sel || sel.rangeCount === 0) return
  const range = sel.getRangeAt(0)
  if (range.collapsed) return
  const span = document.createElement('span')
  span.setAttribute('style', style)
  try {
    range.surroundContents(span)
  } catch {
    const frag = range.extractContents()
    span.appendChild(frag)
    range.insertNode(span)
  }
  sel.removeAllRanges()
  const r = document.createRange()
  r.selectNodeContents(span)
  sel.addRange(r)
  emitChange()
}

function align(command: string): void {
  cmd(command)
  alignOpen.value = false
}

/** 取当前光标/选区处的格式快照 */
function snapshotFormat(): FormatPainter {
  const q = (c: string): boolean => {
    try {
      return document.queryCommandState(c)
    } catch {
      return false
    }
  }
  const v = (c: string): string => {
    try {
      return document.queryCommandValue(c) || ''
    } catch {
      return ''
    }
  }
  return {
    bold: q('bold'),
    italic: q('italic'),
    underline: q('underline'),
    strike: q('strikeThrough'),
    foreColor: v('foreColor'),
    fontName: v('fontName'),
    fontSize: v('fontSize')
  }
}

/** 格式刷：点一次取样并激活，下一次选区高亮后套用（单击格式刷，用一次即失效） */
function togglePainter(): void {
  if (painterActive.value) {
    painterActive.value = false
    painterSnapshot = null
    return
  }
  focusEditor()
  painterSnapshot = snapshotFormat()
  painterActive.value = true
}

/** 把快照套用到当前选区 */
function applyPainter(): void {
  const s = painterSnapshot
  painterActive.value = false
  painterSnapshot = null
  // 注意：不重新 focus（mouseup 发生在已聚焦的编辑区内，重新 focus 可能清掉刚拖出的选区）
  if (!s) return
  // 用显式命令切换而非 toggle：确保结果与快照一致
  setToggle('bold', s.bold)
  setToggle('italic', s.italic)
  setToggle('underline', s.underline)
  setToggle('strikeThrough', s.strike)
  if (s.foreColor) document.execCommand('foreColor', false, s.foreColor)
  if (s.fontName) document.execCommand('fontName', false, s.fontName)
  if (s.fontSize) document.execCommand('fontSize', false, s.fontSize)
  emitChange()
}

/** 命令状态与目标不一致时才执行（execCommand 多为 toggle 语义） */
function setToggle(command: string, want: boolean): void {
  let cur = false
  try {
    cur = document.queryCommandState(command)
  } catch {
    cur = false
  }
  if (cur !== want) document.execCommand(command, false)
}

/** 鼠标在编辑区释放时，若格式刷激活则套用 */
function onMouseUp(): void {
  if (painterActive.value) applyPainter()
}

/** 图片扩展名 → MIME（读出的 base64 内联为 data URL，邮件里直接可见） */
function imageMime(path: string): string {
  const ext = path.toLowerCase().split('.').pop() ?? ''
  const map: Record<string, string> = {
    png: 'image/png',
    jpg: 'image/jpeg',
    jpeg: 'image/jpeg',
    gif: 'image/gif',
    webp: 'image/webp',
    bmp: 'image/bmp',
    svg: 'image/svg+xml',
    avif: 'image/avif'
  }
  return map[ext] ?? 'image/png'
}

/** 选本地图片 → base64 内联插入正文光标处 */
async function pickImage(): Promise<void> {
  const paths = await window.api.dialog.openFiles()
  const path = paths?.[0]
  if (!path) return
  imageLoading.value = true
  try {
    const b64 = await window.api.fs.readFileBase64(path)
    if (!b64) return
    focusEditor()
    document.execCommand('insertHTML', false, `<img src="data:${imageMime(path)};base64,${b64}" />`)
    emitChange()
  } finally {
    imageLoading.value = false
  }
}

function setColor(e: Event): void {
  cmd('foreColor', (e.target as HTMLInputElement).value)
}
function setHighlight(e: Event): void {
  cmd('hiliteColor', (e.target as HTMLInputElement).value)
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape' && painterActive.value) {
    painterActive.value = false
    painterSnapshot = null
    return
  }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault()
    const url = window.prompt(t('mail.linkPrompt'), 'https://')
    if (url) cmd('createLink', url)
  }
}

function onPaste(e: ClipboardEvent): void {
  // 粘贴一律走纯文本，避免带入源站点的样式/脚本污染邮件
  e.preventDefault()
  const text = e.clipboardData?.getData('text/plain') ?? ''
  document.execCommand('insertText', false, text)
  emitChange()
}

const empty = computed(() => !props.html || props.html === '<br>' || props.html === '<p></p>')
</script>

<template>
  <div class="rte">
    <div class="toolbar" role="toolbar">
      <!-- 格式刷：窄手柄 + 宽刷头（连成一体）+ 3 根刷毛（参考图2） -->
      <button class="tb" :class="{ active: painterActive }" :title="t('mail.formatPainter')" @mousedown.prevent="togglePainter">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <!-- 手柄（顶部窄长） -->
          <rect x="9.5" y="3" width="5" height="4" />
          <!-- 刷头（宽、圆角，与手柄相接） -->
          <rect x="4.5" y="6.8" width="15" height="5.8" rx="1.6" />
          <!-- 刷毛 -->
          <path d="M8.5 12.6v2.6M12 12.6v2.6M15.5 12.6v2.6" />
        </svg>
      </button>
      <!-- 清除格式：斜置 3D 橡皮（顶面带） + 底线（参考图2） -->
      <button class="tb" :title="t('mail.clearFormat')" @mousedown.prevent="cmd('removeFormat')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <!-- 橡皮主体：斜置块，右上小圆角 -->
          <path d="M14.2 3.6 20.4 9.8a1 1 0 0 1 0 1.4L11.6 20H8.4l-2.6-2.6z" />
          <!-- 顶面厚度线（与主边平行，靠右上） -->
          <path d="m11.4 6.4 5.8 5.8" />
          <!-- 底线 -->
          <path d="M3.5 20.6h17" />
        </svg>
      </button>

      <!-- 撤销 / 重做（前进后退），紧邻格式刷/清除 -->
      <button class="tb" :title="t('mail.undo')" @mousedown.prevent="cmd('undo')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 14 4 9l5-5" /><path d="M4 9h11a5 5 0 0 1 0 10h-4" /></svg>
      </button>
      <button class="tb" :title="t('mail.redo')" @mousedown.prevent="cmd('redo')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 14 5-5-5-5" /><path d="M20 9H9a5 5 0 0 0 0 10h4" /></svg>
      </button>

      <span class="sep" />

      <!-- 字体 / 字号 / 行高：三者相邻 -->
      <select class="tb-select font" :title="t('mail.fontFamily')" @change="setFontFamily">
        <option value="">{{ t('mail.fontDefault') }}</option>
        <option v-for="f in FONT_FAMILIES" :key="f.value" :value="f.value" :style="{ fontFamily: f.value }">
          {{ f.label }}
        </option>
      </select>
      <select class="tb-select size" :title="t('mail.fontSize')" @change="setFontSize">
        <option value="">{{ t('mail.sizeDefault') }}</option>
        <option v-for="s in FONT_SIZES" :key="s" :value="s">{{ s }}</option>
      </select>
      <select class="tb-select size" :title="t('mail.lineHeight')" @change="setLineHeight">
        <option value="">{{ t('mail.lineHeight') }}</option>
        <option v-for="l in LINE_HEIGHTS" :key="l" :value="l">{{ l }}</option>
      </select>

      <span class="sep" />

      <button class="tb glyph bold" :title="t('mail.bold')" @mousedown.prevent="cmd('bold')"><span>B</span></button>
      <button class="tb glyph italic" :title="t('mail.italic')" @mousedown.prevent="cmd('italic')"><span>I</span></button>
      <button class="tb glyph underline" :title="t('mail.underline')" @mousedown.prevent="cmd('underline')"><span>U</span></button>
      <button class="tb glyph strike" :title="t('mail.strike')" @mousedown.prevent="cmd('strikeThrough')"><span>S</span></button>

      <!-- 字体色：A + 彩色下划线（上下堆叠）+ ▾（参考图1） -->
      <label class="tb color" :title="t('mail.textColor')">
        <span class="color-glyph">
          <span class="color-a">A</span>
          <span class="color-bar" :style="{ background: textColor }" />
        </span>
        <svg class="color-caret" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
        <input type="color" v-model="textColor" @input="setColor" />
      </label>
      <!-- 背景/高亮色：斜置荧光笔 + 彩色下划线（上下堆叠）+ ▾（参考图1） -->
      <label class="tb color hl" :title="t('mail.highlight')">
        <span class="color-glyph">
          <svg class="hl-pen" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
            <path d="M13.8 4.4 19.6 10.2 14 15.8 8.2 10z" />
            <path d="m8.2 10 5.8 5.8-2.9 2.9H7.4l-1.2-1.2z" fill="currentColor" stroke="none" />
          </svg>
          <span class="color-bar" :style="{ background: highlightColor }" />
        </span>
        <svg class="color-caret" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
        <input type="color" v-model="highlightColor" @input="setHighlight" />
      </label>

      <!-- 插入图片：选本地文件 → base64 data URL 内联进正文 -->
      <button class="tb" :title="t('mail.insertImage')" :disabled="imageLoading" @mousedown.prevent="pickImage">
        <icon-loading v-if="imageLoading" class="spin" />
        <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="4.5" width="18" height="15" rx="2" />
          <circle cx="8.5" cy="9.5" r="1.6" />
          <path d="m4 17 5-5 3.5 3.5L16 12l4 4" />
        </svg>
      </button>

      <span class="sep" />

      <button class="tb" :title="t('mail.bulletList')" @mousedown.prevent="cmd('insertUnorderedList')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="9" y1="6" x2="20" y2="6" /><line x1="9" y1="12" x2="20" y2="12" /><line x1="9" y1="18" x2="20" y2="18" /><circle cx="4.5" cy="6" r="1.2" fill="currentColor" stroke="none" /><circle cx="4.5" cy="12" r="1.2" fill="currentColor" stroke="none" /><circle cx="4.5" cy="18" r="1.2" fill="currentColor" stroke="none" /></svg>
      </button>
      <button class="tb" :title="t('mail.numberList')" @mousedown.prevent="cmd('insertOrderedList')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="10" y1="6" x2="20" y2="6" /><line x1="10" y1="12" x2="20" y2="12" /><line x1="10" y1="18" x2="20" y2="18" /><path d="M4 5.5 5.5 4.8V9" stroke-width="1.6" /><path d="M4 12.5h2.2L4 15h2.4" stroke-width="1.6" /><path d="M4 17.5h2.2v1.2H4.6v1.3h1.8" stroke-width="1.6" /></svg>
      </button>
      <span class="sep" />
      <!-- 对齐：单个下拉，含左/中/右/两端对齐 -->
      <div ref="ddEl" class="dd">
        <button class="tb" :title="t('mail.align')" @mousedown.prevent="alignOpen = !alignOpen">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="4" y1="6" x2="20" y2="6" /><line x1="4" y1="12" x2="14" y2="12" /><line x1="4" y1="18" x2="18" y2="18" /></svg>
          <svg class="caret" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
        </button>
        <div v-if="alignOpen" class="dd-menu">
          <button
            v-for="a in ALIGNS"
            :key="a.cmd"
            class="dd-item"
            @mousedown.prevent="align(a.cmd)"
          >
            <span class="dd-ico" v-html="a.icon" />
            <span>{{ t(a.label) }}</span>
          </button>
        </div>
      </div>

      <span class="sep" />

      <button class="tb" :title="t('mail.quote')" @mousedown.prevent="cmd('formatBlock', 'blockquote')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 7H4v6h4V9c0 3-1 5-3 6" /><path d="M17 7h-3v6h4V9c0 3-1 5-3 6" /></svg>
      </button>
      <button class="tb mono" :title="t('mail.code')" @mousedown.prevent="cmd('formatBlock', 'pre')"><span>&lt;/&gt;</span></button>
    </div>

    <div class="edit-area" @click="focusEditor">
      <div
        ref="root"
        class="editor"
        :style="{ minHeight: (minHeight ?? 200) + 'px' }"
        contenteditable="true"
        spellcheck="false"
        @input="emitChange"
        @focus="focused = true"
        @blur="focused = false"
        @keydown="onKeydown"
        @mouseup="onMouseUp"
        @paste="onPaste"
      />
      <span v-if="empty && !focused" class="placeholder">{{ placeholder }}</span>
    </div>
  </div>
</template>

<style scoped>
.rte {
  display: flex;
  flex-direction: column;
}

/* ---------- 工具条（单行，不换行） ---------- */
.toolbar {
  display: flex;
  align-items: center;
  gap: 1px;
  flex-wrap: nowrap;
  padding: 6px 8px;
  border: 1px solid var(--border-light);
  border-radius: 10px;
  background: var(--bg-2);
}
.tb {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  flex: none;
  padding: 0;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast),
    transform 0.1s ease;
}
/* 对齐下拉容器 */
.dd {
  position: relative;
  display: inline-flex;
  flex: none;
}
.dd > .tb {
  width: auto;
  gap: 1px;
  padding: 0 3px;
}
.dd .caret {
  width: 11px;
  height: 11px;
  opacity: 0.7;
}
.dd-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  z-index: 20;
  min-width: 132px;
  padding: 4px;
  border: 1px solid var(--panel-border);
  border-radius: 10px;
  background: var(--panel-bg);
  box-shadow: var(--shadow-md);
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.dd-item {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 32px;
  padding: 0 10px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color var(--transition-fast), color var(--transition-fast);
}
.dd-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.dd-ico {
  display: inline-flex;
  flex: none;
}
.dd-ico :deep(svg) {
  width: 17px;
  height: 17px;
}
.tb:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.tb:active {
  transform: scale(0.94);
}
/* 格式刷激活态：强调色底，提示「正等待套用」 */
.tb.active {
  background: var(--primary-light);
  color: var(--accent-text);
}
.tb svg {
  width: 17px;
  height: 17px;
}
/* 文字字形按钮（B/I/U/S）：衬线字体更贴近图标的字重观感；行高与按钮同高以垂直居中 */
.glyph {
  line-height: 28px;
}
.glyph span {
  display: inline-block;
  font-family: Georgia, 'Times New Roman', serif;
  font-size: var(--font-size-title);
  line-height: 1;
}
.bold span {
  font-weight: 800;
}
.italic span {
  font-style: italic;
}
.underline span {
  text-decoration: underline;
}
.strike span {
  text-decoration: line-through;
}
.mono span {
  font-family: ui-monospace, monospace;
  font-size: var(--font-size-sm);
}
/* 颜色按钮：图标 + 彩色下划线（上下堆叠）+ ▾（参考图1）。
   隐藏原生取色器覆盖整块。 */
.color {
  position: relative;
  width: auto;
  gap: 2px;
  padding: 0 3px;
}
/* 图标 + 色条：竖向堆叠，色条紧贴图标底部且同宽 */
.color-glyph {
  display: inline-flex;
  flex-direction: column;
  align-items: center;
  gap: 1px;
}
.color .color-a {
  font-family: Georgia, 'Times New Roman', serif;
  font-weight: 700;
  font-size: var(--font-size-title);
  line-height: 1;
}
.color .hl-pen {
  width: 15px;
  height: 15px;
}
.color .color-bar {
  width: 15px;
  height: 3px;
  border-radius: 1px;
}
.color .color-caret {
  width: 10px;
  height: 10px;
  opacity: 0.7;
}
.color input[type='color'] {
  position: absolute;
  inset: 0;
  opacity: 0;
  cursor: pointer;
  border: none;
  padding: 0;
}
/* 下拉：去掉原生外观，自绘 caret，保证文字/箭头与图标按钮垂直居中一致 */
.tb-select {
  height: 28px;
  min-width: 52px;
  max-width: 92px;
  padding: 0 22px 0 6px;
  border: none;
  border-radius: 7px;
  appearance: none;
  -webkit-appearance: none;
  background-color: transparent;
  /* caret：与按钮图标同色、垂直居中 */
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%2386909c' stroke-width='2.4' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 6px center;
  background-size: 11px 11px;
  color: var(--text-secondary);
  font-size: var(--font-size-code);
  line-height: 28px;
  font-family: inherit;
  text-align: center;
  text-align-last: center;
  cursor: pointer;
  transition: background-color var(--transition-fast);
}
.tb-select.font {
  min-width: 78px;
  max-width: 108px;
}
.tb-select.size {
  min-width: 46px;
  max-width: 56px;
}
.tb-select:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}
.sep {
  width: 1px;
  height: 18px;
  margin: 0 3px;
  background: var(--border-base);
  flex: none;
}

/* ---------- 编辑区 ---------- */
.edit-area {
  position: relative;
  flex: 1;
  min-height: 0;
}
.editor {
  padding: 12px 2px;
  outline: none;
  color: var(--text-primary);
  font-size: var(--font-size-md);
  line-height: 1.65;
  letter-spacing: 0.003em;
  word-break: break-word;
}
.placeholder {
  position: absolute;
  top: 12px;
  left: 2px;
  color: var(--text-disabled);
  font-size: var(--font-size-md);
  pointer-events: none;
}
/* 编辑器内基础排版 */
.editor :deep(blockquote) {
  margin: 8px 0;
  padding: 2px 0 2px 12px;
  border-left: 3px solid var(--border-strong);
  color: var(--text-secondary);
}
.editor :deep(pre) {
  margin: 8px 0;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--bg-2);
  font-family: ui-monospace, monospace;
  font-size: var(--font-size-sm);
  white-space: pre-wrap;
}
.editor :deep(a) {
  color: var(--accent-text);
  text-decoration: underline;
}
.editor :deep(ul),
.editor :deep(ol) {
  margin: 6px 0;
  padding-left: 22px;
}
/* 插入的图片：限宽自适应，不撑破正文 */
.editor :deep(img) {
  max-width: 100%;
  height: auto;
  border-radius: 6px;
  vertical-align: bottom;
}
/* 插入图片按钮的加载态 */
.spin {
  width: 15px;
  height: 15px;
  animation: rte-spin 0.8s linear infinite;
}
@keyframes rte-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
