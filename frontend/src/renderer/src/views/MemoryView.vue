<script setup lang="ts">
/**
 * 记忆页：长期记忆（OKF v0.2 Bundle）的**内容**浏览 + 编译触发。
 *
 * 与「设置 → 记忆」分工：设置页管配置（开关 / 分类表 / 预算），本页管内容
 * （概念列表 → 详情正文 / 变更历史 / 来源会话）。编译是异步的，进度由 store 订阅的
 * `type === 'memory'` 事件推进（见 `stores/memory.ts` 的 `initMemorySync`）。
 */
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { MdPreview } from 'md-editor-v3'
import KbMarkdownPreview from '@/components/preview/KbMarkdownPreview.vue'
import MarkdownFrontmatterHeader from '@/components/common/MarkdownFrontmatterHeader.vue'
import { useMemoryStore, MEMORY_STAGES } from '@/stores/memory'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import { resolveMemoryLink, rawStemOf } from '@/utils/memoryLinks'
import { emptyFrontmatterMeta, stripFrontmatter, type FrontmatterMeta } from '@/utils/frontmatter'
import type { MemoryConcept } from '@/services/dto'

const { t } = useI18n()
const memory = useMemoryStore()
const settings = useSettingsStore()

/** markdown 明暗主题跟随应用（与 AgentFormModal 一致） */
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

/** 分类过滤（空串 = 全部） */
const activeCategory = ref('')
const searchInput = ref('')
const drawerVisible = ref(false)
const drawerTab = ref<'log' | 'sources'>('log')
const rawPreview = ref<{ stem: string; text: string; anchor: string | null } | null>(null)
/** 「更多」菜单展开态（自绘面板：Arco 下拉会被 teleport 到 body，作用域样式够不到） */
const menuOpen = ref(false)

/** 是否已选中概念 ⇒ 进入「列表 + 正文」分栏；未选中时列表独占整宽 */
const hasDetail = computed(() => memory.selectedId !== '')

/**
 * 左侧列表：**分类与检索叠加生效**（先检索再点分类 = 只看「命中且属于该分类」）。
 *
 * 之前检索态直接返回全量命中、无视分类：chip 会高亮着但内容不理它，
 * 出现「选中 Instruction、列表里却是全部 23 条」这种自相矛盾的画面。
 */
const visibleConcepts = computed<MemoryConcept[]>(() => {
  if (!activeCategory.value) return memory.concepts
  return memory.concepts.filter((c) => c.categoryId === activeCategory.value)
})

/**
 * chip 计数：**与列表同口径**。
 *
 * 检索中按当前命中集算（如 Instruction 0），否则用后端全库统计；
 * 否则会出现「chip 说 4 条、点进去只有 2 条」。
 */
function chipCount(categoryId: string): number {
  if (!memory.searching) {
    if (!categoryId) return memory.status?.conceptTotal ?? 0
    return memory.status?.categories.find((c) => c.id === categoryId)?.count ?? 0
  }
  if (!categoryId) return memory.concepts.length
  return memory.concepts.filter((c) => c.categoryId === categoryId).length
}

/** 当前生效分类的 type 名（列表底部说明用） */
const activeCategoryLabel = computed(
  () =>
    memory.status?.categories.find((c) => c.id === activeCategory.value)?.type ??
    activeCategory.value
)

/* ---------- 检索关键词高亮 ---------- */

interface Seg {
  text: string
  hit: boolean
}

/**
 * 参与高亮的词：**整串 + 空白分隔的各词**。
 *
 * 后端 BM25 走 jieba 分词，命中的可能是整串里的某几个词（「认证 方案」↔「认证方案」），
 * 所以整串与分词都要作为高亮词，宁可多标也不漏标。
 */
const hlTerms = computed<string[]>(() => {
  if (!memory.searching) return []
  const q = memory.query.trim()
  if (!q) return []
  const terms = q.split(/\s+/).filter(Boolean)
  if (!terms.includes(q)) terms.push(q)
  return terms
})

/** 把文本按高亮词切成段（大小写不敏感；无检索时整段返回） */
function segmentsOf(text: string): Seg[] {
  const terms = hlTerms.value
  if (!text || terms.length === 0) return [{ text, hit: false }]
  const pattern = terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')
  const re = new RegExp(`(${pattern})`, 'gi')
  const out: Seg[] = []
  let last = 0
  for (const m of text.matchAll(re)) {
    const at = m.index ?? 0
    if (at > last) out.push({ text: text.slice(last, at), hit: false })
    out.push({ text: m[0], hit: true })
    last = at + m[0].length
  }
  if (last < text.length) out.push({ text: text.slice(last), hit: false })
  return out.length > 0 ? out : [{ text, hit: false }]
}

/**
 * 详情正文：剥掉 frontmatter 再渲染（frontmatter 已由右上角的元数据块呈现，
 * `MarkdownFrontmatterHeader` + 同一套 `stripFrontmatter`，与文件预览共用一处实现）。
 */
const detailBody = computed(() => stripFrontmatter(memory.detail?.markdown ?? ''))

