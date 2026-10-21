<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

/** 与 package.json 的 version 保持一致 */
const APP_VERSION = '0.1.0'
</script>

<template>
  <div class="page">
    <div class="about-card">
      <div class="about-icon">
        <img src="@/assets/logo.png" alt="Astorm" class="logo" />
      </div>
      <h1 class="about-name">{{ t('common.appName') }}</h1>
      <p class="about-version">v{{ APP_VERSION }}</p>
      <p class="about-desc">{{ t('settings.about.desc') }}</p>
      <div class="about-meta">
        <div class="meta-item">
          <span class="meta-label">{{ t('settings.about.techStack') }}</span>
          <span class="meta-value">Electron / Vue 3 / Arco Design / UnoCSS</span>
        </div>
        <div class="meta-item">
          <span class="meta-label">{{ t('settings.about.platform') }}</span>
          <span class="meta-value">macOS / Windows / Linux</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page {
  min-height: calc(100vh - 96px);
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.about-card {
  text-align: center;
  max-width: 360px;
}

.about-icon {
  margin-bottom: 16px;
}

.logo {
  width: 64px;
  height: 64px;
  border-radius: var(--radius-lg);
  object-fit: cover;
}

.about-name {
  font-size: var(--font-size-2xl);
  font-weight: 700;
  color: var(--text-primary);
  margin: 0;
}

.about-version {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  margin-top: 4px;
}

.about-desc {
  font-size: var(--font-size-md);
  color: var(--text-secondary);
  margin-top: 12px;
  line-height: 1.6;
}

.about-meta {
  margin-top: 24px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  text-align: left;
}

.meta-item {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 8px 0;
  border-bottom: 1px solid var(--border-base);
}

.meta-label {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.meta-value {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  text-align: right;
}
</style>