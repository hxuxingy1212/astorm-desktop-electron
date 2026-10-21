<script setup lang="ts">
/**
 * 引导第三步：偏好（**可跳过**）——明暗、语言、电源（保持电脑运行）。
 *
 * 与设置页同一套控件与排布语言（行内左文右控），改动**即时生效**（OnboardingApp 的
 * watcher 负责 applyTheme / i18n / powerSaveBlocker 并落盘）——所见即所得，跳不跳都不亏。
 */
import { useI18n } from 'vue-i18n'
import { useSettingsStore } from '@/stores/settings'
import type { LocaleKey } from '@/i18n'

defineProps<{ busy: boolean }>()
const emit = defineEmits<{ finish: []; skip: []; back: [] }>()

const { t } = useI18n()
const settings = useSettingsStore()
</script>

<template>
  <div class="ob-prefs">
    <header class="ob-head">
      <h2 class="ob-title">{{ t('onboarding.prefsTitle') }}</h2>
      <p class="ob-sub">{{ t('onboarding.prefsSub') }}</p>
    </header>

    <div class="ob-rows">
      <!-- 外观：明暗 -->
      <div class="ob-row">
        <span class="ob-row-text">
          <span class="ob-row-title">{{ t('settings.system.themeMode') }}</span>
          <span class="ob-row-desc">{{ t('settings.system.themeModeDesc') }}</span>
        </span>
        <a-radio-group
          :model-value="settings.themeMode"
          class="ob-seg"
          type="button"
          data-ob="theme-mode"
          @change="settings.setThemeMode($event as 'light' | 'dark' | 'system')"
        >
          <a-radio value="light">{{ t('settings.system.themeLight') }}</a-radio>
          <a-radio value="dark">{{ t('settings.system.themeDark') }}</a-radio>
          <a-radio value="system">{{ t('settings.system.themeAuto') }}</a-radio>
        </a-radio-group>
      </div>

      <!-- 语言 -->
      <div class="ob-row">
        <span class="ob-row-text">
          <span class="ob-row-title">{{ t('settings.system.language') }}</span>
          <span class="ob-row-desc">{{ t('settings.system.languageDesc') }}</span>
        </span>
        <a-radio-group
          :model-value="settings.locale"
          class="ob-seg"
          type="button"
          data-ob="locale"
          @change="settings.setLocale($event as LocaleKey)"
        >
          <a-radio value="zh-CN">{{ t('settings.system.localeZh') }}</a-radio>
          <a-radio value="en-US">{{ t('settings.system.localeEn') }}</a-radio>
        </a-radio-group>
      </div>

      <!-- 电源 -->
      <div class="ob-row">
        <span class="ob-row-text">
          <span class="ob-row-title">{{ t('settings.system.keepAwake') }}</span>
          <span class="ob-row-desc">{{ t('settings.system.keepAwakeDesc') }}</span>
        </span>
        <a-switch v-model="settings.keepAwake" data-ob="keep-awake" />
      </div>
    </div>

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
        :loading="busy"
        data-ob="finish"
        @click="emit('finish')"
      >
        {{ t('onboarding.finish') }}
      </a-button>
    </footer>
  </div>
</template>

<style scoped>
.ob-prefs {
  display: flex;
  flex-direction: column;
  gap: 18px;
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

/* 设置行：无分组底板（引导页整体无面板），靠发丝线分隔 */
.ob-rows {
  display: flex;
  flex-direction: column;
}
/* 行：左文右控 + 发丝线分隔（与设置页同款排布） */
.ob-row {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 11px 2px;
}
.ob-row + .ob-row {
  border-top: 0.5px solid var(--border-base);
}
.ob-row-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.ob-row-title {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}
.ob-row-desc {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.ob-row > :last-child {
  margin-left: auto;
  flex: none;
}

/* 分段控件：不套槽，未选透明，选中 --fill-3（§3.4） */
.ob-seg.arco-radio-group-button {
  display: flex;
  gap: 8px;
  padding: 0;
  border-radius: 0;
  background: transparent !important;
}
.ob-seg :deep(.arco-radio-button) {
  height: 34px;
  margin: 0;
  padding: 0 14px;
  border: none !important;
  border-radius: var(--radius-md);
  background: transparent !important;
  color: var(--text-secondary) !important;
  font-size: var(--font-size-title);
  font-weight: 500;
  line-height: 34px;
}
.ob-seg :deep(.arco-radio-button)::before,
.ob-seg :deep(.arco-radio-button)::after {
  display: none !important;
}
.ob-seg :deep(.arco-radio-button:hover) {
  background: var(--fill-1) !important;
  color: var(--text-primary) !important;
}
.ob-seg :deep(.arco-radio-button.arco-radio-checked) {
  background: var(--fill-3) !important;
  color: var(--text-primary) !important;
  box-shadow: none !important;
}

/* 开关：关 --fill-3 / 开 accent（§3.4） */
.ob-row :deep(.arco-switch) {
  background-color: var(--fill-3) !important;
}
.ob-row :deep(.arco-switch-checked) {
  background-color: var(--accent-text) !important;
}
.ob-row :deep(.arco-switch-handle) {
  background-color: #fff !important;
}

.ob-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: auto;
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
  min-width: 96px;
  height: 32px;
  border-radius: 8px;
}
</style>