/** 元数据块数据：后端解析好的 frontmatter 字段 → 与文件预览共用的 `MarkdownFrontmatterHeader` */
const detailMeta = computed<FrontmatterMeta>(() => {
  const d = memory.detail
  if (!d) return emptyFrontmatterMeta()
  return {
    ...emptyFrontmatterMeta(),
    title: d.title || d.conceptId,
    type: d.type,
    status: d.status ?? '',
    description: d.description,
    tags: d.tags,
    observedAt: d.observedAt ?? '',
    updatedAt: fmtTime(d.updatedAt),
    sources: d.sources.map((s) => ({
      resource: s.resource,
      title: s.title,
      lastModified: s.lastModified ? fmtIso(s.lastModified) : null
    })),
    fragIds: d.fragIds,
    generatedBy: d.generatedBy ?? '',
    generatedAt: d.generatedAt ? fmtIso(d.generatedAt) : '',
    model: d.model ?? ''
  }
})

/** 来源 chip 点击：打开该来源会话底稿 */
function onSourceClick(resource: string): void {
  void openRaw(rawStemOf(resource))
}

const stages = MEMORY_STAGES

/** 当前进行到第几个阶段（无阶段 = -1；用于把已过的阶段标灰） */
const stageIndex = computed(() => (memory.stage ? MEMORY_STAGES.indexOf(memory.stage) : -1))

function stageLabel(stage: string): string {
  return t(`memory.progress.${stage}`)
}

function fmtTime(secs: number): string {
  if (!secs) return '—'
  return new Date(secs * 1000).toLocaleString()
}

