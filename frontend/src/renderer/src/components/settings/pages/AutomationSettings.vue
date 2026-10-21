<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { useSettingsStore } from '@/stores/settings'
import { isWebShell } from '@/utils/platform'
import { clearBrowserCache, clearBrowserData, importChromeCookies } from '@/services/browserData'

const { t } = useI18n()
const settings = useSettingsStore()

const importing = ref(false)
const clearingCache = ref(false)
const clearingAll = ref(false)

/** 把一次导入的统计拼成可读提示（失败项单独追加，不掩盖） */
function importSummary(r: Awaited<ReturnType<typeof importChromeCookies>>): string {
  const parts = [t('settings.automation.importDone', { imported: r.imported, total: r.total, profile: r.profile })]
  if (r.skipped > 0) parts.push(t('settings.automation.importSkipped', { n: r.skipped }))
  if (r.decryptFailed > 0) parts.push(t('settings.automation.importFailed', { n: r.decryptFailed }))
  if (r.writeFailed > 0) parts.push(t('settings.automation.importWriteFailed', { n: r.writeFailed }))
  return parts.join('；')
}

/** 导入本机 Chrome Cookie（手动按钮，不受自动导入的只跑一次守卫限制） */
async function onImport(): Promise<void> {
  importing.value = true
  try {
    const result = await importChromeCookies()
    Message.success(importSummary(result))
  } catch (e) {
    // 常见失败：Chrome 未安装 / macOS 钥匙串授权被拒 / 后端 dll 未同步
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    importing.value = false
  }
}

async function onClearCache(): Promise<void> {
  clearingCache.value = true
  try {
    await clearBrowserCache()
    Message.success(t('settings.automation.cacheCleared'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    clearingCache.value = false
  }
}

function onClearAll(): void {
  void macConfirm({
    title: t('settings.automation.clearAll'),
    content: t('settings.automation.clearAllConfirm'),
    tone: 'danger',
    confirmText: t('settings.automation.clearAll'),
    onConfirm: async () => {
      clearingAll.value = true
      try {
        const result = await clearBrowserData()
        Message.success(
          t('settings.automation.clearAllDone', { sites: result.sites, cookies: result.cookies })
        )
      } catch (e) {
        Message.error(e instanceof Error ? e.message : String(e))
      } finally {
        clearingAll.value = false
      }
    }
  })
}
</script>

<template>
  <div class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.automation.title') }}</h2>
    </div>
    <div class="page-body">
      <p class="page-desc">{{ t('settings.automation.desc') }}</p>

      <div class="section-title">{{ t('settings.automation.browserData') }}</div>
      <div class="data-card">
        <!-- 导入本机 Chrome Cookie：内置浏览器是跨域 iframe，站点登录态带不进来，靠导入补齐 -->
        <!-- 浏览器部署没有「本机 Chrome」且服务端未实现该命令，入口直接隐藏 -->
        <div v-if="!isWebShell" class="data-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.automation.importTitle') }}</span>
            <span class="row-desc">{{ t('settings.automation.importDesc') }}</span>
          </div>
          <a-button :loading="importing" @click="onImport">
            {{ t('settings.automation.importAction') }}
          </a-button>
        </div>

        <!-- 自动导入：第一次打开网页页签时跑一次（本进程内只跑一次） -->
        <div v-if="!isWebShell" class="data-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.automation.autoImportTitle') }}</span>
            <span class="row-desc">{{ t('settings.automation.autoImportDesc') }}</span>
          </div>
          <a-switch v-model="settings.autoImportChromeCookies" />
        </div>

        <div class="data-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.automation.clearCacheTitle') }}</span>
            <span class="row-desc">{{ t('settings.automation.clearCacheDesc') }}</span>
          </div>
          <a-button :loading="clearingCache" @click="onClearCache">
            {{ t('settings.automation.clearCacheAction') }}
          </a-button>
        </div>

        <div class="data-row">
          <div class="row-text">
            <span class="row-title">{{ t('settings.automation.clearAllTitle') }}</span>
            <span class="row-desc">{{ t('settings.automation.clearAllDesc') }}</span>
          </div>
          <a-button status="danger" :loading="clearingAll" @click="onClearAll">
            {{ t('settings.automation.clearAllAction') }}
          </a-button>
        </div>
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

.page-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  margin-bottom: 24px;
  line-height: 1.6;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  padding: 4px 0;
  margin-bottom: 12px;
}

/* 卡片:一行一个动作,行间以分割线区隔(参考「浏览器数据」分组样式) */
.data-card {
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-lg);
  overflow: hidden;
}

.data-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 16px 20px;
}

.data-row + .data-row {
  border-top: 1px solid var(--border-base);
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
  line-height: 1.5;
}

/* 动作按钮不参与压缩,文字行优先换行 */
.data-row > :deep(.arco-btn) {
  flex-shrink: 0;
}

/* 开关(macOS 绿色滑块):与系统设置页一致 */
.data-row :deep(.arco-switch) {
  border-radius: 999px;
}

.data-row :deep(.arco-switch-checked) {
  background-color: #34c759 !important;
}

.data-row :deep(.arco-switch-handle) {
  background-color: #fff !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}

.data-row :deep(.arco-btn) {
  border-radius: 6px;
}
</style>
