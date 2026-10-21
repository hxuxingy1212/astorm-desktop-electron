<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { invoke } from '@/services/backend'
import type { AuditLogEntry } from '@/services/dto'
import DenialBadge from '@/components/chat/DenialBadge.vue'
import { isWebShell } from '@/utils/platform'
import { downloadText } from '@/utils/webDownload'
import { localizedSessionTitle } from '@/utils/sessionTitle'

const { t } = useI18n()

interface AuditSession {
  id: string
  title?: string
  titleZh?: string
  titleEn?: string
  session_mode?: string
  created_at?: number
  updated_at?: number
}

/** 全部会话（供下拉筛选；来自后端 listSessions） */
const allSessions = ref<AuditSession[]>([])
const loadingSessions = ref(false)
/** 会话关键词搜索框 */
const keyword = ref('')
/** 当前选中的会话 id */
const selectedId = ref('')
/** 审计日志加载 */
const entries = ref<AuditLogEntry[]>([])
const loading = ref(false)
const loadedError = ref('')

/** 时间范围筛选 */
type RangeKey = 'all' | '1h' | 'today' | '7d' | '30d'
const range = ref<RangeKey>('all')
const RANGE_OPTIONS: { value: RangeKey; label: string }[] = [
  { value: '1h', label: t('settings.audit.range1h') },
  { value: 'today', label: t('settings.audit.rangeToday') },
  { value: '7d', label: t('settings.audit.range7d') },
  { value: '30d', label: t('settings.audit.range30d') },
  { value: 'all', label: t('settings.audit.rangeAll') }
]

/** 事件类型筛选（多选；空 = 全部）——操作种类（文件读/写、命令、命令执行、网络、资源、浏览器）。
 *  注：原先还有一个「拒绝原因」筛选（denial 分类），其取值与这里的操作种类
 *  一一对应（仅命令↔危险命令不同），界面上读起来是同一件事，故只保留本项；
 *  条目卡片上的拦截分类徽标（denial-badge）不受影响，仍按 category 展示。 */
const typeFilter = ref<string[]>([])
const TYPE_FILTERS: { value: string; label: string }[] = [
  { value: 'fileread', label: t('settings.audit.typeFileRead') },
  { value: 'filewrite', label: t('settings.audit.typeFileWrite') },
  { value: 'bash', label: t('settings.audit.typeBash') },
  // 「命令执行」= 每次 bash 执行的轻量事实记录（与「命令」的决策记录区分）
  { value: 'bash_exec', label: t('settings.audit.typeBashExec') },
  { value: 'network', label: t('settings.audit.typeNetwork') },
  { value: 'resource', label: t('settings.audit.typeResource') },
  { value: 'browser', label: t('settings.audit.typeBrowser') }
]

/** 处理类型筛选（同意/拒绝/自动放行/拦截；多选；空 = 全部） */
const approvalFilter = ref<string[]>([])
const APPROVAL_FILTERS: { value: string; label: string }[] = [
  { value: 'agree', label: t('settings.audit.approvalAgree') },
  { value: 'deny', label: t('settings.audit.approvalDeny') },
  { value: 'auto', label: t('settings.audit.approvalAuto') },
  { value: 'none', label: t('settings.audit.approvalNone') }
]

const hasFilter = computed(
  () => range.value !== 'all' || typeFilter.value.length > 0 || approvalFilter.value.length > 0
)

function clearFilters(): void {
  range.value = 'all'
  typeFilter.value = []
  approvalFilter.value = []
}

/** 分页 */
const page = ref(1)
const pageSize = ref(10)
function onPageChange(p: number): void {
  page.value = p
}

/** 会话下拉标签：标题为空回退 id 前缀 */
function sessionLabel(s: AuditSession): string {
  const title = localizedSessionTitle(s)
  return title || s.id.slice(0, 8)
}

/** 会话下拉选项（allow-search 按 label 关键词过滤） */
const sessionOptions = computed(() =>
  sessionsFiltered.value.map((s) => ({ value: s.id, label: sessionLabel(s) }))
)

