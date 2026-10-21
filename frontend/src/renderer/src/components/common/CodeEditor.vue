<script setup lang="ts">
import { ref, watch, onMounted, onBeforeUnmount } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightSpecialChars,
  drawSelection,
  rectangularSelection
} from '@codemirror/view'
import { EditorState, type Extension } from '@codemirror/state'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import {
  syntaxHighlighting,
  defaultHighlightStyle,
  bracketMatching,
  indentOnInput,
  foldGutter,
  foldKeymap,
  type LanguageSupport
} from '@codemirror/language'
import { oneDark } from '@codemirror/theme-one-dark'
import { javascript } from '@codemirror/lang-javascript'
import { json } from '@codemirror/lang-json'
import { python } from '@codemirror/lang-python'
import { rust } from '@codemirror/lang-rust'
import { cpp } from '@codemirror/lang-cpp'
import { go } from '@codemirror/lang-go'
import { java } from '@codemirror/lang-java'
import { html } from '@codemirror/lang-html'
import { css } from '@codemirror/lang-css'
import { xml } from '@codemirror/lang-xml'
import { markdown } from '@codemirror/lang-markdown'
import { yaml } from '@codemirror/lang-yaml'
import { php } from '@codemirror/lang-php'
import { sql } from '@codemirror/lang-sql'

const props = withDefaults(
  defineProps<{ modelValue?: string; language?: string | null; readonly?: boolean }>(),
  {
    modelValue: '',
    language: null,
    readonly: false
  }
)

const emit = defineEmits<{ (e: 'update:modelValue', value: string): void }>()

const containerRef = ref<HTMLDivElement | null>(null)
let view: EditorView | null = null

const settings = useSettingsStore()

const langExtensions: Record<string, LanguageSupport> = {
  javascript: javascript(),
  typescript: javascript({ typescript: true }),
  json: json(),
  python: python(),
  rust: rust(),
  cpp: cpp(),
  go: go(),
  java: java(),
  html: html(),
  css: css(),
  xml: xml(),
  markdown: markdown(),
  yaml: yaml(),
  php: php(),
  sql: sql(),
  ini: json(),
  dockerfile: javascript()
}

function getLangExt(lang: string | null | undefined): LanguageSupport | null {
  return (lang && langExtensions[lang]) || null
}

/** 当前是否暗色(跟随系统时按媒体查询解析) */
function isDarkNow(): boolean {
  const mode = settings.themeMode
  return mode === 'dark' || (mode === 'system' && resolveSystemDark())
}

function createEditor(): void {
  if (!containerRef.value || view) return

  const lang = getLangExt(props.language)
  const extensions: Extension[] = [
    // 只读模式:不可编辑(光标/选区保留,但不响应输入)
    EditorState.readOnly.of(props.readonly),
    EditorView.editable.of(!props.readonly),
    lineNumbers(),
    foldGutter(),
    highlightActiveLine(),
    highlightSpecialChars(),
    drawSelection(),
    rectangularSelection(),
    bracketMatching(),
    indentOnInput(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    history(),
    keymap.of([...defaultKeymap, ...historyKeymap, ...foldKeymap, indentWithTab]),
    EditorView.updateListener.of((update) => {
      if (!props.readonly && update.docChanged) {
        emit('update:modelValue', update.state.doc.toString())
      }
    }),
    EditorView.theme({
      '&': { height: '100%', fontSize: '13px' },
      '.cm-scroller': { overflow: 'auto', fontFamily: "'SF Mono','Fira Code','Cascadia Code',monospace" }
    })
  ]
  if (isDarkNow()) extensions.push(oneDark)

  if (lang) extensions.push(lang)

  const state = EditorState.create({
    doc: props.modelValue != null ? String(props.modelValue) : '',
    extensions
  })

  view = new EditorView({
    state,
    parent: containerRef.value
  })
}

function destroyEditor(): void {
  if (view) {
    view.destroy()
    view = null
  }
}

watch(
  () => props.modelValue,
  (val) => {
    if (!view) return
    const current = view.state.doc.toString()
    const next = val !== null && val !== undefined ? String(val) : ''
    if (next !== current) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: next }
      })
    }
  }
)

watch(
  () => props.language,
  () => {
    destroyEditor()
    createEditor()
  }
)

watch(
  () => settings.themeMode,
  () => {
    destroyEditor()
    createEditor()
  }
)

onMounted(() => {
  createEditor()
})

defineExpose({ view: () => view })

onBeforeUnmount(() => {
  destroyEditor()
})
</script>

<template>
  <div ref="containerRef" class="codemirror-container" />
</template>

<style scoped>
.codemirror-container {
  flex: 1;
  overflow: hidden;
}

.codemirror-container :deep(.cm-editor) {
  height: 100%;
}

.codemirror-container :deep(.cm-editor.cm-focused) {
  outline: none;
}

.codemirror-container :deep(.cm-gutters) {
  background-color: var(--bg-1);
  color: var(--text-tertiary);
  border-right: 1px solid var(--border-base);
}

.codemirror-container :deep(.cm-activeLineGutter) {
  background-color: var(--bg-2);
}

.codemirror-container :deep(.cm-activeLine) {
  background-color: var(--bg-hover);
}

.codemirror-container :deep(.cm-selectionBackground) {
  background-color: color-mix(in srgb, var(--primary) 20%, transparent) !important;
}

.codemirror-container :deep(.cm-cursor) {
  border-left-color: var(--text-primary);
}

.codemirror-container :deep(.cm-matchingBracket) {
  background-color: color-mix(in srgb, var(--primary) 10%, transparent);
  color: var(--primary) !important;
}
</style>