<script setup lang="ts">
/**
 * Composer 的「模型 + 推理等级」组合选择（参考 DeepSeek Harness ui-model-selection）：
 *
 * - 触发器：28px 胶囊 chip，`模型名 推理等级 ⌄`（等级为 caption 色、空间不足先省略），
 *   展开 时 chevron 旋转 180°。
 * - 浮层：Teleport 到 body（避免被输入栏 overflow 裁切），fixed 定位在触发器**上方、
 *   右缘对齐**（锚 right/bottom，无需测量卡片尺寸），padding 4px、 prominence 阴影。
 * - 两级面板：根面板「模型 › / 推理等级 ›」两行（34px，值右对齐 tertiary），
 *   下钻模型列表（>4 个模型出现搜索框；供应商分组标题；选中行尾部 ✓）与等级列表。
 * - 键盘：Esc 先从下钻面板退回根面板再关闭；↑/↓ 在浮层行间移动焦点；
 *   搜索框 Enter 选中第一个命中。外部 mousedown 关闭，选中后焦点还原到触发器。
 *
 * 数据：模型目录读 modelsStore（供应商 → 模型，均以 name 为标识，与设置/会话设置一致）；
 * 推理等级沿用会话设置 thinkingLevel(0-10) 的阈值区间（0→无，1-3→低，4-6→中，7-10→高），
 * 菜单选项取各区间代表值（0/2/5/8）。改动经 `change` 事件抛出补丁，由父组件下发会话设置。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useModelsStore } from '@/stores/models'

const props = defineProps<{
  /** 当前供应商(name) */
  provider: string
  /** 当前模型(name) */
  model: string
  /** 思考强度 0-10（阈值区间映射级别） */
  thinkingLevel: number
}>()

const emit = defineEmits<{
  change: [patch: { provider?: string; model?: string; thinkingLevel?: number }]
}>()

const { t } = useI18n()
const modelsStore = useModelsStore()

/** 供应商分组（无模型的供应商不出现） */
interface ModelGroup {
  id: string
  name: string
  models: string[]
}
const groups = computed<ModelGroup[]>(() =>
  modelsStore.providers
    .map((p) => ({
      id: p.name,
      name: p.name || t('models.unnamed'),
      models: p.models.map((m) => m.name || t('models.unnamed'))
    }))
    .filter((g) => g.models.length > 0)
)
const hasModels = computed(() => groups.value.length > 0)
const totalModels = computed(() => groups.value.reduce((n, g) => n + g.models.length, 0))
/** 目录超过 4 个模型才出现搜索（与参考实现同阈值） */
const showSearch = computed(() => totalModels.value > 4)

/* ---------- 推理等级 ---------- */

/** 等级选项：各阈值区间的代表值 */
const EFFORT_OPTIONS = [
  { value: 0, labelKey: 'chat.thinkingNone' },
  { value: 2, labelKey: 'chat.thinkingLow' },
  { value: 5, labelKey: 'chat.thinkingMedium' },
  { value: 8, labelKey: 'chat.thinkingHigh' }
] as const

/** 数值 → 级别文案（阈值：0→无, 1-3→低, 4-6→中, 7-10→高） */
function effortLabelOf(level: number): string {
  if (level <= 0) return t('chat.thinkingNone')
  if (level <= 3) return t('chat.thinkingLow')
  if (level <= 6) return t('chat.thinkingMedium')
  return t('chat.thinkingHigh')
}
const effortLabel = computed(() => effortLabelOf(props.thinkingLevel))
function isActiveEffort(value: number): boolean {
  return effortLabelOf(value) === effortLabel.value
}
function chooseEffort(value: number): void {
  const patch = { thinkingLevel: value }
  if (effortLabelOf(props.thinkingLevel) === effortLabelOf(value)) {
    close(true)
    return
  }
  emit('change', patch)
  close(true)
}

/* ---------- 模型 ---------- */

const modelLabel = computed(() => props.model || t('chat.modelPlaceholder'))
function isSelected(providerId: string, modelName: string): boolean {
  return props.provider === providerId && props.model === modelName
}
function chooseModel(providerId: string, modelName: string): void {
  if (isSelected(providerId, modelName)) {
    close(true)
    return
  }
  emit('change', { provider: providerId, model: modelName })
  close(true)
}