function fmtIso(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function actionLabel(action: string): string {
  if (action === 'CREATE') return t('memory.drawer.actionCreate')
  if (action === 'UPDATE') return t('memory.drawer.actionUpdate')
  return t('memory.drawer.actionSkip')
}

function errText(e: unknown): string {
  const msg = e instanceof Error ? e.message : String(e)
  return memory.backendMissing(e) ? t('memory.toast.backendUnavailable') : msg
}

/* ---------- 动作 ---------- */

async function refreshAll(): Promise<void> {
  await memory.refreshAll()
  if (memory.error) Message.error(t('memory.toast.statusFailed', { msg: memory.error }))
}

async function onCompile(): Promise<void> {
  menuOpen.value = false
  const ok = await memory.compile(false)
  if (ok) Message.info(t('memory.toast.compileStarted'))
  else Message.error(errText(memory.error))
}

function onCompileForce(): void {
  // 原为 Modal.warning(hideCancel: false)：是二选一确认，取消按钮有实际含义（不强制编译），
  // 故保留两按钮、只把色调换成 warning
  void macConfirm({
    title: t('memory.action.compileForce'),
    content: t('memory.action.compileForceConfirm'),
    tone: 'warning',
    confirmText: t('memory.action.compileForce'),
    onConfirm: async () => {
      const ok = await memory.compile(true)
      if (ok) Message.info(t('memory.toast.compileStarted'))
      else Message.error(errText(memory.error))
    }
  })
}

async function onCancel(): Promise<void> {
  await memory.cancel()
  Message.info(t('memory.toast.cancelRequested'))
}

async function onReindex(): Promise<void> {
  const n = await memory.reindex()
  if (n < 0) Message.error(errText(memory.error))
  else Message.success(t('memory.toast.reindexed', { count: n }))
}

async function onValidate(): Promise<void> {
  const report = await memory.validateBundle()
  if (!report) {
    Message.error(errText(memory.error))
    return
  }
  if (report.errors.length === 0) {
    Message.success(t('memory.toast.validateOk', { count: report.checked }))
  } else {
    Message.warning(
      t('memory.toast.validateIssues', { errors: report.errors.length, warnings: report.warnings.length })
    )
  }
}

async function onOpenRoot(): Promise<void> {
  const err = await memory.openRoot()
  if (err) Message.error(err)
}

function onSearch(): void {
  void memory.searchConcepts(searchInput.value)
}

function clearSearch(): void {
  searchInput.value = ''
  void memory.fetchConcepts()
}

async function onSelect(concept: MemoryConcept): Promise<void> {
  await memory.openConcept(concept.conceptId)
  if (memory.error) Message.error(errText(memory.error))
}

/** 收起正文 → 回到列表独占整宽的浏览态（同一卡片再点一次也收起） */
function onSelectCard(concept: MemoryConcept): void {
  if (memory.selectedId === concept.conceptId) {
    memory.closeConcept()
    return
  }
  void onSelect(concept)
}

function onCloseDetail(): void {
  memory.closeConcept()
}

/** 菜单项统一入口：先收起菜单再执行（否则面板会盖在确认弹窗上） */
function runMenu(action: () => void | Promise<void>): void {
  menuOpen.value = false
  void action()
}

// 编译期间「更多」整块不渲染（索引被占，校验/重建索引会互相打架）；
// 若此时 menuOpen 仍为 true，编译结束后菜单会自己弹回来并带上遮罩挡住页面点击 —— 故一并收起
watch(
  () => memory.running,
  (running) => {
    if (running) menuOpen.value = false
  }
)

async function openDrawer(tab: 'log' | 'sources'): Promise<void> {
  drawerTab.value = tab
  drawerVisible.value = true
  if (tab === 'log') await memory.fetchLog()
  else await memory.fetchRawSources()
}



function copyId(): void {
  const id = memory.detail?.conceptId
  if (!id) return
  void navigator.clipboard.writeText(id).then(
    () => Message.success(t('common.copied')),
    () => Message.error(t('common.copyFailed'))
  )
}

/* ---------- 站内跳转（来源 / 依据锚点 / 正文内链） ---------- */

/** 正文里的链接点击：概念 → 站内切换；原文 → 打开底稿抽屉；其余不拦。 */
function onBodyClick(e: MouseEvent): void {
  const anchor = (e.target as HTMLElement | null)?.closest('a')
  const href = anchor?.getAttribute('href')
  if (!href) return
  const target = resolveMemoryLink(href, memory.detail?.dir ?? '')
  if (!target) return
  e.preventDefault()
  if (target.kind === 'concept') void memory.openConcept(target.id)
  else void openRaw(target.stem)
}

/** 打开来源会话底稿；带 anchor 时定位并高亮该锚点行。 */
async function openRaw(stem: string, anchor: string | null = null): Promise<void> {
  try {
    const text = await memory.readRawSource(stem)
    rawPreview.value = { stem, text, anchor: text.includes(`[${anchor}]`) ? anchor : null }
  } catch (e) {
    Message.error(errText(e))
  }
}

/** 点「依据」锚点：在**来源会话**里找到含该锚点的那一份并跳过去（同一概念可能来自多个会话）。 */
async function onOpenFragment(anchor: string): Promise<void> {
  const sources = memory.detail?.sources ?? []
  for (const s of sources) {
    const stem = rawStemOf(s.resource)
    try {
      const text = await memory.readRawSource(stem)
      if (text.includes(`[${anchor}]`)) {
        rawPreview.value = { stem, text, anchor }
        return
      }
    } catch {
      /* 单个来源读失败就试下一个 */
    }
  }
  Message.warning(t('memory.detail.evidenceMissing', { anchor }))
}

/** 底稿文本 + 可选锚点高亮（必须转义：底稿是用户数据，不能当 HTML 注入）。 */
const rawHtml = computed(() => {
  const r = rawPreview.value
  if (!r) return ''
  const escaped = r.text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
  if (!r.anchor) return escaped
  const needle = `[${r.anchor}]`
  const at = escaped.indexOf(needle)
  if (at < 0) return escaped
  return `${escaped.slice(0, at)}<mark class="raw-mark" id="raw-anchor">${needle}</mark>${escaped.slice(at + needle.length)}`
})

/** 打开含锚点的底稿后滚到该行（第一次出现处） */
watch(rawPreview, async (r) => {
  if (!r?.anchor) return
  await nextTick()
  document.getElementById('raw-anchor')?.scrollIntoView({ block: 'center' })
})

/** 编译结束后刷新当前打开的详情（该概念可能刚被合并更新） */
watch(
  () => memory.lastReport,
  () => {
    if (memory.selectedId) void memory.openConcept(memory.selectedId)
  }
)

/**
 * 选中的概念被过滤掉就自动收起正文。
 *
 * 否则会出现「左列表里没有任何选中项、右侧却还摊着一篇正文」的错位（换分类 / 换检索词时）。
 */
watch(visibleConcepts, (list) => {
  if (memory.selectedId && !list.some((c) => c.conceptId === memory.selectedId)) {
    memory.closeConcept()
  }
})

/** 切到抽屉的「来源会话」时按需拉列表 */
watch(drawerTab, (tab) => {
  if (drawerVisible.value && tab === 'sources' && memory.rawSources.length === 0) {
    void memory.fetchRawSources()
  }
})

onMounted(() => {
  void refreshAll()
  void memory.fetchLog(50)
})
</script>

<template>
  <div class="page">
    <!-- 状态卡 + 动作 -->
    <div class="mem-header">
      <div class="mem-title">
        <icon-memory :size="20" class="title-icon" />
        <span>{{ t('memory.title') }}</span>
        <a-tag v-if="memory.status && !memory.status.enabled" size="small" color="orange">
          {{ t('memory.disabled') }}
        </a-tag>
      </div>
      <div class="mem-actions">
        <!-- 只留三件事：刷新 / 浏览历史与来源 / 编译；诊断与破坏性操作收进「更多」 -->
        <a-button
          size="small"
          class="icon-btn"
          :loading="memory.statusLoading"
          :title="t('memory.action.refresh')"
          @click="refreshAll"
        >
          <icon-refresh :size="14" />
        </a-button>
        <a-button size="small" @click="openDrawer('log')">
          <icon-history :size="13" />
          {{ t('memory.drawer.historyAndSources') }}
        </a-button>
        <template v-if="memory.running">
          <a-button size="small" status="danger" @click="onCancel">
            {{ t('memory.action.cancel') }}
          </a-button>
        </template>
        <template v-else>
          <a-button size="small" @click="onCompile">
            {{ t('memory.action.compile') }}
          </a-button>
          <div class="menu-wrap">
            <a-button
              size="small"
              class="icon-btn"
              :title="t('memory.action.more')"
              @click="menuOpen = !menuOpen"
            >
              <icon-more :size="14" />
            </a-button>
            <div v-if="menuOpen" class="menu-backdrop" @click="menuOpen = false" />
            <div v-if="menuOpen" class="menu-panel">
              <button class="menu-item" @click="runMenu(onOpenRoot)">
                <icon-folder :size="12" />
                {{ t('memory.action.openRoot') }}
              </button>
              <button class="menu-item" @click="runMenu(onValidate)">
                <icon-check-circle :size="12" />
                {{ t('memory.action.validate') }}
              </button>
              <button class="menu-item" @click="runMenu(onReindex)">
                <icon-db :size="12" />
                {{ t('memory.action.reindex') }}
              </button>
              <div class="menu-sep" />
              <button class="menu-item danger" @click="runMenu(onCompileForce)">
                {{ t('memory.action.compileForce') }}
              </button>
            </div>
          </div>
        </template>
      </div>
    </div>

    <!-- 统计条 -->
    <div v-if="memory.status" class="mem-stats">
      <span class="stat">
        <b>{{ memory.status.conceptTotal }}</b>
        {{ t('memory.status.concepts') }}
      </span>
      <span class="stat" :class="{ warn: memory.status.pending > 0 }">
        <b>{{ memory.status.pending }}</b>
        {{ t('memory.status.pending') }}
      </span>
      <span class="stat">
        {{ t('memory.status.lastSync') }}
        <b>{{ memory.status.lastSync ? fmtIso(memory.status.lastSync) : t('memory.status.never') }}</b>
      </span>
      <button
        class="stat stat-path"
        :title="t('memory.action.openRoot')"
        @click="onOpenRoot"
      >
        <icon-folder :size="12" />
        {{ memory.status.root }}
      </button>
    </div>

    <!-- 编译进度 -->
    <div v-if="memory.running || memory.lastFailedMessage" class="mem-progress">
      <template v-if="memory.running">
        <div class="prog-stages">
          <span
            v-for="(s, i) in stages"
            :key="s"
            class="prog-stage"
            :class="{ active: memory.stage === s, done: stageIndex > i }"
          >
            {{ stageLabel(s) }}
          </span>
        </div>
        <a-progress
          :percent="memory.percent"
          :show-text="false"
          size="small"
          :animation="true"
          class="prog-bar"
        />
        <span class="prog-msg" :title="memory.progressMessage">{{ memory.progressMessage }}</span>
        <span v-if="memory.total > 0" class="prog-count">{{ memory.current }} / {{ memory.total }}</span>
      </template>
      <a-alert v-else type="error" :title="t('memory.result.failed')">{{ memory.lastFailedMessage }}</a-alert>
    </div>

    <!-- 上一轮结果摘要 -->
    <div v-if="memory.lastReport && !memory.running" class="mem-report">
      <span class="rep-item">
        {{ t('memory.result.sessions', { processed: memory.lastReport.processedSessions, total: memory.lastReport.totalSessions }) }}
      </span>
      <span class="rep-item">{{ t('memory.result.units', { units: memory.lastReport.extractedUnits }) }}</span>
      <span class="rep-item ok">{{ t('memory.result.created', { count: memory.lastReport.created.length }) }}</span>
      <span class="rep-item">{{ t('memory.result.updated', { count: memory.lastReport.updated.length }) }}</span>
      <span class="rep-item">{{ t('memory.result.skipped', { count: memory.lastReport.skipped.length }) }}</span>
      <span class="rep-item">
        {{ t('memory.result.tokens', { calls: memory.lastReport.calls, prompt: memory.lastReport.promptTokens, completion: memory.lastReport.completionTokens }) }}
      </span>
      <a-popover v-if="memory.lastReport.warnings.length" position="bottom">
        <span class="rep-item warn">{{ t('memory.result.warnings', { count: memory.lastReport.warnings.length }) }}</span>
        <template #content>
          <div class="warn-list">
            <div v-for="(w, i) in memory.lastReport.warnings" :key="i">{{ w }}</div>
          </div>
        </template>
      </a-popover>
      <a-popover v-if="memory.lastReport.failedSessions.length" position="bottom">
        <span class="rep-item err">
          {{ t('memory.result.failedCount', { count: memory.lastReport.failedSessions.length }) }}
        </span>
        <template #content>
          <div class="warn-list">
            <div v-for="(f, i) in memory.lastReport.failedSessions" :key="i">
              [{{ f.stage }}] {{ f.sessionId }}：{{ f.message }}
            </div>
          </div>
        </template>
      </a-popover>
    </div>

    <a-alert
      v-if="memory.status && memory.status.parseErrors.length"
      type="warning"
      :title="t('memory.parseErrorTitle')"
      class="mem-alert"
    >
      <div v-for="(e, i) in memory.status.parseErrors" :key="i" class="warn-list-item">{{ e }}</div>
    </a-alert>

    <!-- 主体：未选中 = 列表独占整宽；选中 = 列表 + 正文分栏（宽度平滑过渡） -->
    <div class="mem-body" :class="{ 'has-detail': hasDetail }">
      <div class="mem-list">
        <div class="list-toolbar">
          <a-input
            v-model="searchInput"
            size="small"
            allow-clear
            :placeholder="t('memory.list.searchPlaceholder')"
            @press-enter="onSearch"
            @clear="clearSearch"
          >
            <template #prefix><icon-search :size="12" /></template>
          </a-input>
          <a-button size="small" class="toolbar-btn" @click="onSearch">
            {{ t('memory.list.search') }}
          </a-button>
        </div>

        <!-- 分类筛选（与检索叠加；计数随检索命中集走） -->
        <div class="cat-chips">
          <button class="chip" :class="{ active: activeCategory === '' }" @click="activeCategory = ''">
            {{ t('memory.list.all') }}
            <span class="chip-n">{{ chipCount('') }}</span>
          </button>
          <button
            v-for="c in memory.status?.categories ?? []"
            :key="c.id"
            class="chip"
            :class="{ active: activeCategory === c.id }"
            :title="c.hint || c.type"
            @click="activeCategory = c.id"
          >
            {{ c.type }}
            <span class="chip-n">{{ chipCount(c.id) }}</span>
          </button>
        </div>

        <div class="list-scroll">
          <a-spin v-if="memory.listLoading" class="list-spin" />
          <p v-else-if="visibleConcepts.length === 0" class="list-empty">
            {{ memory.searching ? t('memory.list.searchEmpty') : t('memory.list.empty') }}
            <span v-if="!memory.searching" class="list-empty-desc">{{ t('memory.list.emptyDesc') }}</span>
            <!-- 叠加过滤下命中为空时，给出退路而不是让人自己猜是哪个条件卡住了 -->
            <button v-if="activeCategory" class="list-empty-reset" @click="activeCategory = ''">
              {{ t('memory.list.clearCategory') }}
            </button>
          </p>
          <div
            v-for="c in visibleConcepts"
            :key="c.conceptId"
            class="concept-item"
            :class="{ active: memory.selectedId === c.conceptId }"
            @click="onSelectCard(c)"
          >
            <div class="ci-head">
              <span class="ci-title">
                <span
                  v-for="(seg, i) in segmentsOf(c.title || c.conceptId)"
                  :key="i"
                  :class="{ hl: seg.hit }"
                >{{ seg.text }}</span>
              </span>
              <!-- 用 `!= null` 而不是 `!== null`：napi 对 None 的 Option<f32> 会**整个省略**该字段，
                   非检索结果里 score 是 undefined，写成 !== null 会放行 undefined.toFixed() 把整页渲染打断 -->
              <span v-if="c.score != null" class="ci-score">{{ c.score.toFixed(1) }}</span>
            </div>
            <div class="ci-meta">
              <span class="ci-type">{{ c.type }}</span>
              <span v-if="c.observedAt" class="ci-date">{{ c.observedAt }}</span>
            </div>
            <div v-if="c.description" class="ci-desc">
              <span
                v-for="(seg, i) in segmentsOf(c.description)"
                :key="i"
                :class="{ hl: seg.hit }"
              >{{ seg.text }}</span>
            </div>
          </div>
        </div>
        <div class="list-foot">
          {{
            activeCategory
              ? t('memory.list.countScoped', { count: visibleConcepts.length, type: activeCategoryLabel })
              : t('memory.list.count', { count: visibleConcepts.length })
          }}
        </div>
      </div>

      <div class="mem-detail">
        <div v-if="memory.detailLoading" class="detail-loading"><a-spin /></div>
        <template v-else-if="memory.detail">
          <div class="detail-head">
            <!-- 元数据块：与文件预览的 frontmatter 呈现共用同一个组件（标题/徽标/chips/细行） -->
            <MarkdownFrontmatterHeader
              :meta="detailMeta"
              interactive
              @source-click="onSourceClick"
              @frag-click="onOpenFragment"
            >
              <template #actions>
                <button class="dh-close" :title="t('common.close')" @click="onCloseDetail">
                  <icon-close :size="12" />
                </button>
              </template>
              <template #subtitle>
                <div class="dh-id" :title="t('common.copy')" @click="copyId">
                  <icon-copy :size="12" />
                  {{ memory.detail.conceptId }}
                </div>
              </template>
            </MarkdownFrontmatterHeader>
          </div>
          <!-- 相对链接点击拦在容器上：概念互链站内切换、原文链接开底稿，不让渲染器去导航 -->
          <div class="detail-body" @click="onBodyClick">
            <MdPreview
              :model-value="detailBody"
              :theme="isDark ? 'dark' : 'light'"
              :code-theme="isDark ? 'dark' : 'light'"
              :preview-component="KbMarkdownPreview"
            />
          </div>
        </template>
        <div v-else class="detail-empty">
          <icon-memory :size="36" class="empty-icon" />
          <p>{{ t('memory.detail.empty') }}</p>
        </div>
      </div>
    </div>

    <!-- 抽屉：变更历史 / 来源会话 -->
    <a-drawer v-model:visible="drawerVisible" :width="620" :footer="false" unmount-on-close>
      <template #title>
        <!-- 自绘分段控件（macOS 观感）：不依赖 Arco radio 的内部结构，点击即改 drawerTab -->
        <div class="seg">
          <button
            class="seg-item"
            :class="{ active: drawerTab === 'log' }"
            @click="drawerTab = 'log'"
          >
            {{ t('memory.drawer.log') }}
          </button>
          <button
            class="seg-item"
            :class="{ active: drawerTab === 'sources' }"
            @click="drawerTab = 'sources'"
          >
            {{ t('memory.drawer.sources') }}
          </button>
        </div>
      </template>

      <template v-if="drawerTab === 'log'">
        <a-empty v-if="memory.log.length === 0" :description="t('memory.drawer.logEmpty')" />
        <div v-else class="log-list">
          <!-- 三列：动作 / 概念 ID / 时间。log.md 不落变更原因，故没有第四列（见 dto.ts MemoryLogEntry） -->
          <div v-for="(e, i) in memory.log" :key="i" class="log-row">
            <span class="log-action" :class="e.action.toLowerCase()">{{ actionLabel(e.action) }}</span>
            <span class="log-id" :title="e.conceptId">{{ e.conceptId }}</span>
            <span class="log-time">{{ fmtIso(e.at) }}</span>
          </div>
        </div>
      </template>

      <template v-else>
        <a-empty v-if="memory.rawSources.length === 0" :description="t('memory.drawer.sourcesEmpty')" />
        <div v-else class="raw-list">
          <div v-for="s in memory.rawSources" :key="s.stem" class="raw-row" @click="openRaw(s.stem)">
            <icon-file-text :size="14" />
            <span class="raw-name">{{ s.stem }}</span>
            <span class="raw-size">{{ fmtSize(s.size) }}</span>
            <span class="raw-time">{{ fmtTime(s.updatedAt) }}</span>
          </div>
        </div>
      </template>
    </a-drawer>

    <!-- 来源会话原文 -->
    <a-drawer
      :visible="rawPreview !== null"
      :width="820"
      :footer="false"
      unmount-on-close
      @cancel="rawPreview = null"
    >
      <template #title>{{ rawPreview?.stem }}</template>
      <pre class="raw-pre" v-html="rawHtml"></pre>
    </a-drawer>
  </div>
</template>

<style scoped>
/*
 * macOS 观感（与「会话搜索」SearchModal 同一套做法）：
 * - 填充层 `--fill-1/2/3` 全部由**文字色**混合而来 ⇒ 明暗主题自动跟随，不需要各写一套
 * - 分隔靠发丝线（--border-*），不靠粗边框；选中靠主色填充，不靠左侧色条
 * - 缓动用 macOS 的降速曲线，交互元素按下有轻微缩放
 * 注：这里不用 backdrop-filter —— 页面底色是不透明的，毛玻璃没有可透的内容，加了只是摆设。
 */
.page {
  /* 填充层 --fill-1/2/3 是全局令牌（见 themes/base.css）：抽屉/弹窗会被 teleport 到 body，
     页面级自定义属性在里面解析不到，故不能定义在本页 */
  --ease-mac: cubic-bezier(0.32, 0.72, 0, 1);
  --dur-mac: 0.32s;

  height: 100%;
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  padding: 16px 24px;
  overflow: hidden;
  background-color: var(--panel-bg);
}

/* ---------- 头部 ---------- */

.mem-header {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 4px 4px 12px;
  border-bottom: 1px solid var(--border-light);
}

.mem-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
  flex-shrink: 0;
}

