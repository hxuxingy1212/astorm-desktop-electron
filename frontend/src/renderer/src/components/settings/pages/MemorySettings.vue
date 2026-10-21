<script setup lang="ts">
import SettingsSection from '../SettingsSection.vue'
/**
 * 记忆设置页（长期记忆 / OKF 编译管道）。
 *
 * 只管**配置**：开关、编译模型、分类表（types）、预算与上限。记忆内容的浏览在侧栏
 * 「记忆」页（`views/MemoryView.vue`）—— 配置与内容分两处，避免一个页面既改设置又翻内容。
 *
 * 保存方式与其它设置页一致：`get_settings` 读全量 → 只改 `memory` 段 → `save_settings` 写回，
 * 主进程随即 `syncBackendConfig()` 热更后端（下次编译 / 查询即生效，不重启）。
 */
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { invoke } from '@/services/backend'
import { useModelsStore } from '@/stores/models'
import type { AppSettings, MemoryCategorySettings, MemorySettings } from '@/services/dto'

const { t } = useI18n()
const models = useModelsStore()

/** 与 Rust `MemoryConfig::default` / 主进程 `DEFAULT_MEMORY` 一致 */
const DEFAULTS: MemorySettings = {
  enabled: true,
  autoCompile: false,
  provider: null,
  model: null,
  categories: [],
  maxUnitsPerCompileCall: 20,
  rawCharBudget: 24000,
  toolResultChars: 1200,
  judgeCandidates: 8,
  extractSelfCritique: false,
  curate: true,
  curateUnitsPerCall: 60,
  minConfidence: 0.6,
  maxUnitsPerSession: 40,
  candidateMinOverlap: 0.25,
  maxTags: 8,
  maxAppendSections: 3,
  relatedConcepts: 3,
  injectMemories: true,
  injectedConcepts: 5
}

/** 与后端 `MemoryConfig::default_categories` 一致（顺序即提示词取值集顺序；信号强的排在 fact 前） */
const DEFAULT_CATEGORIES: MemoryCategorySettings[] = [
  { id: 'decision', dir: 'decisions', type: 'Decision', hint: t('settings.memory.catDecisionHint') },
  { id: 'preference', dir: 'preferences', type: 'Preference', hint: t('settings.memory.catPreferenceHint') },
  { id: 'instruction', dir: 'instructions', type: 'Instruction', hint: t('settings.memory.catInstructionHint') },
  { id: 'convention', dir: 'conventions', type: 'Convention', hint: t('settings.memory.catConventionHint') },
  { id: 'procedure', dir: 'procedures', type: 'Procedure', hint: t('settings.memory.catProcedureHint') },
  { id: 'constraint', dir: 'constraints', type: 'Constraint', hint: t('settings.memory.catConstraintHint') },
  { id: 'fact', dir: 'concepts', type: 'Fact', hint: t('settings.memory.catFactHint') },
  { id: 'relationship', dir: 'entities', type: 'Entity', hint: t('settings.memory.catEntityHint') },
  { id: 'other', dir: 'concepts', type: 'Other', hint: t('settings.memory.catOtherHint') }
]

const form = reactive<MemorySettings>({ ...DEFAULTS, categories: clone(DEFAULT_CATEGORIES) })
const saving = ref(false)

function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T
}

/** 供应商下拉：已配置的模型供应商 */
const providerOptions = computed(() => models.providers.map((p) => p.name))
/** 缺省供应商 = 列表第一个（与后端 LlmRegistry 一致） */
const effectiveProvider = computed(() => form.provider || providerOptions.value[0] || '')
/** 模型下拉：跟随所选供应商；未指定供应商时跟随默认供应商 */
const modelOptions = computed(() => {
  const p = models.providers.find((x) => x.name === effectiveProvider.value)
  return (p?.models ?? []).map((m) => m.name)
})

/* ---------- 分类表校验 ---------- */

/** 行号 → 错误提示（行内红字；保存前拦截） */
const rowErrors = computed<Record<number, string>>(() => {
  const errs: Record<number, string> = {}
  const seen = new Set<string>()
  form.categories.forEach((c, i) => {
    const id = c.id.trim().toLowerCase()
    if (!id) {
      errs[i] = t('settings.memory.idRequired')
      return
    }
    if (!/^[a-z0-9_-]+$/.test(id)) {
      errs[i] = t('settings.memory.idInvalid')
      return
    }
    if (seen.has(id)) {
      errs[i] = t('settings.memory.idDuplicate')
      return
    }
    seen.add(id)
    if (c.dir.includes('/') || c.dir.includes('\\')) errs[i] = t('settings.memory.dirInvalid')
  })
  return errs
})
const categoriesInvalid = computed(() => Object.keys(rowErrors.value).length > 0)

