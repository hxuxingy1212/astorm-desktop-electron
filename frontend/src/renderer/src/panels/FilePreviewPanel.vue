<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, provide, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import hljs from 'highlight.js/lib/core'
// 常用语言全家桶(highlight.js 11 按需注册;toml 无模块 → 扩展名映射到 ini)
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
import less from 'highlight.js/lib/languages/less'
import lua from 'highlight.js/lib/languages/lua'
import makefile from 'highlight.js/lib/languages/makefile'
import markdown from 'highlight.js/lib/languages/markdown'
import nginx from 'highlight.js/lib/languages/nginx'
import perl from 'highlight.js/lib/languages/perl'
import php from 'highlight.js/lib/languages/php'
import powershell from 'highlight.js/lib/languages/powershell'
import python from 'highlight.js/lib/languages/python'
import r from 'highlight.js/lib/languages/r'
import ruby from 'highlight.js/lib/languages/ruby'
import rust from 'highlight.js/lib/languages/rust'
import scss from 'highlight.js/lib/languages/scss'
import shell from 'highlight.js/lib/languages/shell'
import sql from 'highlight.js/lib/languages/sql'
import swift from 'highlight.js/lib/languages/swift'
import typescript from 'highlight.js/lib/languages/typescript'
import xml from 'highlight.js/lib/languages/xml'
import yaml from 'highlight.js/lib/languages/yaml'
import apache from 'highlight.js/lib/languages/apache'
import arduino from 'highlight.js/lib/languages/arduino'
import clojure from 'highlight.js/lib/languages/clojure'
import cmake from 'highlight.js/lib/languages/cmake'
import crystal from 'highlight.js/lib/languages/crystal'
import d from 'highlight.js/lib/languages/d'
import dart from 'highlight.js/lib/languages/dart'
import django from 'highlight.js/lib/languages/django'
import dns from 'highlight.js/lib/languages/dns'
import dos from 'highlight.js/lib/languages/dos'
import elixir from 'highlight.js/lib/languages/elixir'
import elm from 'highlight.js/lib/languages/elm'
import erlang from 'highlight.js/lib/languages/erlang'
import fortran from 'highlight.js/lib/languages/fortran'
import fsharp from 'highlight.js/lib/languages/fsharp'
import gherkin from 'highlight.js/lib/languages/gherkin'
import glsl from 'highlight.js/lib/languages/glsl'
import gradle from 'highlight.js/lib/languages/gradle'
import graphql from 'highlight.js/lib/languages/graphql'
import groovy from 'highlight.js/lib/languages/groovy'
import haml from 'highlight.js/lib/languages/haml'
import handlebars from 'highlight.js/lib/languages/handlebars'
import haskell from 'highlight.js/lib/languages/haskell'
import haxe from 'highlight.js/lib/languages/haxe'
import http from 'highlight.js/lib/languages/http'
import julia from 'highlight.js/lib/languages/julia'
import lisp from 'highlight.js/lib/languages/lisp'
import llvm from 'highlight.js/lib/languages/llvm'
import matlab from 'highlight.js/lib/languages/matlab'
import nim from 'highlight.js/lib/languages/nim'
import objectivec from 'highlight.js/lib/languages/objectivec'
import ocaml from 'highlight.js/lib/languages/ocaml'
import plaintext from 'highlight.js/lib/languages/plaintext'
import properties from 'highlight.js/lib/languages/properties'
import protobuf from 'highlight.js/lib/languages/protobuf'
import qml from 'highlight.js/lib/languages/qml'
import reasonml from 'highlight.js/lib/languages/reasonml'
import scala from 'highlight.js/lib/languages/scala'
import stan from 'highlight.js/lib/languages/stan'
import stylus from 'highlight.js/lib/languages/stylus'
import tcl from 'highlight.js/lib/languages/tcl'
import thrift from 'highlight.js/lib/languages/thrift'
import vala from 'highlight.js/lib/languages/vala'
import vbnet from 'highlight.js/lib/languages/vbnet'
import verilog from 'highlight.js/lib/languages/verilog'
import vhdl from 'highlight.js/lib/languages/vhdl'
import vim from 'highlight.js/lib/languages/vim'
import wasm from 'highlight.js/lib/languages/wasm'
import hlDarkUrl from 'highlight.js/styles/github-dark.css?url'
import hlLightUrl from 'highlight.js/styles/github.css?url'
import { useTabsStore } from '@/stores/tabs'
import { useFilesStore } from '@/stores/files'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import { parseFrontmatter, type FrontmatterMeta } from '@/utils/frontmatter'
import { upgradeRelativeImages } from '@/utils/markdownImages'
import { FILE_REF_BASE } from '@/utils/fileRefs'
import { isWebShell } from '@/utils/platform'
import { downloadBytes } from '@/utils/webDownload'
import MarkdownBody from '@/components/common/MarkdownBody.vue'
import MarkdownFrontmatterHeader from '@/components/common/MarkdownFrontmatterHeader.vue'
import OfficeViewerRenderer from '@/components/preview/OfficeViewerRenderer.vue'