const sessionsFiltered = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  return allSessions.value.filter((s) => {
    if (!kw) return true
    const title = localizedSessionTitle(s).toLowerCase()
    return title.includes(kw) || s.id.toLowerCase().includes(kw)
  })
})

async function loadSessions(): Promise<void> {
  loadingSessions.value = true
  try {
    if (!window.api.chat) {
      allSessions.value = []
      return
    }
    const list = (await window.api.chat.listSessions()) as unknown
    allSessions.value = Array.isArray(list) ? (list as AuditSession[]) : []
    // 时间倒序（近的在前）
    allSessions.value.sort((a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0))
  } catch {
    allSessions.value = []
  } finally {
    loadingSessions.value = false
  }
}

/** 选择会话 → 拉取审计日志 */
async function onSelect(id: string | number | boolean | Record<string, unknown> | (string | number | boolean | Record<string, unknown>)[] | undefined): Promise<void> {
  if (typeof id !== 'string') {
    selectedId.value = ''
    return
  }
  selectedId.value = id
  entries.value = []
  loadedError.value = ''
  if (!id) return
  loading.value = true
  try {
    const raw = (await invoke('read_audit_log', { session_id: id })) as string
    const parsed = JSON.parse(raw ?? '[]') as AuditLogEntry[]
    entries.value = Array.isArray(parsed) ? parsed : []
  } catch (e) {
    loadedError.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

/** 时间 + 类型 + 处理类型过滤后的日志（新→旧） */
const entriesFiltered = computed(() => {
  const now = Date.now()
  const cutoff = (() => {
    switch (range.value) {
      case '1h':
        return now - 3600_000
      case 'today': {
        const d = new Date()
        d.setHours(0, 0, 0, 0)
        return d.getTime()
      }
      case '7d':
        return now - 7 * 86400_000
      case '30d':
        return now - 30 * 86400_000
      default:
        return 0
    }
  })()
  return entries.value
    .filter((e) => e.time >= cutoff)
    .filter((e) => typeFilter.value.length === 0 || typeFilter.value.includes(e.type))
    .filter((e) => approvalFilter.value.length === 0 || approvalFilter.value.includes(e.approval))
    .sort((a, b) => b.time - a.time)
})

type DecoratedEntry = AuditLogEntry & { note: string; bash: string }
/** 过滤后条目：说明取自 detail（去掉已内嵌的命令原文），bash 取自后端原始 command */
const decoratedEntries = computed<DecoratedEntry[]>(() =>
  entriesFiltered.value.map((e) => {
    const bash = e.command || ''
    let note = e.detail || ''
    // detail 通常会以「：{command}」结尾；移除命令原文得到纯说明
    if (bash && note.includes(bash)) {
      note = note.replace(bash, '').replace(/[：:]\s*$/, '').trim()
    }
    return { ...e, note, bash }
  })
)

/** 当前分页条目 */
const pagedEntries = computed<DecoratedEntry[]>(() => {
  const start = (page.value - 1) * pageSize.value
  return decoratedEntries.value.slice(start, start + pageSize.value)
})

/** 是否有可展示的记录（驱动底部固定分页条显隐） */
const showPager = computed(() => decoratedEntries.value.length > 0)

/** 导出筛选后的所有审批数据为 JSONL */
async function exportData(): Promise<void> {
  if (decoratedEntries.value.length === 0) {
    Message.info(t('settings.audit.exportEmpty'))
    return
  }
  const name = `audit-${(selectedId.value || 'all').slice(0, 8)}-${Date.now()}.jsonl`
  const lines = decoratedEntries.value.map((e) => JSON.stringify(e))
  const content = lines.join('\n') + (lines.length ? '\n' : '')
  // 浏览器部署没有原生保存对话框 → 直接 Blob 下载（桌面仍走「另存为」+ 写文件）
  if (isWebShell) {
    downloadText(name, content, 'application/x-ndjson')
    Message.success(t('settings.audit.exportSuccess'))
    return
  }
  const path = await window.api.dialog.saveFile(name)
  if (!path) return
  const res = await window.api.fs.writeTextFile(path, content)
  if (res.ok) {
    Message.success(t('settings.audit.exportSuccess'))
  } else {
    Message.error(res.message || t('settings.audit.exportFailed'))
  }
}

const TYPE_META: Record<string, { label: string; cls: string }> = {
  fileread: { label: t('settings.audit.typeFileRead'), cls: 't-blue' },
  filewrite: { label: t('settings.audit.typeFileWrite'), cls: 't-orange' },
  bash: { label: t('settings.audit.typeBash'), cls: 't-purple' },
  // 与「命令」同色（同属命令族），标签区分「决策记录 / 每次执行的轻量事实」
  bash_exec: { label: t('settings.audit.typeBashExec'), cls: 't-purple' },
  network: { label: t('settings.audit.typeNetwork'), cls: 't-cyan' },
  resource: { label: t('settings.audit.typeResource'), cls: 't-red' },
  browser: { label: t('settings.audit.typeBrowser'), cls: 't-cyan' }
}

const APPROVAL_META: Record<string, { label: string; cls: string }> = {
  agree: { label: t('settings.audit.approvalAgree'), cls: 'a-green' },
  deny: { label: t('settings.audit.approvalDeny'), cls: 'a-red' },
  auto: { label: t('settings.audit.approvalAuto'), cls: 'a-blue' },
  none: { label: t('settings.audit.approvalNone'), cls: 'a-gray' }
}

function fmtTime(ms: number): string {
  const d = new Date(ms)
  const p = (n: number): string => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

/** 复制命令文本 */
async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    Message.success(t('common.copied'))
  } catch {
    Message.error(t('common.copyFailed'))
  }
}

// 筛选/会话变化时回到第一页
watch([range, typeFilter, approvalFilter, selectedId], () => {
  page.value = 1
})

onMounted(() => {
  loadSessions()
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <a-popover position="bottom" trigger="click">
        <h2 class="page-title">
          <icon-file-text :size="20" class="title-icon" />
          <span>{{ t('settings.audit.title') }}</span>
          <icon-info :size="15" class="title-info-icon" />
        </h2>
        <template #content>
          <div class="title-desc">{{ t('settings.audit.desc') }}</div>
        </template>
      </a-popover>
    </div>

    <div class="page-body">
      <!-- 筛选（吸顶）：上层会话 + 下层审计属性 -->
      <div class="filter-stack">
        <!-- 上层：会话筛选 -->
        <div class="filter-layer">
          <div class="layer-label">
            <icon-user-group :size="14" />
            <span>{{ t('settings.audit.sessionFilter') }}</span>
          </div>
          <div class="layer-controls">
            <div class="ctl ctl-session">
              <a-select
                v-model="selectedId"
                :placeholder="t('settings.audit.sessionPlaceholder')"
                :loading="loadingSessions"
                :options="sessionOptions"
                allow-search
                :allow-clear="true"
                @change="onSelect"
              />
            </div>
            <!-- 时间范围:紧邻会话筛选(按会话 + 时间锁定范围),与关键词搜索同一行 -->
            <div class="ctl ctl-range">
              <a-select v-model="range" :options="RANGE_OPTIONS" />
            </div>
            <div class="ctl ctl-kw">
              <a-input
                v-model="keyword"
                :placeholder="t('settings.audit.searchPlaceholder')"
                allow-clear
              >
                <template #prefix>
                  <icon-search :size="14" />
                </template>
              </a-input>
            </div>
          </div>
        </div>

        <!-- 下层：审计属性筛选 -->
        <div class="filter-layer">
          <div class="layer-label">
            <icon-filter :size="14" />
            <span>{{ t('settings.audit.attrFilter') }}</span>
          </div>
          <div class="layer-controls">
            <div class="ctl ctl-type">
              <a-select
                v-model="typeFilter"
                multiple
                :options="TYPE_FILTERS"
                :placeholder="t('settings.audit.typePlaceholder')"
              />
            </div>
            <div class="ctl ctl-handling">
              <a-select
                v-model="approvalFilter"
                multiple
                :options="APPROVAL_FILTERS"
                :placeholder="t('settings.audit.handlingPlaceholder')"
              />
            </div>
            <div class="filter-actions">
              <a-button size="small" @click="exportData">
                <template #icon><icon-download :size="13" /></template>
                {{ t('settings.audit.export') }}
              </a-button>
              <a-button v-if="hasFilter" size="small" @click="clearFilters">
                <template #icon><icon-close :size="13" /></template>
                {{ t('settings.audit.clearFilters') }}
              </a-button>
            </div>
          </div>
        </div>
      </div>

      <!-- 状态行：空态水平垂直居中 -->
      <div class="state-wrap">
        <div v-if="!selectedId" class="empty-tip">
          <icon-idcard :size="26" class="empty-icon" />
          <div>{{ t('settings.audit.emptyPickSession') }}</div>
        </div>
        <div v-else-if="loading" class="empty-tip">
          <icon-loading :size="26" class="empty-icon spin" />
          <div>{{ t('common.loading') }}</div>
        </div>
        <div v-else-if="loadedError" class="empty-tip err">{{ loadedError }}</div>
        <div v-else-if="entries.length === 0" class="empty-tip">
          <icon-empty :size="26" class="empty-icon" />
          <div>{{ t('settings.audit.emptyLogs') }}</div>
        </div>
        <div v-else-if="entriesFiltered.length === 0" class="empty-tip">
          <icon-filter :size="26" class="empty-icon" />
          <div>{{ t('settings.audit.emptyFiltered') }}</div>
          <a-button size="mini" @click="clearFilters">{{ t('settings.audit.clearFilters') }}</a-button>
        </div>

        <template v-else>
          <div class="list-meta">
            <span>{{ t('settings.audit.listCount', { n: decoratedEntries.length }) }}</span>
          </div>
          <div class="audit-list">
            <div v-for="(e, i) in pagedEntries" :key="i" class="audit-item">
              <div class="audit-head">
                <span class="badge" :class="TYPE_META[e.type]?.cls ?? 't-gray'">
                  {{ TYPE_META[e.type]?.label ?? e.type }}
                </span>
                <span class="badge" :class="APPROVAL_META[e.approval]?.cls ?? 'a-gray'">
                  {{ APPROVAL_META[e.approval]?.label ?? e.approval }}
                </span>
                <!-- 拦截分类徽标：仅在真正被拦截/拒绝的记录上展示，避免与类型徽标重复 -->
                <denial-badge
                  v-if="e.category && (e.approval === 'deny' || e.approval === 'none')"
                  :denial="{ category: e.category, source: e.source ?? undefined, detail: e.detail }"
                />
                <span class="audit-time">{{ fmtTime(e.time) }}</span>
              </div>
              <p v-if="e.note" class="audit-note">{{ e.note }}</p>
              <!-- 命令/动作：卡片底部一行等宽文本 + 复制(与技能/记忆卡片 footer 同风格) -->
              <div v-if="e.bash" class="audit-footer">
                <code class="audit-cmd">{{ e.bash }}</code>
                <button type="button" class="audit-copy" :title="t('common.copy')" @click="copyText(e.bash)">
                  <icon-copy :size="13" />
                </button>
              </div>
            </div>
          </div>
        </template>
      </div>
    </div>

    <!-- 底部分页条：固定于页面底部，不随列表滚动 -->
    <div v-if="showPager" class="audit-pagination">
      <a-pagination
        :total="decoratedEntries.length"
        :current="page"
        :page-size="pageSize"
        :show-total="false"
        :show-page-size="false"
        :show-jumper="false"
        @change="onPageChange"
      />
    </div>
  </div>
</template>

<style scoped>
.page {
  height: 100%;
  display: flex;
  flex-direction: column;
  padding: 28px 56px 16px;
  font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'SF Pro Display',
    'PingFang SC', 'Helvetica Neue', 'Segoe UI', Arial, sans-serif;
}

.page-header {
  margin-bottom: 12px;
  flex-shrink: 0;
}

.page-title {
  display: inline-flex;
  align-items: center;
  gap: 9px;
  font-size: var(--font-size-xl);
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--text-primary);
}

.title-icon {
  color: var(--accent-text);
}

.title-info-icon {
  color: var(--text-tertiary);
  cursor: pointer;
  transition: color 0.15s ease;
}

.title-info-icon:hover {
  color: var(--text-secondary);
}

.title-desc {
  max-width: 340px;
  line-height: 1.6;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

.page-body {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: auto;
  /* 去掉右侧额外留白,卡片铺满到页面内边距;滚动条走全局那套(空闲隐藏/滚动时出现) */
  padding: 0 0 12px 0;
}

/* 状态区：空态水平垂直居中 */
.state-wrap {
  flex: 1;
  min-height: 320px;
  display: flex;
  flex-direction: column;
}

/* ---------- 两层筛选（吸顶） ---------- */
.filter-stack {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  flex-direction: column;
  gap: 6px;
  /* 底部内边距并入背景，滚动时列表内容被完整遮挡（避免露出透明缝隙） */
  padding-bottom: 10px;
  /* macOS 工具栏:半透明材质 + 毛玻璃,滚动内容从下方虚化掠过;不画分隔线,
     与列表的分层全靠材质本身表达 */
  background: color-mix(in srgb, var(--panel-bg) 88%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
}

.filter-layer {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 2px 0;
  background-color: transparent;
  border: none;
}

.layer-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  font-size: var(--font-size-sm);
  font-weight: 500;
  letter-spacing: -0.01em;
  color: var(--text-secondary);
}

.layer-label :deep(svg) {
  color: var(--text-tertiary);
}

.layer-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  flex: 1;
  min-width: 0;
}

/* 每个控件用固定宽度包一层，避免 Arco 组件自身拉伸/换行。
   宽度分配:
   - 会话下拉:固定 240px(要放下会话标题);
   - 时间范围:固定 112px(只放「近30天」这类短标签);
   - 关键词:吃掉剩余宽度(180-360px),窄窗口优先收缩它,故两行都不换行;
   - 审计属性三个筛选:等分一行(152-240px 之间),按钮贴右。 */
.ctl {
  flex: 0 0 auto;
}

.ctl-session { width: 240px; }
.ctl-range { width: 112px; }
.ctl-kw {
  flex: 1 1 200px;
  min-width: 180px;
  max-width: 360px;
}

.ctl-type,
.ctl-handling {
  flex: 1 1 0;
  min-width: 152px;
  max-width: 240px;
}

.ctl :deep(.arco-select),
.ctl :deep(.arco-select-view),
.ctl :deep(.arco-input),
.ctl :deep(.arco-input-outer) {
  width: 100%;
}

.filter-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

/* 说明：Arco 下拉面板经 popup-container 渲染到 body，scoped :deep 无法命中；
   其宽度跟随触发宽度(上文已加宽)，故不再用 :deep 覆盖下拉面板。 */
.filter-layer :deep(.arco-select-view),
.filter-layer :deep(.arco-input-wrapper),
.filter-layer :deep(.arco-input-outer) {
  background-color: var(--color-bg-5);
  border-color: var(--border-base);
  /* macOS 控件形状:8px 圆角(Arco 默认 2px 偏方) */
  border-radius: 8px;
  font-family: inherit;
}

/* 关键词搜索框:macOS 搜索框观感(柔和填充 + 放大镜前缀) */
.ctl-kw :deep(.arco-input-prefix) {
  color: var(--text-tertiary);
}

.ctl-kw :deep(.arco-input-wrapper:hover),
.ctl-kw :deep(.arco-input-wrapper:focus-within) {
  background-color: var(--color-fill-3);
}

/* 本页 Arco 控件/按钮统一 macOS 圆角与字体(Arco 默认 2px) */
.page :deep(.arco-btn) {
  border-radius: 6px;
  font-family: inherit;
}

/* ---------- 审计列表 ---------- */
.list-meta {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 8px;
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-tertiary);
  margin-bottom: 10px;
}

.audit-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  max-width: 100%;
}