.title-icon {
  color: var(--accent-text);
}

.mem-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
  flex-wrap: wrap;
  justify-content: flex-end;
}

/*
 * 工具条按钮：无边框、半透明填充、悬停变亮、按下回缩。
 * 不再给任何按钮加 `type="primary"` —— 截图里那颗实心蓝按钮是页面里唯一「另一种语言」的控件。
 */
.page :deep(.arco-btn) {
  height: 26px;
  padding: 0 10px;
  border: none;
  border-radius: 7px;
  background-color: var(--fill-1);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  box-shadow: none;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast),
    transform var(--transition-fast);
}

.page :deep(.arco-btn:hover) {
  background-color: var(--fill-2);
  color: var(--text-primary);
}

.page :deep(.arco-btn:active) {
  background-color: var(--fill-3);
  transform: scale(0.97);
}

/* 取消编译：保留红色语义，但同样走填充而非描边 */
.page :deep(.arco-btn-status-danger) {
  color: rgb(var(--danger-6));
  background-color: color-mix(in srgb, rgb(var(--danger-6)) 14%, transparent);
}

.page :deep(.arco-btn-status-danger:hover) {
  color: rgb(var(--danger-6));
  background-color: color-mix(in srgb, rgb(var(--danger-6)) 22%, transparent);
}

