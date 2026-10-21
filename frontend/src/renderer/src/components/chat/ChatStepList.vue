<script setup lang="ts">
/**
 * 对话区「过程收纳块」内的步骤列表：把一轮里的思考与工具调用压成**紧凑轨迹行**
 * （图标 + 类型 · 标题；可展开看完整内容），替代此前逐条渲染的 think 卡 + 工具卡片。
 *
 * 行类型：
 * - 思考：记忆图标 + 「思考」· 推理首行预览；展开看完整推理。
 * - 工具：折叠箭头 + 按工具/动作取图标与类型名 + · 调用摘要；展开看 Arguments / Result。
 * 失败（工具 error / 被拦）标题标红。
 */
import { computed, defineAsyncComponent, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Markdown } from 'vue-stream-markdown'
import { markdownNodeRenderers } from '@/components/markdown/renderers'
import type { ToolCallView } from '@/types'
import type { TurnStep } from './turnGrouping'
import { toolMeta, toolTitle, toolDescription, deliveryStatOf, type DeliveryStat } from './toolDisplay'
import { fileChangeOf, toolCountOf, type FileChange, type ToolCount } from './editChange'
import { collectTurnSources } from './sources'

/** 文件 diff 预览器按需加载：避免把 highlight.js 多语言拉进聊天主包。 */
const CodeDiffView = defineAsyncComponent(() => import('@/panels/CodeDiffView.vue'))

const props = withDefaults(
  defineProps<{
    steps: TurnStep[]
    /** 进行中相位(仅流式中传入):在列表末尾追加一条「正在…」流光行,随事件相位切换 */
    phase?: 'awaiting' | 'thinking' | 'tool' | 'summarizing'
    /** 暗色主题(摘要展开体的 markdown 渲染用) */
    isDark?: boolean
  }>(),
  { isDark: false }
)

const { t } = useI18n()

/** 相位行文案(空 = 不显示;相位随 store 事件推进,见 sessions.setRunPhase) */
const phaseText = computed(() => {
  if (props.phase === 'awaiting') return t('chat.turnPhaseAwaiting')
  if (props.phase === 'thinking') return t('chat.turnPhaseThinking')
  if (props.phase === 'tool') return t('chat.turnPhaseTool')
  if (props.phase === 'summarizing') return t('chat.turnPhaseSummarizing')
  return ''
})

/* ── 流式 think 行的滑动窗口预览 ────────────────────────────────
   限定宽度下 think 首行预览只能露一小截,而推理是持续流入的:
   对**正在流式输出的** think 行,把单行化全文按窗口切片,每 500ms 流光翻页到下一段
   (循环;新内容流入时窗口数自然增加)。结束/非流式的 think 行保持静态首行预览。 */
const THINK_WINDOW = 40
const THINK_ROTATE_MS = 500
const rotTick = ref(0)
let rotTimer: ReturnType<typeof setInterval> | null = null

/** 当前流式 think 行(phase=thinking 时的最后一条 think 行);无 → null */
const liveThinkId = computed(() => {
  if (props.phase !== 'thinking') return null
  for (let i = props.steps.length - 1; i >= 0; i -= 1) {
    const step = props.steps[i]
    // 与 rows 构造同口径(parts 含 reasoning 且推理非空),否则没有对应行可轮播
    if (step.parts.includes('reasoning') && step.msg.reasoning) return `${step.msg.id}:think`
  }
  return null
})

function stopRotate(): void {
  if (rotTimer) {
    clearInterval(rotTimer)
    rotTimer = null
  }
}

watch(
  liveThinkId,
  (id) => {
    stopRotate()
    rotTick.value = 0
    if (id) rotTimer = setInterval(() => (rotTick.value += 1), THINK_ROTATE_MS)
  },
  { immediate: true }
)
onBeforeUnmount(stopRotate)

/** 降低动效偏好:不轮播,直接给最新一段(尾部) */
function reducedMotion(): boolean {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
}

/** 行标题:流式 think 行走轮播窗口,其余行用静态首行预览 */
function displayTitle(row: StepRow): string {
  if (row.kind !== 'think' || row.id !== liveThinkId.value) return row.title
  const text = (row.reasoning ?? row.title).replace(/\s+/g, ' ').trim()
  if (text.length <= THINK_WINDOW || reducedMotion()) return text.slice(-THINK_WINDOW)
  const chunks = Math.ceil(text.length / THINK_WINDOW)
  const start = (rotTick.value % chunks) * THINK_WINDOW
  return text.slice(start, start + THINK_WINDOW)
}


