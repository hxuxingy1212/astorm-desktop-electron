<script setup lang="ts">
/**
 * 设置页「连接方式」（仅 Electron 壳子可见，浏览器部署隐藏 —— 见 registry 的 webHidden）。
 *
 * 壳子启动时的「去哪找后端」：本机（进程内 Rust 后端，数据在本地）或云端（连自己的
 * 单用户云容器，窗口直接加载服务端托管的 Web UI）。切换由主进程换壳导航完成，
 * 这里只展示当前状态并把人送到欢迎页。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

const info = window.api?.shellConnection?.info ?? {
  mode: 'unset' as const,
  endpoint: '',
  hasToken: false,
  welcome: false,
  onboarded: false
}

const modeText = computed(() =>
  info.mode === 'cloud'
    ? t('settings.connection.modeCloud')
    : info.mode === 'local'
      ? t('settings.connection.modeLocal')
      : t('settings.connection.modeUnset')
)

function openWelcome(): void {
  void window.api.shellConnection.openWelcome()
}
</script>

<template>
  <div class="page">
    <div class="config-section conn-card">
      <div class="conn-row">
        <span class="conn-label">{{ t('settings.connection.current') }}</span>
        <span class="conn-value">{{ modeText }}</span>
      </div>
      <div v-if="info.mode === 'cloud'" class="conn-row">
        <span class="conn-label">{{ t('settings.connection.endpoint') }}</span>
        <span class="conn-value mono">{{ info.endpoint }}</span>
      </div>
      <p class="conn-desc">{{ t('settings.connection.desc') }}</p>
      <a-button type="primary" @click="openWelcome">
        {{ t('settings.connection.switch') }}
      </a-button>
    </div>
  </div>
</template>

<style scoped>
.page {
  padding: 20px 24px;
}
.conn-card {
  max-width: 560px;
  padding: 16px 18px;
}
.conn-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 8px;
  font-size: var(--font-size-sm);
}
.conn-label {
  flex: none;
  width: 84px;
  color: var(--text-tertiary);
}
.conn-value {
  color: var(--text-primary);
}
.mono {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  word-break: break-all;
}
.conn-desc {
  margin: 10px 0 14px;
  font-size: var(--font-size-code);
  line-height: 1.6;
  color: var(--text-tertiary);
}
</style>
