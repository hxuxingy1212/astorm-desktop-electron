<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useLocalEnvStore, type LocalEnvRow } from '@/stores/localEnv'

const { t } = useI18n()
const store = useLocalEnvStore()

/** 正在保存注入开关的工具 bin（用于局部 loading/锁定） */
const busyBin = ref('')


function injectLabel(item: LocalEnvRow): string {
  if (!item.available) return ''
  return item.disabled ? t('settings.localEnv.injectOff') : t('settings.localEnv.injectOn')
}

function injectClass(item: LocalEnvRow): string {
  if (!item.available) return 'badge-off'
  // 注入开关用强调色徽章，与「已安装」的绿色状态徽章区分：一个是状态，一个是开关
  return item.disabled ? 'badge-off' : 'badge-inject'
}

async function onToggle(item: LocalEnvRow, inject: boolean): Promise<void> {
  busyBin.value = item.bin
  try {
    await store.toggleInjection(item.bin, inject)
    Message.success(inject ? t('settings.localEnv.injectOn') : t('settings.localEnv.injectOff'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    busyBin.value = ''
  }
}

onMounted(() => {
  // 进入页面：复用已缓存检测结果，仅拉最新注入状态（不重跑后端检测）
  store.bootstrap()
})
</script>

<template>
  <div class="local-env">
    <div class="toolbar">
        <span class="toolbar-count">{{ t('settings.localEnv.listCount', { n: store.items.length }) }}</span>
        <a-button size="small" :loading="store.loading" @click="store.refresh">
          <template #icon><icon-refresh :size="13" /></template>
          {{ t('common.refresh') }}
        </a-button>
      </div>

      <div class="state-wrap">
        <div v-if="store.loading" class="empty-tip">
          <icon-loading :size="26" class="empty-icon spin" />
          <div>{{ t('common.loading') }}</div>
        </div>
        <div v-else-if="store.error" class="empty-tip err">{{ store.error }}</div>
        <div v-else-if="store.items.length === 0" class="empty-tip">
          <icon-code :size="26" class="empty-icon" />
          <div>{{ t('settings.localEnv.empty') }}</div>
        </div>

        <template v-else>
          <div class="env-list">
            <div v-for="item in store.rows" :key="item.name" class="env-card">
              <!-- 工具信息与配置始终展开 -->
              <div
                class="env-card-head"
              >
                <div class="env-name">
                  <span class="env-dot" :class="item.available ? 'dot-on' : 'dot-off'" />
                  <span class="env-name-text">{{ item.name }}</span>
                </div>
                <div class="env-meta">
                  <span v-if="item.available" class="badge badge-ok">{{ t('settings.localEnv.installed') }}</span>
                  <span v-else class="badge badge-off">{{ t('settings.localEnv.missing') }}</span>
                  <span v-if="item.version" class="env-version">{{ item.version }}</span>
                  <span v-if="injectLabel(item)" class="badge" :class="injectClass(item)">
                    {{ injectLabel(item) }}
                  </span>
                </div>
              </div>

              <div class="env-card-body">
                <!-- 注入开关 -->
                <div class="inject-row">
                  <div class="inject-text">
                    <div class="row-title">{{ t('settings.localEnv.injectSwitch') }}</div>
                    <div class="row-desc">
                      <template v-if="!item.available">{{ t('settings.localEnv.unavailable') }}</template>
                      <template v-else-if="item.disabled">{{ t('settings.localEnv.injectOffDesc') }}</template>
                      <template v-else>{{ t('settings.localEnv.injectOnDesc') }}</template>
                    </div>
                  </div>
                  <a-switch
                    :model-value="!item.disabled"
                    :disabled="!item.available || busyBin !== ''"
                    :loading="busyBin === item.bin"
                    @change="(v: string | number | boolean) => onToggle(item, Boolean(v))"
                  />
                </div>

                <!-- 解析目录 / 附加目录 -->
                <div v-if="item.resolvedDir" class="dir-block">
                  <div class="dir-label">{{ t('settings.localEnv.resolvedDir') }}</div>
                  <div class="dir-value mono">{{ item.resolvedDir }}</div>
                </div>
                <div v-if="item.extraDirs.length" class="dir-block">
                  <div class="dir-label">{{ t('settings.localEnv.extraDirs') }}</div>
                  <div class="dir-chips">
                    <span v-for="d in item.extraDirs" :key="d" class="dir-chip mono">{{ d }}</span>
                  </div>
                </div>
                <div v-if="item.available && item.sharedDir" class="share-hint">
                  <icon-exclamation-circle :size="13" />
                  <span>{{ t('settings.localEnv.sharedHint') }}</span>
                </div>
              </div>
            </div>
          </div>
        </template>
      </div>
  </div>
</template>

<style scoped>
.local-env {
  display: flex;
  flex-direction: column;
  font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'SF Pro Display',
    'PingFang SC', 'Helvetica Neue', 'Segoe UI', Arial, sans-serif;
}

.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding-bottom: 10px;
  margin-bottom: 12px;
  border-bottom: 1px solid var(--border-light);
  flex-shrink: 0;
}

.toolbar :deep(.arco-btn) {
  border-radius: 6px;
}

.toolbar :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}