interface StepRow {
  id: string
  kind: 'think' | 'tool' | 'content' | 'summary'
  icon: string
  /** 品牌彩色图标 URL（office 行按格式取 Word/Excel/PPT/PDF，文件树同款字形）；有则取代 icon */
  iconUrl?: string
  /** 文件操作行派生(读取/写入/编辑/追加):文件名/目录/图标/统计/diff;无则普通行 */
  change?: FileChange | null
  /** 目录类结果计数(列目录/查找文件→N个文件,搜索文件内容→N处) */
  stat?: ToolCount | null
  /** 类型名（已本地化） */
  label: string
  /** 摘要（思考=推理首行；工具=调用摘要:关键 action / 重要字段） */
  title: string
  /** 说明（工具 schema 的 `description`：模型填写的本次调用目的；office 缺省回落子命令说法） */
  description?: string
  error: boolean
  /** 展开内容 */
  reasoning?: string
  tool?: ToolCallView
  /** 中间叙述正文（原样渲染，不折叠；summary 行则为折叠在展开体里的摘要正文） */
  content?: string
}

/** 思考预览：推理首行（折叠空白） */
function firstLine(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}

/**
 * 交付上报行的关键词：参数清单计数（交付物 / 修改文件 / 相关记忆 id）+ 本轮相关资料数
 * （web 检索结果汇总，与交付卡片同源），零段省略 —— 如「2 个交付物 · 3 个修改文件 · 5 个相关资料」。
 */
function deliveryTitle(stat: DeliveryStat, sourceCount: number): string {
  const segs = [
    stat.deliverables > 0 ? t('chat.step.deliverableCount', { n: stat.deliverables }) : '',
    stat.changedFiles > 0 ? t('chat.step.changedFileCount', { n: stat.changedFiles }) : '',
    sourceCount > 0 ? t('chat.step.sourceCount', { n: sourceCount }) : '',
    stat.memoryIds > 0 ? t('chat.step.memoryIdCount', { n: stat.memoryIds }) : ''
  ].filter(Boolean)
  return segs.join(' · ')
}

const rows = computed<StepRow[]>(() => {
  const out: StepRow[] = []
  // 本轮相关资料数（web 检索结果，按 url 去重）：供交付上报行的关键词段使用
  const sourceCount = collectTurnSources(props.steps).filter((s) => s.kind === 'web').length
  for (const step of props.steps) {
    const m = step.msg
    if (step.parts.includes('reasoning') && m.reasoning) {
      out.push({
        id: `${m.id}:think`,
        kind: 'think',
        icon: 'icon-think',
        label: t('chat.step.think'),
        title: firstLine(m.reasoning),
        error: false,
        reasoning: m.reasoning
      })
    }
    if (step.parts.includes('tools')) {
      for (const tool of m.tools ?? []) {
        const meta = toolMeta(tool.name, tool.params)
        const dstat = deliveryStatOf(tool.name, tool.params)
        out.push({
          id: tool.id || `${m.id}:${out.length}`,
          kind: 'tool',
          icon: meta.icon,
          iconUrl: meta.iconUrl,
          change: fileChangeOf(tool),
          stat: toolCountOf(tool),
          label: t(meta.labelKey),
          title: dstat ? deliveryTitle(dstat, sourceCount) : toolTitle(tool.name, tool.params),
          description: toolDescription(tool.name, tool.params),
          error: tool.status === 'error' || !!tool.denial,
          tool
        })
      }
    }
    // 会话摘要（metadata.summary）：与工具行同款的可展开步骤行——专属图标 + 关键词，
    // 摘要正文折叠在展开体里（不占列表版面）
    if (m.summary === true && m.content) {
      out.push({
        id: `${m.id}:summary`,
        kind: 'summary',
        icon: 'icon-summary',
        label: t('chat.summaryTag'),
        title: '',
        error: false,
        content: m.content
      })
    }
    // 中间叙述正文：原样在步骤间呈现（结尾正文不在此，由块外单独渲染）
    if (m.summary !== true && step.parts.includes('content') && m.content) {
      out.push({
        id: `${m.id}:content`,
        kind: 'content',
        icon: '',
        label: '',
        title: '',
        error: false,
        content: m.content
      })
    }
  }
  return out
})

// 展开态（按行 id 记忆）
const open = reactive(new Set<string>())
/** 展开判定：按本地手动状态 */
function isOpen(id: string): boolean {
  return open.has(id)
}
function toggle(id: string): void {
  if (open.has(id)) open.delete(id)
  else open.add(id)
}

function formatJSON(v: unknown): string {
  if (v === undefined) return ''
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}

/** 工具是否可展开（有参数或结果） */
function expandable(row: StepRow): boolean {
  if (row.kind === 'think') return !!row.reasoning
  if (row.kind === 'summary') return !!row.content
  const tl = row.tool
  return !!tl && (tl.params !== undefined || !!tl.result)
}
</script>

