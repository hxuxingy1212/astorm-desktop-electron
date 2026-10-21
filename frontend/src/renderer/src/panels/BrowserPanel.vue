<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import SegmentedControl from '@/components/common/SegmentedControl.vue'
import { useTabsStore } from '@/stores/tabs'
import { useElementPickStore } from '@/stores/elementPick'
import { useSessionsStore } from '@/stores/sessions'
import { useSettingsStore } from '@/stores/settings'
import { autoImportChromeCookies } from '@/services/browserData'
import { buildElementBlock, elementChipLabel, withCard } from '@/utils/elementBlock'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { BROWSER_DESKTOP_1280, ZOOM_LEVELS, ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, clampZoomPercent } from '@/utils/browserViewport'
import { isWebShell } from '@/utils/platform'
import { takePendingNav } from '@/services/webLinks'
import type { WebViewportState } from '@/types'

const { t } = useI18n()
const tabs = useTabsStore()
const elementPick = useElementPickStore()
const sessions = useSessionsStore()
const settings = useSettingsStore()

/**
 * 本组件对应的浏览器页签 id：**每页签一个实例**（PanelContainer 用 v-for 常驻挂载、
 * 非激活只 v-show 隐藏），这样每个页签的 iframe/OOPIF 都存活，供「一会话一浏览器实例」
 * 同时驱动多个页签。
 */
const props = defineProps<{ tabId: string }>()
/** 是否当前激活页签:激活实例加 `.bw-pane--active` 标记,主进程据此定位可见 iframe(截图/拾取/几何) */
const isActiveTab = computed(() => tabs.activeId === props.tabId)

/** 本页签的浏览状态(无则空态) */
const web = computed(() => tabs.tabs.find((x) => x.id === props.tabId)?.web ?? null)
/** 空态:有网页页签但尚无有效网址 */
const isEmpty = computed(() => !web.value?.url)

/** 页签内导航历史(每次导航 push;后退/前进只在栈内移动) */
const history = ref<{ url: string; title: string }[]>([])
const historyIndex = ref(-1)
const iframeRef = ref<HTMLIFrameElement | null>(null)
/** 本实例的视口舞台(多实例时必须用 ref,不能 document.querySelector 拿错页签) */
const stageRef = ref<HTMLDivElement | null>(null)
const loading = ref(false)

/* ── 容器形态：服务端 headless Chromium 画面流（canvas + 输入回传）──────────
 * 桌面走 iframe（主进程 CDP 桥）；浏览器部署没有本机 Chromium，改为连服务端 WS：
 * 下行 JPEG 帧画到 canvas，上行鼠标 / 键盘 / 导航 / 页签操作——与 AI 的 browser 工具
 * 操作同一个页面（"AI 点哪里，用户看得见"）。 */
const canvasRef = ref<HTMLCanvasElement | null>(null)
/** 页面视口尺寸（CSS 像素，来自服务端 state）：坐标回传按它换算——帧可能被 maxWidth
 * 降采样，故不能用帧图像尺寸当视口 */
const viewportSize = ref<{ w: number; h: number }>({ w: BROWSER_DESKTOP_1280.w, h: BROWSER_DESKTOP_1280.h })
let streamWs: WebSocket | null = null
let streamTimer: ReturnType<typeof setTimeout> | null = null
let streamClosedByUs = false
/** 连接代次：会话切换/重连时自增，旧 socket 的迟到回调据此作废 */
let streamGen = 0
/** 画面流按**当前激活会话**取（服务端浏览器按根会话归属） */
const streamSessionId = computed(() => sessions.activeId || '')

/** 建连前的待发消息：空态首次导航时 WS 尚未建立，直接丢会丢导航（页签停在空态） */
const pendingStreamMsgs: string[] = []
/** 最近一帧：首帧可能先于 canvas 挂载到达（空态导航），留下待 canvas 就绪后补画 */
let lastStreamFrame: string | null = null
/** 本次连接是否已把「本页签网址」同步给服务端空白页（防止失败时反复重发成环） */
let streamNavSynced = false

function streamSend(msg: Record<string, unknown>): void {
  const text = JSON.stringify(msg)
  if (streamWs && streamWs.readyState === WebSocket.OPEN) {
    streamWs.send(text)
    return
  }
  if (!isWebShell) return
  pendingStreamMsgs.push(text)
  if (pendingStreamMsgs.length > 20) pendingStreamMsgs.shift()
  if (!streamWs) connectStream()
}

function drawStreamFrame(dataUrl: string): void {
  const canvas = canvasRef.value
  if (!canvas) return
  const img = new Image()
  img.onload = () => {
    if (img.width && img.height && (canvas.width !== img.width || canvas.height !== img.height)) {
      canvas.width = img.width
      canvas.height = img.height
    }
    canvas.getContext('2d')?.drawImage(img, 0, 0, canvas.width, canvas.height)
  }
  img.src = dataUrl
}

// 空态首帧先于 canvas 挂载时，帧会被丢掉而静态页面不会再发帧 → canvas 一就绪补画最近一帧
watch(canvasRef, (canvas) => {
  if (canvas && lastStreamFrame) drawStreamFrame(lastStreamFrame)
})

/** canvas 坐标 → 页面 CSS 像素（服务端按视口坐标派发鼠标事件） */
function toPagePoint(e: MouseEvent): { x: number; y: number } {
  const canvas = canvasRef.value
  if (!canvas) return { x: 0, y: 0 }
  const rect = canvas.getBoundingClientRect()
  const sx = viewportSize.value.w / (rect.width || 1)
  const sy = viewportSize.value.h / (rect.height || 1)
  return { x: Math.round((e.clientX - rect.left) * sx), y: Math.round((e.clientY - rect.top) * sy) }
}

function onCanvasClick(e: MouseEvent): void {
  const p = toPagePoint(e)
  streamSend({ type: 'mouse', action: 'click', x: p.x, y: p.y, button: e.button === 2 ? 'right' : 'left' })
}
function onCanvasMove(e: MouseEvent): void {
  const p = toPagePoint(e)
  streamSend({ type: 'mouse', action: 'move', x: p.x, y: p.y })
}
function onCanvasWheel(e: WheelEvent): void {
  e.preventDefault()
  const p = toPagePoint(e)
  streamSend({ type: 'mouse', action: 'wheel', x: p.x, y: p.y, deltaX: e.deltaX, deltaY: e.deltaY })
}
/** 键盘：可打印字符走 insertText（对输入框最稳），特殊键走 CDP 按键事件 */
const SPECIAL_KEYS = new Set([
  'Enter', 'Tab', 'Escape', 'Backspace', 'Delete', 'ArrowUp', 'ArrowDown',
  'ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown'
])
function onCanvasKeydown(e: KeyboardEvent): void {
  if (e.ctrlKey || e.metaKey) {
    if (e.key.length === 1) {
      streamSend({ type: 'press', key: `${e.ctrlKey ? 'Control' : 'Meta'}+${e.key}` })
      e.preventDefault()
    }
    return
  }
  if (SPECIAL_KEYS.has(e.key)) {
    streamSend({ type: 'press', key: e.key })
    e.preventDefault()
    return
  }
  if (e.key.length === 1) {
    streamSend({ type: 'text', text: e.key })
    e.preventDefault()
  }
}

function closeStream(): void {
  streamClosedByUs = true
  streamGen += 1
  if (streamTimer) clearTimeout(streamTimer)
  streamTimer = null
  const ws = streamWs
  streamWs = null
  ws?.close()
}

