<script setup lang="ts">
/**
 * 悬浮卡片包装器：触发元素 + **悬浮/点击**弹出的卡片。
 *
 * 与 `SessionInfoCard` 同一套交互配方（悬浮预览、点击固定、点外部收起、Teleport 到 body
 * 后 fixed 定位）。对话框工具行默认在按钮正上方；侧栏行用 `placement="right"` +
 * `openDelayMs` 贴在行右侧，避免打断列表阅读。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'

const props = withDefaults(
  defineProps<{
    /** 悬浮多久后弹出（ms）；点击固定不受此限制 */
    openDelayMs?: number
    /** 卡片相对触发元素的方位 */
    placement?: 'top' | 'right'
    /** 禁用（如右键菜单打开 / 拖拽中） */
    disabled?: boolean
    /** 触发槽撑满父容器（侧栏整行） */
    block?: boolean
  }>(),
  { openDelayMs: 0, placement: 'top', disabled: false, block: false }
)

const hovered = ref(false)
const pinned = ref(false)
const open = ref(false)
const btnRef = ref<HTMLElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)
const panelStyle = ref<Record<string, string>>({})

/** 隐藏延时：鼠标从触发元素移到卡片之间的空隙不至于立刻收起 */
let hideTimer: ReturnType<typeof setTimeout> | null = null
/** 悬浮延迟（侧栏 800ms；避免鼠标扫过列表时卡片乱弹） */
let openTimer: ReturnType<typeof setTimeout> | null = null
function clearHide(): void {
  if (hideTimer) {
    clearTimeout(hideTimer)
    hideTimer = null
  }
}
function clearOpen(): void {
  if (openTimer) {
    clearTimeout(openTimer)
    openTimer = null
  }
}
function show(): void {
  clearHide()
  clearOpen()
  open.value = true
}
function hide(): void {
  clearHide()
  clearOpen()
  hideTimer = setTimeout(() => {
    if (!pinned.value) open.value = false
  }, 120)
}
function onTriggerEnter(): void {
  hovered.value = true
  clearHide()
  clearOpen()
  if (props.disabled) return
  if (props.openDelayMs <= 0) {
    show()
    return
  }
  openTimer = setTimeout(() => {
    openTimer = null
    if (hovered.value && !props.disabled) show()
  }, props.openDelayMs)
}
function onTriggerLeave(): void {
  hovered.value = false
  hide()
}
function onTriggerClick(): void {
  pinned.value = !pinned.value
  open.value = pinned.value || hovered.value
}

/**
 * 定位：`top` = 卡片在触发元素上方，水平中心对齐、贴边收敛；
 * `right` = 侧栏行右侧，垂直居中，右侧空间不足时翻到左侧。
 */
function place(): void {
  const rect = btnRef.value?.getBoundingClientRect()
  const panel = panelRef.value
  if (!rect || !panel) return
  const MARGIN = 12
  const panelW = panel.offsetWidth || 300
  if (props.placement === 'right') {
    const panelH = panel.offsetHeight || 120
    const gap = 8
    const rightSpace = window.innerWidth - rect.right - MARGIN
    const left =
      rightSpace >= panelW
        ? rect.right + gap
        : Math.max(MARGIN, rect.left - panelW - gap)
    const top = Math.min(
      Math.max(rect.top + rect.height / 2 - panelH / 2, MARGIN),
      Math.max(MARGIN, window.innerHeight - panelH - MARGIN)
    )
    panelStyle.value = {
      left: `${left}px`,
      right: 'auto',
      top: `${top}px`,
      bottom: 'auto'
    }
    return
  }
  const btnCenter = rect.left + rect.width / 2
  const left = Math.min(
    Math.max(btnCenter - panelW / 2, MARGIN),
    Math.max(MARGIN, window.innerWidth - panelW - MARGIN)
  )
  panelStyle.value = {
    left: `${left}px`,
    right: 'auto',
    bottom: `${window.innerHeight - rect.top + 8}px`,
    '--arrow-left': `${btnCenter - left - 5}px`
  }
}

