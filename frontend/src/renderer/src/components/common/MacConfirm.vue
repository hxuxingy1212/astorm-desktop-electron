<script lang="ts">
/** 无障碍 id 计数器：模块级（`<script setup>` 里的变量是每实例重置的） */
let uid = 0
</script>

<script setup lang="ts">
/**
 * macOS 风格确认 / 提示卡片（纯展示组件，命令式调用见 `composables/useMacConfirm.ts`）。
 *
 * 视觉与 `components/chat/ApprovalBar.vue` 同一套配方：毛玻璃底（backdrop blur + 半透明
 * --bg-2）+ 16px 圆角 + 柔和投影 + 分类色圆角图标块 + macOS 按钮形态（28px 高 / 7px 圆角 /
 * 中性灰次级 + 主色（删除为红）主要）+ 右对齐按钮组 + 0.5px 发丝线分区。
 *
 * 交互（对齐 macOS 警示框）：
 * - Esc = 取消；Enter = 确认（焦点在输入类元素内时不劫持）
 * - 点击遮罩 = 取消（用 mousedown.self：按下在卡片、松开在遮罩不误判为「点遮罩」）
 * - 打开时焦点落在**取消**（破坏性操作不默认聚焦确认）；Tab 在卡片内循环（简单焦点集中）
 * - 关闭后把焦点还给打开前的元素
 *
 * 该组件自身 `teleport` 到 body，所以既能被命令式挂载（挂载容器也在 body）也能被模板直接使用。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Info, TriangleAlert } from 'lucide-vue-next'

const props = withDefaults(
  defineProps<{
    title?: string
    /** 正文：字符串，或用默认插槽自定义 */
    content?: string
    tone?: 'danger' | 'warning' | 'primary'
    confirmText?: string
    cancelText?: string
    /** 异步确认进行中：按钮转圈并禁止重复提交 */
    loading?: boolean
    /** 单按钮提示模式（macAlert）：隐藏取消按钮，焦点落在确认上 */
    hideCancel?: boolean
  }>(),
  { tone: 'primary', loading: false, hideCancel: false }
)

const emit = defineEmits<{ (e: 'confirm'): void; (e: 'cancel'): void }>()

const { t } = useI18n()

/** 无障碍：标题/正文的 id（自己挂载的场景下不依赖版本差异的 useId，用模块计数器） */
const seq = ++uid
const titleId = `mac-confirm-title-${seq}`
const descId = `mac-confirm-desc-${seq}`

const cardRef = ref<HTMLElement | null>(null)
const cancelRef = ref<HTMLButtonElement | null>(null)
const confirmRef = ref<HTMLButtonElement | null>(null)

const icon = computed(() => (props.tone === 'primary' ? Info : TriangleAlert))
/** 默认文案走 i18n：确认框「确认 / 确定」，提示框（单按钮）「提示 / 知道了」 */
const titleText = computed(() => props.title ?? t(props.hideCancel ? 'common.notice' : 'common.confirm'))
const confirmText = computed(() => props.confirmText ?? t(props.hideCancel ? 'common.gotIt' : 'common.ok'))
const cancelText = computed(() => props.cancelText ?? t('common.cancel'))

/** 打开前的焦点元素：关闭后归还（避免焦点丢失到 body） */
let restoreEl: HTMLElement | null = null

/** 取消：异步确认进行中也允许（与原 Arco 一致）；宿主用 settled 守卫，重复决议无效 */
function onCancel(): void {
  emit('cancel')
}

function onConfirm(): void {
  if (props.loading) return
  emit('confirm')
}