const props = defineProps<{
  /** 页签模式:从 tabs store 读取(文件树点击打开的页签) */
  tabId?: string
  /** 直连模式:直接给定文件(知识库预览等);提供 filePath 即生效 */
  filePath?: string
  fileName?: string
  /** 直连模式 markdown 相对图片的知识库相对目录(空串 = 根层;未提供 = 不做内联) */
  imageBaseRel?: string
}>()

/** 直连模式判定 */
const isDirect = computed(() => !!props.filePath)

/** 直连模式本地加载状态 */
const directState = reactive({
  content: '',
  loading: false,
  error: ''
})
const { t } = useI18n()
const tabs = useTabsStore()
const files = useFilesStore()
const settings = useSettingsStore()

// 注册高亮语言(按需集合;VSCode/GitHub 风格主题由下方 <link> 动态切换)
type LangMod = Parameters<typeof hljs.registerLanguage>[1]
const LANG_LIST: Array<[LangMod, string]> = [
  [bash, 'bash'], [clike, 'c'], [cpp, 'cpp'], [csharp, 'csharp'], [css, 'css'],
  [diff, 'diff'], [dockerfile, 'dockerfile'], [go, 'go'], [ini, 'ini'], [java, 'java'],
  [javascript, 'javascript'], [json, 'json'], [kotlin, 'kotlin'], [less, 'less'], [lua, 'lua'],
  [makefile, 'makefile'], [markdown, 'markdown'], [nginx, 'nginx'], [perl, 'perl'], [php, 'php'],
  [powershell, 'powershell'], [python, 'python'], [r, 'r'], [ruby, 'ruby'], [rust, 'rust'],
  [scss, 'scss'], [shell, 'shell'], [sql, 'sql'], [swift, 'swift'],
  [typescript, 'typescript'], [xml, 'xml'], [yaml, 'yaml'],
  [apache, 'apache'], [arduino, 'arduino'], [clojure, 'clojure'], [cmake, 'cmake'],
  [crystal, 'crystal'], [d, 'd'], [dart, 'dart'], [django, 'django'], [dns, 'dns'], [dos, 'dos'],
  [elixir, 'elixir'], [elm, 'elm'], [erlang, 'erlang'], [fortran, 'fortran'], [fsharp, 'fsharp'],
  [gherkin, 'gherkin'], [glsl, 'glsl'], [gradle, 'gradle'], [graphql, 'graphql'], [groovy, 'groovy'],
  [haml, 'haml'], [handlebars, 'handlebars'], [haskell, 'haskell'], [haxe, 'haxe'], [http, 'http'],
  [julia, 'julia'], [lisp, 'lisp'], [llvm, 'llvm'], [matlab, 'matlab'], [nim, 'nim'],
  [objectivec, 'objectivec'], [ocaml, 'ocaml'], [plaintext, 'plaintext'], [properties, 'properties'],
  [protobuf, 'protobuf'], [qml, 'qml'], [reasonml, 'reasonml'], [scala, 'scala'], [stan, 'stan'],
  [stylus, 'stylus'], [tcl, 'tcl'], [thrift, 'thrift'], [vala, 'vala'], [vbnet, 'vbnet'],
  [verilog, 'verilog'], [vhdl, 'vhdl'], [vim, 'vim'], [wasm, 'wasm']
]
for (const [mod, name] of LANG_LIST) {
  hljs.registerLanguage(name, mod)
}

