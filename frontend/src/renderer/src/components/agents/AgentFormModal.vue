<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { MdPreview } from 'md-editor-v3'
import { invoke } from '@/services/backend'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import KbMarkdownPreview from '@/components/preview/KbMarkdownPreview.vue'
import MultilineTextField from '@/components/common/MultilineTextField.vue'
import type {
  AgentCard,
  AgentExecMode,
  AgentPermissionMode,
  ToolInfo,
  SkillSummary
} from '@/services/dto'

const props = defineProps<{
  visible: boolean
  agent: AgentCard | null
  /** 只读预览（内置智能体卡片点击进入）：字段全部锁定，指引按 markdown 渲染后展示 */
  preview?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', visible: boolean): void
  (e: 'save', data: AgentForm): void
}>()

const { t } = useI18n()
const settings = useSettingsStore()

/**
 * 尺寸上限:宽 ≤ 75vw、高 ≤ 75vh —— 即弹窗最多占窗口的 3/4(超出部分在弹窗体内滚动)。
 * 与技能弹窗同款写法(width/body-style),但宽度用 min() 收口、高度按 75vh 反推 body 上限。
 */
const MODAL_WIDTH = 'min(960px, 75vw)'
const MODAL_TOP = '12.5vh'
const BODY_STYLE = { maxHeight: 'calc(75vh - 140px)', overflow: 'auto' } as const

// markdown 明暗主题跟随应用（仅只读预览用）
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

interface AgentForm {
  name: string
  description: string
  guidance: string
  toolIds: string[]
  skillIds: string[]
  /** 执行模式:决定会话模式(只读=plan,工具层硬拒写) */
  mode: AgentExecMode
  /** 权限模式:决定会话审批 */
  permission: AgentPermissionMode
}

/** 执行模式选项(与后端 AgentExecMode 的 snake_case 值一致) */
const MODE_OPTIONS: { value: AgentExecMode; key: string }[] = [
  { value: 'inherit', key: 'modeInherit' },
  { value: 'read_only', key: 'modeReadOnly' },
  { value: 'read_write', key: 'modeReadWrite' },
  { value: 'auto', key: 'modeAuto' }
]

/** 权限模式选项 */
const PERMISSION_OPTIONS: { value: AgentPermissionMode; key: string }[] = [
  { value: 'inherit', key: 'permissionInherit' },
  { value: 'manual', key: 'permissionManual' },
  { value: 'auto', key: 'permissionAuto' }
]

const form = ref<AgentForm>(createEmptyForm())
const tools = ref<ToolInfo[]>([])
const skills = ref<SkillSummary[]>([])

function createEmptyForm(): AgentForm {
  return {
    name: '',
    description: '',
    guidance: '',
    toolIds: [],
    skillIds: [],
    mode: 'inherit',
    permission: 'inherit'
  }
}

/** 工具 / 技能候选：进后端目录与技能索引（失败静默——表单仍可提交，只是选项为空） */
async function loadOptions(): Promise<void> {
  try {
    tools.value = (await invoke('list_tools', {})) as ToolInfo[]
  } catch {
    tools.value = []
  }
  try {
    skills.value = (await invoke('list_skills', { enabledOnly: false })) as SkillSummary[]
  } catch {
    skills.value = []
  }
}

onMounted(loadOptions)

/** 用当前 props 回填表单（打开时 + 挂载时已是可见态，两种情况都要填） */
function loadForm(): void {
  if (props.agent) {
    form.value = {
      name: props.agent.name || '',
      description: props.agent.description || '',
      guidance: props.agent.guidance || '',
      toolIds: [...props.agent.toolIds],
      skillIds: [...props.agent.skillIds],
      mode: props.agent.mode ?? 'inherit',
      permission: props.agent.permission ?? 'inherit'
    }
  } else {
    form.value = createEmptyForm()
  }
}

// immediate:挂载时若已是可见态(visible 没有 false→true 变化)同样要回填，
// 否则会显示上一次/空的表单；同时监听 agent 变化，支持「已打开时切到另一张卡片」
watch(
  () => [props.visible, props.agent] as const,
  ([visible]) => {
    if (visible) loadForm()
  },
  { immediate: true }
)

const title = computed(() => {
  if (props.preview) return t('settings.agents.previewTitle')
  return props.agent ? t('settings.agents.editTitle') : t('settings.agents.createTitle')
})

const canSave = computed(() => form.value.name.trim().length > 0)

/** 技能名 → 描述（下拉里显示更易判断） */
const skillOptions = computed(() =>
  skills.value.map((s) => ({ value: s.name, label: s.description ? `${s.name} — ${s.description}` : s.name }))
)

function handleSubmit(): void {
  if (props.preview || !canSave.value) return
  emit('save', {
    name: form.value.name.trim(),
    description: form.value.description,
    guidance: form.value.guidance,
    toolIds: [...form.value.toolIds],
    skillIds: [...form.value.skillIds],
    mode: form.value.mode,
    permission: form.value.permission
  })
}

