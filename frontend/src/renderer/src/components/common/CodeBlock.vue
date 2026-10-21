<script setup lang="ts">
/**
 * 代码块（markdown 代码块样式）：头部语言标识 + 一键复制，内置 highlight.js 离线高亮，
 * 明暗主题跟随应用（github / github-dark），与助手正文里的 ``` 代码块观感一致；
 * 默认带行号（左侧固定栏，长行折行时缩进对齐内容列）。
 *
 * 用于展示命令与文件片段（审计页 bash 预览、审批卡被拦命令等）。
 * 未知语言按纯文本渲染（转义输出，不做自动探测，避免误高亮）。
 */
import { computed, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import hljs from 'highlight.js/lib/core'
import bash from 'highlight.js/lib/languages/bash'
import json from 'highlight.js/lib/languages/json'
import hlLightUrl from 'highlight.js/styles/github.css?url'
import hlDarkUrl from 'highlight.js/styles/github-dark.css?url'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'

const props = withDefaults(
  defineProps<{
    code: string
    /** 高亮语言（未知语言按纯文本渲染）；命令用 bash，JSON 用 json */
    lang?: string
    /** 头部显示的标签；不传则显示 lang */
    label?: string
    /** 正文最大高度（超出滚动） */
    maxHeight?: string
    /** 显示行号（左侧栏） */
    lineNumbers?: boolean
  }>(),
  { lang: 'bash', label: '', maxHeight: '280px', lineNumbers: true }
)

const { t } = useI18n()
const settings = useSettingsStore()

hljs.registerLanguage('bash', bash)
hljs.registerLanguage('json', json)

const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

/** 已知语言用 hljs 高亮（输出已转义），未知语言纯文本转义 */
const highlighted = computed(() => {
  const code = props.code || ''
  if (hljs.getLanguage(props.lang)) {
    try {
      return hljs.highlight(code, { language: props.lang, ignoreIllegals: true }).value
    } catch {
      /* 落到纯文本 */
    }
  }
  return escapeHtml(code)
})

/**
 * 按逻辑行拆分高亮 HTML（行号栏需要逐行渲染）。
 * hljs 的 `<span>` 可跨行（多行字符串 / 注释），故用标签栈为每行补齐开/闭：
 * 行首重开上一行遗留的标签，行尾按**本行处理完后**栈内剩余标签数闭合。
 */
const lines = computed<string[]>(() => {
  const out: string[] = []
  const open: string[] = []
  const tagRe = /<span\b[^>]*>|<\/span>/g
  for (const raw of highlighted.value.split('\n')) {
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
})

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

const hljsThemeLoaded = computed(() => !!hljs.getLanguage(props.lang))

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

// 主题 link 常驻（多个代码块共用同一 id，卸载时不摘除，避免互相摘掉）

async function copy(): Promise<void> {
  try {
    await navigator.clipboard.writeText(props.code)
    Message.success(t('common.copied'))
  } catch {
    Message.error(t('common.copyFailed'))
  }
}
</script>

<template>
  <div class="code-block">
    <div class="cb-head">
      <span class="cb-dot" />
      <span class="cb-lang">{{ label || lang }}</span>
      <span class="cb-spacer" />
      <button type="button" class="cb-copy" :title="t('common.copy')" @click="copy">
        <icon-copy :size="13" />
      </button>
    </div>
    <pre class="cb-body" :style="{ maxHeight }"><code
      class="hljs"
      :class="hljsThemeLoaded ? `language-${lang}` : undefined"
    ><template v-if="lineNumbers"><span v-for="(line, i) in lines" :key="i" class="cb-line"><span
        class="cb-ln"
        aria-hidden="true"
      >{{ i + 1 }}</span><span class="cb-lc" v-html="line" /></span></template><span
      v-else
      v-html="highlighted"
    /></code></pre>
  </div>
</template>

<style scoped>
.code-block {
  min-width: 0;
  max-width: 100%;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  background: var(--color-fill-1);
  overflow: hidden;
}

.cb-head {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 5px 10px;
  background: var(--color-fill-1);
  border-bottom: 1px solid var(--border-base);
}

.cb-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--accent-text);
  opacity: 0.8;
  flex-shrink: 0;
}

.cb-lang {
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  font-weight: var(--font-weight-medium);
  letter-spacing: 0.01em;
  color: var(--text-tertiary);
}

.cb-spacer {
  flex: 1;
}

.cb-copy {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.cb-copy:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.cb-body {
  margin: 0;
  padding: 10px 12px;
  min-width: 0;
  max-width: 100%;
  overflow: auto;
  font-family: var(--font-mono);
  font-size: var(--font-size-code);
  line-height: var(--line-height-code);
  color: var(--text-primary);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  tab-size: 2;
  background: var(--color-bg-5);
}

.cb-body .hljs {
  display: block;
  padding: 0;
  margin: 0;
  background: transparent;
  color: inherit;
}

/* 行号栏：每逻辑行一行（flex），折行时内容缩进对齐内容列而非顶到行号下 */
.cb-line {
  display: flex;
  gap: 10px;
  min-height: 1.55em;
}

.cb-ln {
  flex: none;
  width: 2ch;
  text-align: right;
  color: var(--text-tertiary);
  opacity: 0.6;
  user-select: none;
}

.cb-lc {
  flex: 1;
  min-width: 0;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

</style>