function addCategory(): void {
  form.categories.push({ id: '', dir: '', type: '', hint: '' })
}

function removeCategory(index: number): void {
  form.categories.splice(index, 1)
}

/** 供应商变更：换供应商后原模型可能不在新列表里，清掉以免存下一个不存在的模型 */
function onProviderChange(v: unknown): void {
  form.provider = v ? String(v) : null
  if (form.model && !modelOptions.value.includes(form.model)) form.model = null
}

function onModelChange(v: unknown): void {
  form.model = v ? String(v) : null
}

function resetCategories(): void {
  form.categories = clone(DEFAULT_CATEGORIES)
}

/* ---------- 读写 ---------- */

async function loadAll(): Promise<void> {
  try {
    const s = (await invoke('get_settings')) as AppSettings
    if (s.memory) {
      Object.assign(form, clone(s.memory))
      // 空表在后端会回退默认表；前端一并对齐，免得显示"没有分类"而后端在用默认 9 条
      if (!Array.isArray(form.categories) || form.categories.length === 0) {
        form.categories = clone(DEFAULT_CATEGORIES)
      }
    }
  } catch {
    /* 保持默认值 */
  }
}

function resetAll(): void {
  Object.assign(form, clone(DEFAULTS))
  form.categories = clone(DEFAULT_CATEGORIES)
}

