<script setup lang="ts">
/**
 * 工具执行组（两级折叠：组 → 每步工具），供对话区消息与「会话轨迹」页签共用。
 * 从 ChatMessageItem 抽出，保证两处工具卡片的参数 / 结果视觉与行为完全一致。
 */
import { computed, defineAsyncComponent, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ToolCallView } from '@/types'
import { awaitingDecision, isQuietFailure, needsAttention } from './toolState'
import { fileChangeOf, type FileChange } from './editChange'
import DenialBadge from './DenialBadge.vue'
import SandboxBadge from './SandboxBadge.vue'

/** 文件 diff 预览器按需加载（避免把 highlight.js 拉进主包）。 */
const CodeDiffView = defineAsyncComponent(() => import('@/panels/CodeDiffView.vue'))

const props = withDefaults(
  defineProps<{
    tools: ToolCallView[]
    /** 失败工具也展示 Result(会话轨迹读历史时需要看到失败输出;对话区保持原行为 false) */
    showErrorResult?: boolean
  }>(),
  { tools: () => [], showErrorResult: false }
)

const { t } = useI18n()

/** 每个工具的文件操作派生（写类 diff / 读类统计）；WeakMap 缓存，命中 O(1)。 */
const changes = computed(() => {
  const map = new Map<string, FileChange | null>()
  for (const tool of props.tools) map.set(tool.id, fileChangeOf(tool))
  return map
})

function formatJSON(v: unknown): string {
  if (v === undefined) return ''
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}

/** 工具组默认展开:有待执行 / 需用户注意(安全拦截) / 待答提问 */
const groupDefaultOpen = computed(() => {
  const tools = props.tools
  if (tools.some((tool) => tool.status === 'pending')) return true
  if (tools.some((tool) => needsAttention(tool))) return true
  return tools.some((tool) => awaitingDecision(tool) && tool.name === 'chat_with_user')
})
const groupManual = ref<boolean | null>(null)
const groupOpen = computed(() => groupManual.value ?? groupDefaultOpen.value)
function toggleToolGroup(): void {
  groupManual.value = !groupOpen.value
}

// toolBodyState:普通工具正文手动展开;toolBodyClosed:错误工具手动折叠(默认展开)
const toolBodyState = reactive(new Set<string>())
const toolBodyClosed = reactive(new Set<string>())

/** 工具正文是否展开:需要用户注意的失败默认展开;静默失败与正常工具一样默认收起 */
function toolBodyOpen(toolId: string, attention = false): boolean {
  if (attention) return !toolBodyClosed.has(toolId)
  return toolBodyState.has(toolId)
}
function toggleToolBody(toolId: string, attention = false): void {
  if (attention) {
    if (toolBodyClosed.has(toolId)) toolBodyClosed.delete(toolId)
    else toolBodyClosed.add(toolId)
  } else {
    if (toolBodyState.has(toolId)) toolBodyState.delete(toolId)
    else toolBodyState.add(toolId)
  }
}
</script>