const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

/** 高亮主题 <link>:亮/暗主题切换(github / github-dark,贴近 VSCode/IDEA 观感) */
const THEME_LINK_ID = 'hljs-theme-style'
watch(
  isDark,
  (dark) => {
    let link = document.getElementById(THEME_LINK_ID) as HTMLLinkElement | null
    if (!link) {
      link = document.createElement('link')
      link.id = THEME_LINK_ID
      link.rel = 'stylesheet'
      document.head.appendChild(link)
    }
    link.href = dark ? hlDarkUrl : hlLightUrl
  },
  { immediate: true }
)

/** 当前文件状态:直连模式用本地状态,页签模式从 store 读取;渲染层统一读取 */
const file = computed<{
  fileName: string
  filePath: string
  content: string | null
  loading: boolean
  error?: string
  /** 页签模式:正文文件引用带来的定位行与定位序号(见 FileTabState;直连模式恒为 0) */
  targetLine?: number
  revealSeq?: number
} | null>(() => {
  if (isDirect.value) {
    return {
      fileName: props.fileName ?? '',
      filePath: props.filePath ?? '',
      content: directState.content,
      loading: directState.loading,
      error: directState.error || undefined
    }
  }
  return tabs.tabs.find((x) => x.id === props.tabId)?.file ?? null
})

/** 触发一次内容加载(路径变化时由 watch 驱动;会话换挡恢复的元数据快照 content=null 会自然重读) */
const loadedPath = ref('')
watch(
  file,
  async (f) => {
    if (!f) return
    if (loadedPath.value === f.filePath) return
    loadedPath.value = f.filePath

    // 直连模式:image/office 由各自分支自行读取,仅 markdown/text/code 读文本
    if (isDirect.value) {
      const k = kindOf(props.fileName ?? '')
      if (k === 'image' || k === 'office') return
      directState.loading = true
      directState.error = ''
      directState.content = ''
      const { content, error } = await files.readContent(f.filePath)
      if (loadedPath.value !== f.filePath) return
      // markdown 原始文本;相对图片经 transformImgUrl 在渲染时解析,不污染源文本
      directState.content = content ?? ''
      directState.error = error ?? ''
      directState.loading = false
      return
    }

    // 页签模式:内容已就绪即跳过;恢复的元数据快照(content=null)则重新加载
    if (f.content !== null) return
    const tabId = props.tabId as string
    const path = f.filePath
    tabs.setFileState(tabId, { loading: true, error: undefined, content: null })
    const { content, error } = await files.readContent(path)
    if (props.tabId !== tabId) return // 已切走:丢弃过期结果(当前页签内容由它自己的 watcher 负责)
    tabs.setFileState(tabId, { loading: false, content, error })
  },
  { immediate: true }
)

