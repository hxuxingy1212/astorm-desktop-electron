<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useModelsStore } from '@/stores/models'
import type { ModelProvider } from '@/stores/models'

/**
 * 模型配置页:左侧供应商列表 +「添加供应商」按钮,
 * 点击供应商后在右侧显示它的表单(名称/Base URL/API Key)与模型列表。
 * 模型通过「新增模型」添加,名称行内可编辑,每行有「检测 / 删除」按钮。
 */
const { t } = useI18n()
const models = useModelsStore()

const activeProvider = computed<ModelProvider | null>(
  () => models.providers[models.activeIndex] ?? null
)

// 「检测」按钮的加载态(模型名维度,同一时刻只测一个)
const testingName = ref('')

/**
 * 真实检测:调用主进程按协议发起一次图文请求,验证模型同时具备文本指令遵循与图片识别能力。
 * 失败时展示 API 返回的可读错误(如 HTTP 401: invalid api key)。
 */
async function onTest(modelName: string): Promise<void> {
  if (testingName.value) return
  const p = activeProvider.value
  if (!p) return
  testingName.value = modelName
  try {
    const res = await window.api.providers.test(p.name, modelName)
    if (res.ok) {
      Message.success(t('models.testOk'))
      return
    }
    if (res.message === 'test_need_base_url') {
      Message.error(t('models.testNeedBaseUrl'))
      return
    }
    if (res.message === 'test_need_api_key') {
      Message.error(t('models.testFail'))
      return
    }
    if (res.message === 'vision_unsupported') {
      Message.error(t('models.visionUnsupported'))
      return
    }
    Message.error(`${t('models.testFail')}: ${res.message}`)
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    testingName.value = ''
  }
}

function onAddProvider(): void {
  models.addProvider()
}

function onAddModel(): void {
  models.addModel()
}

function onSaveProvider(): void {
  const p = activeProvider.value
  if (!p) return
  if (!p.name.trim() || !p.baseUrl.trim() || !p.apiKey.trim()) {
    Message.error(t('models.fillRequired'))
    return
  }
  if (models.isProviderNameTaken(p.name)) {
    Message.error(t('models.nameTaken'))
    return
  }
  Message.success(t('models.saved'))
}

function onRemoveProvider(): void {
  const p = activeProvider.value
  if (p) models.removeProvider(p.name)
}

function onRemoveModel(modelName: string): void {
  models.removeModel(modelName)
}

// ---------- 拖拽排序:供应商列表与模型列表(列表第一项即「默认」) ----------
const draggingProvider = ref<number>(-1)
const dragOverProvider = ref<number>(-1)
const draggingModel = ref<number>(-1)
const dragOverModel = ref<number>(-1)

function onProviderDragStart(index: number): void {
  draggingProvider.value = index
}
function onProviderDrop(index: number): void {
  const from = draggingProvider.value
  if (from >= 0 && from !== index) models.moveProvider(from, index)
  clearProviderDrag()
}
function clearProviderDrag(): void {
  draggingProvider.value = -1
  dragOverProvider.value = -1
}
function onModelDragStart(e: DragEvent, index: number): void {
  // 从输入框/按钮上发起拖动时不进入排序(保证编辑与按钮操作正常)
  const target = e.target as HTMLElement | null
  if (target && target.closest('input, button, textarea')) {
    e.preventDefault()
    return
  }
  draggingModel.value = index
}
// 拖拽经过其他行:实时换序,让整体拖拽即时可见生效
function onModelDragOver(index: number): void {
  const from = draggingModel.value
  if (from < 0 || from === index) return
  models.moveModel(from, index)
  draggingModel.value = index
  dragOverModel.value = index
}
function onModelDrop(index: number): void {
  const from = draggingModel.value
  if (from >= 0 && from !== index) models.moveModel(from, index)
  clearModelDrag()
}
function clearModelDrag(): void {
  draggingModel.value = -1
  dragOverModel.value = -1
}
</script>

