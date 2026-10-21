<script setup lang="ts">
/**
 * 统一 diff 渲染（代码审查页签的文件展开区）。
 *
 * 数据来自后端 `git_file_diff`（`git diff` 原样输出 + 新侧总行数）：
 * - 解析 `@@ -a,b +c,d @@` 分块，**不展示 hunk 头**，改为按 `newLines` 补齐「未修改区段」
 *   折叠条（前导 / 块间由 hunk 头算出，尾部靠 `newLines` 才能算准）
 * - 行号取**新侧**（审查视角看的是当前状态）；删除行没有新侧号，显示旧侧号并压暗
 * - 增删用底色区分（`--success` / `--danger` 的 14% 混色，明暗主题自适应）
 *
 * 高亮：新侧行（上下文 + 新增）**按显示顺序拼成一整块**再交给 highlight.js，
 * 这样多行字符串 / 注释的跨行 `span` 不会被切断；随后用标签栈按行补齐开闭
 * （做法同 `components/common/CodeBlock.vue`）。删除行单独高亮。
 *
 * 折叠条 v1 为**静态展示**：`git diff` 只带 3 行上下文，展开需要文件正文或按更大的
 * `-U` 重取 diff，留待后续。
 */
import { computed, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import hljs from 'highlight.js/lib/core'
import bash from 'highlight.js/lib/languages/bash'
import clike from 'highlight.js/lib/languages/c'
import cpp from 'highlight.js/lib/languages/cpp'
import csharp from 'highlight.js/lib/languages/csharp'
import css from 'highlight.js/lib/languages/css'
import diff from 'highlight.js/lib/languages/diff'
import dockerfile from 'highlight.js/lib/languages/dockerfile'
import go from 'highlight.js/lib/languages/go'
import ini from 'highlight.js/lib/languages/ini'
import java from 'highlight.js/lib/languages/java'
import javascript from 'highlight.js/lib/languages/javascript'
import json from 'highlight.js/lib/languages/json'
import kotlin from 'highlight.js/lib/languages/kotlin'
import lua from 'highlight.js/lib/languages/lua'
import markdown from 'highlight.js/lib/languages/markdown'
import php from 'highlight.js/lib/languages/php'
import python from 'highlight.js/lib/languages/python'
import ruby from 'highlight.js/lib/languages/ruby'
import rust from 'highlight.js/lib/languages/rust'
import scss from 'highlight.js/lib/languages/scss'
import shell from 'highlight.js/lib/languages/shell'
import sql from 'highlight.js/lib/languages/sql'
import swift from 'highlight.js/lib/languages/swift'
import typescript from 'highlight.js/lib/languages/typescript'
import xml from 'highlight.js/lib/languages/xml'
import yaml from 'highlight.js/lib/languages/yaml'
import hlLightUrl from 'highlight.js/styles/github.css?url'
import hlDarkUrl from 'highlight.js/styles/github-dark.css?url'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'

const props = withDefaults(
  defineProps<{
    /** 统一 diff 文本（`git diff` 原样输出） */
    diff: string
    /** 新侧总行数：用于补齐**尾部**未修改区段的折叠条（0 / 缺失则不显示尾部条） */
    newLines?: number
    /** 文件路径：按扩展名选高亮语言 */
    path?: string
    /**
     * 语法高亮行数上限：超过则只做转义（跳过 highlight.js）。
     * 大 diff（如整份文件写入）高亮成本高，跳过后仍保证内容可读与增删底色。
     */
    highlightLimit?: number
  }>(),
  { newLines: 0, path: '', highlightLimit: 1500 }
)

const { t } = useI18n()
const settings = useSettingsStore()

// 常用语言按需注册（highlight.js 11）;长尾语言由 FilePreviewPanel 在同一 hljs 单例上注册
type LangMod = Parameters<typeof hljs.registerLanguage>[1]
const LANG_LIST: Array<[LangMod, string]> = [
  [bash, 'bash'], [clike, 'c'], [cpp, 'cpp'], [csharp, 'csharp'], [css, 'css'],
  [diff, 'diff'], [dockerfile, 'dockerfile'], [go, 'go'], [ini, 'ini'], [java, 'java'],
  [javascript, 'javascript'], [json, 'json'], [kotlin, 'kotlin'], [lua, 'lua'],
  [markdown, 'markdown'], [php, 'php'], [python, 'python'], [ruby, 'ruby'], [rust, 'rust'],
  [scss, 'scss'], [shell, 'shell'], [sql, 'sql'], [swift, 'swift'],
  [typescript, 'typescript'], [xml, 'xml'], [yaml, 'yaml']
]
for (const [mod, name] of LANG_LIST) {
  hljs.registerLanguage(name, mod)
}

/** 扩展名 / 特征文件名 → highlight.js 语言名（未命中 → 空 = 纯文本） */
const EXT_LANG: Record<string, string> = {
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  vue: 'xml', svelte: 'xml', html: 'xml', htm: 'xml', svg: 'xml', xml: 'xml',
  rs: 'rust', py: 'python', go: 'go', java: 'java', kt: 'kotlin', kts: 'kotlin',
  c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp',
  cs: 'csharp', rb: 'ruby', php: 'php', swift: 'swift', lua: 'lua',
  css: 'css', scss: 'scss', less: 'css', json: 'json', json5: 'json',
  yaml: 'yaml', yml: 'yaml', toml: 'ini', ini: 'ini', cfg: 'ini', conf: 'ini', properties: 'ini',
  sh: 'bash', bash: 'bash', zsh: 'bash', sql: 'sql',
  md: 'markdown', markdown: 'markdown', diff: 'diff', patch: 'diff'
}

const language = computed(() => {
  const name = (props.path ?? '').split('/').pop() ?? ''
  const lower = name.toLowerCase()
  if (lower === 'dockerfile' || lower.startsWith('dockerfile.')) return 'dockerfile'
  if (lower === 'makefile' || lower === 'cmakelists.txt') return 'bash'
  return EXT_LANG[lower.split('.').pop() ?? ''] ?? ''
})

/* ---------- 解析 ---------- */

interface DiffLine {
  kind: 'ctx' | 'add' | 'del'
  oldNo?: number
  newNo?: number
  text: string
}
interface Hunk {
  oldStart: number
  oldCount: number
  newStart: number
  newCount: number
  lines: DiffLine[]
}
type Row = { kind: 'gap'; count: number } | { kind: 'line'; line: DiffLine }

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/
/** diff 的头部元信息行（不渲染） */
const META_PREFIXES = [
  'diff --git', 'index ', '--- ', '+++ ', 'new file mode', 'deleted file mode',
  'old mode', 'new mode', 'similarity index', 'rename ', 'copy '
]

const parsed = computed<{ hunks: Hunk[]; binary: boolean }>(() => {
  const hunks: Hunk[] = []
  let binary = false
  let cur: Hunk | null = null
  for (const raw of (props.diff ?? '').split('\n')) {
    if (raw.startsWith('Binary files')) {
      binary = true
      continue
    }
    if (META_PREFIXES.some((p) => raw.startsWith(p))) continue
    const m = HUNK_RE.exec(raw)
    if (m) {
      cur = {
        oldStart: Number(m[1]),
        oldCount: m[2] === undefined ? 1 : Number(m[2]),
        newStart: Number(m[3]),
        newCount: m[4] === undefined ? 1 : Number(m[4]),
        lines: []
      }
      hunks.push(cur)
      continue
    }
    if (!cur) continue
    // "\ No newline at end of file" 无内容,跳过
    if (raw.startsWith('\\')) continue
    const sign = raw.charAt(0)
    // 上下文行一定带前导空格;空行(文件末尾换行产生的残片)不进
    if (sign === '+') cur.lines.push({ kind: 'add', text: raw.slice(1) })
    else if (sign === '-') cur.lines.push({ kind: 'del', text: raw.slice(1) })
    else if (sign === ' ') cur.lines.push({ kind: 'ctx', text: raw.slice(1) })
  }
  // 行号回填
  for (const h of hunks) {
    let o = h.oldStart
    let n = h.newStart
    for (const l of h.lines) {
      if (l.kind === 'add') l.newNo = n++
      else if (l.kind === 'del') l.oldNo = o++
      else {
        l.oldNo = o++
        l.newNo = n++
      }
    }
  }
  return { hunks, binary }
})

const rows = computed<Row[]>(() => {
  const { hunks } = parsed.value
  if (!hunks.length) return []
  const out: Row[] = []
  let covered = 0 // 已渲染到的最后一条新侧行号
  for (const h of hunks) {
    const gap = h.newStart - 1 - covered
    if (gap > 0) out.push({ kind: 'gap', count: gap })
    for (const line of h.lines) out.push({ kind: 'line', line })
    covered = Math.max(covered, h.newStart + h.newCount - 1)
  }
  const tail = (props.newLines || 0) - covered
  if (tail > 0) out.push({ kind: 'gap', count: tail })
  return out
})

/* ---------- 高亮 ---------- */

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function highlight(code: string): string {
  const lang = language.value
  if (lang && hljs.getLanguage(lang)) {
    try {
      return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value
    } catch {
      /* 落到纯文本 */
    }
  }
  return escapeHtml(code)
}

/** 按逻辑行拆分高亮 HTML（标签栈为每行补齐开/闭，跨行 span 不断） */
function splitLines(html: string): string[] {
  const out: string[] = []
  const open: string[] = []
  const tagRe = /<span\b[^>]*>|<\/span>/g
  for (const raw of html.split('\n')) {
    const prefix = open.join('')
    tagRe.lastIndex = 0
    let m: RegExpExecArray | null
    while ((m = tagRe.exec(raw))) {
      if (m[0] === '</span>') open.pop()
      else open.push(m[0])
    }
    out.push(prefix + raw + '</span>'.repeat(open.length))
  }
  return out
}

/** 新侧行（ctx + add）按显示顺序拼块后整体高亮；超上限只转义不高亮 */
const newSideHtml = computed(() => {
  const parts: string[] = []
  for (const h of parsed.value.hunks) {
    for (const l of h.lines) if (l.kind !== 'del') parts.push(l.text)
  }
  const source = parts.join('\n')
  const html =
    props.highlightLimit > 0 && parts.length > props.highlightLimit
      ? escapeHtml(source)
      : highlight(source)
  return splitLines(html)
})

/** 与 `rows` 对齐的渲染项（gap 行不带 html） */
const rendered = computed<{ row: Row; html: string; no?: number }[]>(() => {
  const newHtml = newSideHtml.value
  let ni = 0
  const out: { row: Row; html: string; no?: number }[] = []
  for (const row of rows.value) {
    if (row.kind === 'gap') {
      out.push({ row, html: '' })
      continue
    }
    const line = row.line
    if (line.kind === 'del') {
      out.push({ row, html: highlight(line.text), no: line.oldNo })
    } else {
      const html = newHtml[ni] ?? escapeHtml(line.text)
      ni++
      out.push({ row, html, no: line.newNo })
    }
  }
  return out
})

/** 二进制 / 空 diff 的提示态 */
const emptyReason = computed<'binary' | 'none' | ''>(() => {
  if (parsed.value.binary) return 'binary'
  if (!parsed.value.hunks.length) return 'none'
  return ''
})

/* ---------- 主题 ---------- */

const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

// 与 CodeBlock 共用同一 link id（多个代码视图共享一份 hljs 主题）
const THEME_ID = 'astorm-hljs-theme'
watch(
  isDark,
  (dark) => {
    let link = document.getElementById(THEME_ID) as HTMLLinkElement | null
    if (!link) {
      link = document.createElement('link')
      link.id = THEME_ID
      link.rel = 'stylesheet'
      document.head.appendChild(link)
    }
    link.href = dark ? hlDarkUrl : hlLightUrl
  },
  { immediate: true }
)
</script>

<template>
  <div class="cd-root">
    <div v-if="emptyReason" class="cd-empty">
      {{ emptyReason === 'binary' ? t('panel.codeReview.binary') : t('panel.codeReview.noDiff') }}
    </div>
    <template v-else>
      <div
        v-for="(item, i) in rendered"
        :key="i"
        class="cd-row"
        :class="item.row.kind === 'line' ? `cd-${item.row.line.kind}` : 'cd-gap-row'"
      >
        <template v-if="item.row.kind === 'gap'">
          <div class="cd-gap">
            <icon-down :size="11" class="cd-gap-icon" />
            {{ t('panel.codeReview.unmodifiedLines', { n: item.row.count }) }}
          </div>
        </template>
        <template v-else>
          <span class="cd-ln" aria-hidden="true">{{ item.no ?? '' }}</span>
          <span class="cd-code" v-html="item.html" />
        </template>
      </div>
    </template>
  </div>
</template>

<style scoped>
/* macOS 风格：系统色增删底、弱化等宽行号、胶囊折叠条 */
.cd-root {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  line-height: 1.65;
  color: var(--text-primary);
  padding: 6px 0 10px;
}

/* 每行:行号栏 + 内容(长行折行,不横向滚动) */
.cd-row {
  display: flex;
  align-items: flex-start;
}

.cd-ln {
  flex: none;
  width: 4ch;
  padding-right: 12px;
  text-align: right;
  color: var(--text-tertiary);
  opacity: 0.55;
  user-select: none;
}

.cd-code {
  flex: 1;
  min-width: 0;
  padding-right: 12px;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

/* 增删底色:macOS 系统绿 / 系统红 */
.cd-add {
  background: rgba(52, 199, 89, 0.13);
}

.cd-del {
  background: rgba(255, 59, 48, 0.12);
}

.cd-del .cd-code {
  opacity: 0.75;
}

/* 未修改区段折叠条 */
.cd-gap-row {
  display: block;
}

.cd-gap {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 5px 10px;
  padding: 2px 10px;
  border-radius: 999px;
  background: var(--bg-2);
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-2xs);
}

.cd-gap-icon {
  opacity: 0.55;
  transform: rotate(-90deg);
}

.cd-empty {
  padding: 10px 14px;
  font-family: -apple-system, system-ui, sans-serif;
  font-size: var(--font-size-code);
  color: var(--text-tertiary);
}
</style>
