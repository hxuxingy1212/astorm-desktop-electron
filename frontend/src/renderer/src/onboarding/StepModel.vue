<script setup lang="ts">
/**
 * 引导第三步（**仅本机分支**）：配置 LLM 供应商。
 *
 * 本机模式后端要有一个模型才能干活，而全新安装是空的——这一步就是让它「开箱可用」；
 * 云容器分支不出现这一步（容器的模型在容器自己的「设置 → 模型」里配，壳子不碰远端配置）。
 *
 * 与设置页同一套数据面：写进 `useModelsStore` 再 `persist()`（主进程 safeStorage 加密落盘 +
 * 热更本机后端）。探测走 `providers:test`（真实发一次最小请求），**它读的是已落盘的配置**，
 * 所以点「测试」/「保存并继续」都是先存后测。
 *
 * 保存语义：同名供应商就地替换（保留其它供应商与其顺序），否则追加——重复跑引导不会把
 * 用户已有的多个供应商冲掉。
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useModelsStore } from '@/stores/models'
import type { ApiFormat } from '@/services/providers'

defineProps<{ busy: boolean }>()
const emit = defineEmits<{ next: []; skip: []; back: [] }>()

const { t } = useI18n()
const models = useModelsStore()

/** 常见供应商预设：点一下填好地址/协议/默认模型（名称也一并给，用户仍可改） */
const PRESETS: Array<{
  id: string
  label: string
  name: string
  baseUrl: string
  apiFormat: ApiFormat
  model: string
}> = [
  {
    id: 'deepseek',
    label: 'DeepSeek',
    name: 'deepseek',
    baseUrl: 'https://api.deepseek.com/v1',
    apiFormat: 'chat_completions',
    model: 'deepseek-chat'
  },
  {
    id: 'openai',
    label: 'OpenAI',
    name: 'openai',
    baseUrl: 'https://api.openai.com/v1',
    apiFormat: 'chat_completions',
    model: 'gpt-4o-mini'
  },
  {
    id: 'zhipu',
    label: '智谱 GLM',
    name: 'zhipu',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    apiFormat: 'chat_completions',
    model: 'glm-4-flash'
  },
  {
    id: 'moonshot',
    label: 'Moonshot',
    name: 'moonshot',
    baseUrl: 'https://api.moonshot.cn/v1',
    apiFormat: 'chat_completions',
    model: 'moonshot-v1-8k'
  },
  {
    id: 'anthropic',
    label: 'Anthropic',
    name: 'anthropic',
    baseUrl: 'https://api.anthropic.com',
    apiFormat: 'anthropic_messages',
    model: 'claude-3-5-haiku-latest'
  }
]

const name = ref('')
const baseUrl = ref('')
const apiKey = ref('')
const modelName = ref('')
const apiFormat = ref<ApiFormat>('chat_completions')
const presetId = ref('')
const testing = ref(false)
const testResult = ref<{ ok: boolean; text: string } | null>(null)

const filled = computed(
  () => !!name.value.trim() && !!baseUrl.value.trim() && !!apiKey.value.trim() && !!modelName.value.trim()
)

/** 已有配置就带出来（重复进引导时不至于从空白开始）；否则默认选中 DeepSeek 预设 */
onMounted(() => {
  const existing = models.providers[0]
  if (existing) {
    name.value = existing.name
    baseUrl.value = existing.baseUrl
    apiKey.value = existing.apiKey
    apiFormat.value = existing.apiFormat ?? 'chat_completions'
    modelName.value = existing.models[0]?.name ?? ''
    presetId.value = PRESETS.find((p) => p.baseUrl === existing.baseUrl)?.id ?? ''
  } else {
    applyPreset(PRESETS[0])
  }
})

function applyPreset(p: (typeof PRESETS)[number] | null): void {
  if (!p) {
    // 自定义：只清空预设标记，已填内容留给用户
    presetId.value = ''
    return
  }
  presetId.value = p.id
  name.value = p.name
  baseUrl.value = p.baseUrl
  apiFormat.value = p.apiFormat
  modelName.value = p.model
  // 换预设时旧密钥/旧结果不再对应当前地址
  testResult.value = null
}

