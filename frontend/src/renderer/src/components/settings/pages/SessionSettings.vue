<script setup lang="ts">
import SettingsSection from '../SettingsSection.vue'
import { ref, reactive, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@/services/backend'
import { Message } from '@arco-design/web-vue'
import type { AppSettings, CompactSettings, GoalSettings, SummarySettings } from '@/services/dto'

const { t } = useI18n()

const saving = ref(false)

/** 后端真实支持的会话摘要配置(与 Rust AppConfig.summary 字段对齐) */
const DEFAULTS: SummarySettings = {
  token_threshold: 200000,
  min_messages: 8,
  keep_token_threshold: 20000,
  keep_tail_rounds: 5
}

const summary = reactive<SummarySettings>({ ...DEFAULTS })

/** 后端真实支持的会话消息剪支配置(与 Rust AppConfig.compact 字段对齐) */
const COMPACT_DEFAULTS: CompactSettings = {
  compact_threshold: 32768,
  compact_recent_rounds: 5,
  compact_byte_count: 10240,
  compact_head_chars: 200,
  compact_tail_chars: 400
}

const compact = reactive<CompactSettings>({ ...COMPACT_DEFAULTS })

/** 后端真实支持的目标循环配置(与 Rust AppConfig.goal 字段对齐) */
const GOAL_DEFAULTS: GoalSettings = {
  max_tokens: 10000000,
  max_iterations: 5
}

const goal = reactive<GoalSettings>({ ...GOAL_DEFAULTS })

const summaryFields: { key: keyof SummarySettings; label: string; desc: string; min: number; max: number; step?: number }[] = [
  { key: 'token_threshold', label: t('settings.session.summaryTokenThreshold'), desc: t('settings.session.summaryTokenThresholdDesc'), min: 4096, max: 1048576, step: 4096 },
  { key: 'min_messages', label: t('settings.session.summaryMinMessages'), desc: t('settings.session.summaryMinMessagesDesc'), min: 1, max: 200 },
  { key: 'keep_token_threshold', label: t('settings.session.summaryKeepTokenThreshold'), desc: t('settings.session.summaryKeepTokenThresholdDesc'), min: 1024, max: 262144, step: 1024 },
  { key: 'keep_tail_rounds', label: t('settings.session.summaryKeepTailRounds'), desc: t('settings.session.summaryKeepTailRoundsDesc'), min: 1, max: 100 }
]

const compactFields: { key: keyof CompactSettings; label: string; desc: string; min: number; max: number; step?: number }[] = [
  { key: 'compact_threshold', label: t('settings.session.compactThreshold'), desc: t('settings.session.compactThresholdDesc'), min: 1024, max: 1048576, step: 1024 },
  { key: 'compact_recent_rounds', label: t('settings.session.compactRecentRounds'), desc: t('settings.session.compactRecentRoundsDesc'), min: 1, max: 200 },
  { key: 'compact_byte_count', label: t('settings.session.compactByteCount'), desc: t('settings.session.compactByteCountDesc'), min: 1024, max: 262144, step: 1024 },
  { key: 'compact_head_chars', label: t('settings.session.compactHeadChars'), desc: t('settings.session.compactHeadCharsDesc'), min: 16, max: 8192 },
  { key: 'compact_tail_chars', label: t('settings.session.compactTailChars'), desc: t('settings.session.compactTailCharsDesc'), min: 16, max: 8192 }
]

const goalFields: { key: keyof GoalSettings; label: string; desc: string; min: number; max: number; step?: number }[] = [
  { key: 'max_tokens', label: t('settings.session.goalMaxTokens'), desc: t('settings.session.goalMaxTokensDesc'), min: 10000, max: 100000000, step: 10000 },
  { key: 'max_iterations', label: t('settings.session.goalMaxIterations'), desc: t('settings.session.goalMaxIterationsDesc'), min: 1, max: 100 }
]

async function loadAll(): Promise<void> {
  try {
    const s = (await invoke('get_settings')) as AppSettings
    Object.assign(summary, s.summary)
    Object.assign(compact, s.compact)
    if (s.goal) Object.assign(goal, s.goal)
  } catch {
    /* 保持默认值 */
  }
}

function resetAll(): void {
  Object.assign(summary, DEFAULTS)
  Object.assign(compact, COMPACT_DEFAULTS)
  Object.assign(goal, GOAL_DEFAULTS)
}

async function saveAll(): Promise<void> {
  saving.value = true
  try {
    const s = (await invoke('get_settings')) as AppSettings
    s.summary = { ...summary }
    s.compact = { ...compact }
    s.goal = { ...goal }
    await invoke('save_settings', { settings: s })
    Message.success(t('common.saveSuccess'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    saving.value = false
  }
}

onMounted(() => {
  loadAll()
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.session.title') }}</h2>
    </div>
    <div class="page-body">
      <!-- 会话摘要配置(后端 AppConfig.summary,保存即热更后端) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.session.summaryConfig') }}
        </template>

          <div class="settings-row" v-for="field in summaryFields" :key="field.key">
            <div class="row-text">
              <span class="row-title">{{ field.label }}</span>
              <span class="row-desc">{{ field.desc }}</span>
            </div>
            <a-input-number
              v-model="summary[field.key]"
              :min="field.min"
              :max="field.max"
              :step="field.step || 1"
              size="small"
              style="width: 130px"
            />
          </div>

      </SettingsSection>

      <!-- 会话消息剪支配置(后端 AppConfig.compact,保存即热更后端) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.session.compactConfig') }}
        </template>

          <div class="settings-row" v-for="field in compactFields" :key="field.key">
            <div class="row-text">
              <span class="row-title">{{ field.label }}</span>
              <span class="row-desc">{{ field.desc }}</span>
            </div>
            <a-input-number
              v-model="compact[field.key]"
              :min="field.min"
              :max="field.max"
              :step="field.step || 1"
              size="small"
              style="width: 130px"
            />
          </div>

      </SettingsSection>

      <!-- 目标循环配置(后端 AppConfig.goal,/goal 外循环预算;保存即热更后端) -->
      <SettingsSection>
        <template #title>
          {{ t('settings.session.goalConfig') }}
        </template>

          <div class="settings-row" v-for="field in goalFields" :key="field.key">
            <div class="row-text">
              <span class="row-title">{{ field.label }}</span>
              <span class="row-desc">{{ field.desc }}</span>
            </div>
            <a-input-number
              v-model="goal[field.key]"
              :min="field.min"
              :max="field.max"
              :step="field.step || 1"
              size="small"
              style="width: 130px"
            />
          </div>

      </SettingsSection>
    </div>

    <div class="page-footer">
      <div class="footer-actions">
        <a-button @click="resetAll">{{ t('common.reset') }}</a-button>
        <a-button type="primary" :loading="saving" @click="saveAll">
          {{ t('common.save') }}
        </a-button>
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
  /* 预占滚动条位置,内容增删时视图不晃动 */
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

.page-footer {
  flex-shrink: 0;
  padding: 8px 0 0;
  margin-top: 0;
}

.footer-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

/* 数字输入框 / 下拉:更柔和的 macOS 圆角与浅边框 */
.settings-row :deep(.arco-input-number),
.settings-row :deep(.arco-select) {
  border-radius: 6px;
}

/* 底部主按钮(保存):macOS 系统蓝填充 + 白字 */
.footer-actions :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* 底部操作按钮:macOS 圆角 */
.footer-actions :deep(.arco-btn) {
  border-radius: 6px;
}
</style>
