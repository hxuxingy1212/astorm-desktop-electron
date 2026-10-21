<script setup lang="ts">
/**
 * 「交付物」页签:跨会话汇总对话产物,两级折叠分组 + 时间筛选(分段控件)。
 *
 * - 时间筛选(今天/昨天/近一周/近一月/全部):macOS 分段控件,窗口**包含式**
 *   (近一周含今天与昨天;筛选只影响可见范围,不改变数据拉取);
 * - 一级分组 = 会话(折叠头:会话标题 + 「N 个文件」徽标);
 * - 二级分组 = 该会话每一轮的用户消息(折叠头:摘录 + 文件数 + 时间),文件行挂其下;
 * - 数据源:sqlite `session_deliverables` 表,经 `all_deliverables`(napi
 *   `list_all_deliverables`,按更新时间倒序限量)取回;分组/筛选在前端完成。
 * - 同一路径文件跨轮重复上报时只保留最新一轮(视为新版本);
 *   会话行点击跳对应会话;文件行点击弹统一文件预览弹窗(FilePreviewModal)。
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { ChevronDown, MessageSquareMore, RefreshCw } from 'lucide-vue-next'
import { invoke } from '@/services/backend'
import { useSessionsStore } from '@/stores/sessions'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { getFileIconUrl } from '@/utils/fileIcons'
import FilePreviewModal from '@/components/common/FilePreviewModal.vue'
import type { DeliverableRow, Session } from '@/types'

/** 查询上限(行 = 轮次;一页 500 足够近期浏览) */
const QUERY_LIMIT = 500
const DAY = 86_400_000

/** 时间筛选窗口(key 即 i18n 键 deliverables.<key>) */
type WindowKey = 'today' | 'yesterday' | 'week' | 'month' | 'all'
const TIME_WINDOWS: WindowKey[] = ['today', 'yesterday', 'week', 'month', 'all']

interface FileEntry {
  path: string
  name: string
  size?: number
  /** 所属轮次的时间(unix ms) */
  ts: number
}

/** 二级分组:一轮对话(用户消息)及其交付物文件 */
interface RoundGroup {
  key: string
  round: number
  /** 用户消息单行摘录 */
  excerpt: string
  ts: number
  files: FileEntry[]
}

/** 一级分组:会话及其各轮 */
interface SessionGroup {
  sessionId: string
  title: string
  rounds: RoundGroup[]
  fileCount: number
  latestTs: number
}

const { t } = useI18n()
const router = useRouter()
const sessionsStore = useSessionsStore()

/** 文件预览弹窗(统一组件 FilePreviewModal;与会话内交付卡片/邮件附件同款) */
const previewFile = ref<{ name: string; path: string } | null>(null)

const rows = ref<DeliverableRow[]>([])
const loading = ref(false)
const loadError = ref('')
const activeWindow = ref<WindowKey>('week')

