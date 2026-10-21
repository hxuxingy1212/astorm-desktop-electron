<script setup lang="ts">
/**
 * 任务看板：把**全部会话**按后端状态归到四列 —— 待办 / 待处理（审批 + 表单）/ 进行中 / 完成。
 *
 * 数据全部来自 store（`sessionInfo` 由 `session_info` 事件实时刷新，`streamingIds` 提供
 * 0 延迟的「本端正在跑」，冷启动另有 `bootstrapFromBackend` 灌入），所以看板是**活的**：
 * 会话在别处跑起来/停下来，这里当场换列，不需要轮询。
 * 列归属用 `sessions.boardColumnOf`（与侧栏状态点同一条判定链，避免两处说法不一致）。
 *
 * 只读：不做拖拽换列 —— 列归属是后端状态的映射，拖拽没有对应语义。
 * 点卡片 = 打开该会话的聊天页。
 */
import { computed, onActivated, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { useSessionsStore, type BoardColumnKey } from '@/stores/sessions'
import { loadSummaryThreshold } from '@/composables/useContextCapacity'
import BoardCard from '@/components/board/BoardCard.vue'
import type { Session, SessionType } from '@/types'
import { localizedSessionTitle } from '@/utils/sessionTitle'

const { t } = useI18n()
const router = useRouter()
const sessions = useSessionsStore()

/** 四列的展示顺序与列头配色（颜色语义与侧栏状态点一致），恒显四列 */
const COLUMNS: { key: BoardColumnKey; labelKey: string }[] = [
  { key: 'todo', labelKey: 'board.column.todo' },
  { key: 'pending', labelKey: 'board.column.pending' },
  { key: 'running', labelKey: 'board.column.running' },
  { key: 'done', labelKey: 'board.column.done' }
]

/** 任务分类筛选（看板不含子任务：它们是子会话工具的执行痕迹，随父任务走，不单独成卡） */
type BoardFilter = 'all' | Exclude<SessionType, 'child'>
const FILTERS: { key: BoardFilter; labelKey: string }[] = [
  { key: 'all', labelKey: 'board.filter.all' },
  { key: 'scheduled', labelKey: 'board.filter.scheduled' },
  { key: 'normal', labelKey: 'board.filter.normal' },
  { key: 'project', labelKey: 'board.filter.project' }
]
const filter = ref<BoardFilter>('all')

/** 关键词搜索（会话名 + 用户消息，忽略大小写）——与分类筛选叠加生效 */
const searchQuery = ref('')
const hasQuery = computed(() => searchQuery.value.trim().length > 0)

/**
 * 关键词是否命中该会话：**会话名**或任一条**用户消息**正文包含关键词。
 *
 * 只看 user 消息（助手正文通常很长，且用户找的是「我说过什么」）；命中即返回，
 * 不关心命中在标题还是正文 —— 卡片上本来就有正文预览，能看出为什么被搜到。
 * 消息来自内存（`messages[id]`）：看板进入时会懒加载各卡片历史（见 ensurePreviews），
 * 因此通常都有；确实没加载过的会话只能按标题命中。
 */
function matchesQuery(session: Session, q: string): boolean {
  if (!q) return true
  if (localizedSessionTitle(session).toLowerCase().includes(q)) return true
  const list = sessions.messages[session.id] ?? []
  return list.some((msg) => msg.role === 'user' && (msg.content ?? '').toLowerCase().includes(q))
}

/**
 * 会话 → 列（每条只判一次，避免每列各自过滤一遍）。
 * 先剔掉子任务、再按分类筛、按关键词筛（计数与列渲染同口径）；
 * 子任务（`sessionType: 'child'`）在看板不出现——含搜索：它们是 `sub_session`
 * 工具的执行痕迹，内容归属父任务，单独成卡/被搜到都会干扰主任务视野。
 */
const staged = computed(() => {
  const q = searchQuery.value.trim().toLowerCase()
  return sessions.sessions
    .filter((session) => (session.sessionType ?? 'normal') !== 'child')
    .filter((session) => filter.value === 'all' || (session.sessionType ?? 'normal') === filter.value)
    .filter((session) => matchesQuery(session, q))
    .map((session) => ({ session, column: sessions.boardColumnOf(session.id) }))
})

/**
 * 恒显四列；列内按最近活动倒序（刚动过的排前面）。
 */
const columns = computed(() => {
  return COLUMNS.map((col) => ({
    ...col,
    items: staged.value
      .filter((item) => item.column === col.key)
      .map((item) => item.session)
      .sort((a, b) => (b.updatedAt ?? b.createdAt) - (a.updatedAt ?? a.createdAt))
  }))
})

const refreshing = ref(false)

/**
 * 待办列「新建待办」：跳到新建会话页并用 `?todo=1` 把提交档位默认成「添加待办」
 * （不弹独立弹窗——新建表单只有新建会话页一份，避免两处状态漂移）。
 */
function goNewTodo(): void {
  void router.push({ path: '/newChat', query: { todo: '1' } })
}

/**
 * 打开看板时刷新一次：重新拉后端会话列表（含本端列表里还没有的后端子会话/定时任务会话）。
 * `keepActive` 保住当前激活会话——直接复用启动时的 bootstrap 会把 activeId 重置成列表首条，
 * 从看板点回聊天时就会跳到别的会话上。
 */
async function refresh(): Promise<void> {
  refreshing.value = true
  try {
    await sessions.bootstrapFromBackend({ keepActive: true })
  } finally {
    refreshing.value = false
  }
}

/**
 * 每次进入看板重拉一次卡片依赖的全局配置：「上下文容量」分母来自系统设置（摘要阈值）——
 * 全局一份，模块级缓存（幂等）。卡片不再显示名称类标签（智能体/定时任务名），无需查表。
 */
function loadLookups(): void {
  void loadSummaryThreshold()
}

/**
 * 卡片正文懒加载。
 *
 * 卡片正文（用户提问 + 助手回复）来自 `sessions.messages[id]`（内存态）：只有**本次运行打开过**或
 * **后端刚跑过**的会话才有。其余（历史会话）会退回显示状态文案 —— 而完成列的静默状态连文案都
 * 没有，卡片正文直接空壳。于是进入看板时**全量补齐**历史（曾经设过「单次进入封顶 30 条」的
 * 预算，正文成为卡片主体后那会让大部分卡片没有正文）：
 * - 按看板展示顺序入队（列内最近活动优先，可视区的卡片先填上）；
 * - 并发 3 节流，避免一次性全量拉把本地 IPC 顶起来；
 * - 每个会话只拉一次（`loadHistory` 自带去重 `historyLoaded` 与流式推迟；失败不重试）。
 */
const PREVIEW_CONCURRENCY = 3
/** 本轮已发起过的会话（成功与否都算：失败不重试，避免坏会话每帧重拉） */
const previewTried = new Set<string>()
let previewQueue: string[] = []
let previewBusy = false

async function drainPreviews(): Promise<void> {
  if (previewBusy) return
  previewBusy = true
  try {
    while (previewQueue.length) {
      const batch = previewQueue.splice(0, PREVIEW_CONCURRENCY)
      await Promise.all(
        batch.map(async (id) => {
          try {
            await sessions.loadHistory(id)
          } catch {
            /* 单条失败不影响其他卡片 */
          }
        })
      )
    }
  } finally {
    previewBusy = false
  }
}

/** 找出「还没有内存消息」的卡片入队（按看板展示顺序：列内最近活动优先，可视区先填上） */
function ensurePreviews(): void {
  for (const col of columns.value) {
    for (const session of col.items) {
      const id = session.id
      if (previewTried.has(id)) continue
      if (sessions.historyLoaded[id]) continue
      if ((sessions.messages[id] ?? []).length) continue
      // 正在流式的会话由 store 自己推迟到本轮收尾再建历史（loadHistory 内部处理），这里不抢
      previewTried.add(id)
      previewQueue.push(id)
    }
  }
  void drainPreviews()
}

/** 每次进入看板：重置去重表（每条都有机会重试一轮），再全量补一次 */
function enterBoard(): void {
  previewTried.clear()
  ensurePreviews()
}

// 卡片集合变化（新会话出现）时补一次；previewTried 保证同一会话在本次进入内不重复入队
watch(() => staged.value.map((s) => s.session.id).join(','), ensurePreviews)

/** 进入看板的首次挂载：拉两张查表 + 全量对一次会话列表（此后靠事件实时推进）+ 补卡片正文 */
onMounted(() => {
  void loadLookups()
  void refresh()
  enterBoard()
})

/**
 * KeepAlive 下每次**重新进入**看板：查表重拉 + 继续补正文。
 * 首次挂载时 onMounted 与 onActivated 在同一 tick 内先后触发，这里跳过那一次，避免重复请求。
 */
let activatedOnce = false
onActivated(() => {
  if (!activatedOnce) {
    activatedOnce = true
    return
  }
  void loadLookups()
  enterBoard()
})

/** 点卡片：打开该会话（本地会话 id 即路由参数） */
function open(session: Session): void {
  router.push(`/chat/${session.id}`)
}
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div class="header-left">
        <h2 class="page-title">{{ t('board.title') }}</h2>
        <!-- 计数常驻（含 0）：不能按 length 显隐，否则筛到空分类时计数消失、筛选组整体左移 -->
        <span class="page-count">{{ staged.length }}</span>
        <!-- 任务分类筛选：与标题同一行（控件观感与设置页技能分类筛选一致） -->
        <div class="filter-bar">
          <span
            v-for="item in FILTERS"
            :key="item.key"
            class="filter-tag"
            :class="{ active: filter === item.key }"
            @click="filter = item.key"
          >
            {{ t(item.labelKey) }}
          </span>
        </div>
      </div>
      <div class="header-actions">
        <!-- 与设置页技能管理同款：Arco 搜索框（圆角描边 + 搜索图标在右） -->
        <a-input-search
          v-model="searchQuery"
          class="board-search"
          :placeholder="t('board.searchPlaceholder')"
          allow-clear
          size="small"
        />
        <a-tooltip :content="t('common.refresh')">
          <button
            type="button"
            class="board-refresh"
            :class="{ spinning: refreshing }"
            :aria-label="t('common.refresh')"
            @click="refresh"
          >
            <icon-refresh :size="15" />
          </button>
        </a-tooltip>
      </div>
    </div>

    <!-- 四列：宽屏四等分，窄屏横向滚动（列宽上下限见样式） -->
    <div class="board-columns">
      <section v-for="col in columns" :key="col.key" class="board-column">
        <header class="col-head">
          <span class="col-dot" :class="`dot-${col.key}`" />
          <span class="col-name">{{ t(col.labelKey) }}</span>
          <span class="col-count">{{ col.items.length }}</span>
        </header>

        <div class="col-body">
          <!-- 待办列顶部固定入口（样式恒定，不随列空/非空变化；列空时空态砖显示在它下方）。
               点击跳新建会话页，提交档位默认「添加待办」（见 goNewTodo） -->
          <button
            v-if="col.key === 'todo' && !hasQuery"
            type="button"
            class="col-create"
            @click="goNewTodo"
          >
            <icon-plus :size="14" />
            <span>{{ t('board.newTask') }}</span>
          </button>

          <BoardCard
            v-for="session in col.items"
            :key="session.id"
            :session="session"
            @open="open(session)"
            @start="sessions.startParkedSession(session.id)"
          />

          <!-- 空列：图标 + 文字，撑满剩余高度居中（待办列也不例外） -->
          <div v-if="!col.items.length" class="col-empty">
            <icon-empty :size="26" class="col-empty-icon" />
            <span>{{ hasQuery ? t('board.emptySearch') : t('board.empty') }}</span>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
/* 页面容器：与设置/知识库页同一套（占满路由视图，内部自己滚）。
   左右 24px 留白：看板整体与窗口圆角/边缘留出呼吸位（同 KnowledgeView 的 16px 24px 量级，
   不用定时任务页的 56px —— 那会白吃掉 112px 列宽） */
.page {
  height: 100%;
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  /* 列间通道：相邻两列内容相距 16px（8px × 2）。
     它直接决定卡片能有多宽 —— 调大 = 分栏更松、卡片更窄；调小反之。 */
  --board-gutter: 8px;
  padding: 24px 24px 16px;
  overflow: hidden;
}

.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 14px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.page-title {
  margin: 0;
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
}

/* 任务分类筛选：与设置页技能分类筛选（SkillsSettings.vue 的 .filter-bar / .filter-tag）同款观感：
   药丸底 + 选中项浅底 + 主色文字 */
.filter-bar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  padding: 4px 6px;
  margin-left: 6px;
  flex-shrink: 0;
  background: var(--bg-2);
  border-radius: 10px;
  width: fit-content;
  max-width: 100%;
}