/** 简单焦点集中：Tab / Shift+Tab 在卡片内循环，不跑到卡片外 */
function trapTab(e: KeyboardEvent): void {
  const focusables = cardRef.value?.querySelectorAll<HTMLElement>(
    'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
  )
  if (!focusables?.length) {
    e.preventDefault()
    cardRef.value?.focus()
    return
  }
  const first = focusables[0]
  const last = focusables[focusables.length - 1]
  const active = document.activeElement
  if (e.shiftKey && (active === first || !cardRef.value?.contains(active))) {
    e.preventDefault()
    last.focus()
  } else if (!e.shiftKey && (active === last || !cardRef.value?.contains(active))) {
    e.preventDefault()
    first.focus()
  }
}

/**
 * 全局捕获阶段监听：Esc/Enter 要在输入框的「发送」等业务按键之前截住
 * （弹窗是模态的，此时键盘归它）。输入类元素内的 Enter 不劫持（留给未来的自定义内容）。
 */
function onKeydown(e: KeyboardEvent): void {
  if (e.isComposing) return
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    onCancel()
    return
  }
  if (e.key === 'Tab') {
    trapTab(e)
    return
  }
  if (e.key === 'Enter') {
    const el = e.target as HTMLElement | null
    const tag = el?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el?.isContentEditable) return
    e.preventDefault()
    e.stopPropagation()
    onConfirm()
  }
}

onMounted(async () => {
  restoreEl = (document.activeElement as HTMLElement | null) ?? null
  document.addEventListener('keydown', onKeydown, true)
  await nextTick()
  ;(props.hideCancel ? confirmRef.value : cancelRef.value)?.focus()
})

onBeforeUnmount(() => {
  document.removeEventListener('keydown', onKeydown, true)
  // 只在原元素还在文档里时归还焦点（原元素可能已随列表刷新被移除）
  if (restoreEl?.isConnected) restoreEl.focus()
})
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩：点空白 = 取消；mousedown 判定避免「卡片内按下、遮罩上松开」误关 -->
    <div class="mac-confirm-mask" @mousedown.self="onCancel">
      <div
        ref="cardRef"
        class="mac-confirm"
        :class="tone"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        :aria-describedby="descId"
        tabindex="-1"
      >
        <div class="mac-confirm-head">
          <span class="mac-confirm-icon"><component :is="icon" :size="12" /></span>
          <span :id="titleId" class="mac-confirm-title">{{ titleText }}</span>
        </div>

        <div :id="descId" class="mac-confirm-body">
          <slot>{{ content }}</slot>
        </div>

        <div class="mac-confirm-actions">
          <button
            v-if="!hideCancel"
            ref="cancelRef"
            type="button"
            class="mac-confirm-btn secondary"
            @click="onCancel"
          >
            {{ cancelText }}
          </button>
          <button
            ref="confirmRef"
            type="button"
            class="mac-confirm-btn"
            :class="tone === 'danger' ? 'danger' : 'primary'"
            :disabled="loading"
            @click="onConfirm"
          >
            <span v-if="loading" class="mac-confirm-spinner" aria-hidden="true" />
            {{ confirmText }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
/*
 * 分层：遮罩 1002（高于 Arco 弹窗 1001 / 抽屉，低于 Message 1003——异步确认失败时
 * 的提示条要能被看见）。卡片自身的材质与 ApprovalBar 同源。
 */
.mac-confirm-mask {
  position: fixed;
  inset: 0;
  z-index: 1002;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background-color: rgba(0, 0, 0, 0.28);
  animation: mac-confirm-mask-in 0.16s ease;
  /* 浮层压在窗口拖拽区（侧栏头部/状态栏）之上时必须显式 no-drag，否则「看得见、点不动」 */
  -webkit-app-region: no-drag;
}

[data-theme='dark'] .mac-confirm-mask {
  background-color: rgba(0, 0, 0, 0.45);
}

.mac-confirm {
  --mac-confirm-tint: var(--accent-text);
  box-sizing: border-box;
  width: 340px;
  max-width: 100%;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 16px 16px 12px;
  outline: none;
  /* 圆角与审批卡 / 发送框一致（同一纵向栈内观感统一） */
  border-radius: 16px;
  background-color: color-mix(in srgb, var(--bg-2) 90%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  box-shadow:
    0 0 0 0.5px color-mix(in srgb, var(--border-base) 70%, transparent),
    0 10px 32px rgba(0, 0, 0, 0.2),
    0 1px 2px rgba(0, 0, 0, 0.08);
  animation: mac-confirm-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}

/* 分类色：只作用于图标块与聚焦环（与 DenialBadge / ApprovalBar 的口径一致） */
.mac-confirm.warning {
  --mac-confirm-tint: var(--warning);
}

.mac-confirm.danger {
  --mac-confirm-tint: var(--danger);
}

@keyframes mac-confirm-mask-in {
  from {
    opacity: 0;
  }
}

@keyframes mac-confirm-in {
  from {
    opacity: 0;
    transform: translateY(5px) scale(0.99);
  }
}

@media (prefers-reduced-motion: reduce) {
  .mac-confirm-mask,
  .mac-confirm {
    animation: none;
  }
}

.mac-confirm-head {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

/* 圆角方块图标（SF Symbols 风格的着色块） */
.mac-confirm-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex: none;
  border-radius: 6px;
  color: var(--mac-confirm-tint);
  background-color: color-mix(in srgb, var(--mac-confirm-tint) 16%, transparent);
}

.mac-confirm-title {
  min-width: 0;
  font-size: var(--font-size-sm);
  font-weight: 600;
  line-height: 1.4;
  color: var(--text-primary);
  letter-spacing: 0.01em;
  word-break: break-word;
}

.mac-confirm-body {
  padding: 0 2px;
  font-size: var(--font-size-xs);
  line-height: 1.55;
  color: var(--text-secondary);
  word-break: break-word;
}

/* 按钮组：发丝线分隔，macOS 右对齐（取消在左、默认按钮在右） */
.mac-confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 2px;
  padding-top: 10px;
  border-top: 0.5px solid var(--border-base);
}

.mac-confirm-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-width: 68px;
  height: 28px;
  padding: 0 14px;
  font-family: inherit;
  font-size: var(--font-size-sm);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  border: 0;
  border-radius: 7px;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    filter var(--transition-fast),
    box-shadow var(--transition-fast);
}