function connectStream(): void {
  if (!isWebShell) return
  const sid = streamSessionId.value
  if (!sid) return
  const url = (window.api as unknown as { browser?: { screencastUrl?: (s: string) => string } })
    .browser?.screencastUrl?.(sid)
  if (!url) return
  streamClosedByUs = false
  streamNavSynced = false
  // 代次：切换会话/重连后，旧 socket 的迟到回调（onclose 重连、onmessage 画帧）一律作废
  const gen = ++streamGen
  const ws = new WebSocket(url)
  streamWs = ws
  ws.onopen = () => {
    if (gen !== streamGen) return
    // 补发建连前积压的操作（典型：空态地址栏回车 → nav）
    const queued = pendingStreamMsgs.splice(0)
    for (const text of queued) ws.send(text)
  }
  ws.onmessage = (ev) => {
    if (gen !== streamGen) return
    try {
      const msg = JSON.parse(String(ev.data)) as {
        type: string
        data?: unknown
        message?: string
        width?: number
        height?: number
      }
      if (msg.type === 'frame' && typeof msg.data === 'string') {
        lastStreamFrame = `data:image/jpeg;base64,${msg.data}`
        drawStreamFrame(lastStreamFrame)
      } else if (msg.type === 'state') {
        const st = msg.data as { url?: string; title?: string; width?: number; height?: number } | undefined
        if (st?.width && st?.height) viewportSize.value = { w: st.width, h: st.height }
        // 服务端还停在空白页而本页签有网址（重开面板续看 / 首帧先于导航）→ 把页面导航过去
        const local = web.value?.url
        if (st?.url === 'about:blank' && local && local !== 'about:blank') {
          if (!streamNavSynced) {
            streamNavSynced = true
            streamSend({ type: 'nav', url: local })
          }
          return
        }
        if (st?.url) {
          address.value = st.url
          const w = web.value
          if (w) {
            w.url = st.url
            tabs.setWebState(props.tabId, { url: st.url, title: st.title || st.url })
          }
        }
      } else if (msg.type === 'error') {
        console.warn('[browser] 画面流错误:', msg.message)
      }
    } catch {
      /* 忽略非 JSON 帧 */
    }
  }
  ws.onclose = () => {
    if (gen !== streamGen) return
    streamWs = null
    if (streamClosedByUs) return
    // 断线重连（面板常驻，服务端重启/空闲回收后自动恢复）
    streamTimer = setTimeout(connectStream, 2000)
  }
}

/**
 * 历史栈是**页签级**状态,而组件实例与页签一一对应(v-for key = tab.id),故初始化一次即可,
 * 不需要在切换时重置 —— 实例随页签关闭而销毁。
 */
function resetHistory(): void {
  const w = web.value
  history.value = w?.url ? [{ url: w.url, title: w.title || w.url }] : []
  historyIndex.value = w?.url ? 0 : -1
}
resetHistory()

/** 地址栏(本页签状态同步) */
const address = ref('')
watch(
  [web, () => web.value?.url],
  () => {
    address.value = web.value?.url ?? ''
  },
  { immediate: true }
)

/*
 * AI 代理操控的桥生命周期已**上移到会话语义**(见 services/tabsSync.ts):
 * 由它把本会话全部浏览器页签(含非激活)一次性上报主进程,由主进程按会话起停一条多目标桥。
 * 这里不再按单页签起停 —— 每个 BrowserPanel 实例都是常驻的,自行起停会互相拆桥。
 */

/*
 * 自动化控制(设置页「自动化控制」):打开网页页签时自动从本机 Chrome 导入 Cookie。
 * 内置浏览器与宿主 UI 共用默认会话,而它是跨域 iframe —— 站点自己的登录态带不进来,
 * 靠导入补齐。本进程只跑一次(守卫在服务里),避免反复弹 macOS 钥匙串授权框。
 */
watch(
  () => web.value?.url,
  (url) => {
    if (!url || !settings.autoImportChromeCookies) return
    // 浏览器部署没有本机 Chrome（服务端也未实现该命令），自动导入只会弹失败 toast
    if (isWebShell) return
    // 静默自动导入(本进程一次,守卫在服务里):成功不弹提示,避免每次启动后首次打开
    // 网页页签都冒一条 toast；失败仍提示——它是「站点没带上登录态」的唯一解释
    // (钥匙串被拒 / 未装 Chrome / 后端未同步)
    void autoImportChromeCookies().catch((e) => {
      Message.warning(t('panel.browser.autoImportFailed', { msg: e instanceof Error ? e.message : String(e) }))
    })
  },
  { immediate: true }
)

function canGoBack(): boolean {
  return historyIndex.value > 0 && !!web.value
}
function canGoForward(): boolean {
  return historyIndex.value < history.value.length - 1 && !!web.value
}

/** 统一导航入口:入栈当前页签 url(清空后续),再加载新 url;同 url 刷新 */
function navigate(url: string, opts?: { replace?: boolean }): void {
  const target = normalizeUrl(url)
  const w = web.value
  if (!w) return
  if (!opts?.replace) {
    const stack = history.value.slice(0, historyIndex.value + 1)
    stack.push({ url: target, title: address.value || target })
    history.value = stack
    historyIndex.value = stack.length - 1
  }
  w.url = target
  address.value = target
  tabs.setWebState(props.tabId, { url: target, title: address.value || target })
  // 容器形态：导航交给服务端浏览器（iframe 分支保持原样，由 src 绑定驱动）
  if (isWebShell) streamSend({ type: 'nav', url: target })
}

