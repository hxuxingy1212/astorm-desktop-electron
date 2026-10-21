<script setup lang="ts">
/**
 * 会话轨迹页签：按**会话 jsonl**（`chat:list-messages`）展示当前会话的完整消息轨迹（只读）。
 *
 * 分段：活动日志形如 `[系统提示词?, 会话摘要?, 尾部消息…]`——
 * - 顶部先把「系统提示词」与「历史摘要」作为**分段**展示（有才显示，可折叠）；
 * - 其下是按 `seq` 排列的轨迹时间线（用户 / 助手 / 工具）。
 *
 * 数据源：直接读原始记录（而非对话区的映射结果），以保留摘要标记 / 工具调用 / 思考链等原始信息。
 * 切换会话自动重载；一轮结束后自动刷新（流式进行中不刷新，避免读到半截）。
 */
import { computed, onMounted, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Markdown } from 'vue-stream-markdown'
import { markdownNodeRenderers } from '@/components/markdown/renderers'
import MentionText from '@/components/mentions/MentionText.vue'
import ToolCallList from '@/components/chat/ToolCallList.vue'
import ThinkCard from '@/components/chat/ThinkCard.vue'
import { useSessionsStore } from '@/stores/sessions'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import type { ToolCallView } from '@/types'
import { localizedSessionTitle } from '@/utils/sessionTitle'

/** 后端消息记录（chat:list-messages 返回；字段 camelCase） */
interface TraceRecord {
  id?: string
  role?: string
  content?: string
  toolCalls?: unknown
  seq?: number
  createdAt?: number
  reasoningContent?: string | null
  toolCallId?: string | null
  metadata?: Record<string, unknown> | null
}

interface TraceItem {
  key: string
  role: string
  seq?: number
  createdAt: number
  content: string
  reasoning: string
  /** 工具调用（用对话区同款 ToolCallList 渲染） */
  tools: ToolCallView[]
  error?: boolean
  /** 原始消息元数据（落盘 JSON；可折叠展示） */
  metadata?: Record<string, unknown> | null
}

const { t } = useI18n()
const sessions = useSessionsStore()
const settings = useSettingsStore()


const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

const activeSession = computed(
  () => sessions.sessions.find((s) => s.id === sessions.activeId) ?? null
)
const streaming = computed(() => sessions.streamingIds.includes(sessions.activeId))

const loading = ref(false)
const error = ref('')
const records = ref<TraceRecord[]>([])
/** 系统提示词（按当前配置实时拼装，不落盘；jsonl 里通常没有，故单独取） */
const systemPrompt = ref('')

// 折叠状态（按 key 记忆）
const summaryOpen = ref(true)
const systemOpen = ref(false)
/** 各条消息「元数据」展开态（按条目 key 记忆） */
const openMeta = reactive(new Set<string>())

function toggleMeta(key: string): void {
  if (openMeta.has(key)) openMeta.delete(key)
  else openMeta.add(key)
}

/** 元数据是否可展示（非空对象） */
function hasMeta(m: Record<string, unknown> | null | undefined): boolean {
  return !!m && Object.keys(m).length > 0
}

function formatMeta(m: unknown): string {
  try {
    return JSON.stringify(m, null, 2)
  } catch {
    return String(m)
  }
}

const systemRecords = computed(() => records.value.filter((r) => r.role === 'system'))
const summaryRecords = computed(() => records.value.filter((r) => r.metadata?.summary === true))

/** 系统提示词文本:优先用实时拼装的（jsonl 落盘副本可能过期/缺失），否则回退活动日志里的 system 记录 */
const systemText = computed(
  () => systemPrompt.value || systemRecords.value.map((r) => r.content).join('\n\n')
)
const hasSystem = computed(() => !!systemText.value.trim())

/**
 * 轨迹（去掉 system 与摘要记录——它们已作为顶部分段展示）。
 * 单次遍历：assistant 的工具调用登记到 `tool_call_id → 调用对象`，随后到达的 tool 记录
 * 折叠进对应调用（行数精简，且结果与调用一一对应）；无归属的 tool 记录作为独立行展示。
 */