.filter-tag {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-sm);
  padding: 3px 12px;
  line-height: 1.4;
  cursor: pointer;
  border-radius: 6px;
  color: var(--text-secondary);
  white-space: nowrap;
  user-select: none;
  transition:
    color var(--transition-fast),
    background-color var(--transition-fast);
}

.filter-tag:hover {
  color: var(--text-primary);
  background: var(--bg-3);
}

.filter-tag.active {
  color: var(--accent-text);
  background: var(--bg-base);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

/* 会话总数（与定时任务页的 .page-count 同款小圆角计数）。
   **固定宽度**：数字位数会随筛选变化（全部 15 / 子任务 1），用 min-width+padding 时药丸宽度
   跟着字符数变，右边那组筛选按钮就会被推来推去。等宽数字（tabular-nums）保证位数不同也居中对齐。 */
.page-count {
  width: 28px;
  height: 20px;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  background: var(--bg-3);
  color: var(--text-tertiary);
  font-size: var(--font-size-2xs);
  font-weight: 500;
  font-variant-numeric: tabular-nums;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

/* 会话名搜索框：去边框款（保留圆角浅底 + 搜索图标在右）。
   注意 class 落在输入框根元素（即 `.arco-input-wrapper`）上，直接覆盖其边框即可；
   用 `.board-search :deep(.arco-input-wrapper)` 是对后代匹配，匹配不到根元素本身，故无效。 */
.board-search {
  width: 240px;
  border-radius: 8px;
  border: none;
  background: var(--bg-2);
}

.board-search:focus-within {
  background: var(--bg-3);
  border: none;
}

.board-refresh {
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.board-refresh:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.board-refresh.spinning :deep(svg) {
  animation: board-spin 0.9s linear infinite;
}

@keyframes board-spin {
  to {
    transform: rotate(360deg);
  }
}

/* 四列：等分铺满整行（无 gap，列间靠 --board-gutter 的留白隔开）
   - 列宽下限 200px：正常窗口（对话区 ≈1000px）下四列必然全部可见，
     只有把对话区拖到 800px 以下（或右侧页签面板占掉大半）才退化为横向滚动；
   - 不设 max-width：宽窗口下四列始终把整行占满，不会右侧留一片空。 */
.board-columns {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 0;
  overflow-x: auto;
  overflow-y: hidden;
  padding-bottom: 4px;
  /* 唯一的反向特例：横向滚动条整体隐藏（横向只能靠滚轮/触控板或 Shift+滚轮）。
     横向槽占的是高度，而 scrollbar-gutter 只管纵向槽，条一出现列高就矮 4px —— 与
     「滚动条必须预留位置」冲突，故此处的选择是彻底不显示，而不是「滚动时出现」。 */
  scrollbar-width: none;
}

.board-columns::-webkit-scrollbar {
  display: none;
}

/* 列：不要边框/底色/分割线，列与列之间只靠留白隔开（列本身仍是一个纵向 flex 容器）。
   注意：水平内边距**不放在列上**，而是下放到列头/列体 —— 列自身 padding 为 0 时四列
   border-box 才完全等宽（否则带 padding 的中间列会比首末列宽十几像素）。 */
.board-column {
  /* 列宽由 flex 分配，内部文本不参与其它列的内在尺寸计算。 */
  contain: inline-size;
  flex: 1 1 0;
  min-width: 200px;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

/* 列头：状态点 + 列名 + 计数（计数与列名同基线）
   不给列头加下边框 —— 四列各画一条横线会把它读成表格表头 */
.col-head {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 0 var(--board-gutter) 10px;
}

/* 首列贴标题左缘、末列贴页面右缘（外侧不留内边距，与页头标题/刷新按钮对齐） */
.board-column:first-child .col-head,
.board-column:first-child .col-body {
  padding-left: 0;
}

.board-column:last-child .col-head,
.board-column:last-child .col-body {
  padding-right: 0;
}

.col-dot {
  width: 8px;
  height: 8px;
  flex: none;
  border-radius: 50%;
  background: var(--text-tertiary);
}

.dot-todo {
  background: var(--text-tertiary);
}

.dot-pending {
  background: var(--warning);
}

.dot-running {
  background: var(--accent-text);
}

.dot-done {
  background: var(--success);
}

.col-name {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.col-count {
  min-width: 18px;
  height: 18px;
  padding: 0 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 9px;
  background: var(--bg-3);
  color: var(--text-tertiary);
  font-size: var(--font-size-2xs);
}

/* 列体：各自独立竖向滚动（列头固定）；水平内边距在这里（见 .board-column 的说明）。
   滚动条走全局那套（4px 细条 / 空闲隐藏 / 滚动时出现 + 预留位）。
   position: relative 作为空态砖（.col-empty）的定位参照 */
.col-body {
  flex: 1;
  min-height: 0;
  position: relative;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 0 var(--board-gutter) 10px;
}

/* 待办列的「新建任务」：虚线框，与卡片区分开 */
.col-create {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 9px 10px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius-lg);
  background: transparent;
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition:
    border-color var(--transition-fast),
    color var(--transition-fast),
    background-color var(--transition-fast);
}

.col-create:hover {
  border-color: var(--accent-text);
  background: var(--bg-hover);
  color: var(--text-primary);
}

/* 空列：图标 + 文字，落在列正中。
   待办列顶部还有「新建任务」按钮 —— 若走常规流（flex:1 占剩余空间），空态中心会被按钮
   顶低半截，与其它列的空态不对齐。改为**绝对定位铺满列体内容盒**（inset 底部 10px =
   列体 padding-bottom，即内容盒下缘），所有列的空态都以同一块区域居中；
   pointer-events 放行：空态不拦截点击，待办列的按钮悬在其下仍可点 */
.col-empty {
  position: absolute;
  inset: 0 0 10px 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  pointer-events: none;
  color: var(--text-tertiary);
  font-size: var(--font-size-xs);
}

.col-empty-icon {
  color: var(--text-tertiary);
}
</style>