function normalizeUrl(input: string): string {
  const v = input.trim()
  if (!v) return ''
  if (/^https?:\/\//i.test(v)) return v
  if (/^[\w-]+(\.[\w-]+)+/.test(v)) return `https://${v}`
  return v
}

function onAddressSubmit(): void {
  const v = normalizeUrl(address.value)
  if (v && v !== web.value?.url) navigate(v)
  else if (v) refresh()
}

function goBack(): void {
  if (!canGoBack()) return
  historyIndex.value -= 1
  const target = history.value[historyIndex.value]
  const w = web.value
  if (!w) return
  w.url = target.url
  address.value = target.url
  tabs.setWebState(props.tabId, { url: target.url })
}

function goForward(): void {
  if (!canGoForward()) return
  historyIndex.value += 1
  const target = history.value[historyIndex.value]
  const w = web.value
  if (!w) return
  w.url = target.url
  address.value = target.url
  tabs.setWebState(props.tabId, { url: target.url })
}

/** 刷新护送定时器:加载超时(如站点无法访问)时兜底收起 spinner */
let refreshTimer: ReturnType<typeof setTimeout> | null = null
async function refresh(): Promise<void> {
  const w = web.value
  if (!w) return
  if (isWebShell) {
    loading.value = true
    streamSend({ type: 'history', action: 'reload' })
    if (refreshTimer) clearTimeout(refreshTimer)
    refreshTimer = setTimeout(() => {
      loading.value = false
    }, 2000)
    return
  }
  loading.value = true
  const src = w.url
  if (iframeRef.value) iframeRef.value.src = ''
  await nextTick()
  if (iframeRef.value) iframeRef.value.src = src
  // spinner 由 iframe 的 load 事件(onFrameLoad)收起;此处兜底防卡死
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(() => {
    loading.value = false
  }, 6000)
}

/** iframe 载入完成:同域可读 title;跨域回退 url;回写 favicon */
function onFrameLoad(): void {
  loading.value = false
  const w = web.value
  const frame = iframeRef.value
  if (!w || !frame || !address.value) return
  let title = ''
  let favicon = ''
  try {
    title = frame.contentWindow?.document.title?.trim() || ''
    const ico = frame.contentWindow?.document.querySelector<HTMLLinkElement>('link[rel*="icon"]')
    if (ico?.href) favicon = ico.href
  } catch {
    // 跨域:不可读,保持 url 与默认图标
  }
  tabs.setWebState(props.tabId, {
    title: title || address.value,
    favicon: favicon || defaultFavicon(w.url)
  })
  // 修正栈内标题
  if (historyIndex.value >= 0 && history.value[historyIndex.value]) {
    history.value[historyIndex.value].title = title || address.value
  }
}

/** 默认站点图标:Google favicon 服务;加载失败由模板回退图标 */
function defaultFavicon(url: string): string {
  try {
    const u = new URL(url)
    return `https://www.google.com/s2/favicons?domain=${u.hostname}&sz=32`
  } catch {
    return ''
  }
}

/* ── 缩放:百分比列表,transform 缩放页面(布局视口 = 视觉尺寸 / zoom);
      档位表已提到 @/utils/browserViewport(页签同步层也要做百分比换算) ── */

/* ── 尺寸:自定义长宽 + 预设方案 ── */
interface SizePreset {
  label: string
  w: number
  h: number
}
const SIZE_PRESETS: SizePreset[] = [
  { label: '桌面 1366', w: 1366, h: 800 },
  { label: '桌面 1280', w: 1280, h: 800 },
  { label: '桌面 1024', w: 1024, h: 768 },
  { label: '笔记本 900', w: 900, h: 600 },
  { label: '平板 768', w: 768, h: 1024 },
  { label: '手机 375', w: 375, h: 667 }
]

/**
 * 视口/缩放的默认值:页签内无记录(web.viewport 缺省)时使用。
 * 统一为桌面 1280（1280×800）+ 50%（与所有打开浏览器页签的入口一致）。
 */
const DEFAULT_VIEWPORT: WebViewportState = { ...BROWSER_DESKTOP_1280 }

const frameW = ref(DEFAULT_VIEWPORT.w)
const frameH = ref(DEFAULT_VIEWPORT.h)
const presetIndex = ref(DEFAULT_VIEWPORT.presetIndex)
/** 视口模式:adaptive = 铺满窗口(默认);custom = 按自定义尺寸居中 */
const frameMode = ref<WebViewportState['mode']>(DEFAULT_VIEWPORT.mode)
const zoomPercent = ref(DEFAULT_VIEWPORT.zoomPercent)
function currentZoom(): number {
  return zoomPercent.value / 100
}

/** 缩放步进（⋯ 菜单的 − / +）：跳到相邻档位，越过端点时夹在边界 */
function stepZoom(delta: 1 | -1): void {
  const levels = ZOOM_LEVELS.map((z) => Math.round(z * 100))
  const next =
    delta > 0
      ? (levels.find((p) => p > zoomPercent.value) ?? ZOOM_MAX_PERCENT)
      : ([...levels].reverse().find((p) => p < zoomPercent.value) ?? ZOOM_MIN_PERCENT)
  zoomPercent.value = next
}

/** 预设按钮:写入宽高并切到自定义视口（自适应下点预设也应立刻看到效果） */
function applyPreset(i: number): void {
  const p = SIZE_PRESETS[i]
  if (!p) return
  presetIndex.value = i
  frameW.value = p.w
  frameH.value = p.h
  frameMode.value = 'custom'
}

/** 「铺满窗口 / 自定义尺寸」分段选项 */
const frameModeOptions = computed<{ label: string; value: WebViewportState['mode'] }[]>(() => [
  { label: t('panel.browser.fitWindow'), value: 'adaptive' },
  { label: t('panel.browser.customSize'), value: 'custom' }
])

/** 外层画板 = 视觉尺寸(缩放后),供舞台水平垂直居中;铺满模式占满舞台 */
const boxStyle = computed(() => {
  if (frameMode.value === 'adaptive') return { width: '100%', height: '100%' }
  const z = currentZoom()
  return { width: `${Math.round(frameW.value * z)}px`, height: `${Math.round(frameH.value * z)}px` }
})

/** iframe = 布局尺寸(视觉 / zoom),transform 缩回视觉大小;zoom<1 可见范围更大,>1 放大 */
const frameStyle = computed(() => {
  const z = currentZoom()
  if (frameMode.value === 'adaptive') {
    if (z === 1) return { width: '100%', height: '100%' }
    return {
      width: `${100 / z}%`,
      height: `${100 / z}%`,
      transform: `scale(${z})`,
      transformOrigin: '0 0'
    }
  }
  const size = { width: `${frameW.value}px`, height: `${frameH.value}px` }
  if (z === 1) return size
  return { ...size, transform: `scale(${z})`, transformOrigin: '0 0' }
})

function onCustomW(e: Event): void {
  frameW.value = clamp(Number((e.target as HTMLInputElement).value) || 0)
  presetIndex.value = -1
}
function onCustomH(e: Event): void {
  frameH.value = clamp(Number((e.target as HTMLInputElement).value) || 0)
  presetIndex.value = -1
}
function clamp(v: number): number {
  return Math.min(2400, Math.max(200, v))
}

/* ── 画布四角锚点：拖拽改缩放百分比 ─────────────────────────────
   画板在舞台里居中（margin: auto），四条边随缩放同时向两侧伸缩，
   于是锚点沿**对角线**跟随指针：角点到中心的距离 d 随投影位移 p 线性变化，
   缩放比 = (d + p) / d。宽高不变，只改百分比；拖拽中显示当前值。
   锚点层盖在**可见画板区域**上（画板比面板宽时自动贴住可见边缘）。 */
const frameBoxRef = ref<HTMLElement | null>(null)
const viewportRef = ref<HTMLElement | null>(null)
const resizing = ref(false)
type ResizeCorner = 'nw' | 'ne' | 'sw' | 'se'
const RESIZE_CORNERS: ResizeCorner[] = ['nw', 'ne', 'sw', 'se']
/** 可见画板区域（相对 .bw-viewport 的 px）；null = 不可见 */
const anchorBox = ref<{ left: number; top: number; width: number; height: number } | null>(null)
const anchorLayerStyle = computed(() => {
  const b = anchorBox.value
  if (!b) return { display: 'none' }
  return {
    left: `${b.left}px`,
    top: `${b.top}px`,
    width: `${b.width}px`,
    height: `${b.height}px`
  }
})

function updateAnchorBox(): void {
  const wrap = viewportRef.value
  const frame = frameBoxRef.value
  if (!wrap || !frame) {
    anchorBox.value = null
    return
  }
  const w = wrap.getBoundingClientRect()
  const f = frame.getBoundingClientRect()
  const left = Math.max(0, f.left - w.left)
  const top = Math.max(0, f.top - w.top)
  const right = Math.min(w.width, f.right - w.left)
  const bottom = Math.min(w.height, f.bottom - w.top)
  if (right <= left || bottom <= top) {
    anchorBox.value = null
    return
  }
  anchorBox.value = { left, top, width: right - left, height: bottom - top }
}

let anchorObserver: ResizeObserver | null = null
function onStageScroll(): void {
  updateAnchorBox()
}
watch([viewportRef, frameBoxRef], ([wrap, frame], [oldWrap, oldFrame]) => {
  if (!anchorObserver) return
  if (oldWrap) anchorObserver.unobserve(oldWrap)
  if (oldFrame) anchorObserver.unobserve(oldFrame)
  if (wrap) anchorObserver.observe(wrap)
  if (frame) anchorObserver.observe(frame)
  updateAnchorBox()
})

let resizeState: {
  pointerId: number
  startX: number
  startY: number
  startPercent: number
  /** 拖拽开始时角点到画板中心的像素距离 */
  diag: number
  sx: 1 | -1
  sy: 1 | -1
} | null = null
/** 拖拽期间的指针样式（左右两组对角线的 cursor 不同） */
const resizeCursor = ref<'nwse-resize' | 'nesw-resize'>('nwse-resize')
/** 正在拖拽的角（高亮该手柄） */
const activeCorner = ref<ResizeCorner | null>(null)

function onResizeStart(e: PointerEvent, corner: ResizeCorner): void {
  const box = frameBoxRef.value
  if (!box || frameMode.value !== 'custom') return
  e.preventDefault()
  const rect = box.getBoundingClientRect()
  resizeState = {
    pointerId: e.pointerId,
    startX: e.clientX,
    startY: e.clientY,
    startPercent: zoomPercent.value,
    diag: Math.max(1, 0.5 * Math.hypot(rect.width, rect.height)),
    sx: corner === 'ne' || corner === 'se' ? 1 : -1,
    sy: corner === 'sw' || corner === 'se' ? 1 : -1
  }
  resizeCursor.value = corner === 'ne' || corner === 'sw' ? 'nesw-resize' : 'nwse-resize'
  activeCorner.value = corner
  resizing.value = true
  // 不用 setPointerCapture：拖拽中锚点层/画面可能重排，捕获会丢；
  // 改由全屏 shield 吃掉指针事件 + window 监听，跨过 iframe 也不断。
  window.addEventListener('pointermove', onResizeMove)
  window.addEventListener('pointerup', onResizeEnd)
  window.addEventListener('pointercancel', onResizeEnd)
}

function onResizeMove(e: PointerEvent): void {
  const st = resizeState
  if (!st || st.pointerId !== e.pointerId) return
  // 指针位移在「角点方向」上的投影（右上/左下为对角线反方向，故带符号）
  const projection = (st.sx * (e.clientX - st.startX) + st.sy * (e.clientY - st.startY)) / Math.SQRT2
  zoomPercent.value = clampZoomPercent(st.startPercent * ((st.diag + projection) / st.diag))
}

function onResizeEnd(e: PointerEvent): void {
  if (!resizeState || resizeState.pointerId !== e.pointerId) return
  resizeState = null
  resizing.value = false
  activeCorner.value = null
  window.removeEventListener('pointermove', onResizeMove)
  window.removeEventListener('pointerup', onResizeEnd)
  window.removeEventListener('pointercancel', onResizeEnd)
  // 等本轮 DOM 尺寸落地后再把锚点层贴回新的可见画板
  void nextTick(updateAnchorBox)
}

/*
 * 视口/缩放 ↔ 页签状态同步。
 * web 页签是全局单例,PanelContainer 用 v-else-if 渲染**同一个组件实例**,
 * 切会话时组件不卸载,因此必须在 activeId 变化时把该页签自己的视口回填到本地 ref。
 * 这些值只存页签状态(内存),随会话快照换挡,不落磁盘。
 */

/** 回填当前页签的视口/缩放;页签内无记录则用默认值 */
function seedViewport(): void {
  const v = web.value?.viewport ?? DEFAULT_VIEWPORT
  frameMode.value = v.mode
  frameW.value = v.w
  frameH.value = v.h
  presetIndex.value = v.presetIndex
  zoomPercent.value = clampZoomPercent(v.zoomPercent)
}

/** 用户调整视口/缩放 → 写回本页签(回填时也会触发一次,写入的是同值,幂等) */
watch([frameMode, frameW, frameH, presetIndex, zoomPercent], () => {
  const id = props.tabId
  if (!id) return
  tabs.setWebState(id, {
    viewport: {
      mode: frameMode.value,
      w: frameW.value,
      h: frameH.value,
      presetIndex: presetIndex.value,
      zoomPercent: zoomPercent.value
    }
  })
})

/** 视口状态变化回填(包括后端 `tabs` 工具在同一页签上就地覆盖视口);
 *  本实例与页签一一对应,无需再监听 activeId。回填后写回 watch 会以同值触发一次,幂等。 */
watch(() => web.value?.viewport, seedViewport, { immediate: true })

/**
 * 选择元素:主进程向 iframe 注入拾取脚本(跨域亦可),页面内悬停高亮 + 点击拾取;
 * 结果转成 chip 数据推入 elementPick 桥,由 ChatPanel 插入聊天发送框。
 * 再次点击按钮 / 切换网址 / 面板卸载 → 取消进行中的拾取。
 */
const picking = ref(false)
/** 拾取轮次:取消/重新拾取会让在途响应作废,避免旧响应把新一轮的 picking 状态清掉 */
let pickSeq = 0

/** 悬停信息卡片的字段标签(注入脚本在主进程,无 i18n,故在此取好文案传过去) */
function inspectLabels(): Record<string, string> {
  return {
    color: t('panel.browser.inspect.color'),
    background: t('panel.browser.inspect.background'),
    font: t('panel.browser.inspect.font'),
    padding: t('panel.browser.inspect.padding'),
    margin: t('panel.browser.inspect.margin'),
    component: t('panel.browser.inspect.component'),
    path: t('panel.browser.inspect.path'),
    xpath: t('panel.browser.inspect.xpath'),
    bold: t('panel.browser.inspect.bold'),
    italic: t('panel.browser.inspect.italic')
  }
}

async function onPickElement(): Promise<void> {
  const w = web.value
  if (!w?.url) return
  if (picking.value) {
    cancelPick(w.url)
    return
  }
  const seq = ++pickSeq
  picking.value = true
  let res: Awaited<ReturnType<typeof window.api.browser.pickElement>>
  try {
    res = await window.api.browser.pickElement(w.url, {
      hint: t('panel.browser.pickingHint'),
      labels: inspectLabels()
    })
  } catch (err) {
    // IPC 不可用(主进程未重建/未注册该通道)时会 reject:必须显式提示,否则按钮看起来「点了没反应」
    if (seq === pickSeq) {
      picking.value = false
      Message.error(t('panel.browser.pickFailed', { msg: err instanceof Error ? err.message : String(err) }))
    }
    return
  }
  if (seq !== pickSeq) return
  picking.value = false
  if (!res.ok) {
    if (res.canceled) return
    if (res.message === 'NO_FRAME') Message.warning(t('panel.browser.pickNoFrame'))
    else Message.error(t('panel.browser.pickFailed', { msg: res.message ?? '' }))
    return
  }
  if (!res.data) return
  // 主进程若未返回卡片数据(旧版主进程/异常分支),这里补齐再序列化,保证胶囊与文本块同源
  const data = withCard(res.data)
  const label = elementChipLabel(data)
  elementPick.push({ data, token: buildElementBlock(data, t) })
  Message.success(t('panel.browser.picked', { name: label }))
}

/** 取消进行中的拾取(页面内 Esc 同样可取消) */
function cancelPick(url?: string): void {
  pickSeq += 1
  picking.value = false
  if (url) void window.api.browser.cancelPick(url)
}

/** 导航到新网址即失效:页面已换,覆盖层随之销毁 */
watch(
  () => web.value?.url,
  (_url, prev) => {
    if (picking.value) cancelPick(prev)
  }
)

// 容器形态：挂载即连画面流（等 canvas 就绪）；会话切换 → 重连到新会话的浏览器
watch(
  [() => isWebShell, streamSessionId, () => isEmpty.value],
  () => {
    if (!isWebShell) return
    closeStream()
    if (!isEmpty.value) void nextTick(() => connectStream())
  },
  { immediate: true }
)

/**
 * 外链路由（浏览器部署）：应用里 `window.open` 被 web-bridge 拦下后派发 `astorm:browser-navigate`，
 * 由**当前激活的浏览器面板**就地导航（容器里面板页签都镜像同一台会话浏览器，开新页签只会看到
 * 同一页）；未被接管（面板没开）时由路由服务改为新建页签 + 待办导航。
 */
function onBrowserNavigate(raw: Event): void {
  if (!isWebShell || !isActiveTab.value) return
  const url = (raw as CustomEvent<{ url?: string }>).detail?.url
  if (!url) return
  navigate(url)
  ;(raw as CustomEvent).preventDefault()
}

// 面板挂载时若带着「待办导航」（外链路由新开的页签）→ 立刻把服务端页导航过去
if (isWebShell) {
  window.addEventListener('astorm:browser-navigate', onBrowserNavigate)
  if (takePendingNav(web.value?.url ?? '')) {
    void nextTick(() => navigate(web.value?.url ?? ''))
  }
}

onBeforeUnmount(() => {
  if (picking.value) cancelPick(web.value?.url)
  // 切页签/切会话/关页签都会卸载本组件:录制到此为止并落盘(不留后台采集)
  if (recording.value) stopRecording()
  if (statusTimer) clearTimeout(statusTimer)
  window.removeEventListener('astorm:browser-navigate', onBrowserNavigate)
  anchorObserver?.disconnect()
  anchorObserver = null
  bindStageScroll(null)
  closeStream()
})

/* 锚点层跟随可见画板：尺寸/位置变化（缩放、面板拖宽）与舞台滚动都会重算 */
let stageScrollEl: HTMLElement | null = null
function bindStageScroll(el: HTMLElement | null): void {
  if (stageScrollEl) stageScrollEl.removeEventListener('scroll', onStageScroll)
  stageScrollEl = el
  if (el) el.addEventListener('scroll', onStageScroll, { passive: true })
}
watch(stageRef, (el) => bindStageScroll(el))

onMounted(() => {
  anchorObserver = new ResizeObserver(() => updateAnchorBox())
  if (viewportRef.value) anchorObserver.observe(viewportRef.value)
  if (frameBoxRef.value) anchorObserver.observe(frameBoxRef.value)
  bindStageScroll(stageRef.value)
  updateAnchorBox()
})

/** ⋯ 菜单显隐（受控：菜单内是设置控件，动作项点击后主动收起） */
const moreVisible = ref(false)

/** 更多菜单动作:devtools = 打开 iframe 的调试控制台;其余暂占位 */
function moreAction(val: string): void {
  moreVisible.value = false
  if (val === 'devtools') {
    void window.api.browser.openDevtools(web.value?.url ?? '')
    return
  }
  Message.info(t('common.comingSoon'))
}

/* ── 截图 / 录像:主进程 capturePage 采集 iframe 可见区域 ──
   结果一律用面板内联状态行呈现(不弹 Arco 气泡提示)。 */

const capturing = ref(false)

/** 内联状态行:截图/录像的结果或错误(替代 toast)。ok/err 自动消失,录制中常驻 */
const status = ref<{ kind: 'ok' | 'err'; text: string; title?: string } | null>(null)
let statusTimer: ReturnType<typeof setTimeout> | null = null

function showStatus(kind: 'ok' | 'err', text: string, title?: string): void {
  status.value = { kind, text, title }
  if (statusTimer) clearTimeout(statusTimer)
  statusTimer = setTimeout(() => {
    status.value = null
  }, kind === 'err' ? 8000 : 6000)
}

/**
 * iframe 可视区域 = iframe 视觉矩形 ∩ 舞台可视矩形。
 * `.bw-stage` 是 overflow:auto(可滚动)、`.bw-frame` 被 transform 缩放,
 * 但 getBoundingClientRect() 返回的已是**缩放后的视觉矩形**,正是用户所见。
 * 求交用于处理「舞台滚动后 iframe 部分移出可视区」——那部分像素取不到,capturePage 同样不行。
 * 跨域 iframe 拿不到自身像素,故必须交给主进程采集(渲染层 canvas 方案不可行)。
 */
function visibleFrameRect(): { x: number; y: number; width: number; height: number } | null {
  const frame = iframeRef.value
  // 本实例的舞台(多浏览器页签实例共存,必须用 ref,不能全局 querySelector 命中别的页签)
  const stage = stageRef.value
  if (!frame || !stage) return null
  const fr = frame.getBoundingClientRect()
  const sr = stage.getBoundingClientRect()
  const x = Math.max(fr.left, sr.left)
  const y = Math.max(fr.top, sr.top)
  const right = Math.min(fr.right, sr.right)
  const bottom = Math.min(fr.bottom, sr.bottom)
  if (right - x <= 0 || bottom - y <= 0) return null
  return { x, y, width: right - x, height: bottom - y }
}

/** 当前会话工作区绝对目录(与后端同口径:相对工作区 → <用户目录>/workspace/<相对路径>) */
async function currentWorkspaceDir(): Promise<string> {
  const active = sessions.sessions.find((s) => s.id === sessions.activeId)
  return resolveSessionWorkspaceDir(active?.workspace)
}

async function onCapture(): Promise<void> {
  if (capturing.value) return
  const rect = visibleFrameRect()
  if (!rect) {
    showStatus('err', t('panel.browser.captureNoArea'))
    return
  }
  capturing.value = true
  try {
    const res = await window.api.browser.capture(rect, {
      tabId: props.tabId,
      workspaceDir: await currentWorkspaceDir()
    })
    if (res.ok && res.path) showStatus('ok', t('panel.browser.captureSaved', { path: res.path }), res.path)
    else showStatus('err', t('panel.browser.captureFailed', { msg: res.message ?? '' }))
  } catch (err) {
    showStatus('err', t('panel.browser.captureFailed', { msg: err instanceof Error ? err.message : String(err) }))
  } finally {
    capturing.value = false
  }
}

/* ── 录像:主进程逐帧 capturePage → JPEG → 渲染层画到 canvas → MediaRecorder 编码 webm ──
   为什么这么绕:跨域 iframe 拿不到像素,采集只能在主进程;而 MediaRecorder 是渲染层 Web API,
   故帧必须回传渲染层编码。canvas.captureStream(24) 保证**输出固定 24 帧**;
   实际采集帧率受 capturePage 开销限制(内联状态里如实显示实测值)。 */

/** 输出帧率(用户指定) */
const REC_FPS = 24
/** 单次录制上限,避免无限增长 */
const REC_MAX_SECONDS = 300
/** 编码码率:24fps + 面板尺寸,4Mbps 足够清晰 */
const REC_BITRATE = 4_000_000
/** webm 编码优先级(vp9 体积更小) */
const REC_MIMES = ['video/webm;codecs=vp9', 'video/webm;codecs=vp8', 'video/webm']

const recording = ref(false)
const recSeconds = ref(0)
/** 实测采集帧率(每秒刷新;低于 24 说明机器采集跟不上,输出仍按 24fps 补帧) */
const recFps = ref(0)

let recCanvas: HTMLCanvasElement | null = null
let recCtx: CanvasRenderingContext2D | null = null
let recStream: MediaStream | null = null
let recorder: MediaRecorder | null = null
let recChunks: Blob[] = []
let recRect: { x: number; y: number; width: number; height: number } | null = null
let recStopFlag = false
let recTick: ReturnType<typeof setInterval> | null = null
let recFramesInTick = 0
let recStartedAt = 0
/** 录制开始时的页签名:落盘用它而非 tabs.activeId —— 切页签会触发停录,
 *  那时 activeId 已指向新页签,用它会把文件名挂到错误页签上 */
let recTabId = ''

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms))