/* 次级：中性灰填充（macOS 非默认按钮） */
.mac-confirm-btn.secondary {
  color: var(--text-primary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
}

.mac-confirm-btn.secondary:hover {
  background-color: color-mix(in srgb, var(--bg-5) 32%, transparent);
}

/* 主按钮：主色实心 + 极轻投影 */
.mac-confirm-btn.primary {
  color: #fff;
  background-color: var(--accent-text);
  box-shadow: 0 1px 2.5px rgba(0, 0, 0, 0.18);
}

.mac-confirm-btn.primary:hover {
  filter: brightness(1.06);
}

/* 破坏性主按钮：红底（macOS 删除按钮），与次级按钮同形 */
.mac-confirm-btn.danger {
  color: #fff;
  background-color: var(--danger);
  box-shadow: 0 1px 2.5px rgba(0, 0, 0, 0.18);
}

.mac-confirm-btn.danger:hover {
  filter: brightness(1.06);
}

.mac-confirm-btn:active {
  filter: brightness(0.94);
}

.mac-confirm-btn:disabled {
  cursor: default;
  opacity: 0.6;
  filter: none;
}

/* 键盘聚焦环（仅键盘可达时显示，取分类色） */
.mac-confirm-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--mac-confirm-tint) 32%, transparent);
}

/* 异步确认中的转圈（与按钮文字同色） */
.mac-confirm-spinner {
  width: 11px;
  height: 11px;
  flex: none;
  border-radius: 50%;
  border: 1.5px solid color-mix(in srgb, currentColor 35%, transparent);
  border-top-color: currentColor;
  animation: mac-confirm-spin 0.6s linear infinite;
}

@keyframes mac-confirm-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
