<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { isWebShell } from '@/utils/platform'
defineProps<{ path: string; loading?: boolean }>()
defineEmits<{ open: []; edit: [] }>()
const { t } = useI18n()
</script>

<template>
  <!-- 浏览器部署（云端 Web UI）打不开服务端容器里的文件，整行隐藏 -->
  <div v-if="!isWebShell" class="settings-row settings-config-file">
    <div class="row-text">
      <code class="settings-config-path">{{ path || t('common.loading') }}</code>
    </div>
    <div class="config-actions">
      <a-button size="small" @click="$emit('edit')">
        {{ t('settings.security.editConfig') }}
      </a-button>
      <a-button size="small" :disabled="!path" :loading="loading" @click="$emit('open')">
        {{ t('settings.security.openConfig') }}
      </a-button>
    </div>
  </div>
</template>