function onViewportChange(): void {
  place()
}

function onDocMouseDown(e: MouseEvent): void {
  const target = e.target as Node | null
  if (!target) return
  if (btnRef.value?.contains(target)) return
  if (panelRef.value?.contains(target)) return
  pinned.value = false
  hovered.value = false
  open.value = false
}

watch(open, (v) => {
  if (v) {
    document.addEventListener('mousedown', onDocMouseDown, true)
    window.addEventListener('scroll', onViewportChange, true)
    window.addEventListener('resize', onViewportChange)
    nextTick(place)
  } else {
    document.removeEventListener('mousedown', onDocMouseDown, true)
    window.removeEventListener('scroll', onViewportChange, true)
    window.removeEventListener('resize', onViewportChange)
  }
})
watch(
  () => props.disabled,
  (disabled) => {
    if (disabled) close()
  }
)
onBeforeUnmount(() => {
  clearHide()
  clearOpen()
  document.removeEventListener('mousedown', onDocMouseDown, true)
  window.removeEventListener('scroll', onViewportChange, true)
  window.removeEventListener('resize', onViewportChange)
})

/** 供外层主动收起（如点击卡片内条目完成跳转后） */
function close(): void {
  clearHide()
  clearOpen()
  pinned.value = false
  hovered.value = false
  open.value = false
}
/** 供外层按「卡片已展开」触发懒加载（避免鼠标扫过时预取所有行） */
const isOpen = computed(() => open.value)
defineExpose({ close, isOpen })
</script>

<template>
  <div
    ref="btnRef"
    class="hover-card-wrap"
    :class="{ 'hover-card-wrap--block': block }"
    @mouseenter="onTriggerEnter"
    @mouseleave="onTriggerLeave"
  >
    <slot name="trigger" :active="open" :toggle="onTriggerClick" />
  </div>
  <Teleport to="body">
    <transition name="hover-card-fade">
      <div
        v-if="open"
        ref="panelRef"
        class="hover-card-panel"
        :class="`hover-card-panel--${placement}`"
        :style="panelStyle"
        @mouseenter="show()"
        @mouseleave="hovered = false; hide()"
      >
        <slot />
      </div>
    </transition>
  </Teleport>
</template>

<style scoped>
.hover-card-wrap {
  display: inline-flex;
  flex-shrink: 0;
}

.hover-card-wrap--block {
  display: block;
  width: 100%;
  min-width: 0;
}
</style>

<style>
/* 非 scoped：卡片 Teleport 到 body，scoped 选择器覆盖不到 */
.hover-card-panel {
  position: fixed;
  z-index: 1100;
  width: min(340px, calc(100vw - 120px));
  /* 不设整体滚动：内容高度已由卡片内列表的 max-height 收敛，
     外层再 overflow:auto 会在「内容明明放得下」时也多出一条滚动条 */
  padding: 8px 0;
  background: var(--hover-card-bg);
  /* 无外边框（按需求去掉；靠背景与投影区分层次） */
  border: none;
  border-radius: 10px;
  box-shadow: var(--hover-card-shadow);
}
/* 底部小箭头（指向触发图标中心，位置由 place() 写入 --arrow-left） */
.hover-card-panel--top::after {
  content: '';
  position: absolute;
  bottom: -6px;
  left: var(--arrow-left, 14px);
  width: 10px;
  height: 10px;
  background: var(--hover-card-bg);
  border: none;
  transform: rotate(45deg);
}

/* 侧栏行右侧的卡片不画箭头，贴着行悬浮即可 */
.hover-card-panel--right::after {
  display: none;
}

.hover-card-fade-enter-active {
  transition:
    opacity 0.18s ease,
    transform 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}
.hover-card-fade-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
}
.hover-card-fade-enter-from,
.hover-card-fade-leave-to {
  opacity: 0;
  transform: translateY(6px);
}
</style>