/** 探测失败/成功的可读文案（providers:test 回 message 码或 API 原文） */
function testText(r: { ok: boolean; message: string }): string {
  if (r.ok) return t('onboarding.modelTestOk')
  if (r.message === 'test_need_base_url') return t('onboarding.modelNeedBaseUrl')
  if (r.message === 'test_need_api_key') return t('onboarding.modelNeedApiKey')
  if (r.message === 'test_need_model') return t('onboarding.modelNeedModel')
  if (r.message === 'timeout') return t('onboarding.modelTestTimeout')
  if (r.message === 'vision_unsupported') return t('onboarding.modelVisionUnsupported')
  return r.message
}

/** 落盘（同名替换 / 否则追加），返回是否成功 */
async function save(): Promise<boolean> {
  const entry = {
    name: name.value.trim(),
    baseUrl: baseUrl.value.trim(),
    apiKey: apiKey.value.trim(),
    apiFormat: apiFormat.value,
    models: [{ name: modelName.value.trim(), order: 0 }],
    order: 0
  }
  const idx = models.providers.findIndex((p) => p.name.trim() === entry.name)
  if (idx >= 0) models.providers.splice(idx, 1, { ...models.providers[idx], ...entry })
  else models.providers.push(entry)
  try {
    await models.persist()
    return true
  } catch (e) {
    testResult.value = { ok: false, text: (e as Error).message }
    return false
  }
}

/** 能力探测：先落盘（providers:test 读的是盘上的配置）再真实发一次图文请求 */
async function test(): Promise<boolean> {
  if (!filled.value) {
    testResult.value = { ok: false, text: t('onboarding.modelFillRequired') }
    return false
  }
  testing.value = true
  testResult.value = null
  try {
    if (!(await save())) return false
    const r = await window.api.providers.test(name.value.trim(), modelName.value.trim())
    testResult.value = { ok: r.ok, text: testText(r) }
    return r.ok
  } catch (e) {
    testResult.value = { ok: false, text: (e as Error).message }
    return false
  } finally {
    testing.value = false
  }
}

/** 「保存并继续」：始终自动探测文本+图片能力；一个字没填 = 跳过这一步 */
async function submit(): Promise<void> {
  if (!name.value.trim() && !baseUrl.value.trim() && !apiKey.value.trim() && !modelName.value.trim()) {
    emit('next')
    return
  }
  if (await test()) emit('next')
}
</script>