const items = computed<TraceItem[]>(() => {
  const rows = records.value.filter((r) => r.role !== 'system' && r.metadata?.summary !== true)
  const sorted = [...rows].sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))
  const out: TraceItem[] = []
  const callById = new Map<string, ToolCallView>()
  for (const r of sorted) {
    if (r.role === 'tool') {
      const call = r.toolCallId ? callById.get(r.toolCallId) : undefined
      if (call) {
        call.result = r.content ?? ''
        call.status = r.metadata?.status === 'error' ? 'error' : 'completed'
        continue
      }
      out.push({
        key: r.id ?? `seq-${r.seq}`,
        role: 'tool',
        seq: r.seq,
        createdAt: (r.createdAt ?? 0) * 1000,
        content: r.content ?? '',
        reasoning: '',
        tools: [],
        error: r.metadata?.status === 'error',
        metadata: r.metadata ?? null
      })
      continue
    }
    const tools = parseToolCalls(r.toolCalls)
    out.push({
      key: r.id ?? `seq-${r.seq}`,
      role: r.role ?? 'assistant',
      seq: r.seq,
      createdAt: (r.createdAt ?? 0) * 1000,
      content: r.content ?? '',
      reasoning: r.reasoningContent ?? '',
      tools,
      error: r.metadata?.status === 'error',
      metadata: r.metadata ?? null
    })
    for (const tc of tools) if (tc.id) callById.set(tc.id, tc)
  }
  return out
})

function parseToolCalls(raw: unknown): ToolCallView[] {
  if (!Array.isArray(raw)) return []
  return raw.map((tc) => {
    const c = tc as { id?: string; function?: { name?: string; arguments?: string } }
    const args = c.function?.arguments ?? ''
    let params: unknown = args
    try {
      params = JSON.parse(args)
    } catch {
      /* 非 JSON 参数原样保留 */
    }
    return { id: c.id ?? '', name: c.function?.name ?? 'unknown', params, status: 'pending' as const }
  })
}

function formatTime(ts: number): string {
  if (!ts) return ''
  return new Date(ts).toLocaleTimeString('zh-CN', { hour12: false })
}

function roleLabel(role: string): string {
  if (role === 'user') return t('panel.sessionTrace.roleUser')
  if (role === 'tool') return t('panel.sessionTrace.roleTool')
  if (role === 'system') return t('panel.sessionTrace.roleSystem')
  return t('panel.sessionTrace.roleAssistant')
}

async function load(): Promise<void> {
  const backendId = activeSession.value?.backendId
  if (!backendId || !window.api?.chat?.listMessages) {
    records.value = []
    systemPrompt.value = ''
    return
  }
  loading.value = true
  error.value = ''
  try {
    const list = (await window.api.chat.listMessages(backendId)) as TraceRecord[]
    records.value = [...list].sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))
    // 系统提示词不落盘：按当前配置实时取（失败静默，回退 jsonl 里的 system 记录）
    systemPrompt.value = (await window.api.chat.getSystemPrompt?.(backendId)) ?? ''
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

// 切换会话 → 重载；backendId 晚到（首次消息创建后端会话）也重载
watch(() => activeSession.value?.backendId, () => void load())

// 一轮结束（消息数变化且不在流式）→ 自动刷新；流式中不打扰
let refreshTimer: ReturnType<typeof setTimeout> | null = null
watch(
  () => (sessions.messages[sessions.activeId] ?? []).length,
  () => {
    if (streaming.value) return
    if (refreshTimer) clearTimeout(refreshTimer)
    refreshTimer = setTimeout(() => void load(), 400)
  }
)

onMounted(() => void load())
onBeforeUnmount(() => {
  if (refreshTimer) clearTimeout(refreshTimer)
})
</script>