/**
 * 关闭:本组件对 a-modal 是**全受控**用法(`:visible` 恒为布尔), Arco 内部 `close()` 只改自己的
 * `_visible` 又被 prop 覆盖, 所以关闭必须由这里推回父级。三条路径都收敛到此:
 * 点 X / ESC / 点遮罩(@cancel)、离场动画结束(@close)、底部按钮(直接调用, 不依赖 Arco 事件管线)。
 */
function handleCancel(): void {
  emit('update:visible', false)
}
</script>

<template>
  <a-modal
    :visible="visible"
    :title="title"
    :width="MODAL_WIDTH"
    :top="MODAL_TOP"
    :body-style="BODY_STYLE"
    modal-class="agent-modal"
    unmount-on-close
    @cancel="handleCancel"
    @close="handleCancel"
  >
    <div class="form">
      <div class="form-row">
        <label class="form-label">
          {{ t('settings.agents.fieldName') }} <span class="required">*</span>
        </label>
        <a-input
          v-model="form.name"
          :disabled="preview"
          :max-length="64"
          allow-clear
          :placeholder="t('settings.agents.namePlaceholder')"
          @press-enter="handleSubmit"
        />
      </div>

      <div class="form-row">
        <label class="form-label">{{ t('settings.agents.fieldDescription') }}</label>
        <a-textarea
          v-model="form.description"
          :disabled="preview"
          :max-length="1024"
          :auto-size="{ minRows: 2, maxRows: 4 }"
          allow-clear
          :placeholder="t('settings.agents.descriptionPlaceholder')"
        />
      </div>

      <div class="form-row">
        <label class="form-label">{{ t('settings.agents.fieldGuidance') }}</label>
        <!-- 预览:直接渲染 markdown(与知识库预览同一渲染器,图片/代码块表现一致) -->
        <div v-if="preview" class="editor-pane preview-pane">
          <MdPreview
            v-if="form.guidance"
            :model-value="form.guidance"
            :theme="isDark ? 'dark' : 'light'"
            :code-theme="isDark ? 'dark' : 'light'"
            :preview-component="KbMarkdownPreview"
            :no-katex="false"
          />
          <p v-else class="pane-empty">{{ t('settings.agents.noGuidance') }}</p>
        </div>
        <!-- 编辑:大段文本用多行输入框（内容仍是 Markdown 文本） -->
        <MultilineTextField
          v-else
          v-model="form.guidance"
          :min-height="260"
          :placeholder="t('settings.agents.guidanceEditorPlaceholder')"
        />
        <span class="field-hint">{{ t('settings.agents.guidancePlaceholder') }}</span>
      </div>

      <div class="form-row-inline">
        <div class="form-row" style="flex: 1">
          <label class="form-label">{{ t('settings.agents.fieldMode') }}</label>
          <a-select v-model="form.mode" :disabled="preview">
            <a-option v-for="opt in MODE_OPTIONS" :key="opt.value" :value="opt.value">
              {{ t(`settings.agents.${opt.key}`) }}
            </a-option>
          </a-select>
          <span class="field-hint">{{ t('settings.agents.modeHint') }}</span>
        </div>
        <div class="form-row" style="flex: 1">
          <label class="form-label">{{ t('settings.agents.fieldPermission') }}</label>
          <a-select v-model="form.permission" :disabled="preview">
            <a-option v-for="opt in PERMISSION_OPTIONS" :key="opt.value" :value="opt.value">
              {{ t(`settings.agents.${opt.key}`) }}
            </a-option>
          </a-select>
          <span class="field-hint">{{ t('settings.agents.permissionHint') }}</span>
        </div>
      </div>

      <div class="form-row">
        <label class="form-label">{{ t('settings.agents.fieldTools') }}</label>
        <a-select
          v-model="form.toolIds"
          multiple
          allow-search
          allow-clear
          :disabled="preview"
          :placeholder="t('settings.agents.toolsPlaceholder')"
        >
          <a-option v-for="tool in tools" :key="tool.name" :value="tool.name">
            {{ tool.name }}
          </a-option>
        </a-select>
        <span class="field-hint">{{ t('settings.agents.toolsHint') }}</span>
      </div>

      <div class="form-row">
        <label class="form-label">{{ t('settings.agents.fieldSkills') }}</label>
        <a-select
          v-model="form.skillIds"
          multiple
          allow-search
          allow-clear
          :disabled="preview"
          :placeholder="t('settings.agents.skillsPlaceholder')"
        >
          <a-option v-for="opt in skillOptions" :key="opt.value" :value="opt.value">
            {{ opt.label }}
          </a-option>
        </a-select>
        <span class="field-hint">{{ t('settings.agents.skillsHint') }}</span>
      </div>
    </div>

    <!-- 底部操作:固定弹窗底部(macOS 风格,与技能弹窗一致) -->
    <template #footer>
      <div class="modal-footer">
        <span v-if="preview" class="footer-hint">{{ t('settings.agents.previewHint') }}</span>
        <a-button @click="handleCancel">
          {{ preview ? t('common.close') : t('common.cancel') }}
        </a-button>
        <a-button v-if="!preview" type="primary" :disabled="!canSave" @click="handleSubmit">
          {{ t('common.save') }}
        </a-button>
      </div>
    </template>
  </a-modal>