/* ---------- 面板状态 ---------- */

type Pane = 'root' | 'model' | 'effort'
const open = ref(false)
const pane = ref<Pane>('root')
const query = ref('')

const rootRef = ref<HTMLElement | null>(null)
const triggerRef = ref<HTMLButtonElement | null>(null)
const menuRef = ref<HTMLElement | null>(null)
const searchRef = ref<HTMLInputElement | null>(null)
const cellRefs = ref<(HTMLButtonElement | null)[]>([null, null])
/** 根面板两行 cell 的函数式 ref(字符串 ref 在多元素上会互相覆盖) */
function setCellRef(index: number): (el: unknown) => void {
  return (el) => {
    cellRefs.value[index] = (el as HTMLButtonElement) ?? null
  }
}

/** 过滤后的分组（搜索按子串大小写不敏感；无搜索词即全量） */
const filteredGroups = computed<ModelGroup[]>(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return groups.value
  return groups.value
    .map((g) => ({ ...g, models: g.models.filter((m) => m.toLowerCase().includes(q)) }))
    .filter((g) => g.models.length > 0)
})
/** 搜索命中的第一个模型（Enter 快捷选中） */
const firstMatch = computed(() => {
  for (const g of filteredGroups.value) {
    const m = g.models[0]
    if (m !== undefined) return { provider: g.id, model: m }
  }
  return null
})

const triggerTitle = computed(() => `${modelLabel.value} · ${effortLabel.value}`)

function show(): void {
  query.value = ''
  pane.value = props.model ? 'root' : 'model'
  open.value = true
  nextTick(focusAfterSwitch)
}
function close(restoreFocus = false): void {
  if (!open.value) return
  open.value = false
  pane.value = 'root'
  if (restoreFocus) nextTick(() => triggerRef.value?.focus())
}
function toggle(): void {
  if (open.value) close(true)
  else show()
}
function drill(next: Pane): void {
  query.value = ''
  pane.value = next
  nextTick(focusAfterSwitch)
}
function back(from: Pane): void {
  pane.value = 'root'
  nextTick(() => cellRefs.value[from === 'effort' ? 1 : 0]?.focus())
}

/** 面板切换后把焦点放回菜单内：下钻→搜索框/选中行；返回根→来源 cell */
function focusAfterSwitch(): void {
  if (!open.value) return
  if (pane.value === 'model') {
    if (showSearch.value) {
      searchRef.value?.focus()
      return
    }
  }
  if (pane.value === 'root') {
    cellRefs.value[0]?.focus()
    return
  }
  const buttons = menuButtons()
  const checked = buttons.find((b) => b.dataset.checked === 'true')
  ;(checked ?? buttons[0])?.focus()
}
function menuButtons(): HTMLButtonElement[] {
  return Array.from(menuRef.value?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])
}

/** 浮层内键盘：Esc 逐级退回；↑/↓ 在行间移动焦点 */
function onMenuKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    if (pane.value !== 'root') back(pane.value)
    else close(true)
    return
  }
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault()
    const buttons = menuButtons()
    if (buttons.length === 0) return
    const active = buttons.findIndex((b) => b === document.activeElement)
    const next =
      active === -1 ? (e.key === 'ArrowDown' ? 0 : buttons.length - 1) : (active + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length
    buttons[next]?.focus()
  }
}

/* 外部 mousedown 关闭（Teleport 后菜单在 root 子树之外，两边都查） */
function onDocMousedown(e: MouseEvent): void {
  const target = e.target as Node
  if (rootRef.value?.contains(target) === true) return
  if (menuRef.value?.contains(target) === true) return
  close()
}
watch(open, (v) => {
  if (v) document.addEventListener('mousedown', onDocMousedown, true)
  else document.removeEventListener('mousedown', onDocMousedown, true)
})
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocMousedown, true))

/* ---------- 定位：触发器上方、右缘对齐（锚 right/bottom，免测卡片尺寸） ---------- */