<template>
  <div class="tool-group-wrap">
    <button type="button" class="tool-group-header" @click="toggleToolGroup">
      <icon-tool :size="14" />
      <span class="tool-group-title">工具执行 {{ props.tools.length }} 步</span>
      <span class="tool-group-arrow">{{ groupOpen ? '▼' : '▶' }}</span>
    </button>
    <div v-if="groupOpen" class="tool-group-body">
      <div v-for="tool in props.tools" :key="tool.id" class="tool-card" :class="tool.status">
        <button
          type="button"
          class="tool-header"
          @click="toggleToolBody(tool.id, needsAttention(tool))"
        >
          <span class="tool-status">
            <icon-loading v-if="tool.status === 'pending'" :size="13" class="tool-spin" />
            <icon-check-circle v-else-if="tool.status === 'completed'" :size="13" class="tool-ok" />
            <!-- 纯执行失败（已知可预测异常，已交模型改道）：中性灰，不打扰用户 -->
            <icon-close
              v-else-if="isQuietFailure(tool)"
              :size="13"
              class="tool-soft-err"
              :title="t('chat.toolSoftError')"
            />
            <icon-close v-else :size="13" class="tool-err" />
          </span>
          <code class="tool-id">{{ tool.id }}</code>
          <span class="tool-name">{{ tool.name }}</span>
          <!-- 写文件:文件名 + 行字符/增删统计(读取/写入/编辑/追加) -->
          <template v-if="changes.get(tool.id)">
            <span class="tool-file-name">{{ changes.get(tool.id)?.fileName }}</span>
            <span
              v-if="changes.get(tool.id)?.lines !== undefined"
              class="tool-read-stat"
              >{{
                t('chat.step.readStats', {
                  lines: changes.get(tool.id)?.lines,
                  chars: changes.get(tool.id)?.chars
                })
              }}</span
            >
            <span
              v-else-if="
                (changes.get(tool.id)?.added ?? 0) + (changes.get(tool.id)?.removed ?? 0) > 0
              "
              class="tool-diff"
            >
              <span v-if="(changes.get(tool.id)?.added ?? 0) > 0" class="tool-diff-add"
                >+{{ changes.get(tool.id)?.added }}</span
              >
              <span v-if="(changes.get(tool.id)?.removed ?? 0) > 0" class="tool-diff-del"
                >-{{ changes.get(tool.id)?.removed }}</span
              >
            </span>
          </template>
          <!-- 安全拦截分类（拒绝读/写/危险命令/网络/资源/浏览器；推断来源带「?」） -->
          <denial-badge :denial="tool.denial" />
          <!-- 沙箱运行事实（无防护执行 / 被驱逐 / 输出截断；仅 bash 产出，正常时不渲染） -->
          <sandbox-badge :sandbox="tool.sandbox" />
          <span class="tool-time">
            {{
              tool.status === 'pending'
                ? tool.awaitingApproval
                  ? t('approval.awaiting')
                  : t('chat.reasonTool')
                : tool.duration !== undefined
                  ? `${tool.duration}ms`
                  : ''
            }}
          </span>
          <span class="tool-arrow">{{ toolBodyOpen(tool.id, needsAttention(tool)) ? '▼' : '▶' }}</span>
        </button>
        <!-- 折叠时不挂载：Arguments/Result 的大段文本不进 DOM，也不做 JSON.stringify -->
        <div v-if="toolBodyOpen(tool.id, needsAttention(tool))" class="tool-body">
          <!-- 写类：diff 预览替代原始 Arguments JSON -->
          <div v-if="changes.get(tool.id)?.patch" class="tool-section">
            <CodeDiffView
              :diff="changes.get(tool.id)?.patch ?? ''"
              :path="changes.get(tool.id)?.path ?? ''"
              :new-lines="changes.get(tool.id)?.newLines ?? 0"
            />
          </div>
          <div v-else-if="tool.params !== undefined" class="tool-section">
            <div class="tool-section-title">Arguments</div>
            <pre class="tool-json">{{ formatJSON(tool.params) }}</pre>
          </div>
          <!-- 被拦/被拒：展示分类明细（原始结果是拒绝信封 JSON，不直出） -->
          <div v-if="tool.denial" class="tool-section">
            <div class="tool-section-title">{{ t('denial.reason') }}</div>
            <pre class="tool-result">{{ tool.denial.detail || tool.result }}</pre>
          </div>
          <div
            v-else-if="
              tool.result &&
              !awaitingDecision(tool) &&
              (tool.status !== 'error' || props.showErrorResult)
            "
            class="tool-section"
          >
            <div class="tool-section-title">Result</div>
            <pre class="tool-result">{{ tool.result }}</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 工具执行:组折叠 + 每步卡片折叠 */
.tool-group-wrap {
  margin: 4px 0 8px;
}

.tool-group-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 0;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  user-select: none;
}

.tool-group-title {
  font-weight: 500;
}

.tool-group-arrow {
  margin-left: auto;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.tool-group-body {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-left: 14px;
  margin-top: 4px;
}

.tool-card {
  background: var(--bg-1);
  border-radius: var(--radius-sm);
  overflow: hidden;
}

.tool-header {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  user-select: none;
}

.tool-status {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

.tool-spin {
  color: var(--text-tertiary);
  animation: tool-spin 1s linear infinite;
}

@keyframes tool-spin {
  to {
    transform: rotate(360deg);
  }
}

.tool-ok {
  color: var(--success);
}

.tool-err {
  color: var(--danger);
}

/* 纯执行失败(静默态):中性灰,与成功/失败的红绿语义区分开——
   这类异常已回给模型改道,不需要用户介入 */
.tool-soft-err {
  color: var(--text-tertiary);
}

.tool-id {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.tool-name {
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 写文件：文件名 + 增删统计 / 读取行字符统计 */
.tool-file-name {
  flex-shrink: 0;
  color: var(--text-secondary);
}
.tool-diff {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-variant-numeric: tabular-nums;
}
.tool-diff-add {
  color: var(--success);
}
.tool-diff-del {
  color: var(--danger);
}
.tool-read-stat {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.tool-time {
  margin-left: auto;
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

.tool-arrow {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.tool-body {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 0 10px 8px;
}

.tool-section-title {
  margin-bottom: 2px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.tool-json,
.tool-result {
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
</style>