/* 图标按钮：方形、内容居中（刷新 / 更多） */
.page :deep(.arco-btn.icon-btn) {
  width: 26px;
  padding: 0;
  justify-content: center;
}

/* ---------- 「更多」菜单（自绘：macOS 弹出菜单观感） ---------- */

.menu-wrap {
  position: relative;
}

.menu-backdrop {
  position: fixed;
  inset: 0;
  z-index: 20;
}

/*
 * 菜单本体：半透明面板 + 发丝描边 + 投影 + 背景模糊。
 * 这里用 backdrop-filter 是有意义的（弹层下面就是正文内容），
 * 与页面级「底色不透明就别加模糊」的取舍不同。
 */
.menu-panel {
  position: absolute;
  top: calc(100% + 5px);
  right: 0;
  z-index: 21;
  min-width: 168px;
  padding: 4px;
  border: 1px solid var(--border-base);
  border-radius: 10px;
  background-color: color-mix(in srgb, var(--panel-bg) 88%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  box-shadow: var(--shadow-md);
  animation: menu-in 0.12s var(--ease-mac);
}

@keyframes menu-in {
  from {
    opacity: 0;
    transform: translateY(-4px) scale(0.98);
  }
  to {
    opacity: 1;
    transform: none;
  }
}

.menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 5px 8px;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  text-align: left;
  font-family: inherit;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  background-color: transparent;
  transition: background-color var(--transition-fast), color var(--transition-fast);
}

