<script setup lang="ts">
import { computed, reactive, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { UserChatForm, UserFormField } from '@/types'

/** 提问表单(字段结构见 types/index.ts,与后端中断信封 `form` 对齐) */
const props = defineProps<{ form: UserChatForm | null }>()

const { t } = useI18n()

const formValues = reactive<Record<string, unknown>>({})
/** 选择类字段的自由输入(选项之外的「自己的想法」;单选与选项互斥,多选追加到数组末尾) */
const freeText = reactive<Record<string, string>>({})
const formStep = ref(0)
const formFields = shallowRef<UserFormField[]>([])

watch(
  () => props.form,
  (form) => {
    if (!form?.fields) return
    formFields.value = form.fields
    formStep.value = 0
    for (const key of Object.keys(formValues)) delete formValues[key]
    for (const key of Object.keys(freeText)) delete freeText[key]
    for (const f of form.fields) {
      if (f.field_type === 'multiple_choice') formValues[f.name] = [] as string[]
      // 布尔:**不预置默认值**——「未选」是合法状态(选填字段可以整个不答),
      // 预置 false 会让没碰过的字段被当成「用户答了否」上送
      else if (f.field_type === 'boolean') formValues[f.name] = undefined
      else formValues[f.name] = ''
      freeText[f.name] = ''
    }
  },
  { immediate: true }
)

/** 自由输入的当前值(去空白;空串视为未填) */
function freeOf(name: string): string {
  return (freeText[name] ?? '').trim()
}

function fieldValid(field: UserFormField): boolean {
  if (!field.required) return true
  const val = formValues[field.name]
  // 布尔:必须真的选过(未选 = 未答,不能当 false 通过)
  if (field.field_type === 'boolean') return typeof val === 'boolean'
  // 选择类:选中任一项或自由输入非空,即算已答
  if (field.field_type === 'multiple_choice') {
    return (Array.isArray(val) && val.length > 0) || freeOf(field.name) !== ''
  }
  if (field.field_type === 'single_choice') {
    return (val !== undefined && val !== '') || freeOf(field.name) !== ''
  }
  return val !== undefined && val !== ''
}

/** 每字段单独一页:当前页字段 */
const currentField = computed(() => formFields.value[formStep.value] ?? null)
const totalSteps = computed(() => formFields.value.length)
const canPrev = computed(() => formStep.value > 0)
const canNext = computed(() => {
  const f = currentField.value
  if (!f || formStep.value >= totalSteps.value - 1) return false
  return fieldValid(f)
})
const formValid = computed(() => formFields.value.every((f) => fieldValid(f)))

/** 选择类字段(带自由输入框) */
const isChoiceField = computed(() => {
  const ft = currentField.value?.field_type
  return ft === 'single_choice' || ft === 'multiple_choice'
})

/** boolean 字段的是/否两个选项:文案取字段的 true_label / false_label(缺省回退是/否)。
 *  两行都不预选——「未选」是合法状态(选填字段可整个不答),提交时该键不上送。 */
function booleanOptions(field: UserFormField): Array<{ value: boolean; label: string }> {
  return [
    { value: true, label: field.true_label?.trim() || t('userForm.yes') },
    { value: false, label: field.false_label?.trim() || t('userForm.no') }
  ]
}

function toggleMulti(fieldName: string, value: string): void {
  const arr = (formValues[fieldName] as string[]) || []
  const idx = arr.indexOf(value)
  formValues[fieldName] = idx === -1 ? [...arr, value] : arr.filter((v) => v !== value)
}

/** 单选:选中某项即清空自由输入(单选只有一个答案) */
function pickSingle(fieldName: string): void {
  freeText[fieldName] = ''
}

/** 自由输入:单选下与选项互斥(输入即视为放弃已选项) */
function onFreeInput(field: UserFormField): void {
  if (field.field_type === 'single_choice' && freeOf(field.name) !== '') {
    formValues[field.name] = ''
  }
}

function prev(): boolean {
  if (!canPrev.value) return false
  formStep.value--
  return true
}
function next(): boolean {
  if (!canNext.value) return false
  formStep.value++
  return true
}

/**
 * 收集为 { 字段名: 值 }(空值跳过;布尔**只在用户选过后**才上送);供底部表单栏校验/提交。
 * 自由输入并入所属字段的值(不产生额外键):单选取「自由输入 || 选项」,
 * 多选把自由输入追加到数组末尾(与已有选项重复则不追加)。
 */
function collect(): Record<string, unknown> {
  const data: Record<string, unknown> = {}
  for (const f of formFields.value) {
    const val = formValues[f.name]
    const free = freeOf(f.name)
    if (f.field_type === 'boolean') {
      // 未选(undefined)不上送:没填的字段不出现在回答里,后端据此不展示该行
      if (typeof val === 'boolean') data[f.name] = val
      continue
    }
    if (f.field_type === 'single_choice') {
      const chosen = free || (typeof val === 'string' ? val : '')
      if (chosen) data[f.name] = chosen
      continue
    }
    if (f.field_type === 'multiple_choice') {
      const arr = Array.isArray(val) ? [...(val as string[])] : []
      if (free && !arr.includes(free)) arr.push(free)
      if (arr.length) data[f.name] = arr
      continue
    }
    if (val !== undefined && val !== '') data[f.name] = val
  }
  return data
}

defineExpose({
  getValid: () => formValid.value,
  collect,
  prev,
  next,
  canPrev: () => canPrev.value,
  canNext: () => canNext.value,
  step: () => formStep.value + 1,
  totalSteps: () => totalSteps.value,
  /** 当前页字段(卡片标题取它的 description + 必填/选填) */
  currentField: () => currentField.value
})
</script>

<template>
  <div v-if="form && currentField" class="uf-wrap">
    <!-- 每个 field_type 单独一页(多字段走分步,导航在底部表单栏;标题在卡片顶部) -->
    <div class="uf-field uf-field-first">
      <input
        v-if="currentField.field_type === 'text'"
        v-model="formValues[currentField.name] as string"
        class="uf-input"
        :placeholder="t('userForm.inputPlaceholder')"
        @keydown.enter.prevent="next()"
      />
      <input
        v-else-if="currentField.field_type === 'number'"
        v-model.number="formValues[currentField.name] as number"
        type="number"
        class="uf-input"
        @keydown.enter.prevent="next()"
      />

      <!-- boolean:是/否二选一(复用选择行样式,与单选/多选同一语言)。
           初始两行都不选中——未选 = 用户没填,提交时该键不上送(不隐式补 false) -->
      <div v-else-if="currentField.field_type === 'boolean'" class="uf-options">
        <label v-for="opt in booleanOptions(currentField)" :key="String(opt.value)" class="uf-choice-label">
          <input
            v-model="formValues[currentField.name]"
            type="radio"
            :value="opt.value"
            class="uf-choice-input"
          />
          <span class="uf-choice-indicator" />
          <span class="uf-choice-text">{{ opt.label }}</span>
        </label>
      </div>

      <div v-else-if="currentField.field_type === 'single_choice'" class="uf-options">
        <label v-for="opt in currentField.options || []" :key="opt" class="uf-choice-label">
          <input
            v-model="formValues[currentField.name] as string"
            type="radio"
            :value="opt"
            class="uf-choice-input"
            @change="pickSingle(currentField.name)"
          />
          <span class="uf-choice-indicator" />
          <span class="uf-choice-text">{{ opt }}</span>
        </label>
      </div>

      <div v-else-if="currentField.field_type === 'multiple_choice'" class="uf-options">
        <label v-for="opt in currentField.options || []" :key="opt" class="uf-choice-label">
          <input
            type="checkbox"
            class="uf-choice-input"
            :checked="((formValues[currentField.name] as string[]) || []).includes(opt)"
            @change="toggleMulti(currentField.name, opt)"
          />
          <span class="uf-choice-indicator" />
          <span class="uf-choice-text">{{ opt }}</span>
        </label>
      </div>

      <!-- 选择类字段:自由输入(常驻在选项列表下方、滚动区外,不必先选「其他」;
           单选与选项互斥,多选作为额外一项追加提交) -->
      <div v-if="isChoiceField" class="uf-free">
        <input
          v-model="freeText[currentField.name]"
          class="uf-input"
          :placeholder="t('userForm.otherPlaceholder')"
          @input="onFreeInput(currentField)"
          @keydown.enter.prevent="next()"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
/*
 * macOS 风格表单控件(与 ChatFormBar / ApprovalBar 同一套语言):
 * 发丝线边框(0.5px)+ 6px 控件圆角 + 主色选中态 + 3.5px 主色聚焦环;
 * 选项列表为 macOS 列表行(圆角行 + 悬停填充,选中行主色淡染)。
 */
.uf-wrap {
  margin: 4px 0 2px;
}

.uf-field {
  margin-bottom: 12px;
  /* 分页切换:与卡片入场同一缓动,位移量更小(macOS 的克制过渡) */
  animation: uf-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}

.uf-field-first {
  margin-top: 0;
}

@media (prefers-reduced-motion: reduce) {
  .uf-field {
    animation: none;
  }
}

/* 文本 / 数字输入:macOS 文本框(发丝线 + 聚焦主色环) */
.uf-input {
  width: 100%;
  max-width: 420px;
  height: 28px;
  padding: 0 9px;
  border: 0.5px solid var(--border-strong);
  border-radius: 6px;
  /* 输入域用基础底色(浅色白纸 / 暗色浅一档的凹槽),不用 --panel-bg:
     后者暗色下是纯黑面板色,落在卡片上会成"黑洞" */
  background-color: var(--bg-base);
  color: var(--text-primary);
  font-family: inherit;
  font-size: var(--font-size-sm);
  outline: none;
  box-sizing: border-box;
  /* 去掉 UA 原生控件装饰(数字步进器等),控件外观全部由这里定义 */
  appearance: none;
  -webkit-appearance: none;
  transition:
    border-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

/* 数字框:去掉 UA 步进器(macOS 数字输入无步进器;键盘 ↑/↓ 仍可用),
   否则暗色卡片里会露出一个亮色小方块 */
.uf-input[type='number']::-webkit-outer-spin-button,
.uf-input[type='number']::-webkit-inner-spin-button {
  -webkit-appearance: none;
  appearance: none;
  margin: 0;
}

.uf-input:focus {
  border-color: var(--accent-text);
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--accent-text) 28%, transparent);
}