const menuStyle = ref<Record<string, string>>({})
function place(): void {
  const rect = triggerRef.value?.getBoundingClientRect()
  if (!rect) return
  const MARGIN = 12
  const raw = window.innerWidth - rect.right
  const right = Math.min(Math.max(raw, MARGIN), Math.max(MARGIN, window.innerWidth - MARGIN - 420))
  menuStyle.value = { right: `${right}px`, bottom: `${window.innerHeight - rect.top + 8}px` }
}
watch([open, pane, showSearch], () => {
  if (open.value) nextTick(place)
})
</script>

<template>
  <div ref="rootRef" class="me-root">
    <button
      v-if="hasModels"
      ref="triggerRef"
      type="button"
      class="me-trigger"
      :aria-expanded="open"
      aria-haspopup="menu"
      :title="triggerTitle"
      @click="toggle"
    >
      <icon-box :size="15" class="me-trigger-icon" />
      <span class="me-trigger-label">{{ modelLabel }}</span>
      <span class="me-trigger-effort">{{ effortLabel }}</span>
      <icon-down :size="12" class="me-chevron" :class="{ 'me-chevron-open': open }" />
    </button>

    <Teleport to="body">
      <div
        v-if="open"
        ref="menuRef"
        class="me-menu"
        :style="menuStyle"
        role="menu"
        @keydown="onMenuKeydown"
      >
        <!-- 根面板：模型 / 推理等级 两行 -->
        <template v-if="pane === 'root'">
          <button :ref="setCellRef(0)" type="button" class="me-cell" role="menuitem" @click="drill('model')">
            <span class="me-cell-label">{{ t('chat.model') }}</span>
            <span class="me-cell-value">{{ modelLabel }}</span>
            <icon-right :size="12" class="me-cell-chevron" />
          </button>
          <button :ref="setCellRef(1)" type="button" class="me-cell" role="menuitem" @click="drill('effort')">
            <span class="me-cell-label">{{ t('chat.effortLevel') }}</span>
            <span class="me-cell-value">{{ effortLabel }}</span>
            <icon-right :size="12" class="me-cell-chevron" />
          </button>
        </template>

        <!-- 模型列表：搜索（>4 个模型）+ 供应商分组 + 选中 ✓ -->
        <template v-else-if="pane === 'model'">
          <div v-if="showSearch" class="me-search-row">
            <input
              ref="searchRef"
              v-model="query"
              class="me-search"
              type="text"
              role="searchbox"
              :placeholder="t('chat.searchModel')"
              @keydown.enter.prevent="firstMatch && chooseModel(firstMatch.provider, firstMatch.model)"
            />
            <button v-if="query" type="button" class="me-search-clear" :aria-label="t('common.clear')" @click="query = ''">
              <icon-close :size="12" />
            </button>
          </div>
          <div class="me-groups">
            <template v-for="g in filteredGroups" :key="g.id">
              <div v-if="groups.length > 1" class="me-group-label">{{ g.name }}</div>
              <button
                v-for="m in g.models"
                :key="m"
                type="button"
                class="me-option"
                role="menuitemradio"
                :aria-checked="isSelected(g.id, m)"
                :data-checked="isSelected(g.id, m)"
                :title="m"
                @click="chooseModel(g.id, m)"
              >
                <span class="me-option-name">{{ m }}</span>
                <icon-check v-if="isSelected(g.id, m)" :size="14" class="me-check" />
              </button>
            </template>
            <div v-if="filteredGroups.length === 0" class="me-empty" role="status">
              {{ t('chat.searchEmptyModel') }}
            </div>
          </div>
        </template>

        <!-- 推理等级列表 -->
        <template v-else>
          <button
            v-for="opt in EFFORT_OPTIONS"
            :key="opt.value"
            type="button"
            class="me-option"
            role="menuitemradio"
            :aria-checked="isActiveEffort(opt.value)"
            :data-checked="isActiveEffort(opt.value)"
            @click="chooseEffort(opt.value)"
          >
            <span class="me-option-name">{{ t(opt.labelKey) }}</span>
            <icon-check v-if="isActiveEffort(opt.value)" :size="14" class="me-check" />
          </button>
        </template>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.me-root {
  position: relative;
  min-width: 0;
  flex-shrink: 0;
}