async function saveAll(): Promise<void> {
  if (categoriesInvalid.value) {
    Message.error(t('settings.memory.invalid'))
    return
  }
  if (form.categories.length === 0) {
    Message.error(t('settings.memory.categoryEmpty'))
    return
  }
  saving.value = true
  try {
    const s = (await invoke('get_settings')) as AppSettings
    s.memory = {
      ...clone(form),
      provider: form.provider || null,
      model: form.model || null,
      categories: form.categories.map((c) => ({
        id: c.id.trim().toLowerCase(),
        dir: c.dir.trim(),
        type: c.type.trim(),
        hint: c.hint.trim()
      }))
    }
    await invoke('save_settings', { settings: s })
    Message.success(t('common.saveSuccess'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    saving.value = false
  }
}

/** 预算字段表（渲染数字输入；键都取自数值字段，避免 v-model 需要类型断言） */
type BudgetKey =
  | 'maxUnitsPerCompileCall'
  | 'rawCharBudget'
  | 'toolResultChars'
  | 'judgeCandidates'
  | 'curateUnitsPerCall'
  | 'maxUnitsPerSession'
  | 'maxTags'
  | 'maxAppendSections'
  | 'relatedConcepts'

const budgetFields = computed<
  { key: BudgetKey; label: string; desc: string; min: number; max: number; step: number }[]
>(() => [
  {
    key: 'maxUnitsPerCompileCall',
    label: t('settings.memory.maxUnitsPerCompileCall'),
    desc: t('settings.memory.maxUnitsPerCompileCallDesc'),
    min: 1,
    max: 200,
    step: 1
  },
  {
    key: 'rawCharBudget',
    label: t('settings.memory.rawCharBudget'),
    desc: t('settings.memory.rawCharBudgetDesc'),
    min: 4000,
    max: 200000,
    step: 1000
  },
  {
    key: 'toolResultChars',
    label: t('settings.memory.toolResultChars'),
    desc: t('settings.memory.toolResultCharsDesc'),
    min: 200,
    max: 50000,
    step: 100
  },
  {
    key: 'judgeCandidates',
    label: t('settings.memory.judgeCandidates'),
    desc: t('settings.memory.judgeCandidatesDesc'),
    min: 0,
    max: 50,
    step: 1
  },
  {
    key: 'curateUnitsPerCall',
    label: t('settings.memory.curateUnitsPerCall'),
    desc: t('settings.memory.curateUnitsPerCallDesc'),
    min: 1,
    max: 500,
    step: 5
  },
  {
    key: 'maxUnitsPerSession',
    label: t('settings.memory.maxUnitsPerSession'),
    desc: t('settings.memory.maxUnitsPerSessionDesc'),
    min: 0,
    max: 500,
    step: 5
  },
  {
    key: 'maxTags',
    label: t('settings.memory.maxTags'),
    desc: t('settings.memory.maxTagsDesc'),
    min: 0,
    max: 100,
    step: 1
  },
  {
    key: 'maxAppendSections',
    label: t('settings.memory.maxAppendSections'),
    desc: t('settings.memory.maxAppendSectionsDesc'),
    min: 0,
    max: 100,
    step: 1
  },
  {
    key: 'relatedConcepts',
    label: t('settings.memory.relatedConcepts'),
    desc: t('settings.memory.relatedConceptsDesc'),
    min: 0,
    max: 20,
    step: 1
  }
])

onMounted(() => {
  void loadAll()
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.memory.title') }}</h2>
    </div>

    <div class="page-body">
      <!-- 基础 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.memory.basic') }}
        </template>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.enable') }}</span>
              <span class="row-desc">{{ t('settings.memory.enableDesc') }}</span>
            </div>
            <a-switch v-model="form.enabled" />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.autoCompile') }}</span>
              <span class="row-desc">{{ t('settings.memory.autoCompileDesc') }}</span>
            </div>
            <a-switch v-model="form.autoCompile" />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.model') }}</span>
              <span class="row-desc">
                {{ providerOptions.length ? t('settings.memory.modelDesc') : t('settings.memory.noProvider') }}
              </span>
            </div>
            <div class="model-pick">
              <a-select
                :model-value="form.provider ?? undefined"
                :placeholder="t('settings.memory.providerDefault')"
                size="small"
                allow-clear
                style="width: 140px"
                @change="onProviderChange"
              >
                <a-option v-for="p in providerOptions" :key="p" :value="p">{{ p }}</a-option>
              </a-select>
              <a-select
                :model-value="form.model ?? undefined"
                :placeholder="t('settings.memory.modelDefault')"
                size="small"
                allow-clear
                style="width: 170px"
                @change="onModelChange"
              >
                <a-option v-for="m in modelOptions" :key="m" :value="m">{{ m }}</a-option>
              </a-select>
            </div>
          </div>
          <!-- 会话侧注入：决定模型在会话里能看到多少记忆（不改变编译产出） -->
          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.injectMemories') }}</span>
              <span class="row-desc">{{ t('settings.memory.injectMemoriesDesc') }}</span>
            </div>
            <a-switch v-model="form.injectMemories" />
          </div>
          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.injectedConcepts') }}</span>
              <span class="row-desc">{{ t('settings.memory.injectedConceptsDesc') }}</span>
            </div>
            <a-input-number
              v-model="form.injectedConcepts"
              :min="0"
              :max="20"
              :step="1"
              size="small"
              :disabled="!form.injectMemories"
              style="width: 120px"
            />
          </div>

      </SettingsSection>

      <!-- 分类表（配置驱动的 types） -->
      <SettingsSection>
        <template #title>
          {{ t('settings.memory.categories') }}
          <a-tooltip :content="t('settings.memory.categoriesDesc')" position="top">
            <icon-help :size="14" class="cat-help" />
          </a-tooltip>
        </template>

          <div class="cat-table">
            <div class="cat-head">
              <span>{{ t('settings.memory.colId') }}</span>
              <span>{{ t('settings.memory.colDir') }}</span>
              <span>{{ t('settings.memory.colType') }}</span>
              <span>{{ t('settings.memory.colHint') }}</span>
              <span />
            </div>
            <div v-for="(cat, i) in form.categories" :key="i" class="cat-row">
              <div class="cat-cell">
                <a-input v-model="cat.id" size="small" :placeholder="t('settings.memory.idPlaceholder')" :error="!!rowErrors[i]" />
                <span v-if="rowErrors[i]" class="cat-error">{{ rowErrors[i] }}</span>
              </div>
              <a-input v-model="cat.dir" size="small" :placeholder="t('settings.memory.dirPlaceholder')" />
              <a-input v-model="cat.type" size="small" :placeholder="t('settings.memory.typePlaceholder')" />
              <a-input v-model="cat.hint" size="small" :placeholder="t('settings.memory.hintPlaceholder')" />
              <a-button class="cat-delete" size="small" type="text" :title="t('common.delete')" @click="removeCategory(i)">
                <icon-delete :size="14" />
              </a-button>
            </div>
          </div>

          <div class="cat-actions">
            <a-button size="small" @click="addCategory">
              <icon-plus :size="13" />
              {{ t('settings.memory.addCategory') }}
            </a-button>
            <a-button size="small" @click="resetCategories">
              {{ t('settings.memory.resetCategories') }}
            </a-button>
          </div>

          <!-- 改分类的副作用提示：中性备注卡，仅图标/标题用 warning 色，避免整块棕色 banner -->
          <div class="cat-note">
            <div class="cat-note-head">
              <icon-alert-circle :size="15" class="cat-note-icon" />
              <span>{{ t('settings.memory.footgunTitle') }}</span>
            </div>
            <ul class="cat-note-list">
              <li>{{ t('settings.memory.footgunDir') }}</li>
              <li>{{ t('settings.memory.footgunType') }}</li>
              <li>{{ t('settings.memory.footgunDelete') }}</li>
              <li>{{ t('settings.memory.footgunId') }}</li>
            </ul>
          </div>

      </SettingsSection>

      <!-- 预算与上限 -->
      <SettingsSection>
        <template #title>
          {{ t('settings.memory.budget') }}
        </template>

          <div class="row-desc cat-desc">{{ t('settings.memory.budgetDesc') }}</div>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.curate') }}</span>
              <span class="row-desc">{{ t('settings.memory.curateDesc') }}</span>
            </div>
            <a-switch v-model="form.curate" />
          </div>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.extractSelfCritique') }}</span>
              <span class="row-desc">{{ t('settings.memory.extractSelfCritiqueDesc') }}</span>
            </div>
            <a-switch v-model="form.extractSelfCritique" />
          </div>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.minConfidence') }}</span>
              <span class="row-desc">{{ t('settings.memory.minConfidenceDesc') }}</span>
            </div>
            <a-input-number v-model="form.minConfidence" :min="0" :max="1" :step="0.05" size="small" style="width: 120px" />
          </div>

          <div class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ t('settings.memory.candidateMinOverlap') }}</span>
              <span class="row-desc">{{ t('settings.memory.candidateMinOverlapDesc') }}</span>
            </div>
            <a-input-number
              v-model="form.candidateMinOverlap"
              :min="0"
              :max="1"
              :step="0.05"
              size="small"
              style="width: 120px"
            />
          </div>

          <div v-for="field in budgetFields" :key="field.key" class="settings-row">
            <div class="row-text">
              <span class="row-title">{{ field.label }}</span>
              <span class="row-desc">{{ field.desc }}</span>
            </div>
            <a-input-number
              v-model="form[field.key]"
              :min="field.min"
              :max="field.max"
              :step="field.step"
              size="small"
              style="width: 130px"
            />
          </div>

      </SettingsSection>
    </div>

    <div class="page-footer">
      <div class="footer-actions">
        <a-button @click="resetAll">{{ t('common.reset') }}</a-button>
        <a-button type="primary" :loading="saving" @click="saveAll">{{ t('common.save') }}</a-button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page {
  height: 100%;
  display: flex;
  flex-direction: column;
  padding: 28px 56px 16px;
}

.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 20px;
  flex-shrink: 0;
}

.page-title {
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
}

.page-body {
  flex: 1;
  overflow: auto;
  padding: 0 56px 4px 0;
  scrollbar-gutter: stable;
}

.settings-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 8px 0;
}