.audit-item {
  /* min-width:0：避免长命令把卡片撑得比容器宽 */
  min-width: 0;
  max-width: 100%;
  display: flex;
  flex-direction: column;
  gap: 8px;
  /* macOS 卡片:白底 + 细边框 + 大圆角(与技能/记忆卡片一致) */
  border-radius: var(--radius-lg);
  border: 1px solid var(--border-base);
  background-color: var(--bg-base);
  padding: 14px 16px;
}

.audit-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.audit-time {
  margin-left: auto;
  font-size: var(--font-size-xs);
  font-weight: 500;
  font-variant-numeric: tabular-nums;
  color: var(--text-tertiary);
}

.audit-note {
  margin: 0;
  font-size: var(--font-size-sm);
  line-height: 1.6;
  color: var(--text-secondary);
  word-break: break-word;
  white-space: pre-wrap;
}

/* 命令/动作 footer:细分割线 + 等宽文本 + 复制(与技能卡片 footer 同风格) */
.audit-footer {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--border-light);
}

.audit-cmd {
  flex: 1;
  min-width: 0;
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  line-height: 1.6;
  color: var(--text-tertiary);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.audit-copy {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  flex-shrink: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.audit-copy:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.audit-pagination {
  flex-shrink: 0;
  display: flex;
  justify-content: center;
  padding: 10px 0 4px;
  border-top: 0.5px solid var(--border-base);
}

/* macOS 分页:圆角方块 + 中性色选中态(与全局灰色控件口径一致,不用 Arco 的主色蓝) */
.audit-pagination :deep(.arco-pagination-item) {
  border-radius: 6px;
  border-color: transparent;
  background-color: transparent;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.audit-pagination :deep(.arco-pagination-item:not(.arco-pagination-item-disabled):hover) {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

.audit-pagination :deep(.arco-pagination-item-active),
.audit-pagination :deep(.arco-pagination-item-active:hover) {
  background-color: var(--bg-active);
  color: var(--text-primary);
  font-weight: 600;
}

/* 上一页/下一页箭头按钮同样圆角化 */
.audit-pagination :deep(.arco-pagination-item-previous),
.audit-pagination :deep(.arco-pagination-item-next) {
  border-radius: 6px;
}

/* ---------- 徽标 ---------- */
.badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: var(--font-size-2xs);
  font-weight: 500;
  line-height: 1.6;
  white-space: nowrap;
}

.t-blue { background: rgba(22, 119, 255, 0.15); color: #1677ff; }
.t-orange { background: rgba(255, 125, 0, 0.15); color: #e8862e; }
.t-purple { background: rgba(134, 90, 255, 0.15); color: #722ed1; }
.t-cyan { background: rgba(19, 194, 194, 0.15); color: #08979c; }
.t-red { background: rgba(255, 77, 79, 0.15); color: #cf1322; }
.t-gray { background: var(--color-fill-3); color: var(--text-secondary); }

.a-green { background: rgba(52, 199, 89, 0.18); color: #1a9e4b; }
.a-red { background: rgba(255, 69, 58, 0.15); color: #d4380d; }
.a-blue { background: rgba(0, 122, 255, 0.15); color: #1677ff; }
.a-gray { background: var(--color-fill-3); color: var(--text-secondary); }

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
  animation: audit-spin 1s linear infinite;
}

@keyframes audit-spin {
  to { transform: rotate(360deg); }
}

.empty-tip.err {
  color: var(--color-danger-6);
}
</style>