/** 扩展名 → 渲染类型:md 走 Markdown;图片走内置预览;office 走 OfficeViewerRenderer(astorm-office 高保真 + 悬浮路径拾取,旧版格式内部回退 OfficePreview);其余二进制走外部打开;代码走语法高亮;其余文本 */
type PreviewKind = 'markdown' | 'text' | 'binary' | 'image' | 'office'
function kindOf(name: string): PreviewKind {
  const n = name.toLowerCase()
  if (/\.(md|markdown|mdx)$/.test(n)) return 'markdown'
  if (/\.(png|jpe?g|gif|webp|bmp|svg|ico|avif)$/.test(n)) return 'image'
  if (/\.(docx?|docm|xlsx?|xlsm|csv|pptx?|pptm|ppsx?|pps|pdf)$/.test(n)) return 'office'
  if (/\.(mp3|wav|flac|ogg|mp4|mkv|avi|webm|zip|rar|7z|tar|gz|exe|dll|bin)$/.test(n)) return 'binary'
  return 'text'
}
const kind = computed<PreviewKind>(() => kindOf(file.value?.fileName ?? ''))

/** 图片 MIME(按扩展名;未知回退 application/octet-stream) */
function imageMime(name: string): string {
  const m: Record<string, string> = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
    webp: 'image/webp', bmp: 'image/bmp', svg: 'image/svg+xml', ico: 'image/x-icon', avif: 'image/avif'
  }
  const ext = name.toLowerCase().includes('.') ? name.toLowerCase().slice(name.lastIndexOf('.') + 1) : ''
  return m[ext] ?? 'application/octet-stream'
}

/** 图片预览:二进制 base64(data URL) */
const imageSrc = ref('')
const imageError = ref('')
watch(
  [file, kind],
  async () => {
    imageSrc.value = ''
    imageError.value = ''
    const f = file.value
    if (!f || kind.value !== 'image') return
    try {
      const b64 = await window.api.fs.readFileBase64(f.filePath)
      if (!b64) {
        imageError.value = t('panel.filePreview.imageFailed')
        return
      }
      imageSrc.value = `data:${imageMime(f.fileName)};base64,${b64}`
    } catch {
      imageError.value = t('panel.filePreview.imageFailed')
    }
  },
  { immediate: true }
)

/** 扩展名 → highlight.js 语言名(未收录返回 null,走纯文本) */
function langOf(name: string): string | null {
  const f = name.toLowerCase()
  const map: Record<string, string> = {
    ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
    js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'javascript',
    json: 'json', jsonc: 'json', yaml: 'yaml', yml: 'yaml',
    py: 'python', rs: 'rust', go: 'go', java: 'java', class: 'java',
    c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', hpp: 'cpp',
    cs: 'csharp', rb: 'ruby', php: 'php', swift: 'swift', kt: 'kotlin', kts: 'kotlin',
    lua: 'lua', r: 'r', pl: 'perl', sh: 'bash', bash: 'bash', zsh: 'bash',
    ps1: 'powershell', bat: 'shell', cmd: 'shell',
    sql: 'sql', xml: 'xml', svg: 'xml', html: 'xml', css: 'css',
    scss: 'scss', sass: 'scss', less: 'less', vue: 'xml', svelte: 'javascript',
    dockerfile: 'dockerfile', makefile: 'makefile', nginx: 'nginx', conf: 'nginx',
    ini: 'ini', cfg: 'ini', properties: 'properties', toml: 'ini', md: 'markdown', log: 'plaintext',
    // 补齐的常用语言
    scala: 'scala', sc: 'scala', sbt: 'scala',
    dart: 'dart', hs: 'haskell', lhs: 'haskell',
    ex: 'elixir', exs: 'elixir', erl: 'erlang', hrl: 'erlang', elm: 'elm',
    clj: 'clojure', cljs: 'clojure', cljc: 'clojure',
    groovy: 'groovy', gvy: 'groovy', gradle: 'gradle',
    fs: 'fsharp', fsx: 'fsharp', ml: 'ocaml', mli: 'ocaml', re: 'reasonml',
    jl: 'julia', nim: 'nim', m: 'objectivec', mm: 'objectivec',
    proto: 'protobuf', http: 'http', htaccess: 'apache',
    vim: 'vim', tex: 'tex', tcl: 'tcl', vb: 'vbnet', v: 'verilog', vhd: 'vhdl', vhdl: 'vhdl',
    sty: 'stylus', styl: 'stylus', glsl: 'glsl', frag: 'glsl', vert: 'glsl',
    cmake: 'cmake', d: 'd', cr: 'crystal', apache: 'apache'
  }
  // 特名文件
  if (f === 'dockerfile') return 'dockerfile'
  if (f === '.dockerignore') return 'bash'
  if (f === 'makefile' || f === 'cmakelists') return 'makefile'
  if (f === 'gemfile' || f.endsWith('.gemfile')) return 'ruby'
  if (f === '.gitignore' || f === '.gitattributes' || f.endsWith('.gitignore')) return 'properties'
  const ext = f.includes('.') ? f.slice(f.lastIndexOf('.') + 1) : ''
  return map[ext] ?? null
}