<template>
  <div class="model-page settings-editor-page">
    <!-- 页头:标题 + 描述(macOS 设置页风格) -->
    <div class="settings-editor-heading">
      <h2 class="settings-editor-title">{{ t('settings.menuModels') }}</h2>
      <p class="settings-editor-desc">{{ t('models.desc') }}</p>
    </div>

    <!-- 卡片:左供应商列表 + 右详情 -->
    <div class="settings-editor-card">
      <!-- 左侧:供应商列表 + 添加按钮 -->
      <div class="settings-editor-sidebar">
        <div class="settings-editor-list">
          <button
            v-for="(p, index) in models.providers"
            :key="index"
            type="button"
            class="settings-editor-item"
            :class="[models.activeIndex === index && 'settings-editor-item-active', draggingProvider === index && 'item-dragging', dragOverProvider === index && 'item-drop-over']"
            draggable="true"
            :title="t('models.sortHint')"
            @click="models.selectProvider(index)"
            @dragstart="onProviderDragStart(index)"
            @dragover.prevent="dragOverProvider = index"
            @drop.prevent="onProviderDrop(index)"
            @dragend="clearProviderDrag"
          >
            <span class="settings-editor-icon"><icon-box :size="16" /></span>
            <span class="settings-editor-name">
              <span class="truncate">{{ p.name || t('models.unnamed') }}</span>
              <span v-if="index === 0" class="settings-editor-tag">{{ t('models.default') }}</span>
            </span>
            <span class="settings-editor-dot" />
          </button>
        </div>
        <button type="button" class="settings-editor-add" @click="onAddProvider">
          <icon-plus :size="13" />
          {{ t('models.addProvider') }}
        </button>
      </div>

      <!-- 右侧:选中供应商的详情 -->
      <div class="settings-editor-detail">
        <template v-if="activeProvider">
          <!-- 供应商表单 -->
          <div class="settings-section">
            <div class="settings-editor-header">
              <div class="settings-editor-label">{{ t('models.provider') }}</div>
              <!-- 右上:保存按钮 + 删除按钮 -->
              <div class="settings-editor-actions">
                <a-button type="primary" size="small" @click="onSaveProvider">
                  <template #icon><icon-save :size="13" /></template>
                  {{ t('models.save') }}
                </a-button>
                <a-popconfirm
                  :content="t('models.deleteProviderConfirm')"
                  @ok="onRemoveProvider"
                >
                  <a-button size="small">
                    <template #icon><icon-delete :size="13" /></template>
                    {{ t('models.deleteProvider') }}
                  </a-button>
                </a-popconfirm>
              </div>
            </div>
            <div class="settings-editor-fields">
              <div class="settings-editor-field">
                <span class="settings-editor-field-label"><span class="settings-editor-required">*</span>{{ t('models.providerName') }}</span>
                <a-input v-model="activeProvider.name" size="medium" :placeholder="t('models.providerName')" allow-clear />
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label"><span class="settings-editor-required">*</span>{{ t('models.baseUrl') }}</span>
                <a-input v-model="activeProvider.baseUrl" size="medium" :placeholder="t('models.baseUrl')" allow-clear>
                  <template #prefix><icon-link :size="14" /></template>
                </a-input>
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label"><span class="settings-editor-required">*</span>{{ t('models.apiKey') }}</span>
                <a-input-password v-model="activeProvider.apiKey" size="medium" :placeholder="t('models.apiKey')" allow-clear>
                  <template #prefix><icon-safe :size="14" /></template>
                </a-input-password>
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">{{ t('models.apiFormat') }}</span>
                <a-select v-model="activeProvider.apiFormat" size="medium">
                  <template #prefix><icon-config :size="14" /></template>
                  <a-option value="anthropic_messages">{{ t('models.apiFormatAnthropic') }}</a-option>
                  <a-option value="chat_completions">{{ t('models.apiFormatChat') }}</a-option>
                  <a-option value="responses">{{ t('models.apiFormatResponses') }}</a-option>
                </a-select>
              </div>
            </div>
          </div>

          <!-- 该供应商的模型列表 -->
          <div class="settings-section">
            <div class="settings-editor-header">
              <div class="settings-editor-label">{{ t('models.models') }}</div>
              <a-button size="small" type="secondary" @click="onAddModel">
                <template #icon><icon-plus :size="12" /></template>
                {{ t('models.addModel') }}
              </a-button>
            </div>

            <div v-if="!activeProvider.models.length" class="model-empty">
              {{ t('models.empty') }}
            </div>

            <div v-else class="flex flex-col gap-2">
              <div
                v-for="(m, index) in activeProvider.models"
                :key="index"
                class="model-row"
                :class="[dragOverModel === index && 'item-drop-over-row', draggingModel === index && 'item-dragging']"
                draggable="true"
                @dragstart="onModelDragStart($event, index)"
                @dragover.prevent="onModelDragOver(index)"
                @drop.prevent="onModelDrop(index)"
                @dragend="clearModelDrag"
              >
                <span class="model-drag-handle" :title="t('models.sortHint')">
                  <icon-box :size="15" />
                </span>
                <a-input
                  v-model="m.name"
                  class="model-row-input"
                  size="small"
                  :placeholder="t('models.modelName')"
                />
                <span class="model-status-dot" />
                <a-button size="small" :loading="testingName === m.name" @click="onTest(m.name)">
                  <template #icon><icon-check-circle :size="13" /></template>
                  {{ t('models.test') }}
                </a-button>
                <a-popconfirm :content="t('models.deleteConfirm')" @ok="onRemoveModel(m.name)">
                  <a-button size="small">
                    <template #icon><icon-delete :size="13" /></template>
                  </a-button>
                </a-popconfirm>
              </div>
            </div>
          </div>
        </template>

        <!-- 未选中供应商 -->
        <div v-else class="model-detail-empty">
          <div class="text-xs text-[var(--text-tertiary)]">{{ t('models.detailEmpty') }}</div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>

/* 模型行:卡片式细边框,整行可拖拽 */
.model-row {
  display: flex;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  padding: 4px 10px;
  background: var(--bg-base);
  transition: border-color 0.15s ease;
}

/* 行内输入框:去掉自身边框底色,融入行容器(聚焦由行描边提示) */
.model-row :deep(.arco-input-wrapper) {
  flex: 1;
  background: transparent;
  border: none;
  padding: 0;
}

.model-row:focus-within {
  border-color: var(--accent-text);
}

/* 模型行拖拽手柄:纯视觉提示,拖动源是整行(见行级 draggable) */
.model-drag-handle {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 24px;
  flex-shrink: 0;
  color: var(--text-tertiary);
  border-radius: 4px;
}

.model-drag-handle:hover {
  color: var(--text-primary);
  background-color: var(--bg-hover);
}

/* 拖拽中:源项半透明,目标项描边提示 */
.item-dragging {
  opacity: 0.4;
}

.item-drop-over {
  background-color: var(--bg-hover);
}

.item-drop-over-row {
  border-radius: 8px;
  box-shadow: inset 0 0 0 1px var(--accent-text);
}

.model-empty {
  padding: 16px;
  border: 1px dashed var(--border-base);
  border-radius: var(--radius-md);
  text-align: center;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.model-detail-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 200px;
}
</style>
