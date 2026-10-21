<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import MultilineTextField from '@/components/common/MultilineTextField.vue'
import type { SkillDoc } from '@/services/dto'

const props = defineProps<{ visible: boolean; skill: SkillDoc | null }>()

const emit = defineEmits<{
  (e: 'update:visible', visible: boolean): void
  (e: 'save', data: SkillForm): void
}>()

const { t } = useI18n()

const domainOptions = ['办公自动化', '数据分析', '内容创作', '研发效能', '安全合规', '个人助理']

interface SkillForm {
  name: string
  description: string
  prompt: string
  category: string[]
  version: string
  author: string
  tags: string[]
}

const form = ref<SkillForm>(createEmptyForm())
const saving = ref(false)

function createEmptyForm(): SkillForm {
  return {
    name: '',
    description: '',
    prompt: '',
    category: [],
    version: '1.0.0',
    author: '',
    tags: []
  }
}

function toArray(v: string | string[] | null | undefined): string[] {
  if (Array.isArray(v)) return [...v]
  if (!v) return []
  return v
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
}

watch(
  () => props.visible,
  (v) => {
    if (v && props.skill) {
      form.value = {
        name: props.skill.name || '',
        description: props.skill.description || '',
        prompt: props.skill.prompt || '',
        category: toArray(props.skill.category),
        version: props.skill.version || '1.0.0',
        author: props.skill.author || '',
        tags: Array.isArray(props.skill.tags) ? [...props.skill.tags] : []
      }
    } else if (v) {
      form.value = createEmptyForm()
    }
  }
)

function toggleSuggestion(list: string[], value: string): void {
  const idx = list.indexOf(value)
  if (idx >= 0) {
    list.splice(idx, 1)
  } else {
    list.push(value)
  }
}

function handleOk(): void {
  if (!form.value.name.trim() || !form.value.description.trim()) return
  saving.value = true
  emit('save', {
    ...form.value,
    tags: form.value.tags
  })
  saving.value = false
}

function handleCancel(): void {
  emit('update:visible', false)
}
</script>

<template>
  <a-modal
    :visible="visible"
    :title="skill ? t('settings.skills.edit') : t('settings.skills.create')"
    modal-class="mac-modal"
    @cancel="handleCancel"
    @close="handleCancel"
    unmount-on-close
    width="960px"
    :body-style="{ maxHeight: '70vh', overflow: 'auto' }"
  >
    <div class="form">
      <div class="form-row-inline">
        <div class="form-row" style="flex: 2">
          <label class="form-label">{{ t('settings.skills.name') }} <span class="required">*</span></label>
          <a-input
            v-model="form.name"
            :placeholder="t('settings.skills.namePlaceholder')"
            :max-length="64"
            allow-clear
          />
        </div>
        <div class="form-row" style="flex: 1">
          <label class="form-label">{{ t('settings.skills.author') }}</label>
          <a-input
            v-model="form.author"
            :placeholder="t('settings.skills.authorPlaceholder')"
            allow-clear
          />
        </div>
        <div class="form-row" style="flex: 0 0 100px">
          <label class="form-label">{{ t('settings.skills.version') }}</label>
          <a-input v-model="form.version" placeholder="1.0.0" />
        </div>
      </div>
      <div class="form-row-inline">
        <div class="form-row" style="flex: 1">
          <label class="form-label">{{ t('settings.skills.category') }}</label>
          <a-input-tag v-model="form.category" :placeholder="t('settings.skills.categoryPlaceholder')" />
          <div class="suggestion-row">
            <a-tag
              v-for="d in domainOptions"
              :key="d"
              size="small"
              :class="{ suggested: !form.category.includes(d) }"
              @click="toggleSuggestion(form.category, d)"
            >
              {{ d }}
            </a-tag>
          </div>
        </div>
        <div class="form-row" style="flex: 1">
          <label class="form-label">{{ t('settings.skills.tags') }}</label>
          <a-input-tag v-model="form.tags" :placeholder="t('settings.skills.tagsPlaceholder')" />
        </div>
      </div>
      <div class="form-row">
        <label class="form-label">{{ t('settings.skills.description') }} <span class="required">*</span></label>
        <a-textarea
          v-model="form.description"
          :placeholder="t('settings.skills.descPlaceholder')"
          :max-length="1024"
          :auto-size="{ minRows: 2, maxRows: 3 }"
          allow-clear
        />
      </div>
      <div class="form-row">
        <label class="form-label">{{ t('settings.skills.prompt') }}</label>
        <!-- 大段指令用多行输入框（内容仍是 Markdown 文本） -->
        <MultilineTextField
          v-model="form.prompt"
          :min-height="420"
          :placeholder="t('settings.skills.promptPlaceholder')"
        />
      </div>

    </div>

    <!-- 底部操作:固定弹窗底部(macOS 风格,取消 + 系统蓝保存) -->
    <template #footer>
      <div class="modal-footer">
        <a-button @click="handleCancel">{{ t('common.cancel') }}</a-button>
        <a-button type="primary" :loading="saving" @click="handleOk">{{ t('common.save') }}</a-button>
      </div>
    </template>
  </a-modal>
</template>

<style scoped>
.form {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.form-row-inline {
  display: flex;
  gap: 12px;
}

.form-label {
  font-size: var(--font-size-md);
  font-weight: 500;
  color: var(--text-primary);
}

.required {
  color: var(--danger);
}

.suggestion-row {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 4px;
}

.suggestion-row .suggested {
  cursor: pointer;
  opacity: 0.55;
  transition: opacity 0.15s;
}

.suggestion-row .suggested:hover {
  opacity: 1;
}

.form-row :deep(.arco-input-tag .arco-tag) {
  background: var(--bg-3);
  border-color: var(--border-light);
  color: var(--text-primary);
}

.form-row :deep(.arco-input-tag .arco-input-tag-input) {
  color: var(--text-primary);
}

.form-row :deep(.arco-input-tag .arco-input-tag-input::placeholder) {
  color: var(--text-tertiary);
}

/* 底部操作:macOS 风格 */
.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding-top: 4px;
}

.modal-footer :deep(.arco-btn) {
  border-radius: 6px;
}

.modal-footer :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* ---------- macOS 风格控件覆盖 ---------- */
/* 输入类无边框填充已上移到全局 styles/inputs.css */
</style>