.menu-item:hover {
  color: var(--text-primary);
  background-color: var(--fill-2);
}

.menu-item:active {
  background-color: var(--fill-3);
}

/* 破坏性操作：只染文字，不做实心红块（macOS 菜单的语义色用法） */
.menu-item.danger,
.menu-item.danger:hover {
  color: rgb(var(--danger-6));
}

.menu-sep {
  height: 1px;
  margin: 4px 6px;
  background-color: var(--border-light);
}

/* 徽章：统一成半透明小丸，只有首个（类型）用主色 */
.page :deep(.arco-tag) {
  border: none;
  border-radius: 6px;
  background-color: var(--fill-1);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
}

/* ---------- 统计条 ---------- */

.mem-stats {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 18px;
  padding: 10px 4px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  flex-wrap: wrap;
}

.stat b {
  color: var(--text-primary);
  font-weight: 600;
  margin-right: 2px;
}

.stat.warn b {
  color: rgb(var(--warning-6));
}

.stat-path {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: auto;
  padding: 2px 8px;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  background-color: var(--fill-1);
  max-width: 420px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: monospace;
  font-size: var(--font-size-xs);
  color: inherit;
  transition: background-color var(--transition-fast), color var(--transition-fast);
}

.stat-path:hover {
  color: var(--text-secondary);
  background-color: var(--fill-2);
}