/** 高亮语言展示名(未识别 → text) */
const codeLangLabel = computed(() => langOf(file.value?.fileName ?? '') ?? 'text')

/** 行数(代码/纯文本渲染用,行号栏对齐) */
const lineCount = computed(() => {
  const c = file.value?.content ?? ''
  return c ? c.split('\n').length : 0
})

/* ---------- 引用行定位（正文文件引用 `` `auth.py:47` `` 带行号打开时） ---------- */

/** 滚动容器与行号列：行号列本身就是**逐行 DOM**，定位与量高都用它，不自行推算行高与内边距 */
const codeScroll = ref<HTMLElement | null>(null)
const codeGutter = ref<HTMLElement | null>(null)
/** 高亮带几何（相对滚动内容坐标；show=false 时隐藏） */
const refBand = reactive({ show: false, top: 0, height: 0 })

/** 引用行（1 起；0 = 无）。仅代码 / 纯文本可定位——markdown 渲染后与源行号无对应关系 */
const refLine = computed(() => (kind.value === 'text' ? (file.value?.targetLine ?? 0) : 0))

/**
 * 滚到引用行并画高亮带。
 *
 * 位置全部**实测**（行号元素 → 相对滚动内容的位置），不按行高 × 行号推算：换行策略、
 * tab 宽度、字号缩放任意变化都不会错位。`flush: 'post'` + 内容变化触发，故内容读完才定位；
 * 切页签回来面板会重新挂载（滚动位置本就归零），顺手再定位一次正是想要的落点。
 */
function revealRefLine(): void {
  const line = refLine.value
  const sc = codeScroll.value
  const gutter = codeGutter.value
  const ln = line && gutter ? (gutter.children[line - 1] as HTMLElement | undefined) : undefined
  if (!ln || !sc) {
    refBand.show = false
    return
  }
  ln.scrollIntoView({ block: 'center', inline: 'nearest' })
  refBand.top = ln.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop
  refBand.height = ln.offsetHeight
  refBand.show = true
}

watch(
  // `revealSeq` 只作触发用：重复点同一引用的同一行时其它状态没变化，靠它再滚一次
  [() => file.value?.content, refLine, () => file.value?.revealSeq, codeScroll, codeGutter],
  () => void nextTick(revealRefLine),
  { flush: 'post' }
)

/**
 * 本文件预览里 markdown 正文的文件引用按**所在目录**解析相对路径
 * （对话正文则注入会话工作区，见 ChatPanel）——预览里的 `./other.md` 指向同一目录。
 */
const fileDir = computed(() => {
  const p = file.value?.filePath ?? ''
  const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'))
  return i > 0 ? p.slice(0, i) : ''
})
provide(FILE_REF_BASE, fileDir)

/* ---------- markdown:frontmatter 元数据块 + 与对话正文共用的 Markdown 渲染 ---------- */

/**
 * frontmatter 解析结果:成功才把元数据交给 `MarkdownFrontmatterHeader`、正文用剥好的 body。
 * 半截/非法 YAML 一律 null → 原文一字不动地照常渲染(与 utils/frontmatter 的约定同源,不丢内容)。
 */