function pickRecMime(): string {
  return REC_MIMES.find((m) => MediaRecorder.isTypeSupported(m)) ?? ''
}

/** 录制中/结果的内联状态文案 */
const statusText = computed(() => {
  if (recording.value) {
    const mm = String(Math.floor(recSeconds.value / 60)).padStart(2, '0')
    const ss = String(recSeconds.value % 60).padStart(2, '0')
    return t('panel.browser.recording', { time: `${mm}:${ss}`, fps: recFps.value })
  }
  return status.value?.text ?? ''
})

const statusKind = computed(() => (recording.value ? 'rec' : (status.value?.kind ?? 'ok')))

/** 把一帧 JPEG 画进 canvas(尺寸取首帧像素,避免每帧重算) */
async function drawFrame(data: Uint8Array): Promise<boolean> {
  if (!recCtx || !recCanvas) return false
  try {
    const bmp = await createImageBitmap(new Blob([data as BlobPart], { type: 'image/jpeg' }))
    recCtx.drawImage(bmp, 0, 0, recCanvas.width, recCanvas.height)
    bmp.close()
    return true
  } catch {
    return false
  }
}

/** 采集循环:顺序 await(不排队积压),按 24fps 目标节流 */
async function recordLoop(interval: number): Promise<void> {
  while (!recStopFlag) {
    const t0 = performance.now()
    const res = await window.api.browser.frame(recRect as { x: number; y: number; width: number; height: number })
    if (recStopFlag) break
    if (!res.ok || !res.data) {
      stopRecording()
      showStatus('err', t('panel.browser.recordFailed', { msg: res.message ?? '' }))
      return
    }
    await drawFrame(res.data)
    recFramesInTick++
    const cost = performance.now() - t0
    if (cost < interval) await sleep(interval - cost)
  }
}

