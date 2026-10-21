/**
 * scrollbars — 细滚动条的**行为**层（外观在 `styles/global.css`，全应用只有那一处定义）。
 *
 * 两件事：
 *  ① **空闲不显示、滚动时才出现**：给「用户正在滚动」的容器打 `is-scrolling`，滚完约 1.2s 摘掉。
 *     只认**用户输入**（滚轮 / 触控 / 滚动键 / 拖动滚动条）—— 程序化滚动（流式回答自动到底、
 *     `scrollIntoView`、开面板时恢复位置）一律不显示，否则回答期间滚动条会长亮，等于没隐藏。
 *  ② **预留滚动条位**：给滚动容器打 `data-scroll-gutter`（CSS 里 = `scrollbar-gutter: stable`），
 *     内容不因滚动条出现/消失而左右移位。**只能在 JS 里按容器打**，不能写成全局 CSS 规则：
 *     实测 Chromium 的 `scrollbar-gutter` 对 `overflow: hidden` 的裁剪容器同样生效，全局写会把
 *     所有圆角/截断容器的内容统统缩掉 4px（`*` 规则实测 200px 盒子的 clientWidth 变 196）。
 *
 * 判定滚动容器只看 `computed overflow`，**不读** `scrollHeight/clientHeight`：后者是强制布局，
 * 而流式回答时布局每帧都是脏的，读一次就把整棵树重排一遍（滚轮事件每帧都来）。
 *
 * 与 `docs/ui-style.md` §1.4 配套；改这里要同步改那份文档。
 */

/** 松手/停滚后多久淡出（与 CSS 的 0.15s 过渡叠加 ≈ 1.35s 后完全消失） */
const HIDE_DELAY_MS = 1200

const SCROLLING_CLASS = 'is-scrolling'
/** 预留滚动条位的标记（CSS: `[data-scroll-gutter]`） */
const GUTTER_ATTR = 'data-scroll-gutter'

const SCROLLABLE = /^(auto|scroll)$/
const SCROLL_KEYS = new Set(['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' ', 'Spacebar'])

/** 当前显示着滚动条的那个容器（同一时刻只可能有一个在滚） */
let marked: HTMLElement | null = null
let hideTimer: number | undefined
/** 按住滚动条（拖动 thumb 或点槽）期间不淡出：按住不动时不会有 scroll 事件，靠它兜住 */
let dragging = false

function isScrollContainer(el: HTMLElement): boolean {
  const cs = getComputedStyle(el)
  return SCROLLABLE.test(cs.overflowY) || SCROLLABLE.test(cs.overflowX)
}

/** 从事件目标往上找最近的滚动容器（找不到 = 这次输入不会滚动任何东西，不显示） */
function scrollContainerOf(node: EventTarget | null): HTMLElement | null {
  let el: Element | null = node instanceof Element ? node : null
  while (el instanceof HTMLElement && el !== document.body) {
    if (isScrollContainer(el)) return el
    el = el.parentElement
  }
  return null
}

/** 输入框/编辑器里的方向键是移动光标，不是滚动外层（自身能滚的才认，如多行输入框） */
function isTextEntry(el: HTMLElement): boolean {
  return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable
}

function show(el: HTMLElement | null): void {
  if (!el) return
  if (marked !== el) {
    marked?.classList.remove(SCROLLING_CLASS)
    marked = el
    el.classList.add(SCROLLING_CLASS)
  }
  scheduleHide()
}

function scheduleHide(): void {
  window.clearTimeout(hideTimer)
  if (dragging) return
  hideTimer = window.setTimeout(hide, HIDE_DELAY_MS)
}

function hide(): void {
  marked?.classList.remove(SCROLLING_CLASS)
  marked = null
}

function onWheel(e: WheelEvent): void {
  show(scrollContainerOf(e.target))
}

function onTouchMove(e: TouchEvent): void {
  show(scrollContainerOf(e.target))
}

function onKeyDown(e: KeyboardEvent): void {
  if (!SCROLL_KEYS.has(e.key)) return
  const target = e.target instanceof HTMLElement ? e.target : null
  if (target && isTextEntry(target) && !isScrollContainer(target)) return
  show(scrollContainerOf(target))
}

/**
 * pointerdown 落在滚动条槽上（内容盒右/下边缘之外那条 4px）＝ 用户要拖 thumb 或点槽：
 * 锁住显示到松手。RTL 未启用，横向槽按右侧判。
 */
function onPointerDown(e: PointerEvent): void {
  if (e.button !== 0) return
  const el = scrollContainerOf(e.target)
  if (!el) return
  const rect = el.getBoundingClientRect()
  const onBar = e.clientX >= rect.left + el.clientWidth || e.clientY >= rect.top + el.clientHeight
  if (!onBar) return
  dragging = true
  show(el)
}

function onPointerUp(): void {
  if (!dragging) return
  dragging = false
  scheduleHide()
}

function tag(el: Element): void {
  if (!(el instanceof HTMLElement) || el.hasAttribute(GUTTER_ATTR)) return
  if (isScrollContainer(el)) el.setAttribute(GUTTER_ATTR, '')
}

/** 新增子树里可能有嵌套滚动容器（代码块、下拉列表、面板），逐个看一遍 */
function tagSubtree(root: Element): void {
  tag(root)
  root.querySelectorAll('*').forEach(tag)
}

/** 初始化（在 `main.ts` 挂载后调用一次；引导页与主应用共用同一份） */
export function installScrollbars(): void {
  if ((window as unknown as { __astormScrollbars?: boolean }).__astormScrollbars) return
  ;(window as unknown as { __astormScrollbars?: boolean }).__astormScrollbars = true

  // 首屏同步打标：与 mount 同一个任务、早于首帧绘制，不会出现「先渲染再跳 4px」
  tagSubtree(document.body)

  document.addEventListener('wheel', onWheel, { capture: true, passive: true })
  document.addEventListener('touchmove', onTouchMove, { capture: true, passive: true })
  document.addEventListener('keydown', onKeyDown, { capture: true })
  document.addEventListener('pointerdown', onPointerDown, { capture: true })
  document.addEventListener('pointerup', onPointerUp, { capture: true })
  document.addEventListener('pointercancel', onPointerUp, { capture: true })

  // 后挂的容器（开面板/页签/下拉、流式新增的代码块）增量打标
  new MutationObserver((records) => {
    for (const rec of records) {
      for (const node of rec.addedNodes) if (node instanceof Element) tagSubtree(node)
    }
  }).observe(document.body, { childList: true, subtree: true })
}
