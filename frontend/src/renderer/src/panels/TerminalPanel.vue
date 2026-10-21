<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { useSessionsStore } from '@/stores/sessions'
import { invoke } from '@/services/backend'
import { resolveTerminalScheme } from '@/theme/terminal'
import { Terminal } from 'xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import 'xterm/css/xterm.css'

const props = defineProps<{
  /**
   * 本终端实例绑定的**会话 id(前端会话 id)**。
   * 每个「打开过终端」的会话各挂一个常驻实例（由 PanelContainer v-for 渲染），
   * 切换会话/页签时只显隐、不卸载 → xterm 缓冲与 PTY 会话全程保留，历史不丢。
   */
  sessionId: string
}>()

const { t } = useI18n()
const settings = useSettingsStore()
const tabs = useTabsStore()
const sessions = useSessionsStore()

/** 本实例所属会话(终端 shell 在其工作空间内运行) */
const session = computed(() => sessions.sessions.find((s) => s.id === props.sessionId) ?? null)

/** 该会话当前是否为激活会话(仅激活会话才更新页签标题) */
const isActiveSession = computed(() => sessions.activeId === props.sessionId)

/** 会话工作空间末段目录名(页签标题) */
const workspaceDir = computed(() => {
  const ws = session.value?.workspace?.trim()
  if (!ws) return ''
  const parts = ws.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? ws
})

/** 本会话的终端页签 id(仅当它正是激活会话且激活页签为终端时) */
const tabId = computed(() =>
  isActiveSession.value && tabs.activeTab?.toolId === 'terminal' ? tabs.activeTab.id : ''
)
watch(
  [workspaceDir, tabId],
  () => {
    if (!tabId.value) return
    tabs.setTabLabel(tabId.value, workspaceDir.value || t('panel.terminal.title'))
  },
  { immediate: true }
)

// ── xterm 终端实例(xterm.js 仿真 + ConPTY shell 会话) ──
const termElRef = ref<HTMLElement | null>(null)

/** 当前终端配色(跟随主题时每次显式重算——CSS 变量不在响应式依赖里,computed 会缓存旧值) */
const scheme = ref(resolveTerminalScheme(settings.terminalStyle))

/** 重算配色并应用到 xterm + 容器背景(风格切换/主题变化时调用) */
function refreshScheme(): void {
  scheme.value = resolveTerminalScheme(settings.terminalStyle)
  const s = scheme.value
  // 完整采用方案配色(含随主题自适应的 cursor),不做覆盖,保证明暗适配
  term.options.theme = { ...s }
  if (termElRef.value) termElRef.value.style.background = s.background
  term.refresh(0, term.rows - 1)
}

/** 主题(明暗/主题色)变化 → 重解析配色(固定风格重算结果相同,一并保持容器同步) */
let themeObserver: MutationObserver | null = null

onMounted(() => {
  themeObserver = new MutationObserver(() => refreshScheme())
  themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
  themeObserver.observe(document.body, { attributes: true, attributeFilter: ['data-theme-preset'] })
})
onBeforeUnmount(() => {
  themeObserver?.disconnect()
  themeObserver = null
})

const term = new Terminal({
  fontFamily: "Menlo, Monaco, 'Cascadia Code', 'SF Mono', Consolas, 'Courier New', monospace",
  fontSize: 13,
  lineHeight: 1.35,
  fontWeight: '600',
  fontWeightBold: '700',
  cursorBlink: true,
  cursorStyle: 'bar',
  cursorInactiveStyle: 'bar',
  cursorWidth: 3,
  scrollback: 5000,
  allowProposedApi: false
})
const fitAddon = new FitAddon()

/** 当前 PTY 所属会话 id(后端 shell 按会话持久,切换会话后输出仅归集到对应 shell) */
const currentSid = ref('')

let offEvent: (() => void) | null = null
let resizeObserver: ResizeObserver | null = null
/** 「shell 已断开」提示是否已写过(每次 openPty 重置,避免逐键刷屏) */
let ptyDeadNotified = false

/** 从事件总线上收取本会话 shell 输出(部分输出块直接交给 xterm 缓冲) */
function onBackendEvent(chunk: unknown): void {
  const ev = chunk as { type?: string; session_id?: string; data?: string }
  if (!ev || ev.session_id !== currentSid.value) return
  if (ev.type === 'terminal_output' && ev.data) {
    term.write(ev.data)
  } else if (ev.type === 'terminal_exit') {
    term.write('\r\n[进程已退出,可重新打开终端重建 shell]')
  }
}