async function startRecording(): Promise<void> {
  // 先进入录制态并等布局稳定,再量矩形:内联状态行会占一行高度,
  // 若在它出现前量,录制区域会与稳定布局不一致(要么多截一条面板背景,要么少截一截页面)。
  status.value = null
  recording.value = true
  await nextTick()

  const rect = visibleFrameRect()
  if (!rect) {
    recording.value = false
    showStatus('err', t('panel.browser.captureNoArea'))
    return
  }
  try {
    // 首帧用于确定 canvas 像素尺寸(capturePage 返回的是 DPR 后的物理像素)
    const first = await window.api.browser.frame(rect)
    if (!first.ok || !first.data) {
      recording.value = false
      showStatus('err', t('panel.browser.recordFailed', { msg: first.message ?? '' }))
      return
    }
    recCanvas = document.createElement('canvas')
    recCanvas.width = first.width ?? Math.round(rect.width)
    recCanvas.height = first.height ?? Math.round(rect.height)
    recCtx = recCanvas.getContext('2d')
    if (!recCtx) {
      recording.value = false
      showStatus('err', t('panel.browser.recordFailed', { msg: 'NO_CANVAS' }))
      return
    }
    await drawFrame(first.data)

    recStream = recCanvas.captureStream(REC_FPS)
    recorder = new MediaRecorder(recStream, {
      mimeType: pickRecMime(),
      videoBitsPerSecond: REC_BITRATE
    })
    recChunks = []
    recorder.ondataavailable = (e) => {
      if (e.data.size > 0) recChunks.push(e.data)
    }
    recorder.onstop = () => {
      void saveRecording()
    }
    recorder.start(1000)

    recRect = rect
    recTabId = props.tabId
    recStopFlag = false
    recFramesInTick = 0
    recSeconds.value = 0
    recFps.value = 0
    recTick = setInterval(() => {
      recSeconds.value = Math.floor((Date.now() - recStartedAt) / 1000)
      recFps.value = recFramesInTick
      recFramesInTick = 0
      if (recSeconds.value >= REC_MAX_SECONDS) {
        showStatus('ok', t('panel.browser.recordMaxReached', { min: REC_MAX_SECONDS / 60 }))
        stopRecording()
      }
    }, 1000)
    void recordLoop(1000 / REC_FPS)
  } catch (err) {
    recording.value = false
    showStatus('err', t('panel.browser.recordFailed', { msg: err instanceof Error ? err.message : String(err) }))
  }
}