<template>
  <div class="trace">
    <!-- 头部：标题 + 会话名 + 摘要计数 + 刷新 -->
    <div class="trace-head">
      <div class="trace-title-wrap">
        <span class="trace-title">{{ t('panel.sessionTrace.title') }}</span>
        <span v-if="activeSession" class="trace-session" :title="localizedSessionTitle(activeSession)">
          {{ localizedSessionTitle(activeSession) || t('panel.sessionTrace.untitled') }}
        </span>
      </div>
      <span v-if="summaryRecords.length" class="trace-badge">
        <icon-history :size="12" />
        {{ t('panel.sessionTrace.summaryCount', { count: summaryRecords.length }) }}
      </span>
      <a-tooltip :content="t('panel.sessionTrace.refresh')">
        <button type="button" class="trace-btn" :class="{ spinning: loading }" @click="load">
          <icon-refresh :size="15" />
        </button>
      </a-tooltip>
    </div>

    <div class="trace-body">
      <!-- 无会话 -->
      <div v-if="!activeSession" class="trace-empty">{{ t('panel.sessionTrace.noSession') }}</div>
      <template v-else>
        <!-- 系统提示词分段（实时拼装，不落盘） -->
        <section v-if="hasSystem" class="trace-seg">
          <button type="button" class="trace-seg-head" @click="systemOpen = !systemOpen">
            <span class="trace-seg-arrow">{{ systemOpen ? '▾' : '▸' }}</span>
            <icon-settings :size="13" />
            <span class="trace-tag tag-system">{{ t('panel.sessionTrace.roleSystem') }}</span>
            <span class="trace-seg-title">{{ t('panel.sessionTrace.systemPrompt') }}</span>
          </button>
          <pre v-if="systemOpen" class="trace-pre">{{ systemText }}</pre>
        </section>

        <!-- 历史摘要分段（顶部） -->
        <section v-if="summaryRecords.length" class="trace-summary">
          <button type="button" class="trace-summary-head" @click="summaryOpen = !summaryOpen">
            <span class="trace-seg-arrow">{{ summaryOpen ? '▾' : '▸' }}</span>
            <icon-history :size="13" />
            <span class="trace-summary-title">{{ t('panel.sessionTrace.summary') }}</span>
          </button>
          <div v-if="summaryOpen" class="trace-summary-body">
            <Markdown
              :content="summaryRecords.map((r) => r.content).join('\n\n')"
              :is-dark="isDark"
              mode="static"
              :enable-animate="false"
              :node-renderers="markdownNodeRenderers"
            />
          </div>
        </section>

        <!-- 轨迹时间线 -->
        <div class="trace-divider">
          <span>{{ t('panel.sessionTrace.trajectory', { count: items.length }) }}</span>
        </div>

        <div v-if="error" class="trace-empty trace-err">{{ error }}</div>
        <div v-else-if="!items.length && !loading" class="trace-empty">
          {{ t('panel.sessionTrace.empty') }}
        </div>

        <ol v-else class="trace-list">
          <li v-for="it in items" :key="it.key" class="trace-item" :class="`role-${it.role}`">
            <span class="trace-node" />
            <div class="trace-card">
              <div class="trace-card-head">
                <span class="trace-tag" :class="`tag-${it.role}`">{{ roleLabel(it.role) }}</span>
                <span v-if="it.seq != null" class="trace-seq">#{{ it.seq }}</span>
                <span class="trace-time">{{ formatTime(it.createdAt) }}</span>
                <span v-if="it.tools.length" class="trace-meta">
                  {{ t('panel.sessionTrace.toolCount', { count: it.tools.length }) }}
                </span>
                <span v-if="it.error" class="trace-meta trace-meta-err">
                  {{ t('panel.sessionTrace.error') }}
                </span>
              </div>

              <!-- 思考链:与对话区同款卡片 -->
              <ThinkCard v-if="it.reasoning" :content="it.reasoning" :is-dark="isDark" />

              <!-- 工具执行:与对话区同款(组折叠 + Arguments/Result),读历史时失败也展示结果 -->
              <ToolCallList v-if="it.tools.length" :tools="it.tools" show-error-result />

              <!-- 元数据:折叠样式对齐 think / 工具执行 -->
              <div v-if="hasMeta(it.metadata)" class="trace-meta-block">
                <button type="button" class="trace-meta-toggle" @click="toggleMeta(it.key)">
                  <icon-data :size="13" />
                  <span class="trace-meta-label">{{ t('panel.sessionTrace.metadata') }}</span>
                  <span class="trace-meta-arrow">{{ openMeta.has(it.key) ? '▼' : '▶' }}</span>
                </button>
                <pre v-if="openMeta.has(it.key)" class="trace-pre">{{ formatMeta(it.metadata) }}</pre>
              </div>

              <!-- 正文放最下方;无正文则整块不渲染(不留空白)。
                   工具记录与助手正文都用灰底纯文本(不做 markdown);用户消息保留提及渲染 -->
              <pre
                v-if="it.content && (it.role === 'tool' || it.role === 'assistant')"
                class="trace-pre"
                >{{ it.content }}</pre
              >
              <div v-else-if="it.content" class="trace-content">
                <MentionText :text="it.content" />
              </div>
            </div>
          </li>
        </ol>
      </template>
    </div>
  </div>
</template>

<style scoped>
.trace {
  display: flex;
  height: 100%;
  flex-direction: column;
  min-height: 0;
}

/* 头部 */
.trace-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  padding: 5px 14px;
  border-bottom: 1px solid var(--border-light);
}
.trace-title-wrap {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}
.trace-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
}
.trace-session {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 40vw;
}
.trace-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: auto;
  padding: 2px 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--accent-text) 12%, transparent);
  color: var(--accent-text);
  font-size: var(--font-size-2xs);
  white-space: nowrap;
}
.trace-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition: background-color 0.15s, color 0.15s;
}
.trace-btn:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}
.trace-btn.spinning svg {
  animation: trace-spin 0.9s linear infinite;
}
@keyframes trace-spin {
  to {
    transform: rotate(360deg);
  }
}

