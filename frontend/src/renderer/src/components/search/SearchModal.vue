<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { useSessionsStore } from '@/stores/sessions'
import { useUiStore } from '@/stores/ui'
import { relativeTime } from '@/utils/time'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import type { ChatMessage, Session } from '@/types'

const { t } = useI18n()
const router = useRouter()
const sessions = useSessionsStore()
const ui = useUiStore()

const keyword = ref('')
const inputRef = ref<HTMLInputElement | null>(null)

/** 高亮片段：命中处打标（模板里渲染成 <mark> 观感的 span，不用 v-html） */
interface Seg {
  text: string
  hit: boolean
}
function segments(text: string, k: string): Seg[] {
  if (!k) return [{ text, hit: false }]
  const lower = text.toLowerCase()
  const out: Seg[] = []
  let i = 0
  for (;;) {
    const at = lower.indexOf(k, i)
    if (at < 0) {
      out.push({ text: text.slice(i), hit: false })
      break
    }
    if (at > i) out.push({ text: text.slice(i, at), hit: false })
    out.push({ text: text.slice(at, at + k.length), hit: true })
    i = at + k.length
  }
  return out.filter((s) => s.text)
}

/** 该会话的用户消息（正文检索与展示都只用用户消息） */
function userMessages(sessionId: string): ChatMessage[] {
  return (sessions.messages[sessionId] ?? []).filter((m) => m.role === 'user')
}

/** 展示片段：命中则截取关键词附近，未命中则取开头一段 */
function snippetOf(text: string, k: string): string {
  const flat = text.replace(/\s+/g, ' ').trim()
  const at = flat.toLowerCase().indexOf(k)
  if (at < 0) return flat.slice(0, 80)
  const start = Math.max(0, at - 24)
  return (start > 0 ? '…' : '') + flat.slice(start, at + k.length + 56)
}

/** 定时任务 / 子任务会话不进搜索：前者是 cron 触发产物（入口在「定时任务」页，
 *  与侧栏同一口径，见 AppSidebar.rootTasks），后者是 sub_session 工具的执行痕迹
 *  （归属父任务，与看板口径一致，见 TaskBoardView.staged） */
function searchable(s: Session): boolean {
  const t = s.sessionType ?? 'normal'
  return t !== 'scheduled' && t !== 'child'
}

/**
 * 结果 = 会话名命中，或该会话任一**用户消息**正文命中。
 * 每行都带一条用户消息（命中那条；没命中就取最近一条），并把关键词高亮。
 */
const results = computed(() => {
  const k = keyword.value.trim().toLowerCase()
  if (!k) return []
  return sessions.sessions
    .filter(searchable)
    .map((session) => {
      const users = userMessages(session.id)
      const hit = users.find((m) => (m.content ?? '').toLowerCase().includes(k)) ?? null
      const titleText = localizedSessionTitle(session)
      const titleHit = titleText.toLowerCase().includes(k)
      if (!titleHit && !hit) return null
      // 标题命中时正文没有关键词：退而展示最近一条用户消息（无高亮），
      // 让每行都能看出「这个会话在聊什么」
      const shown = hit ?? users[users.length - 1] ?? null
      return {
        session,
        title: segments(titleText, k),
        message: shown ? segments(snippetOf(shown.content ?? '', k), k) : null
      }
    })
    .filter((r): r is NonNullable<typeof r> => r !== null)
})

// ── 正文检索的前置：内存里没有历史的会话搜不到用户消息 ──
// 关键词一到就按最近活动顺序补拉（loadHistory 自带 historyLoaded 去重与流式推迟），
// 并发 3、单次打开封顶（几十条会话全量拉会把本地 IPC 和内存顶起来；超出封顶的
// 只是本次不参与正文检索，标题检索不受影响）——与看板卡片正文懒加载同一套取舍。
const SEARCH_LOAD_MAX = 60
const SEARCH_LOAD_CONCURRENCY = 3
const loadingMore = ref(false)
let loadBusy = false
let loadTimer: number | undefined

async function loadCandidates(): Promise<void> {
  if (loadBusy) return
  loadBusy = true
  loadingMore.value = true
  try {
    const todo = [...sessions.sessions]
      .filter((s) => searchable(s) && !sessions.historyLoaded[s.id])
      .sort((a, b) => (b.updatedAt ?? b.createdAt) - (a.updatedAt ?? a.createdAt))
      .slice(0, SEARCH_LOAD_MAX)
      .map((s) => s.id)
    for (let i = 0; i < todo.length; i += SEARCH_LOAD_CONCURRENCY) {
      if (!ui.searchVisible) return
      await Promise.all(
        todo.slice(i, i + SEARCH_LOAD_CONCURRENCY).map((id) => sessions.loadHistory(id).catch(() => {}))
      )
    }
  } finally {
    loadBusy = false
    loadingMore.value = false
  }
}

/** 任务类型标签：定时任务｜普通任务｜项目任务｜子任务（与看板筛选同一套文案） */
function typeLabel(session: Session): string {
  return t(`board.filter.${session.sessionType ?? 'normal'}`)
}

function onPick(id: string): void {
  sessions.setActive(id)
  // 跳到该会话的内容页：搜索入口可能停在任意页面（看板/定时任务等），只 setActive 不换页
  // 时视图不会跟着走；与侧栏点击会话项同一目标路由（/chat/:id，本地 id）
  if (router.currentRoute.value.params.id !== id) void router.push(`/chat/${id}`)
  ui.closeSearch()
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape') ui.closeSearch()
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))