<template>
  <div class="step-list">
    <template v-for="row in rows" :key="row.id">
      <!-- 中间叙述正文：原样成段 -->
      <div v-if="row.kind === 'content'" class="step-content">{{ row.content }}</div>

      <div v-else class="step">
        <button
          type="button"
          class="step-row"
          :class="{ error: row.error }"
          :disabled="!expandable(row)"
          @click="toggle(row.id)"
        >
          <!-- 前导：默认显示图标;悬停换成折叠箭头;展开态显示向上箭头 -->
          <span class="step-leading">
            <icon-chevron-up
              v-if="expandable(row) && isOpen(row.id)"
              :size="14"
              class="step-lead-open"
            />
            <template v-else>
              <!-- 品牌彩色图标（office 行按格式取 Word/Excel/PPT/PDF）：与文件树同款字形，
                   悬停同样淡出换折叠箭头（.step-lead-icon 的规则对它一样生效） -->
              <img
                v-if="row.iconUrl"
                class="step-lead-icon step-file-icon"
                :src="row.iconUrl"
                width="14"
                height="14"
                alt=""
              />
              <component v-else :is="row.icon" :size="14" class="step-lead-icon" />
              <icon-chevron-down v-if="expandable(row)" :size="14" class="step-chevron-hover" />
            </template>
          </span>
          <span class="step-label">{{ row.label }}</span>
          <!-- 文件操作行（图1 布局）：文件图标 + 文件名 + 目录 + 增删/行字符统计，无分隔点与说明 -->
          <template v-if="row.change">
            <img
              class="step-file-icon"
              :src="row.change.fileIconUrl"
              width="14"
              height="14"
              alt=""
            />
            <span class="step-file-name">{{ row.change.fileName }}</span>
            <span v-if="row.change.fileDir" class="step-file-dir">{{ row.change.fileDir }}</span>
            <!-- 读取/写入/追加：N行M字符 -->
            <span v-if="row.change.lines !== undefined" class="step-read-stat">
              {{ t('chat.step.readStats', { lines: row.change.lines, chars: row.change.chars }) }}
            </span>
            <!-- 编辑：+N -N -->
            <span v-else-if="row.change.added > 0 || row.change.removed > 0" class="step-diff">
              <span v-if="row.change.added > 0" class="step-diff-add">+{{ row.change.added }}</span>
              <span v-if="row.change.removed > 0" class="step-diff-del"
                >-{{ row.change.removed }}</span
              >
            </span>
          </template>
          <!-- 普通工具行：· 类型摘要 · 说明 -->
          <template v-else>
            <template v-if="row.title">
              <span class="step-sep">·</span>
              <span
                class="step-title"
                :class="{ 'step-title-live': row.kind === 'think' && row.id === liveThinkId }"
                >{{ displayTitle(row) }}</span
              >
            </template>
            <span v-if="row.stat" class="step-stat">{{
              t(row.stat.unit === 'matches' ? 'chat.step.hitCount' : 'chat.step.fileCount', {
                n: row.stat.count
              })
            }}</span>
            <template v-if="row.description">
              <span class="step-sep">·</span>
              <span class="step-desc">{{ row.description }}</span>
            </template>
          </template>
        </button>

        <!-- 展开体：思考=完整推理；工具=diff(写类)/Arguments + Result -->
        <div v-if="expandable(row) && isOpen(row.id)" class="step-body">
          <pre v-if="row.kind === 'think'" class="step-pre">{{ row.reasoning }}</pre>
          <!-- 会话摘要展开体:markdown 渲染(与 ThinkCard 同款渲染器) -->
          <div v-else-if="row.kind === 'summary'" class="step-summary-md">
            <Markdown
              :content="row.content ?? ''"
              :is-dark="isDark"
              mode="static"
              :enable-animate="false"
              :node-renderers="markdownNodeRenderers"
            />
          </div>
          <template v-else-if="row.tool">
            <!-- 写类：diff 预览替代原始 Arguments JSON -->
            <div v-if="row.change?.patch" class="step-section">
              <CodeDiffView
                :diff="row.change.patch"
                :path="row.change.path"
                :new-lines="row.change.newLines ?? 0"
              />
            </div>
            <div v-else-if="row.change?.truncated" class="step-section">
              <div class="step-section-title">{{ t('chat.step.diffTruncated') }}</div>
            </div>
            <div v-else-if="row.tool.params !== undefined" class="step-section">
              <div class="step-section-title">Arguments</div>
              <pre class="step-pre">{{ formatJSON(row.tool.params) }}</pre>
            </div>
            <div v-if="row.tool.denial" class="step-section">
              <div class="step-section-title">{{ t('denial.reason') }}</div>
              <pre class="step-pre">{{ row.tool.denial.detail || row.tool.result }}</pre>
            </div>
            <div v-else-if="row.tool.result" class="step-section">
              <div class="step-section-title">Result</div>
              <pre class="step-pre" :class="{ 'is-error': row.tool.status === 'error' }">{{
                row.tool.result
              }}</pre>
            </div>
          </template>
        </div>
      </div>
    </template>

    <!-- 进行中相位行:过程列表的"下一条"——等待模型回复 / 思考中 / 等待工具结果,
         与步骤行同构(前导 spinner + 文案),文案走流光动画 -->
    <div v-if="phaseText" class="step step-phase" role="status">
      <span class="step-leading">
        <icon-loading :size="14" class="phase-spin" />
      </span>
      <span class="step-phase-text">{{ phaseText }}</span>
    </div>
  </div>