.toolbar-count {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-tertiary);
}

.state-wrap {
  flex: 1 0 auto;
  min-height: 320px;
  display: flex;
  flex-direction: column;
}

/* ---------- macOS 分组列表 ----------
   一个圆角容器装整组环境项:行与行之间只用发丝线分隔(不再每行一个方框),
   行悬浮/展开用中性悬浮底,首末行的底色被容器圆角裁切 */
.env-list {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  min-width: 0;
  max-width: 100%;
  background: var(--bg-1);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-lg);
  overflow: hidden;
}

.env-card {
  background: transparent;
  border-top: 1px solid var(--border-base);
  transition: background-color var(--transition-fast);
}

.env-card:first-child {
  border-top: none;
}

.env-card.expanded {
  background: var(--bg-hover);
}

.env-card-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 11px 16px;
  cursor: pointer;
  user-select: none;
  transition: background-color var(--transition-fast);
}

.env-card-head:hover {
  background: var(--bg-hover);
}

.env-name {
  display: flex;
  align-items: center;
  gap: 9px;
  min-width: 0;
  flex: 1;
}

.env-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}

.dot-on {
  background: var(--success);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--success) 15%, transparent);
}

.dot-off {
  background: var(--bg-4);
  box-shadow: none;
}

.env-name-text {
  font-size: var(--font-size-md);
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.env-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

/* 徽章:macOS 小圆角标签(不是长胶囊),浅色底 + 同色描边 */
.badge {
  display: inline-flex;
  align-items: center;
  height: 20px;
  padding: 0 8px;
  border: 1px solid transparent;
  border-radius: 6px;
  font-size: var(--font-size-2xs);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
}

/* 状态:已安装 */
.badge-ok {
  background: color-mix(in srgb, var(--success) 14%, transparent);
  border-color: color-mix(in srgb, var(--success) 24%, transparent);
  color: var(--success);
}

/* 开关:注入中(强调色,与绿色状态徽章区分开) */
.badge-inject {
  background: color-mix(in srgb, var(--accent-text) 14%, transparent);
  border-color: color-mix(in srgb, var(--accent-text) 24%, transparent);
  color: var(--accent-text);
}

/* 未安装 / 注入已关闭 */
.badge-off {
  background: var(--bg-3);
  border-color: var(--border-base);
  color: var(--text-secondary);
}

.env-version {
  font-size: var(--font-size-sm);
  font-weight: 500;
  font-variant-numeric: tabular-nums;
  color: var(--text-secondary);
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.chevron {
  transition: transform 0.2s;
  color: var(--text-tertiary);
}

.chevron.collapsed {
  transform: rotate(-90deg);
}

/* ---------- 展开区 ---------- */
.env-card-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px 16px 14px 33px;
  border-top: 1px solid var(--border-base);
}

.inject-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 2px 0;
}

/* 开关:选中态跟随应用强调色(Arco 默认蓝与主题色预设不一致) */
.inject-row :deep(.arco-switch-checked) {
  background-color: var(--accent-text);
}

.inject-text {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.row-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.row-desc {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  line-height: 1.5;
}

.dir-block {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}

/* 小标题:macOS 侧栏式灰标签(中文不做大写/字距,避免排版发散) */
.dir-label {
  font-size: var(--font-size-2xs);
  font-weight: 500;
  color: var(--text-tertiary);
}

.dir-value {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  word-break: break-all;
}

.mono {
  font-family: 'SF Mono', Monaco, 'Cascadia Code', monospace;
}

.dir-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.dir-chip {
  font-size: var(--font-size-2xs);
  color: var(--text-secondary);
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  padding: 2px 8px;
  border-radius: 6px;
  word-break: break-all;
}

.share-hint {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  font-size: var(--font-size-xs);
  color: var(--warning);
  line-height: 1.5;
}

/* ---------- 空态 ---------- */
.empty-tip {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px 0;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  text-align: center;
}

.empty-icon {
  opacity: 0.5;
}

.spin {
  animation: env-spin 1s linear infinite;
}

@keyframes env-spin {
  to { transform: rotate(360deg); }
}

.empty-tip.err {
  color: var(--danger);
}
</style>