/** 同步 xterm 与后端 PTY 尺寸(fit 后把真实的行/列回传) */
function fitAndSync(): void {
  // 面板被 v-show 隐藏(display:none)时尺寸为 0：跳过，避免把 0 行列回传给 PTY
  const el = termElRef.value
  if (!el || el.clientWidth === 0 || el.clientHeight === 0) return
  try {
    fitAddon.fit()
  } catch {
    return
  }
  const sid = currentSid.value
  if (!sid) return
  void invoke('pty_resize', { session_id: sid, cols: term.cols, rows: term.rows }).catch(() => {})
}

/** 打开(或复用)会话的交互式 shell,并把输入转发给后端 */
async function openPty(): Promise<void> {
  const sid = session.value?.backendId ?? ''
  currentSid.value = sid
  if (!sid) return
  ptyDeadNotified = false
  try {
    await nextTick()
    // 常驻挂载后可能在「非激活(display:none)」时打开：此时尺寸为 0，先用默认行列，
    // 待真正显示时由 ResizeObserver → fitAndSync 回传正确尺寸
    const el = termElRef.value
    if (el && el.clientWidth > 0 && el.clientHeight > 0) fitAddon.fit()
    await invoke('pty_open', { session_id: sid, cols: term.cols, rows: term.rows })
    term.focus()
  } catch (err) {
    term.writeln(`\r\n[终端打开失败] ${err instanceof Error ? err.message : String(err)}`)
  }
}

term.onData((data) => {
  if (!currentSid.value) return
  void invoke('pty_write', { session_id: currentSid.value, data }).catch((err) => {
    // shell 已不存在(容器/后端重启后页面未刷新,或 shell 自行退出):明确提示一次,
    // 避免"敲键盘没反应"的假死观感（正常网络抖动不触发:按后端错误串精确匹配）
    if (!ptyDeadNotified && String(err).includes('终端会话未打开')) {
      ptyDeadNotified = true
      term.write('\r\n[终端已断开,关闭并重新打开终端页签可重建 shell]')
    }
  })
})

// 点击终端重新聚焦(闪烁依赖焦点;失焦时保留 bar 形态,聚焦后进入闪烁)
function focusTerm(): void {
  term.focus()
  // 兜底:直接聚焦 xterm 辅助输入元素,确保真正持有焦点以启动光标闪烁
  const helper = termElRef.value?.querySelector('.xterm-helper-textarea') as HTMLElement | null
  helper?.focus()
}

onMounted(() => {
  if (!termElRef.value) return
  term.loadAddon(fitAddon)
  term.loadAddon(new WebLinksAddon())
  term.open(termElRef.value)
  refreshScheme()
  offEvent = window.api?.backend?.onEvent?.(onBackendEvent) ?? null
  resizeObserver = new ResizeObserver(() => fitAndSync())
  if (termElRef.value) {
    resizeObserver.observe(termElRef.value)
    termElRef.value.addEventListener('mousedown', focusTerm)
    termElRef.value.addEventListener('focus', focusTerm)
    // xterm 辅助输入元素(承载焦点)不拦截焦点时,兜底让其可聚焦并聚焦
    const helper = termElRef.value.querySelector('.xterm-helper-textarea') as HTMLElement | null
    if (helper) helper.tabIndex = 0
  }
  void openPty()
})

// 终端风格切换:立即换肤(跟随主题随主题变化由 MutationObserver 驱动)
watch(() => settings.terminalStyle, () => refreshScheme())

/**
 * 后端会话 id 变化(本地会话发出首条消息后才有 backendId,或后端重建):
 * 重置屏幕并打开/复用该会话的 shell。
 * **只针对本实例绑定的会话**——实例按会话常驻，会话切换不会触发这里（不丢历史）。
 */
watch(
  () => session.value?.backendId,
  () => {
    term.reset()
    void openPty()
  }
)

onBeforeUnmount(() => {
  offEvent?.()
  offEvent = null
  resizeObserver?.disconnect()
  resizeObserver = null
  if (termElRef.value) {
    termElRef.value.removeEventListener('mousedown', focusTerm)
    termElRef.value.removeEventListener('focus', focusTerm)
  }
  term.dispose()
})
</script>

<template>
  <div class="term-wrap" :style="{ background: scheme.background }">
    <!-- xterm 挂载点:整块终端屏幕,点击/输入天然聚焦,随容器尺寸变化 fit -->
    <div ref="termElRef" class="term-root" />
  </div>
</template>

<style scoped>
/* 终端屏幕:背景由当前配色方案驱动(跟随主题或固定经典方案),不依赖固定值 */
.term-wrap {
  height: 100%;
  display: flex;
  flex-direction: column;
}

.term-root {
  flex: 1;
  min-height: 0;
  /* 左右各 5px 内边距(上下贴边),保持终端与内容区齐平的同时两侧留一点呼吸空间 */
  padding: 0 5px;
  overflow: hidden;
}
</style>