const markdownParsed = computed(() => parseFrontmatter(file.value?.content ?? ''))
const markdownMeta = computed<FrontmatterMeta | null>(() => markdownParsed.value?.meta ?? null)
const markdownBody = computed(() =>
  markdownParsed.value ? markdownParsed.value.body : (file.value?.content ?? '')
)

/** 正文容器:相对图片内联需要拿到渲染后的 DOM */
const markdownRoot = ref<HTMLElement | null>(null)
let stopImgUpgrade: (() => void) | null = null

/**
 * 知识库相对图片内联(未提供 imageBaseRel = 文件树页签,不做内联;提供(空串 = 根层)才升级)。
 * 容器挂载 / 目录变化时重建观察器:markdown 渲染器的图片节点是异步组件,晚于首帧才出现。
 */
watch(
  [markdownRoot, () => props.imageBaseRel],
  ([el, rel]) => {
    stopImgUpgrade?.()
    stopImgUpgrade = el && typeof rel === 'string' ? upgradeRelativeImages(el, rel) : null
  },
  { flush: 'post', immediate: true }
)
onBeforeUnmount(() => {
  stopImgUpgrade?.()
  stopImgUpgrade = null
})

/** 语法高亮后的 HTML(v-html);未识别的语言回落纯文本(escape 由 hljs 处理) */
const highlighted = computed(() => {
  const code = file.value?.content ?? ''
  const lang = langOf(file.value?.fileName ?? '')
  if (!file.value?.content || !lang) return ''
  try {
    return hljs.highlight(code, { language: lang }).value
  } catch {
    return hljs.highlightAuto(code).value
  }
})

/** 二进制提示:黑名单扩展名与后端读取返回空同时成立(空文本文件仍走文本渲染为空) */
const isBinary = computed(() => kind.value === 'binary' && !(file.value?.content))

/** 外部打开(默认应用):桌面走 shell.openPath；浏览器部署没有系统文件管理器 → 下载文件 */
async function openExternal(): Promise<void> {
  const f = file.value
  if (!f) return
  if (isWebShell) {
    try {
      const buf = await window.api.fs.readArrayBuffer(f.filePath)
      if (!buf) throw new Error('读取失败')
      downloadBytes(f.fileName, buf)
    } catch (e) {
      if (props.tabId) {
        tabs.setFileState(props.tabId, {
          error: `${t('panel.filePreview.openFailed')}: ${e instanceof Error ? e.message : String(e)}`
        })
      }
    }
    return
  }
  const err = await window.api.fs.openFolder(f.filePath)
  if (err && props.tabId) {
    tabs.setFileState(props.tabId, { error: `${t('panel.filePreview.openFailed')}: ${err}` })
  }
}
</script>