/* 主体滚动区 */
.trace-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px 24px;
}

/* 顶部分段：系统提示词 */
.trace-seg {
  margin-bottom: 10px;
  border: 1px solid var(--border-light);
  border-radius: 10px;
  overflow: hidden;
  background: var(--bg-1);
}
.trace-seg-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 8px 10px;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
}
.trace-seg-head:hover {
  background: var(--bg-2);
}
.trace-seg-arrow {
  color: var(--text-tertiary);
  font-size: var(--font-size-2xs);
}
.trace-seg-title {
  font-weight: 500;
  color: var(--text-primary);
}
.trace-seg-count {
  margin-left: 2px;
  color: var(--text-tertiary);
}

/* 顶部历史摘要分段 */
.trace-summary {
  margin-bottom: 12px;
  border: 1px solid color-mix(in srgb, var(--accent-text) 30%, var(--border-light));
  border-left: 3px solid var(--accent-text);
  border-radius: 10px;
  overflow: hidden;
  background: color-mix(in srgb, var(--accent-text) 5%, var(--bg-1));
}
.trace-summary-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 8px 10px;
  border: none;
  background: transparent;
  cursor: pointer;
  color: var(--accent-text);
  font-size: var(--font-size-xs);
}
.trace-summary-title {
  font-weight: 600;
}
.trace-summary-body {
  padding: 0 12px 10px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

/* 分界说明 */
.trace-divider {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 14px 2px 10px;
  color: var(--text-tertiary);
  font-size: var(--font-size-2xs);
}
.trace-divider::after {
  content: '';
  flex: 1;
  height: 1px;
  background: var(--border-light);
}

/* 时间线 */
.trace-list {
  margin: 0;
  padding: 0;
  list-style: none;
}
.trace-item {
  position: relative;
  padding-left: 18px;
}
.trace-item::before {
  content: '';
  position: absolute;
  left: 4px;
  top: 14px;
  bottom: -6px;
  width: 1px;
  background: var(--border-light);
}
.trace-item:last-child::before {
  display: none;
}
.trace-node {
  position: absolute;
  left: 0;
  top: 8px;
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--text-tertiary);
}
.trace-item.role-user .trace-node {
  background: var(--accent-text);
}
.trace-item.role-assistant .trace-node {
  background: var(--success);
}
.trace-item.role-tool .trace-node {
  background: var(--text-tertiary);
}

.trace-card {
  margin-bottom: 10px;
  border: 1px solid var(--border-light);
  border-radius: 10px;
  background: var(--bg-1);
  padding: 8px 10px;
}
.trace-card-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
/* 角色 tag:系统/用户/助手/工具 各一色 */
.trace-tag {
  display: inline-flex;
  align-items: center;
  height: 18px;
  padding: 0 7px;
  border-radius: 5px;
  font-size: var(--font-size-2xs);
  font-weight: 600;
  line-height: 1;
  white-space: nowrap;
}
.tag-system {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 14%, transparent);
}
.tag-user {
  color: var(--accent-text);
  background: color-mix(in srgb, var(--accent-text) 14%, transparent);
}
.tag-assistant {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 14%, transparent);
}
.tag-tool {
  color: var(--text-secondary);
  background: var(--bg-2);
}
.trace-seq {
  font-variant-numeric: tabular-nums;
}
.trace-meta {
  margin-left: auto;
  padding: 1px 6px;
  border-radius: 999px;
  background: var(--bg-2);
}
.trace-meta-err {
  margin-left: 6px;
  background: color-mix(in srgb, var(--color-danger-6, #f53f3f) 12%, transparent);
  color: var(--color-danger-6, #f53f3f);
}

.trace-content {
  margin-top: 6px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

/* 元数据折叠:样式对齐 think / 工具执行(图标 + 标题 + 右侧箭头 → 灰底等宽正文) */
.trace-meta-block {
  margin-top: 6px;
}
.trace-meta-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 2px 0;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  font-weight: 500;
  cursor: pointer;
  user-select: none;
}
.trace-meta-toggle:hover {
  color: var(--text-primary);
}
.trace-meta-label {
  line-height: 1;
}
.trace-meta-arrow {
  margin-left: auto;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.trace-pre {
  margin: 6px 0 0;
  padding: 8px 10px;
  border-radius: 8px;
  background: var(--bg-2);
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 320px;
  overflow: auto;
}

.trace-empty {
  padding: 40px 12px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}
.trace-err {
  color: var(--color-danger-6, #f53f3f);
}
</style>