.uf-input::placeholder {
  color: var(--text-tertiary);
}

/* 选择类字段的自由输入:贴在选项列表下方,与列表留一点呼吸 */
.uf-free {
  margin-top: 8px;
}

/* 选项列表:macOS 列表行 */
.uf-options {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 220px;
  overflow-y: auto;
}

.uf-choice-label {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 30px;
  padding: 4px 8px;
  border-radius: 7px;
  cursor: pointer;
  user-select: none;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  transition: background-color var(--transition-fast);
}

.uf-choice-label:hover {
  background-color: color-mix(in srgb, var(--bg-5) 16%, transparent);
}

/* 选中行:主色淡染(macOS 列表选中态) */
.uf-choice-label:has(.uf-choice-input:checked) {
  background-color: color-mix(in srgb, var(--accent-text) 12%, transparent);
}

/* 单选圆点 / 复选圆角方块:主色选中态 */
.uf-choice-input {
  appearance: none;
  -webkit-appearance: none;
  width: 16px;
  height: 16px;
  border: 1px solid var(--border-strong);
  border-radius: 50%;
  /* 同上:未选中用基础底色,暗色下是凹槽而不是纯黑 */
  background: var(--bg-base);
  cursor: pointer;
  flex-shrink: 0;
  margin: 0;
  position: relative;
  transition:
    border-color var(--transition-fast),
    background 0.15s;
}

.uf-choice-input[type='checkbox'] {
  border-radius: 4px;
}

.uf-choice-label:hover .uf-choice-input {
  border-color: var(--text-tertiary);
}

.uf-choice-input:checked {
  border-color: var(--accent-text);
  background: var(--accent-text);
}

.uf-choice-input:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--accent-text) 30%, transparent);
}

.uf-choice-input:checked::after {
  content: '';
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #fff;
}

.uf-choice-input[type='checkbox']:checked::after {
  width: 4px;
  height: 8px;
  border-radius: 0;
  border: solid #fff;
  border-width: 0 2px 2px 0;
  transform: translate(-50%, -60%) rotate(45deg);
  background: transparent;
}

.uf-choice-text {
  font-size: var(--font-size-sm);
  line-height: 1.4;
}

@keyframes uf-in {
  from {
    opacity: 0;
    transform: translateY(3px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>