.row-text {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.row-title {
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.row-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

/* 编译模型:供应商 + 模型两个下拉并排 */
.model-pick {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

/* ---------- 分类表 ---------- */

/* 标题旁的说明入口：悬浮显示分类语义（与设置页字段说明同一套交互） */
.cat-help {
  display: inline-flex;
  align-items: center;
  flex: none;
  color: var(--text-tertiary);
  cursor: help;
  transition: color 0.15s ease;
}

.cat-help:hover {
  color: var(--text-secondary);
}

.cat-table {
  padding-left: 20px;
  padding-right: 20px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.cat-head,
.cat-row {
  display: grid;
  /* ID / 目录 / Type / 语义说明 / 删除（删除列只放图标按钮，固定窄列） */
  grid-template-columns:
    minmax(110px, 150px) minmax(110px, 150px) minmax(110px, 150px)
    minmax(180px, 1fr) 28px;
  gap: 8px;
  align-items: start;
}

.cat-head {
  padding: 0 2px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 删除按钮固定在最后一列右侧，不再撑破卡片内边距 */
.cat-delete {
  justify-self: end;
  width: 28px;
  min-width: 28px;
  padding: 0;
}

.cat-cell {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.cat-error {
  font-size: var(--font-size-2xs);
  color: rgb(var(--danger-6));
}

.cat-actions {
  padding-left: 20px;
  display: flex;
  gap: 8px;
}

/* 改分类副作用提示：作为设置行内的轻量说明，不再套第二层卡片 */
.cat-note {
  margin: 0 20px;
}

.cat-note-head {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.cat-note-icon {
  flex: none;
  color: var(--warning);
}

.cat-note-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin: 6px 0 0;
  padding: 0;
  list-style: none;
}

.cat-note-list li {
  position: relative;
  padding-left: 12px;
  font-size: var(--font-size-xs);
  line-height: 1.55;
  color: var(--text-secondary);
}

.cat-note-list li::before {
  content: '';
  position: absolute;
  top: calc(0.78em - 1.5px);
  left: 2px;
  width: 3px;
  height: 3px;
  border-radius: 50%;
  background: var(--text-tertiary);
}

.page-footer {
  flex-shrink: 0;
  padding: 8px 0 0;
}

.footer-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

/* ---------- macOS 风格控件覆盖 ---------- */

.settings-row :deep(.arco-switch) {
  border-radius: 999px;
}

.settings-row :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.settings-row :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}

.settings-row :deep(.arco-input-number),
.settings-row :deep(.arco-select) {
  border-radius: 6px;
}

.cat-table :deep(.arco-input) {
  border-radius: 6px;
}

.footer-actions :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

.footer-actions :deep(.arco-btn) {
  border-radius: 6px;
}
</style>