/** 停止录制:MediaRecorder.stop() 触发 onstop → saveRecording */
function stopRecording(): void {
  if (!recording.value) return
  recStopFlag = true
  recording.value = false
  if (recTick) {
    clearInterval(recTick)
    recTick = null
  }
  try {
    if (recorder && recorder.state !== 'inactive') recorder.stop()
  } catch {
    /* 已停止 */
  }
}

/** 录制结束:webm 字节交主进程落盘 */
async function saveRecording(): Promise<void> {
  const mime = recorder?.mimeType || 'video/webm'
  const blob = new Blob(recChunks, { type: mime })
  recChunks = []
  releaseRecorder()
  if (blob.size === 0) {
    showStatus('err', t('panel.browser.recordEmpty'))
    return
  }
  try {
    const data = new Uint8Array(await blob.arrayBuffer())
    const res = await window.api.browser.recordSave({
      data,
      tabId: recTabId,
      workspaceDir: await currentWorkspaceDir()
    })
    if (res.ok && res.path) showStatus('ok', t('panel.browser.recordSaved', { path: res.path }), res.path)
    else showStatus('err', t('panel.browser.recordFailed', { msg: res.message ?? '' }))
  } catch (err) {
    showStatus('err', t('panel.browser.recordFailed', { msg: err instanceof Error ? err.message : String(err) }))
  }
}

/** 释放 canvas / 轨道 / 编码器引用 */
function releaseRecorder(): void {
  recStream?.getTracks().forEach((tr) => tr.stop())
  recStream = null
  recorder = null
  recCtx = null
  recCanvas = null
  recRect = null
}

function toggleRecording(): void {
  if (recording.value) stopRecording()
  else {
    recStartedAt = Date.now()
    void startRecording()
  }
}
</script>