</template>

<style scoped>
.step-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

/* 相位行:与步骤行同构(前导 16px 图标位 + 13px 文字),文案走流光横扫 */
.step-phase {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  font-size: var(--font-size-sm);
}
.step-phase-text {
  font-weight: 500;
  background: linear-gradient(
    100deg,
    var(--text-tertiary) 30%,
    var(--text-primary) 50%,
    var(--text-tertiary) 70%
  );
  background-size: 220% 100%;
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
  animation: step-shimmer 1.8s linear infinite;
}
.phase-spin {
  color: var(--text-tertiary);
  animation: phase-spin 0.9s linear infinite;
}
@keyframes step-shimmer {
  from {
    background-position: 130% 0;
  }
  to {
    background-position: -130% 0;
  }
}
@keyframes phase-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
/* 降低动效偏好:保留文案与图标,去动画 */
@media (prefers-reduced-motion: reduce) {
  .step-phase-text {
    animation: none;
    background: none;
    color: var(--text-secondary);
  }
  .phase-spin {
    animation: none;
  }
}

/* 中间叙述正文：原样成段 */
.step-summary-md {
  max-height: 320px;
  overflow-y: auto;
  font-size: var(--font-size-sm);
  line-height: 1.6;
  color: var(--text-secondary);
}
.step-content {
  padding: 2px 0 6px;
  font-size: var(--font-size-md);
  line-height: 1.6;
  color: var(--text-primary);
  white-space: pre-wrap;
}

/* 轨迹行：24px 单行，[前导 16px] 类型 · 摘要；默认 tertiary，悬停 secondary；失败摘要标红 */
.step-row {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  height: 24px;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
}
.step-row:hover {
  color: var(--text-secondary);
}
.step-row:disabled {
  cursor: default;
}
.step-row.error .step-title {
  color: var(--danger);
}

.step-leading {
  position: relative;
  width: 16px;
  height: 16px;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.step-lead-icon,
.step-lead-open {
  color: inherit;
}
/* 文件操作行(图1):文件树同款文件图标 + 文件名(深色) + 目录(灰) + 尾部统计 */
.step-file-icon {
  flex: none;
  display: block;
  object-fit: contain;
}
.step-file-name {
  flex: none;
  color: var(--text-primary);
}
.step-file-dir {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-tertiary);
}
/* 编辑 +N -N / 写入 +N */
.step-diff {
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-variant-numeric: tabular-nums;
}
.step-diff-add {
  color: var(--success);
}
.step-diff-del {
  color: var(--danger);
}
/* 读取 N行M字符 */
.step-read-stat {
  flex: none;
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}
/* 目录类结果计数（N个文件 / N处） */
.step-stat {
  flex: none;
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}
.step-chevron-hover {
  position: absolute;
  inset: 0;
  margin: auto;
  color: inherit;
  opacity: 0;
  transition: opacity 100ms ease;
}
.step-row:hover .step-lead-icon {
  opacity: 0;
}
.step-row:hover .step-chevron-hover {
  opacity: 1;
}
.step-label {
  flex: none;
}
.step-sep {
  flex: none;
  color: var(--text-tertiary);
}
.step-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 流式 think 标题:流光横扫(与相位行同一动效语言;窗口翻页由 JS 驱动) */
.step-title-live {
  font-weight: 500;
  background: linear-gradient(
    100deg,
    var(--text-tertiary) 30%,
    var(--text-primary) 50%,
    var(--text-tertiary) 70%
  );
  background-size: 220% 100%;
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
  animation: step-shimmer 1.8s linear infinite;
}
@media (prefers-reduced-motion: reduce) {
  .step-title-live {
    animation: none;
    background: none;
    color: var(--text-tertiary);
  }
}
/* 说明:可被压缩,占满剩余宽度后省略 */
.step-desc {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-tertiary);
}
.step-row.error .step-desc {
  color: var(--danger);
}

/* 展开体：与工具卡片同款灰底块 */
.step-body {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 2px 0 6px 19px;
}
.step-section-title {
  margin-bottom: 2px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
.step-pre {
  margin: 0;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  background: var(--bg-2);
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  line-height: 1.5;
  max-height: 240px;
  overflow: auto;
  word-break: break-word;
  white-space: pre-wrap;
}
.step-pre.is-error {
  color: var(--danger);
}
</style>