/* ---------- 进度 ---------- */

.mem-progress {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 6px 4px 10px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.prog-stages {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}

.prog-stage {
  padding: 2px 8px;
  border-radius: 999px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  background-color: var(--fill-1);
}

.prog-stage.active {
  color: #fff;
  background-color: var(--accent-text);
}

.prog-stage.done {
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 15%, transparent);
}

.prog-bar {
  flex: 1;
  min-width: 120px;
}

.prog-msg {
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.prog-count {
  font-family: monospace;
  flex-shrink: 0;
}

/* ---------- 结果摘要 ---------- */

.mem-report {
  flex-shrink: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 14px;
  padding: 6px 4px 10px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.rep-item.ok {
  color: rgb(var(--success-6));
}

.rep-item.warn {
  color: rgb(var(--warning-6));
  cursor: help;
}

.rep-item.err {
  color: rgb(var(--danger-6));
  cursor: help;
}

.warn-list {
  max-width: 520px;
  max-height: 260px;
  overflow: auto;
  font-size: var(--font-size-xs);
  line-height: 1.7;
}

.warn-list-item {
  font-size: var(--font-size-xs);
  line-height: 1.7;
}

.mem-alert {
  flex-shrink: 0;
  margin-bottom: 8px;
}

/* ---------- 主体 ---------- */

/*
 * 两种形态：
 * - 未选中：列表独占整宽（卡片自动铺成多列），正文面板在 DOM 里但宽度 0、不可点
 * - 选中：列表收成固定栏宽（336px），正文从右侧展开
 * 用**长度到长度**的 width 过渡（`100%` → `336px`）而不是 grid-template-columns：
 * `1fr` 与 `336px` 之间无法插值，换 grid 就没有动画了。
 */
.mem-body {
  flex: 1;
  min-height: 0;
  display: flex;
  padding-top: 8px;
}

.mem-list {
  flex: 0 0 auto;
  width: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
  padding-right: 16px;
  overflow: hidden;
  transition: width var(--dur-mac) var(--ease-mac);
}

.mem-body.has-detail .mem-list {
  width: 336px;
}

.list-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 4px 4px 10px;
  margin-bottom: 10px;
  border-radius: 9px;
  background-color: var(--fill-1);
}

/* 检索框本体并入工具条填充：去掉 Arco 自带的描边与底色 */
.list-toolbar :deep(.arco-input-wrapper) {
  flex: 1;
  min-width: 0;
  padding: 0;
  border: none;
  background-color: transparent;
  box-shadow: none;
}

.list-toolbar :deep(.arco-input-wrapper:hover),
.list-toolbar :deep(.arco-input-wrapper.arco-input-focus) {
  border: none;
  background-color: transparent;
  box-shadow: none;
}

.list-toolbar :deep(.arco-input) {
  font-size: var(--font-size-sm);
}

.cat-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 0 2px 10px;
  max-height: 96px;
  overflow: auto;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 10px;
  border: none;
  border-radius: 999px;
  background-color: var(--fill-1);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast),
    transform var(--transition-fast);
}

.chip:hover {
  background-color: var(--fill-2);
  color: var(--text-primary);
}

.chip:active {
  transform: scale(0.97);
}

/* 选中 = 主色填充（macOS 列表/分段控件的选法），不再用边框或下划线 */
.chip.active,
.chip.active:hover {
  background-color: var(--accent-text);
  color: #fff;
}

.chip-n {
  opacity: 0.7;
}

/*
 * 卡片区：自动多列。栏宽固定后（336px - 16px 内边距 = 320px 内容宽）
 * `minmax(320px, 1fr)` 自然退化成单列，无需再加形态判断。
 */
.list-scroll {
  flex: 1;
  overflow: auto;
  min-height: 0;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 12px;
  align-content: start;
}

.list-spin,
.list-empty {
  grid-column: 1 / -1;
}