<template>
  <div class="ob-model">
    <header class="ob-head">
      <h2 class="ob-title">{{ t('onboarding.modelTitle') }}</h2>
      <p class="ob-sub">{{ t('onboarding.modelSub') }}</p>
    </header>

    <!-- 预设：点一下填好地址/协议/默认模型 -->
    <div class="ob-presets" data-ob="presets">
      <button
        v-for="p in PRESETS"
        :key="p.id"
        type="button"
        class="ob-chip"
        :class="{ active: presetId === p.id }"
        :data-ob="`preset-${p.id}`"
        @click="applyPreset(p)"
      >
        {{ p.label }}
      </button>
      <button
        type="button"
        class="ob-chip"
        :class="{ active: presetId === '' }"
        data-ob="preset-custom"
        @click="applyPreset(null)"
      >
        {{ t('onboarding.modelPresetCustom') }}
      </button>
    </div>

    <div class="ob-fields">
      <div class="ob-field">
        <span class="ob-field-label">{{ t('onboarding.modelBaseUrl') }}</span>
        <a-input
          v-model="baseUrl"
          :placeholder="t('onboarding.modelBaseUrlHint')"
          data-ob="model-base-url"
        >
          <template #prefix><icon-link :size="14" /></template>
        </a-input>
      </div>
      <div class="ob-field">
        <span class="ob-field-label">{{ t('onboarding.modelApiKey') }}</span>
        <a-input-password
          v-model="apiKey"
          :placeholder="t('onboarding.modelApiKeyHint')"
          data-ob="model-api-key"
        >
          <template #prefix><icon-safe :size="14" /></template>
        </a-input-password>
      </div>
      <div class="ob-field">
        <span class="ob-field-label">{{ t('onboarding.modelName') }}</span>
        <a-input
          v-model="modelName"
          :placeholder="t('onboarding.modelNameHint')"
          data-ob="model-name"
        >
          <template #prefix><icon-robot :size="14" /></template>
        </a-input>
      </div>
      <!-- 名称 / 协议：预设已带好，一般不用动 -->
      <div class="ob-field-row">
        <div class="ob-field">
          <span class="ob-field-label">{{ t('onboarding.modelProviderName') }}</span>
          <a-input
            v-model="name"
            :placeholder="t('onboarding.modelProviderNameHint')"
            data-ob="model-provider-name"
          />
        </div>
        <div class="ob-field">
          <span class="ob-field-label">{{ t('onboarding.modelApiFormat') }}</span>
          <a-select v-model="apiFormat" data-ob="model-api-format">
            <a-option value="chat_completions">{{ t('onboarding.modelFormatChat') }}</a-option>
            <a-option value="responses">{{ t('onboarding.modelFormatResponses') }}</a-option>
            <a-option value="anthropic_messages">{{ t('onboarding.modelFormatAnthropic') }}</a-option>
          </a-select>
        </div>
      </div>
    </div>

    <Transition name="ob-reveal">
      <span v-if="testResult" class="ob-test-line" :class="{ ok: testResult.ok }">
        <icon-check-circle v-if="testResult.ok" :size="14" />
        <icon-exclamation-circle v-else :size="14" />
        <span class="ob-test-text">{{ testResult.text }}</span>
      </span>
    </Transition>

    <footer class="ob-foot">
      <a-button
        class="ob-ghost"
        type="text"
        :disabled="busy"
        data-ob="back"
        @click="emit('back')"
      >
        {{ t('onboarding.back') }}
      </a-button>
      <span class="ob-foot-spacer" />
      <a-button class="ob-ghost" type="text" :disabled="busy" data-ob="skip" @click="emit('skip')">
        {{ t('onboarding.skip') }}
      </a-button>
      <a-button
        class="ob-cta"
        type="primary"
        :loading="busy || testing"
        data-ob="model-next"
        @click="submit"
      >
        {{ t('onboarding.modelSaveNext') }}
      </a-button>
    </footer>
  </div>
</template>

<style scoped>
.ob-model {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: var(--ob-step-pad, 32px 36px 28px);
  flex: 1;
}
.ob-head {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.ob-title {
  margin: 0;
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
}
.ob-sub {
  margin: 0;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

/* 预设：与 §3.4 分段控件同一形态（不套槽、选中 --fill-3、无描边） */
.ob-presets {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.ob-chip {
  height: 34px;
  padding: 0 12px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  font-size: var(--font-size-md);
  font-weight: 500;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color 140ms ease,
    color 140ms ease;
}
.ob-chip:hover {
  background: var(--fill-1);
  color: var(--text-primary);
}
.ob-chip.active {
  background: var(--fill-3);
  color: var(--text-primary);
}

.ob-fields {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.ob-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
  flex: 1;
}
.ob-field-row {
  display: flex;
  gap: 12px;
}
.ob-field-label {
  font-size: var(--font-size-code);
  color: var(--text-secondary);
}

.ob-test-line {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--danger, #f53f3f);
}
.ob-test-line.ok {
  color: var(--success, #00b42a);
}
.ob-test-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ob-reveal-enter-active,
.ob-reveal-leave-active {
  transition:
    opacity 200ms ease,
    transform 220ms cubic-bezier(0.32, 0.72, 0, 1);
}
.ob-reveal-enter-from,
.ob-reveal-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}
@media (prefers-reduced-motion: reduce) {
  .ob-reveal-enter-active,
  .ob-reveal-leave-active {
    transition: none;
  }
}

.ob-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 4px;
}
.ob-ghost {
  border-color: transparent !important;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}
.ob-ghost:hover:not(:disabled) {
  border-color: transparent !important;
  background: var(--fill-1) !important;
  color: var(--text-primary) !important;
}
.ob-foot-spacer {
  margin-left: auto;
}
.ob-cta {
  min-width: 112px;
  height: 32px;
  border-radius: 8px;
}
</style>