watch(
  () => ui.searchVisible,
  async (visible) => {
    if (visible) {
      keyword.value = ''
      await nextTick()
      inputRef.value?.focus()
    }
  }
)

// 开始输入后再补历史（打开就拉会让没打算搜的模态白读一遍磁盘），轻微防抖
watch(keyword, (k) => {
  window.clearTimeout(loadTimer)
  if (!ui.searchVisible || !k.trim()) return
  loadTimer = window.setTimeout(() => void loadCandidates(), 200)
})

onBeforeUnmount(() => window.clearTimeout(loadTimer))
</script>

<template>
  <teleport to="body">
    <div v-if="ui.searchVisible" class="search-overlay" @click.self="ui.closeSearch()">
      <div class="search-card">
        <!-- macOS 搜索框：无边框大字号输入 + 底部发丝线，左置放大镜 -->
        <div class="search-field">
          <icon-search class="search-field-icon" :size="20" />
          <input
            ref="inputRef"
            v-model="keyword"
            class="search-input"
            type="text"
            spellcheck="false"
            :placeholder="t('sidebar.searchPlaceholder')"
            @keydown.enter="results[0] && onPick(results[0].session.id)"
          />
          <button
            v-if="keyword"
            class="search-clear"
            type="button"
            :aria-label="t('common.clear')"
            @click="keyword = ''"
          >
            <icon-close :size="14" />
          </button>
        </div>

        <div class="search-results-area">
          <div v-if="!keyword.trim()" class="search-hint">{{ t('sidebar.searchHint') }}</div>
          <div v-else-if="!results.length" class="search-hint">{{ t('sidebar.searchEmpty') }}</div>
          <template v-else>
            <div
              v-for="r in results"
              :key="r.session.id"
              class="search-result-item"
              @click="onPick(r.session.id)"
            >
              <!-- 标题 + 「类型 · 时间」同一行：时间固定局右 -->
              <div class="search-result-row">
                <div class="search-result-title">
                  <span
                    v-for="(seg, i) in r.title"
                    :key="i"
                    :class="{ 'search-hl': seg.hit }"
                  >{{ seg.text }}</span>
                </div>
                <div class="search-result-meta">
                  {{ typeLabel(r.session) }} ·
                  {{ relativeTime(r.session.updatedAt ?? r.session.createdAt) }}
                </div>
              </div>
              <!-- 用户消息：命中那条（关键词高亮）；标题单独命中时退回最近一条 -->
              <div v-if="r.message" class="search-result-message">
                <span
                  v-for="(seg, i) in r.message"
                  :key="i"
                  :class="{ 'search-hl': seg.hit }"
                >{{ seg.text }}</span>
              </div>
            </div>
          </template>
          <!-- 正文检索要先补历史：还在补时提示一下，避免「结果一条条冒出来」显得像 bug -->
          <div v-if="loadingMore" class="search-loading">{{ t('sidebar.searchLoading') }}</div>
        </div>
      </div>
    </div>
  </teleport>
</template>

<style scoped>
/* 遮罩：与其它弹窗同一套蒙层（Arco --color-mask-bg），保证「弹窗」而非「新页面」的观感 */
.search-overlay {
  position: fixed;
  inset: 0;
  z-index: 999;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 16vh;
  background-color: var(--color-mask-bg, rgba(29, 33, 41, 0.6));
  backdrop-filter: blur(8px);
}

/* 弹窗本体：一张浮起的卡片（圆角 + 描边 + 投影），不铺满视口 */
.search-card {
  width: 580px;
  max-width: calc(100vw - 48px);
  overflow: hidden;
  border: 1px solid var(--panel-border);
  border-radius: 14px;
  background-color: var(--panel-bg);
  box-shadow: var(--shadow-lg);
}

.search-field {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 13px 16px;
  border-bottom: 1px solid var(--border-base);
  background-color: var(--panel-bg);
}

.search-field-icon {
  flex: none;
  color: var(--text-tertiary);
}

.search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  font-family: inherit;
  font-size: var(--font-size-xl);
  line-height: 1.35;
  color: var(--text-primary);
}

.search-input::placeholder {
  color: var(--text-tertiary);
}

.search-clear {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 50%;
  cursor: pointer;
  color: var(--text-tertiary);
  background-color: color-mix(in srgb, var(--text-tertiary) 22%, transparent);
  transition: background-color var(--transition-fast);
}

.search-clear:hover {
  color: var(--text-primary);
  background-color: color-mix(in srgb, var(--text-tertiary) 38%, transparent);
}

/* 结果区：卡片内的一块浅灰内衬（发丝线之下的第二层），行高亮仍在白底上 */
.search-results-area {
  max-height: 46vh;
  padding: 6px;
  overflow-y: auto;
  background-color: color-mix(in srgb, var(--bg-2) 70%, transparent);
}

.search-hint {
  padding: 24px 0;
  text-align: center;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

.search-result-item {
  padding: 9px 12px;
  border-radius: 8px;
  cursor: pointer;
  transition: background-color var(--transition-fast);
}

.search-result-item:hover {
  background-color: var(--bg-hover);
}

/* 首行：标题占满，「类型 · 时间」固定局右（基线对齐） */
.search-result-row {
  display: flex;
  align-items: baseline;
  gap: 12px;
}

.search-result-title {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-title);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.search-result-meta {
  flex: none;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

.search-result-message {
  margin-top: 3px;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 关键词高亮：用主色描底，明暗主题都跟着主色走 */
.search-hl {
  padding: 0 1px;
  border-radius: 3px;
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 18%, transparent);
}

/* 补历史过程中的提示行 */
.search-loading {
  padding: 8px 12px 6px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
</style>