/** 拉取全量交付物(返回 JSON 字符串,与 session_deliverables 同款解析) */
async function load(): Promise<void> {
  loading.value = true
  loadError.value = ''
  try {
    const raw = await invoke('all_deliverables', { limit: QUERY_LIMIT })
    const parsed = typeof raw === 'string' ? (JSON.parse(raw) as DeliverableRow[]) : []
    rows.value = Array.isArray(parsed) ? parsed : []
  } catch (e) {
    rows.value = []
    loadError.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}
onMounted(() => void load())

// ---------- 时间筛选 ----------

function startOfDay(ts: number): number {
  const d = new Date(ts)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

/** 行时间 = 最近覆盖时间(缺省回退首次写入;后端值为 unix 秒) */
function rowTs(row: DeliverableRow): number {
  return (row.updatedAt || row.createdAt) * 1000
}

/** 筛选窗口(包含式:近一周/近一月均含更近的时间;today 为本次求值的当日零点,避免逐行重复取) */
function inWindow(ts: number, win: WindowKey, today: number): boolean {
  switch (win) {
    case 'today':
      return ts >= today
    case 'yesterday':
      return ts >= today - DAY && ts < today
    case 'week':
      return ts >= today - 7 * DAY
    case 'month':
      return ts >= today - 30 * DAY
    case 'all':
      return true
  }
}

// ---------- 分组:会话 → 轮次 → 文件 ----------

function rowKey(row: DeliverableRow): string {
  return `${row.sessionId}:${row.round}`
}

/** 单行提问摘录:压平换行,模板里 CSS 截断 */
function oneLine(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}

/**
 * 会话索引:sessionId → Session(O(1) 查找)。
 * 分组计算里逐行查会话,线性 find 是 O(行数 × 会话数),会话多了以后是主热点,先建倒排表。
 */
const sessionIndex = computed(() => {
  const m = new Map<string, Session>()
  for (const s of sessionsStore.sessions) {
    if (s.backendId) m.set(s.backendId, s)
    if (s.id !== s.backendId) m.set(s.id, s)
  }
  return m
})

const sessionGroups = computed<SessionGroup[]>(() => {
  const win = activeWindow.value
  const today = startOfDay(Date.now())
  const index = sessionIndex.value
  const sessions = new Map<string, SessionGroup>()
  const rounds = new Map<string, RoundGroup>()
  for (const row of rows.value) {
    const ts = rowTs(row)
    if (!inWindow(ts, win, today)) continue
    let rg = rounds.get(rowKey(row))
    if (!rg) {
      rg = { key: rowKey(row), round: row.round, excerpt: oneLine(row.userMessage), ts, files: [] }
      rounds.set(rg.key, rg)
      const s = index.get(row.sessionId)
      const g = sessions.get(row.sessionId) ?? {
        sessionId: row.sessionId,
        title: s?.title || row.sessionTitle || t('common.unnamed'),
        rounds: [],
        fileCount: 0,
        latestTs: ts
      }
      g.rounds.push(rg)
      sessions.set(row.sessionId, g)
    }
    // 同路径文件跨轮重复上报:保留最新一轮(视为新版本)
    for (const d of row.deliverables ?? []) {
      const exist = rg.files.find((f) => f.path === d.path)
      if (exist) {
        if (ts >= exist.ts) {
          exist.name = d.name
          exist.size = d.size
        }
      } else {
        rg.files.push({ path: d.path, name: d.name, size: d.size, ts })
      }
    }
  }
  for (const g of sessions.values()) {
    g.rounds.sort((a, b) => b.ts - a.ts)
    g.fileCount = g.rounds.reduce((n, r) => n + r.files.length, 0)
    g.latestTs = g.rounds[0]?.ts ?? 0
  }
  return [...sessions.values()].sort((a, b) => b.latestTs - a.latestTs)
})

/** 当前筛选下的文件总数(顶部汇总) */
const fileTotal = computed(() => sessionGroups.value.reduce((n, g) => n + g.fileCount, 0))

// ---------- 折叠(一级会话默认展开,二级轮次默认收起) ----------

const collapsedSessions = ref(new Set<string>())
const collapsedRounds = ref(new Set<string>())

function isSessionCollapsed(id: string): boolean {
  return collapsedSessions.value.has(id)
}

function toggleSession(id: string): void {
  const next = new Set(collapsedSessions.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  collapsedSessions.value = next
}

function isRoundCollapsed(key: string): boolean {
  return collapsedRounds.value.has(key)
}

function toggleRound(key: string): void {
  const next = new Set(collapsedRounds.value)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  collapsedRounds.value = next
}

// ---------- 行展示辅助 ----------

/** 所属会话:统一走 sessionIndex(O(1);会话未登记时 undefined) */
function sessionById(id: string): Session | undefined {
  return sessionIndex.value.get(id)
}

function pad(n: number): string {
  return String(n).padStart(2, '0')
}

/** 时间文案:今天/昨天带 HH:mm;一周内「N 天前」;更早带日期 + HH:mm(按系统语言本地化,跨年带年份) */
function fmtTs(ts: number): string {
  const d = new Date(ts)
  const today = startOfDay(Date.now())
  const hhmm = `${pad(d.getHours())}:${pad(d.getMinutes())}`
  if (ts >= today) return `${t('deliverables.today')} ${hhmm}`
  if (ts >= today - DAY) return `${t('deliverables.yesterday')} ${hhmm}`
  const days = Math.floor((today - startOfDay(ts)) / DAY)
  if (days < 7) return t('deliverables.daysAgo', { n: Math.max(days, 1) })
  const sameYear = d.getFullYear() === new Date().getFullYear()
  const date = sameYear
    ? d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
  return `${date} ${hhmm}`
}

// ---------- 类型列:扩展名 → 类型文案 ----------

const DOC_EXT = ['md', 'markdown', 'mdx', 'txt', 'doc', 'docx', 'ppt', 'pptx', 'pdf', 'xls', 'xlsx', 'csv']
const IMAGE_EXT = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico', 'avif']
const WEB_EXT = ['html', 'htm']
const DATA_EXT = ['json', 'yaml', 'yml', 'toml', 'xml']
const ARCHIVE_EXT = ['zip', 'tar', 'gz', 'rar', '7z']

function extOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i < 0 ? '' : name.slice(i + 1).toLowerCase()
}

function typeOf(name: string): string {
  const ext = extOf(name)
  if (WEB_EXT.includes(ext)) return t('deliverables.typeWebsite')
  if (DOC_EXT.includes(ext)) return t('deliverables.typeDoc')
  if (IMAGE_EXT.includes(ext)) return t('deliverables.typeImage')
  if (DATA_EXT.includes(ext)) return t('deliverables.typeData')
  if (ARCHIVE_EXT.includes(ext)) return t('deliverables.typeArchive')
  return t('deliverables.typeFile')
}

function sizeText(size?: number): string {
  if (!size) return ''
  if (size >= 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`
  if (size >= 1024) return `${Math.round(size / 1024)} KB`
  return `${size} B`
}

// ---------- 跳转 / 打开 ----------

/** 一级会话组头点击 → 折叠;双击/中键不做跳转(跳转入口在会话标题 tooltip 说明) */
const isAbsolute = (p: string): boolean => /^[a-zA-Z]:[\\/]/.test(p) || /^\//.test(p)

/** 相对工作区路径 → 绝对路径(会话已删除时回退默认工作区,尽力而为) */
async function absolutePath(sessionId: string, path: string): Promise<string> {
  if (isAbsolute(path)) return path
  const base = (await resolveSessionWorkspaceDir(sessionById(sessionId)?.workspace ?? null)).replace(
    /[\\/]+$/,
    ''
  )
  const rel = path.replace(/^[\\/]+/, '')
  return base ? `${base}/${rel}` : rel
}

/** 交付物文件行点击 → 统一文件预览弹窗(内容按类型分流,复用 FilePreviewPanel 直连渲染) */
async function previewDeliverable(sessionId: string, item: { path: string; name: string }): Promise<void> {
  previewFile.value = { name: item.name, path: await absolutePath(sessionId, item.path) }
}

/** 跳转所属会话(会话行标题双击触发;查不到时静默忽略) */
function openSession(sessionId: string): void {
  if (sessionById(sessionId)) void router.push(`/chat/${sessionId}`)
}
</script>

<template>
  <div class="dv-root">
    <!-- 顶部:时间筛选(分段控件) + 汇总 + 刷新 -->
    <div class="dv-toolbar">
      <div class="dv-seg">
        <button
          v-for="w in TIME_WINDOWS"
          :key="w"
          type="button"
          class="dv-seg-btn"
          :class="{ active: activeWindow === w }"
          @click="activeWindow = w"
        >
          {{ t(`deliverables.${w}`) }}
        </button>
      </div>
      <span class="dv-spacer" />
      <span v-if="fileTotal" class="dv-total">{{ t('deliverables.filesCount', { n: fileTotal }) }}</span>
      <button type="button" class="dv-refresh" :title="t('common.refresh')" @click="load">
        <RefreshCw :size="14" :class="{ 'dv-spin': loading }" />
      </button>
    </div>

    <p v-if="loadError" class="dv-error" :title="loadError">{{ t('deliverables.loadFailed') }}: {{ loadError }}</p>

    <div v-if="loading && !rows.length" class="dv-empty">
      <span class="dv-loading">{{ t('common.loading') }}</span>
    </div>

    <div v-else-if="!sessionGroups.length" class="dv-empty">
      <a-empty :description="t('deliverables.empty')" />
    </div>

    <template v-else>
      <!-- 表头(粘顶):名称 / 类型 / 大小 / 更新时间 / 路径 -->
      <div class="dv-thead dv-grid">
        <span class="dv-col-name">{{ t('knowledge.colName') }}</span>
        <span>{{ t('knowledge.colType') }}</span>
        <span>{{ t('knowledge.colSize') }}</span>
        <span>{{ t('knowledge.colUpdated') }}</span>
        <span>{{ t('knowledge.colPath') }}</span>
      </div>

      <section v-for="g in sessionGroups" :key="g.sessionId" class="dv-group">
        <!-- 一级:会话(折叠头:标题 + 文件数徽标;双击标题跳会话) -->
        <button type="button" class="dv-group-head" @dblclick="openSession(g.sessionId)" @click="toggleSession(g.sessionId)">
          <ChevronDown :size="14" class="dv-chev" :class="{ open: !isSessionCollapsed(g.sessionId) }" />
          <span class="dv-group-title" :title="g.title">{{ g.title }}</span>
          <span class="dv-group-count">{{ t('deliverables.filesCount', { n: g.fileCount }) }}</span>
        </button>

        <template v-if="!isSessionCollapsed(g.sessionId)">
          <div v-for="rg in g.rounds" :key="rg.key" class="dv-round-block">
            <!-- 二级:轮次 = 当轮用户消息(摘录);图标 = 对话气泡(lucide MessageSquareMore),点击折叠/展开文件行 -->
            <button type="button" class="dv-round-head" @click="toggleRound(rg.key)">
              <MessageSquareMore :size="15" class="dv-round-icon" />
              <span class="dv-round-title">{{ rg.excerpt || t('deliverables.roundFallback', { n: rg.round }) }}</span>
              <span class="dv-spacer" />
              <span class="dv-round-count">{{ t('deliverables.filesCount', { n: rg.files.length }) }}</span>
              <span class="dv-round-time">{{ fmtTs(rg.ts) }}</span>
            </button>

            <!-- 交付物文件行:图标 + 名称 | 类型 | 更新人 | 更新时间 | 大小 -->
            <template v-if="!isRoundCollapsed(rg.key)">
              <button
                v-for="f in rg.files"
                :key="f.path"
                type="button"
                class="dv-file-row dv-grid"
                :title="f.path"
                @click="previewDeliverable(g.sessionId, f)"
              >
                <span class="dv-name-cell">
                  <img class="dv-file-icon" :src="getFileIconUrl(f.name)" alt="" />
                  <span class="dv-file-name">{{ f.name }}</span>
                </span>
                <span class="dv-cell">{{ typeOf(f.name) }}</span>
                <span class="dv-cell">{{ sizeText(f.size) }}</span>
                <span class="dv-cell">{{ fmtTs(f.ts) }}</span>
                <span class="dv-cell dv-cell-path">{{ f.path }}</span>
              </button>
            </template>
          </div>
        </template>
      </section>
    </template>

    <!-- 统一文件预览弹窗:毛玻璃遮罩 + FilePreviewPanel 直连渲染(按 path 重挂载防状态串扰) -->
    <FilePreviewModal
      :open="!!previewFile"
      :name="previewFile?.name ?? ''"
      :path="previewFile?.path ?? ''"
      @close="previewFile = null"
    />
  </div>
</template>

<style scoped>
.dv-root {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow: auto;
  scrollbar-gutter: stable;
}

/* ---------- 顶部:时间筛选分段控件 + 汇总 + 刷新 ---------- */
.dv-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  padding: 0 4px 8px;
}

/* macOS 分段控件(与页签同款):浅灰轨道 + 选中白色胶囊 */
.dv-seg {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 6px;
  background: var(--bg-2);
  border-radius: 10px;
}

.dv-seg-btn {
  padding: 5px 14px;
  font-size: var(--font-size-sm);
  line-height: 1.4;
  border: none;
  background: transparent;
  border-radius: 6px;
  color: var(--text-secondary);
  font-family: inherit;
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.15s ease;
}

.dv-seg-btn:hover {
  color: var(--text-primary);
  background: var(--bg-3);
}

.dv-seg-btn.active {
  color: var(--accent-text);
  background: var(--bg-base);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

.dv-spacer {
  flex: 1;
  min-width: 8px;
}

.dv-total {
  flex: none;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}

.dv-refresh {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background-color 0.15s, color 0.15s;
}

.dv-refresh:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}

.dv-spin {
  animation: dv-rotate 0.9s linear infinite;
}

@keyframes dv-rotate {
  to {
    transform: rotate(360deg);
  }
}

.dv-error {
  margin: 0;
  flex-shrink: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  color: var(--danger, #ff3b30);
}

.dv-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.dv-loading {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

/* ---------- 表格:五行栅格(名称/路径弹性,路径多分一些;类型/大小/时间压到内容宽) ---------- */
.dv-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 68px 64px 108px minmax(0, 1.5fr);
  column-gap: 12px;
  align-items: center;
}

/* 表头:粘顶 + 发丝底线;水平 padding 与文件行一致,保证列逐列对齐 */
.dv-thead {
  position: sticky;
  top: 0;
  z-index: 2;
  flex-shrink: 0;
  padding: 8px;
  background: var(--panel-bg);
  border-bottom: 1px solid var(--border-light);
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  user-select: none;
}

/* ---------- 一级:会话组头 ---------- */
.dv-group {
  flex-shrink: 0;
}

.dv-group-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 10px 8px 6px;
  border: none;
  background: transparent;
  font: inherit;
  text-align: left;
  cursor: pointer;
  user-select: none;
  min-width: 0;
}

.dv-chev {
  flex: none;
  color: var(--text-tertiary);
  transition: transform 0.15s;
}

/* 折叠时箭头指向右,展开恢复向下 */
.dv-chev:not(.open) {
  transform: rotate(-90deg);
}

.dv-group-title {
  flex: 0 1 auto;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}

/* 「N 个文件」数量徽标(macOS 灰胶囊) */
.dv-group-count {
  flex: none;
  padding: 1px 8px;
  border-radius: 999px;
  background: var(--bg-3);
  font-size: var(--font-size-2xs);
  line-height: 1.6;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

/* ---------- 二级:轮次(用户消息)折叠头 ---------- */
.dv-round-block {
  display: flex;
  flex-direction: column;
}

.dv-round-head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px 6px 30px;
  border: none;
  background: transparent;
  font: inherit;
  text-align: left;
  cursor: pointer;
  min-width: 0;
  border-radius: 8px;
  transition: background-color 0.12s;
}

.dv-round-head:hover {
  background: var(--bg-3);
}

/* 轮次图标:对话气泡(lucide,currentColor 适配明暗;装饰性的,不随折叠态旋转) */
.dv-round-icon {
  display: block;
  width: 15px;
  height: 15px;
  flex: none;
  color: var(--text-secondary);
}

.dv-round-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-code);
  color: var(--text-primary);
}

.dv-round-count {
  flex: none;
  padding: 1px 7px;
  border: 1px solid var(--border-base);
  border-radius: 999px;
  background: var(--bg-2);
  font-size: var(--font-size-2xs);
  line-height: 1.5;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.dv-round-time {
  flex: none;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}

/* ---------- 文件行:缩进做在名称单元格内部(网格几何与表头一致,列才对得齐) ---------- */
.dv-file-row {
  padding: 7px 8px;
  border: none;
  background: transparent;
  font: inherit;
  text-align: left;
  cursor: pointer;
  min-width: 0;
  border-radius: 8px;
  transition: background-color 0.12s;
}

.dv-file-row:hover {
  background: var(--bg-3);
}

.dv-name-cell {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
  /* 两级缩进(会话 0 → 轮次 30 → 文件图标 64px) */
  padding-left: 56px;
}

.dv-file-icon {
  display: block;
  width: 19px;
  height: 19px;
  flex: none;
}

.dv-file-name {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

.dv-cell {
  font-size: var(--font-size-code);
  color: var(--text-secondary);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-variant-numeric: tabular-nums;
}

/* 路径列:等宽字体 + 次要色,悬停 title 有完整值 */
.dv-cell-path {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
</style>