<template>
  <div class="bw-wrap bw-pane" :class="{ 'bw-pane--active': isActiveTab }">
    <!-- 浏览器页签:页签自带浏览状态,地址栏直接导航,无需新建网页页签 -->
    <template v-if="web">
      <!-- 工具条:导航胶囊(← → | ↻) + 地址栏(圆角胶囊，提示居中) + 操作胶囊 -->
      <div class="bw-bar">
        <div class="bw-group">
          <button type="button" class="bw-btn" :disabled="!canGoBack()" :title="t('panel.browser.back')" @click="goBack">
            <icon-left :size="16" />
          </button>
          <button type="button" class="bw-btn" :disabled="!canGoForward()" :title="t('panel.browser.forward')" @click="goForward">
            <icon-right :size="16" />
          </button>
          <span class="bw-sep" aria-hidden="true" />
          <button type="button" class="bw-btn" :title="t('panel.browser.refresh')" @click="refresh">
            <icon-loading v-if="loading" :size="16" class="bw-spin" />
            <icon-refresh v-else :size="16" />
          </button>
        </div>

        <!-- 地址栏：空态提示居中、光标留在左侧（提示是覆盖层，不占输入流） -->
        <div class="bw-address">
          <input
            v-model="address"
            type="text"
            class="bw-address-input"
            :aria-label="t('panel.browser.placeholder')"
            spellcheck="false"
            @keydown.enter="onAddressSubmit"
          />
          <span v-if="!address" class="bw-address-hint" aria-hidden="true">{{ t('panel.browser.placeholder') }}</span>
        </div>

        <div class="bw-group bw-actions">
          <!-- 选择元素:开启后页面内悬停高亮、点击拾取(再点一次 / Esc 取消) -->
          <button
            type="button"
            class="bw-btn btn"
            :class="{ active: picking }"
            :disabled="!web.url"
            :title="picking ? t('panel.browser.picking') : t('panel.browser.pickElement')"
            @click="onPickElement"
          >
            <icon-pick-element :size="16" />
          </button>

          <!-- 截图:截 iframe 当前可见区域,落盘到工作区 .astorm/browser-capture/ -->
          <button
            type="button"
            class="bw-btn"
            :class="{ active: capturing }"
            :disabled="!web.url || capturing"
            :title="t('panel.browser.screenshot')"
            @click="onCapture"
          >
            <icon-camera :size="16" />
          </button>

          <!-- 录像:逐帧采集编码成 webm,再点一次停止并落盘(录制中按钮高亮) -->
          <button
            type="button"
            class="bw-btn"
            :class="{ active: recording }"
            :disabled="!web.url"
            :title="recording ? t('panel.browser.recordStop') : t('panel.browser.record')"
            @click="toggleRecording"
          >
            <icon-square v-if="recording" :size="14" />
            <icon-video v-else :size="16" />
          </button>

          <!-- 更多:视口 / 缩放设置 + 页面动作 -->
          <a-dropdown
            v-model:popup-visible="moreVisible"
            trigger="click"
            position="br"
            :popup-max-height="false"
          >
            <button type="button" class="bw-btn" :title="t('panel.browser.more')">
              <icon-more :size="16" />
            </button>
            <template #content>
              <div class="bw-menu" @click.stop>
                <div class="bw-menu-title">{{ t('panel.browser.viewport') }}</div>
                <SegmentedControl v-model="frameMode" size="sm" block :options="frameModeOptions" />

                <template v-if="frameMode === 'custom'">
                  <div class="bw-menu-dims">
                    <input v-model.number="frameW" type="number" class="bw-size-input" @input="onCustomW" />
                    <span class="bw-menu-x">×</span>
                    <input v-model.number="frameH" type="number" class="bw-size-input" @input="onCustomH" />
                  </div>
                  <div class="bw-menu-presets">
                    <button
                      v-for="(p, i) in SIZE_PRESETS"
                      :key="p.label"
                      type="button"
                      class="bw-menu-preset"
                      :class="{ on: presetIndex === i }"
                      @click="applyPreset(i)"
                    >
                      {{ p.label }}
                    </button>
                  </div>
                </template>

                <div class="bw-menu-row">
                  <span class="bw-menu-name">{{ t('panel.browser.zoom') }}</span>
                  <div class="bw-stepper">
                    <button type="button" :disabled="zoomPercent <= ZOOM_MIN_PERCENT" @click="stepZoom(-1)">−</button>
                    <span class="bw-stepper-value">{{ zoomPercent }}%</span>
                    <button type="button" :disabled="zoomPercent >= ZOOM_MAX_PERCENT" @click="stepZoom(1)">+</button>
                  </div>
                </div>

                <div class="bw-menu-divider" />
                <button type="button" class="bw-menu-action" @click="moreAction('devtools')">
                  {{ t('panel.browser.openDevtools') }}
                </button>
                <button type="button" class="bw-menu-action" @click="moreAction('copy')">
                  {{ t('panel.browser.copyLink') }}
                </button>
                <button type="button" class="bw-menu-action" @click="moreAction('external')">
                  {{ t('panel.browser.openExternal') }}
                </button>
              </div>
            </template>
          </a-dropdown>
        </div>
      </div>

      <!-- 内联状态行:截图/录像的结果与进度(替代 toast 气泡);录制中常驻,结果数秒后自动消失 -->
      <div v-if="statusText" class="bw-status" :class="`is-${statusKind}`">
        <span v-if="recording" class="bw-rec-dot" />
        <span class="bw-status-text" :title="status?.title ?? ''">{{ statusText }}</span>
      </div>

      <!-- 空态:尚无网址 -->
      <div v-if="isEmpty" class="bw-empty">
        <icon-globe :size="40" class="bw-empty-icon" />
        <div class="bw-empty-title">{{ t('panel.browser.title') }}</div>
        <div class="bw-empty-desc">{{ t('panel.browser.emptyDesc') }}</div>
      </div>

      <!-- 视口:默认铺满窗口;自定义模式按尺寸居中;均支持百分比缩放。
           外层画板取视觉尺寸(缩放后)供舞台居中,iframe 内部按 zoom 缩放;
           锚点层盖在**可见画板区域**上(画板比面板宽时贴住可见边缘) -->
      <div v-else ref="viewportRef" class="bw-viewport">
        <div ref="stageRef" class="bw-stage">
          <div ref="frameBoxRef" class="bw-frame-box" :style="boxStyle">
            <!-- 容器形态：服务端 headless Chromium 的画面流（canvas + 输入回传） -->
            <canvas
              v-if="isWebShell"
              ref="canvasRef"
              class="bw-frame bw-canvas"
              :style="frameStyle"
              tabindex="0"
              @click="onCanvasClick"
              @mousemove="onCanvasMove"
              @wheel="onCanvasWheel"
              @keydown="onCanvasKeydown"
            />
            <iframe
              v-else
              ref="iframeRef"
              class="bw-frame"
              :name="props.tabId"
              :src="web.url"
              :style="frameStyle"
              :title="web.title"
              sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-downloads"
              @load="onFrameLoad"
            />
          </div>
        </div>

        <!-- 拖拽期间挡住 iframe，pointermove 不再被页面吞掉 -->
        <div v-if="resizing" class="bw-resize-shield" :style="{ cursor: resizeCursor }" />

        <!-- 四角锚点：仅自定义视口显示，拖拽改缩放百分比（宽高不变） -->
        <div v-if="frameMode === 'custom'" class="bw-anchor-layer" :style="anchorLayerStyle">
          <span
            v-for="corner in RESIZE_CORNERS"
            :key="corner"
            class="bw-anchor"
            :class="[`is-${corner}`, { 'is-active': activeCorner === corner }]"
            :title="t('panel.browser.dragZoom')"
            @pointerdown="onResizeStart($event, corner)"
          />
          <div v-if="resizing" class="bw-zoom-badge">{{ zoomPercent }}%</div>
        </div>
      </div>
    </template>
  </div>
</template>

<script lang="ts">
export default { name: 'BrowserPanel' }
</script>

<style scoped>
/* 画面流画布：按服务端帧尺寸等比铺满，焦点可键盘输入 */
.bw-canvas {
  display: block;
  cursor: default;
  outline: none;
  background: #fff;
}
.bw-wrap {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  /* 顶部不留额外留白，让工具条贴近页签栏；其余方向保持 8px */
  padding: 0 8px 8px;
  gap: 6px;
}

/* 工具条:与内容同底，无下边框；左右铺满面板内边距 */
.bw-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  height: 32px;
  margin: 0 -8px;
  padding: 0 8px;
}

/* 导航 / 操作按钮组：浅填充 + 发丝描边的胶囊（深色为亮填充，浅色为白底灰底） */
.bw-group {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  height: 28px;
  padding: 0 3px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-full);
  background: var(--bg-2);
}