<template>
  <div class="fp-wrap">
    <!-- 页签不存在(工具页签激活):空态 -->
    <div v-if="!file" class="fp-empty">{{ t('panel.filePreview.title') }}</div>

    <template v-else>
      <!-- 加载中 -->
      <div v-if="file.loading" class="fp-empty">{{ t('common.loading') }}</div>

      <!-- 读取失败 -->
      <div v-else-if="file.error" class="fp-error">
        <span>{{ file.error }}</span>
      </div>

      <!-- 二进制不可预览:提示 + 外部打开 -->
      <div v-else-if="isBinary" class="fp-binary">
        <icon-file :size="28" class="fp-binary-icon" />
        <span class="fp-binary-text">{{ t('panel.filePreview.binary') }}</span>
        <button type="button" class="fp-open-btn" @click="openExternal">
          <icon-launch :size="13" />
          {{ t('panel.filePreview.openExternal') }}
        </button>
      </div>

      <!-- office 文档预览(docx/xlsx/pptx/pdf:astorm-office 高保真 + 悬浮路径拾取;旧版格式内部回退) -->
      <div v-else-if="kind === 'office'" class="fp-office">
        <OfficeViewerRenderer :file-path="file.filePath" :file-name="file.fileName" />
      </div>

      <!-- markdown:frontmatter 元数据块 + 与「正文对话」同一个渲染组件(MarkdownBody),
           排版/代码块/表格/列表样式两处完全一致;本容器只负责滚动与内边距 -->
      <div v-else-if="kind === 'markdown'" ref="markdownRoot" class="fp-markdown">
        <div class="fp-markdown-inner">
          <MarkdownFrontmatterHeader v-if="markdownMeta" :meta="markdownMeta" />
          <MarkdownBody :content="markdownBody" :is-dark="isDark" mode="static" />
        </div>
      </div>

      <!-- 图片预览:居中展示,可滚动看大图 -->
      <div v-else-if="kind === 'image'" class="fp-image">
        <template v-if="imageSrc">
          <img :src="imageSrc" class="fp-image-img" alt="" />
          <span class="fp-image-name">{{ file.fileName }} · {{ imageMime(file.fileName) }}</span>
        </template>
        <div v-else-if="imageError" class="fp-error">
          <span>{{ imageError }}</span>
          <button type="button" class="fp-open-btn" @click="openExternal">
            <icon-launch :size="13" />
            {{ t('panel.filePreview.openExternal') }}
          </button>
        </div>
        <div v-else class="fp-empty">{{ t('common.loading') }}</div>
      </div>

      <!-- 文本/源码:识别到的代码语言走语法高亮(VSCode/GitHub 风格,亮暗跟随主题);否则纯文本等宽。
           行号 + 始终展开跟随面板滚动;引用行高亮带画在该行上 -->
      <div v-else class="fp-text">
        <template v-if="file.content">
          <div class="cv-bar">
            <span class="cv-lang">{{ codeLangLabel }}</span>
            <span class="cv-lines">{{ lineCount }} 行</span>
          </div>
          <div ref="codeScroll" class="cv-scroll">
            <div ref="codeGutter" class="cv-gutter" aria-hidden="true">
              <span v-for="n in lineCount" :key="n" class="cv-ln" :class="{ 'cv-ln-on': n === refLine }">{{ n }}</span>
            </div>
            <pre v-if="highlighted" class="fp-code"><code class="hljs" v-html="highlighted" /></pre>
            <pre v-else class="fp-code">{{ file.content }}</pre>
            <!-- 引用行高亮带:按实测位置绝对定位(随内容滚动),左右铺满整行;不挡选择/复制 -->
            <div
              v-if="refBand.show"
              class="cv-reveal"
              :style="{ top: `${refBand.top}px`, height: `${refBand.height}px` }"
              aria-hidden="true"
            />
          </div>
        </template>
        <div v-else class="fp-empty">{{ t('panel.filePreview.empty') }}</div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.fp-wrap {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* office 预览容器:占满内容区 */
.fp-office {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* markdown:容器只负责「可滚动 + 内边距」(排版全部来自共享的 MarkdownBody,
   与对话正文同一套代码块/表格/列表样式) */
.fp-markdown {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 20px 28px 40px;
  /* 自身铺背景:滚动内容之外的区域(侧边/底部)始终有底色,避免漏出容器色 */
  background: var(--bg-1);
  scrollbar-gutter: stable;
}

/* 阅读列行宽:聊天正文是气泡窄栏,文件预览给一个上限并居中(纯容器布局,不动 markdown 自身样式) */
.fp-markdown-inner {
  max-width: 860px;
  margin: 0 auto;
}

.fp-text {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  /* 自身铺背景:滚动内容之外的区域(侧边/底部)始终有底色,避免漏出容器色 */
  background: var(--bg-1);
  /* 预留右侧滚动条位,内容不因滚动条出现/消失左右移位 */
  scrollbar-gutter: stable;
}

/* 代码预览工具栏:语言 + 行数 + 伸缩(贴合面板滚动时吸顶) */
.cv-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  border-bottom: 1px solid var(--border-base);
  background: var(--bg-1);
  position: sticky;
  top: 0;
  z-index: 1;
  flex-shrink: 0;
}

.cv-lang {
  font-family: var(--font-mono);
  font-size: var(--font-size-md);
  font-weight: 700;
  color: var(--text-primary);
}

.cv-lines {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 代码区:行号列 + 代码列,始终展开(随面板滚动,不再有收起限高)。
   position:relative = 引用行高亮带的定位基准(带子按滚动内容坐标绘制,随内容一起滚) */
.cv-scroll {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  /* 子列高度=内容(flex-start)+min-height:100% 兜底撑满,兼顾短文件铺满与长文件背景覆盖 */
  align-items: flex-start;
  /* 自身铺背景:无论内容高低,滚动区域始终有底色覆盖,避免侧边/底部漏出容器色 */
  background: var(--bg-1);
  /* 预留右侧滚动条位(横向滚动条更细,不左右移位) */
  scrollbar-gutter: stable;
}

/* 行号列:与代码同字号同行高(12.5px × 1.65),逐行严格对齐,滚动到底行号不截断 */
.cv-gutter {
  position: sticky;
  left: 0;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  min-width: 44px;
  /* 与代码列同步:短内容撑满,长内容随内容增长 */
  min-height: 100%;
  padding: 12px 8px 12px 0;
  text-align: right;
  background: var(--bg-1);
  border-right: 1px solid var(--border-light);
  user-select: none;
  color: var(--text-tertiary);
  font-family: var(--font-mono);
  font-size: var(--font-size-code);
  line-height: 1.65;
}

/* 引用行:行号加粗提色(与代码区高亮带同一条线) */
.cv-ln-on {
  color: var(--accent-text);
  font-weight: 700;
}

/* 引用行高亮带:半透明铺在该行上(在代码文字之上,像选区;不挡选择/复制与滚动)。
   左右铺满容器:行号列与代码列在同一行上一起点亮 */
.cv-reveal {
  position: absolute;
  left: 0;
  right: 0;
  z-index: 0;
  pointer-events: none;
  background: color-mix(in srgb, var(--accent-text) 12%, transparent);
  border-top: 1px solid color-mix(in srgb, var(--accent-text) 30%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--accent-text) 30%, transparent);
}

/* 源码:等宽字体,不换行(行号列对齐),超长横向滚动。
   中和 hljs 主题自带样式:padding/白底/overflow 会让代码与行号错位并产生色块。
   flex:1 让代码列占满剩余宽度;align-items:stretch 下高度同步铺满(短文件也覆盖整个内容区) */
.fp-code {
  margin: 0;
  padding: 12px 16px;
  flex: 1;
  min-width: 0;
  /* 短内容时撑满容器高度(覆盖整个内容区);长内容时随内容增长 */
  min-height: 100%;
  font-family: var(--font-mono);
  font-size: var(--font-size-code);
  line-height: 1.65;
  color: var(--text-primary);
  white-space: pre;
  tab-size: 2;
  background: var(--bg-1);
}

.fp-code .hljs {
  display: block;
  padding: 0;
  margin: 0;
  overflow-x: visible;
  background: transparent;
  color: inherit;
}

/* 图片预览:居中,留边滚动,大图可缩放查看 */
.fp-image {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-start;
  gap: 10px;
  padding: 16px 20px;
}

.fp-image-img {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  border-radius: 4px;
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.15);
  background: #ffffff;
}

.fp-image-name {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 空态 / 加载 / 错误:居中 */
.fp-empty,
.fp-error,
.fp-binary {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

.fp-error {
  color: var(--danger);
  word-break: break-all;
}

.fp-binary-icon {
  color: var(--text-tertiary);
}

/* 外部打开按钮 */
.fp-open-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    border-color 0.15s ease;
}

.fp-open-btn:hover {
  background-color: var(--bg-hover);
  border-color: var(--border-strong);
}
</style>