</template>

<style scoped>
/* 表单/底部/编辑器:与技能编辑弹窗同款样式 */
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
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
}

.required {
  color: var(--danger);
}

.field-hint {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  line-height: 1.6;
}

/* markdown 编辑器:与知识库文件预览同款(md-editor-v3),圆角容器(与表单控件同半径) */
.editor-pane {
  height: 300px;
  border: 1px solid var(--border-base);
  border-radius: 6px;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.editor-pane :deep(.md-editor) {
  flex: 1;
  min-height: 0;
  height: 100%;
  box-shadow: none;
}

/* 预览态:纯阅读,高度随内容(MdPreview 自身 height:auto,不要强制 100%) */
.preview-pane {
  height: auto;
  max-height: 300px;
  overflow: auto;
  padding: 4px 14px;
}

.pane-empty {
  margin: 0;
  padding: 16px 0;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 底部操作:macOS 风格 */
.modal-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding-top: 4px;
}

.footer-hint {
  margin-right: auto;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
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

/* 输入框 / 文本域 / 下拉:更柔和的圆角 */
.form :deep(.arco-input-wrapper),
.form :deep(.arco-textarea-wrapper),
.form :deep(.arco-select) {
  border-radius: 8px;
}
</style>

<style>
/* ===== macOS 风格弹窗骨架(Arco Modal 传送至 body,必须非 scoped;agent- 前缀防冲突) =====
   与知识库新建弹窗(.fm-modal)同一套:颜色全走主题变量,明暗主题与强调色自动适配。
   相比 fm- 骨架这里补齐了文本域 / 下拉 / 多选标签 / 禁用态(预览态字段是禁用的) */
.agent-modal.arco-modal,
.agent-modal .arco-modal {
  border-radius: 12px;
  overflow: hidden;
  background: var(--panel-bg);
  box-shadow: 0 18px 50px rgba(0, 0, 0, 0.22);
}

.agent-modal .arco-modal-header {
  padding: 14px 16px 10px;
  background: transparent;
  border-bottom: none;
}

.agent-modal .arco-modal-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
  text-align: left;
}

/* 关闭按钮:方形热区圆角,悬浮才浮现底色(macOS 工具按钮观感) */
.agent-modal .arco-modal-close-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: 6px;
  color: var(--text-tertiary);
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.agent-modal .arco-modal-close-btn:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}

.agent-modal .arco-modal-body {
  padding: 4px 16px 12px;
  background: transparent;
}

.agent-modal .arco-modal-footer {
  padding: 10px 16px 14px;
  background: transparent;
  border-top: 1px solid var(--border-light);
}

/* 表单控件:填充式圆角 + 聚焦主题色描边(macOS 表单观感) */
.agent-modal .arco-input-wrapper,
.agent-modal .arco-textarea-wrapper,
.agent-modal .arco-select-view {
  background: var(--bg-2);
  border: 1px solid transparent;
  border-radius: 6px;
  transition:
    border-color 0.15s ease,
    background-color 0.15s ease;
}

.agent-modal .arco-input-wrapper:hover,
.agent-modal .arco-textarea-wrapper:hover,
.agent-modal .arco-select-view:hover {
  background: var(--bg-hover);
  border-color: transparent;
}

.agent-modal .arco-input-wrapper:focus-within,
.agent-modal .arco-textarea-wrapper:focus-within,
.agent-modal .arco-select-view-focus {
  background: var(--bg-base);
  border-color: var(--accent-text);
}

.agent-modal .arco-input,
.agent-modal .arco-textarea,
.agent-modal .arco-select-view-value {
  color: var(--text-primary);
}

.agent-modal .arco-input::placeholder,
.agent-modal .arco-textarea::placeholder,
.agent-modal .arco-select-view-value-input::placeholder {
  color: var(--text-disabled);
}

/* 多选标签:中性底 + 细边框(与卡片上的工具标签一致) */
.agent-modal .arco-select-view-multiple .arco-tag,
.agent-modal .arco-tag {
  background: var(--bg-3);
  border: 1px solid var(--border-light);
  border-radius: 6px;
  color: var(--text-primary);
}

/* 禁用态(只读预览):保留填充底,压低对比而不是变灰块 */
.agent-modal .arco-input-wrapper:has(input:disabled),
.agent-modal .arco-textarea-wrapper:has(textarea:disabled),
.agent-modal .arco-select-view-disabled {
  background: var(--bg-2);
  border-color: transparent;
  opacity: 0.72;
}

/* 底部按钮:macOS 圆角 + 主按钮实心强调色 */
.agent-modal .arco-modal-footer .arco-btn {
  border-radius: 6px;
}
</style>