/* 触发器 chip:28px 高、13/20/400 secondary、4px 间距、12px caption chevron */
.me-trigger {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  max-width: 260px;
  height: 28px;
  padding: 0 4px 0 8px;
  border: none;
  border-radius: var(--radius-sm);
  outline: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-md);
  line-height: 20px;
  font-weight: 400;
  cursor: pointer;
}
.me-trigger:hover {
  background: var(--bg-3);
}

/* 模型图标（3D 立方体）：窄时保留，只隐藏文字 */
.me-trigger-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.me-trigger-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 等级值:caption 色;空间不足时先省略(shrink 1000 吸收全部亏空) */
.me-trigger-effort {
  flex-shrink: 1000;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-tertiary);
}

.me-chevron {
  flex-shrink: 0;
  color: var(--text-tertiary);
  transition: transform 120ms ease;
}
.me-chevron-open {
  transform: rotate(180deg);
}

/* 对话框宽度不足时只显示模型图标（隐藏名称/等级/箭头）；容器为 .chat-section（全局声明） */
@container (max-width: 720px) {
  .me-trigger-label,
  .me-trigger-effort,
  .me-chevron {
    display: none;
  }
  .me-trigger {
    padding: 0 6px;
  }
}

/* 浮层卡片:body portal + fixed(右下锚定),与输入胶囊同一材质 */
.me-menu {
  position: fixed;
  z-index: 1100;
  display: flex;
  flex-direction: column;
  width: max-content;
  min-width: 240px;
  max-width: min(420px, calc(100vw - 32px));
  max-height: min(360px, calc(100vh - 96px));
  overflow: hidden;
  padding: 4px;
  border: 0.5px solid var(--border-base);
  border-radius: var(--radius-lg);
  background: var(--bg-base);
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.06),
    0 6px 16px rgba(0, 0, 0, 0.1);
  color: var(--text-primary);
}

/* 根面板两行:34px、值右对齐 tertiary、行尾 ›;hover 与参考实现一致用同一底色 */
.me-cell {
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 6px;
  width: auto;
  min-width: 100%;
  height: 34px;
  padding: 0 8px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  line-height: 20px;
  text-align: left;
  cursor: pointer;
  outline: none;
}
.me-cell:hover,
.me-cell:focus-visible {
  background: var(--bg-3);
}

.me-cell-label {
  flex-shrink: 0;
  white-space: nowrap;
}

.me-cell-value {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: right;
  color: var(--text-tertiary);
}

.me-cell-chevron {
  flex-shrink: 0;
  width: 12px;
  height: 12px;
  color: var(--text-tertiary);
}

/* 列表行(模型/等级):与 cell 同节奏,选中标记为尾部 ✓ */
.me-option {
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 6px;
  width: auto;
  min-width: 100%;
  min-height: 34px;
  padding: 5px 7px;
  border: none;
  border-radius: var(--radius-md);
  outline: none;
  background: transparent;
  color: inherit;
  font-size: var(--font-size-sm);
  line-height: 18px;
  text-align: left;
  cursor: pointer;
}
.me-option:hover,
.me-option:focus-visible {
  background: var(--bg-3);
}

.me-option-name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.me-check {
  flex-shrink: 0;
  color: var(--text-primary);
}

/* 供应商分组标题 */
.me-group-label {
  padding: 6px 8px 2px;
  font-size: var(--font-size-2xs);
  line-height: 16px;
  color: var(--text-tertiary);
  user-select: none;
}

/* 搜索行:输入框 + 清除按钮 */
.me-search-row {
  position: relative;
  flex-shrink: 0;
  margin: 2px 0 3px;
}

.me-search {
  box-sizing: border-box;
  width: 100%;
  height: 28px;
  padding: 0 28px 0 8px;
  border: none;
  border-radius: var(--radius-md);
  outline: none;
  background: var(--bg-2);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
}
.me-search::placeholder {
  color: var(--text-tertiary);
}

.me-search-clear {
  position: absolute;
  top: 50%;
  right: 4px;
  transform: translateY(-50%);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.me-search-clear:hover {
  background: var(--bg-3);
}

/* 滚动区 */
.me-groups {
  min-height: 0;
  overflow-y: auto;
}

.me-empty {
  padding: 8px;
  font-size: var(--font-size-xs);
  line-height: 18px;
  color: var(--text-tertiary);
}
</style>