.list-spin {
  display: block;
  margin: 24px auto;
}

.list-empty {
  padding: 32px 8px;
  text-align: center;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

.list-empty-desc {
  display: block;
  margin-top: 6px;
  font-size: var(--font-size-xs);
}

.list-empty-reset {
  display: inline-block;
  margin-top: 10px;
  padding: 3px 10px;
  border: none;
  border-radius: 999px;
  cursor: pointer;
  font-size: var(--font-size-xs);
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 15%, transparent);
  transition:
    background-color var(--transition-fast),
    transform var(--transition-fast);
}

.list-empty-reset:hover {
  background-color: color-mix(in srgb, var(--accent-text) 24%, transparent);
}

.list-empty-reset:active {
  transform: scale(0.97);
}

.concept-item {
  padding: 14px 16px;
  border-radius: 12px;
  cursor: pointer;
  background-color: var(--fill-1);
  transition:
    background-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

.concept-item:hover {
  background-color: var(--fill-2);
}

.concept-item.active,
.concept-item.active:hover {
  background-color: color-mix(in srgb, var(--accent-text) 15%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-text) 32%, transparent);
}

.ci-head {
  display: flex;
  align-items: baseline;
  gap: 6px;
}

.ci-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ci-score {
  margin-left: auto;
  font-size: var(--font-size-2xs);
  font-family: monospace;
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.ci-meta {
  display: flex;
  gap: 8px;
  margin-top: 5px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.ci-type {
  color: var(--accent-text);
}

.ci-desc {
  margin-top: 6px;
  font-size: var(--font-size-xs);
  line-height: 1.6;
  color: var(--text-secondary);
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* 检索命中：与「会话搜索」同一套高亮（主色描底，明暗都跟着主色走） */
.hl {
  padding: 0 1px;
  border-radius: 3px;
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 18%, transparent);
}

.list-foot {
  flex-shrink: 0;
  padding-top: 6px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  border-top: 1px solid var(--border-light);
}

/* ---------- 详情 ---------- */

.mem-detail {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  min-height: 0;
  opacity: 0;
  transform: translateX(10px);
  overflow: hidden;
  pointer-events: none;
  transition:
    opacity var(--dur-mac) var(--ease-mac),
    transform var(--dur-mac) var(--ease-mac);
}

.mem-body.has-detail .mem-detail {
  opacity: 1;
  transform: none;
  pointer-events: auto;
  padding-left: 18px;
  border-left: 1px solid var(--border-light);
}

.detail-loading,
.detail-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

.empty-icon {
  opacity: 0.35;
}

/* 详情头：内边距与分隔线由元数据块自带（与文件预览同一套），这里只保证不被正文压扁 */
.detail-head {
  flex-shrink: 0;
}

/* 收起正文（回到列表独占整宽的浏览态）：macOS 惯用的圆形裸按钮 */
.dh-close {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 50%;
  cursor: pointer;
  color: var(--text-tertiary);
  background-color: var(--fill-1);
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast),
    transform var(--transition-fast);
}

.dh-close:hover {
  color: var(--text-primary);
  background-color: var(--fill-2);
}

.dh-close:active {
  background-color: var(--fill-3);
  transform: scale(0.94);
}

.dh-id {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-top: 4px;
  font-family: monospace;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  cursor: pointer;
}

.dh-id:hover {
  color: var(--accent-text);
}

/* 底稿里的锚点高亮（跳转定位的落点） */
.raw-pre :deep(.raw-mark) {
  padding: 0 2px;
  border-radius: 3px;
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 24%, transparent);
}

.detail-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 4px 0 24px;
}

/* 预览纯阅读:高度随内容,滚动交给外层 */
.detail-body :deep(.md-editor) {
  box-shadow: none;
  background: transparent;
}

.detail-body :deep(.md-editor-preview-wrapper) {
  padding: 0;
}

/* ---------- 抽屉 ---------- */

/* 分段控件（抽屉标题里的页签切换）：半透明轨道 + 选中段抬起的填充 */
.seg {
  display: inline-flex;
  gap: 2px;
  padding: 2px;
  border-radius: 8px;
  background-color: var(--fill-1);
}

.seg-item {
  padding: 3px 10px;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  font-family: inherit;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  background-color: transparent;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.seg-item:hover {
  color: var(--text-primary);
}

.seg-item.active,
.seg-item.active:hover {
  color: var(--text-primary);
  background-color: var(--fill-3);
}

.log-list,
.raw-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.log-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 5px 6px;
  border-radius: 6px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.log-row:hover {
  background-color: var(--bg-2);
}

.log-action {
  flex-shrink: 0;
  width: 42px;
  font-weight: 600;
}

.log-action.create {
  color: rgb(var(--success-6));
}

.log-action.update {
  color: var(--accent-text);
}

.log-action.skip {
  color: var(--text-tertiary);
}

.log-id {
  flex: 1;
  min-width: 0;
  font-family: monospace;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.log-time {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.raw-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 8px;
  border-radius: 6px;
  cursor: pointer;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.raw-row:hover {
  background-color: var(--bg-2);
  color: var(--text-primary);
}

.raw-name {
  flex: 1;
  font-family: monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.raw-size,
.raw-time {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.raw-pre {
  margin: 0;
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  line-height: 1.7;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text-secondary);
}
</style>