/* 组内分隔线（后退/前进 与 刷新 之间） */
.bw-sep {
  flex-shrink: 0;
  width: 1px;
  height: 14px;
  margin: 0 3px;
  border-radius: 1px;
  background: var(--border-base);
}

/* 工具按钮:无边框纯图标,悬停圆形灰底(参考 UI) */
.bw-btn {
  width: 24px;
  height: 24px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: var(--radius-full);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.bw-btn:hover:not(:disabled) {
  background: var(--fill-2);
  color: var(--text-primary);
}

.bw-btn.active {
  background: var(--fill-3);
  color: var(--text-primary);
}

.bw-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.bw-spin {
  animation: bw-spin 0.9s linear infinite;
}

@keyframes bw-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

/* 地址栏:更亮一档的圆角胶囊 + 描边（输入控件语言） */
.bw-address {
  position: relative;
  flex: 1;
  min-width: 0;
  height: 28px;
  display: flex;
  align-items: center;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-full);
  background: var(--bg-3);
  padding: 0 10px;
}

.bw-address-input {
  flex: 1;
  min-width: 0;
  height: 100%;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  padding: 0;
}

/* 空态提示:居中覆盖层,光标仍停在左侧；有内容即消失 */
.bw-address-hint {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  pointer-events: none;
}

/* ── ⋯ 菜单:上半是视口/缩放设置,下半是页面动作 ──
   菜单内容随 a-dropdown 一起 teleport,但仍是本组件模板节点,scoped 样式照常命中。 */
.bw-menu {
  width: 264px;
  padding: 8px 10px 6px;
  box-sizing: border-box;
}

.bw-menu-title {
  margin: 2px 2px 6px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 自定义宽高 */
.bw-menu-dims {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 8px;
}

.bw-menu-x {
  flex-shrink: 0;
  color: var(--text-tertiary);
  font-size: var(--font-size-xs);
}

.bw-size-input {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 8px;
  border: none;
  border-radius: 7px;
  background: var(--fill-1);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  text-align: center;
  outline: none;
  transition:
    background-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

.bw-size-input:hover {
  background: var(--fill-2);
}

.bw-size-input:focus {
  background: var(--fill-2);
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--accent-text) 22%, transparent);
}

/* 尺寸输入:macOS 文本框不显示数字步进箭头(仍可键入 / 方向键微调) */
.bw-size-input::-webkit-outer-spin-button,
.bw-size-input::-webkit-inner-spin-button {
  appearance: none;
  margin: 0;
}

/* 预设尺寸:两列小格 */
.bw-menu-presets {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px;
  margin-top: 6px;
}

.bw-menu-preset {
  height: 26px;
  border: none;
  border-radius: 6px;
  background: var(--fill-1);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.bw-menu-preset:hover {
  background: var(--fill-2);
  color: var(--text-primary);
}

.bw-menu-preset.on {
  background: var(--fill-3);
  color: var(--text-primary);
  font-weight: 500;
}

/* 缩放行 */
.bw-menu-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-top: 10px;
}

.bw-menu-name {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

/* 数值步进:描边胶囊内三格等宽（菜单里的数值调节统一形态） */
.bw-stepper {
  display: grid;
  grid-template-columns: 26px 48px 26px;
  align-items: stretch;
  height: 26px;
  overflow: hidden;
  border: 1px solid var(--border-base);
  border-radius: 7px;
}

.bw-stepper button {
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  cursor: pointer;
}

.bw-stepper button:hover:not(:disabled) {
  background: var(--fill-1);
  color: var(--text-primary);
}

.bw-stepper button:disabled {
  opacity: 0.35;
  cursor: default;
}

.bw-stepper-value {
  display: flex;
  align-items: center;
  justify-content: center;
  border-left: 1px solid var(--border-base);
  border-right: 1px solid var(--border-base);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
}

/* 设置区与动作区之间 */
.bw-menu-divider {
  height: 1px;
  margin: 10px -10px 4px;
  background: var(--border-light);
}

.bw-menu-action {
  display: flex;
  align-items: center;
  width: 100%;
  height: 28px;
  padding: 0 6px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
}

.bw-menu-action:hover {
  background: var(--fill-1);
}

/* 内联状态行:截图/录像结果与录制进度(替代 toast 气泡) */
.bw-status {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 0 2px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.bw-status-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 录制中:正常文字色 + 左侧脉冲红点,提示「正在进行」 */
.bw-status.is-rec {
  color: var(--text-primary);
}

.bw-status.is-err {
  color: var(--danger, #f53f3f);
}

.bw-rec-dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--danger, #f53f3f);
  animation: bw-rec-pulse 1.2s ease-in-out infinite;
}

@keyframes bw-rec-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.25;
  }
}

/* 空态:图标 + 标题 + 说明,居中 */
.bw-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
}

.bw-empty-icon {
  color: var(--text-tertiary);
}

.bw-empty-title {
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.bw-empty-desc {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

/* 视口:舞台 + 覆盖在可见画板上的锚点层 */
.bw-viewport {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
}

/* 视口:默认铺满(无边框),自定义模式居中画板 */
.bw-stage {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  overflow: auto;
  background: var(--bg-base);
}

/* 画板 = 视觉尺寸(缩放后):margin auto 实现水平垂直居中,
   溢出时从起点滚动不裁剪(flex + overflow 的安全居中写法) */
.bw-frame-box {
  position: relative;
  margin: auto;
  flex-shrink: 0;
  overflow: hidden;
  border-radius: 6px;
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.12);
  background: #ffffff;
}

/* 拖拽期间的透明挡板：避免指针进入 iframe 后丢掉 pointermove */
.bw-resize-shield {
  position: absolute;
  inset: 0;
  z-index: 3;
  touch-action: none;
}

/* 四角锚点层:盖住可见画板区域,只在四颗锚点上接收指针 */
.bw-anchor-layer {
  position: absolute;
  z-index: 4;
  pointer-events: none;
}

/* 锚点:四角各一颗圆形手柄,圆心落在可见画板的角上 */
.bw-anchor {
  position: absolute;
  width: 12px;
  height: 12px;
  border: 1.5px solid color-mix(in srgb, var(--text-primary) 55%, transparent);
  border-radius: 50%;
  background: var(--bg-base);
  box-shadow:
    0 0 0 1px color-mix(in srgb, var(--bg-base) 70%, transparent),
    0 1px 4px rgba(0, 0, 0, 0.28);
  pointer-events: auto;
  touch-action: none;
  transition:
    border-color var(--transition-fast),
    background-color var(--transition-fast),
    transform var(--transition-fast);
}

.bw-anchor:hover {
  border-color: var(--accent-text);
  transform: scale(1.15);
}

.bw-anchor.is-active {
  border-color: var(--accent-text);
  background: var(--accent-text);
  transform: scale(1.15);
}

.bw-anchor.is-nw {
  top: -6px;
  left: -6px;
  cursor: nwse-resize;
}

.bw-anchor.is-ne {
  top: -6px;
  right: -6px;
  cursor: nesw-resize;
}

.bw-anchor.is-sw {
  bottom: -6px;
  left: -6px;
  cursor: nesw-resize;
}

.bw-anchor.is-se {
  right: -6px;
  bottom: -6px;
  cursor: nwse-resize;
}

/* 拖拽中的百分比浮标 */
.bw-zoom-badge {
  position: absolute;
  top: 8px;
  left: 50%;
  z-index: 3;
  transform: translateX(-50%);
  padding: 2px 8px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-full);
  background: var(--bg-base);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  box-shadow: var(--shadow-sm);
  pointer-events: none;
}

.bw-frame {
  border: none;
  display: block;
  background: #ffffff;
}
</style>
