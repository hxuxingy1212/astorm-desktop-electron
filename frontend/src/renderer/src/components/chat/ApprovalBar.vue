<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionsStore, type PendingInterrupt } from '@/stores/sessions'
import type { DenialCategory, DenialSource, ToolCallDenial } from '@/types'
import DenialBadge from './DenialBadge.vue'
import CodeBlock from '@/components/common/CodeBlock.vue'

const props = defineProps<{ sessionId: string; interrupt: PendingInterrupt | null }>()

const { t } = useI18n()
const sessions = useSessionsStore()

const visible = computed(() => !!props.sessionId && !!props.interrupt)

/** 展示字段取自中断信封的 approval 段（分类走统一契约 category，旧记录回退 kind） */
const view = computed(() => {
  const a = props.interrupt?.request.approval
  const category = (a?.category ?? a?.kind) as DenialCategory | undefined
  const denial: ToolCallDenial | null = category
    ? {
        category,
        source: (a?.source as DenialSource) ?? 'static_precheck',
        detail: a?.reason ?? '',
        targets: a?.targets ?? [],
        // 被拦操作原文（重放闸门 key）：补齐契约字段，避免下游拿不到
        command: a?.command
      }
    : null
  return { command: a?.command ?? '', reason: a?.reason, targets: a?.targets ?? [], denial }
})

/**
 * 审批决议经专用接口提交(不追加用户消息;后端就地把结果写回 tool 消息并续跑)。
 *
 * **必须带上本卡所属会话**：子会话页签里渲染的是**非激活**会话的审批卡，
 * 默认会提交到激活会话（后端按 request_id 找不到该决议而失败）。
 */
function decide(allow: boolean, scope?: 'session'): void {
  if (!props.interrupt) return
  void sessions.submitInterrupt(
    allow ? { decision: 'allow', ...(scope ? { scope } : {}) } : { decision: 'deny' },
    props.sessionId
  )
}

/** 「当前会话允许」仅对**能归纳出目标**的分类显示(browser / resource 无目标)。
 *  沙箱 preflight 的文件分类拒绝只给原因文本、没有路径 → 后端记 0 条,故隐藏避免按钮空转;
 *  网络在拿不到 host 时后端按整条命令记,故始终显示。 */
const canAllowSession = computed(() => {
  const category = view.value.denial?.category
  if (category === 'network') return true
  if (category === 'file_read' || category === 'file_write') return view.value.targets.length > 0
  return false
})
</script>

<template>
  <!-- 审批卡(macOS 风格浮层):毛玻璃底 + 分类色图标块 + 等宽命令块 + 右对齐按钮组 -->
  <div v-if="visible" class="approval-card" :class="view.denial?.category ?? 'plain'">
    <div class="approval-head">
      <span class="approval-icon"><icon-safe :size="12" /></span>
      <span class="approval-title">{{ t('approval.title') }}</span>
      <denial-badge :denial="view.denial" />
      <span v-if="view.targets.length" class="approval-targets">{{ view.targets.join('，') }}</span>
    </div>

    <!-- 说明文本在命令上方：先讲清为什么被拦，再给命令原文 -->
    <p v-if="view.reason" class="approval-reason">{{ view.reason }}</p>
    <!-- 被拦命令：markdown 代码块样式（bash 高亮 + 行号 + 语言标识 + 复制），与正文代码块同源 -->
    <code-block v-if="view.command" :code="view.command" lang="bash" max-height="104px" />

    <div class="approval-actions">
      <button type="button" class="approval-btn secondary" @click="decide(false)">
        {{ t('approval.deny') }}
      </button>
      <!-- 会话级授权:命中同一目标后续不再弹卡(仅内存态,重启 / 删除会话失效) -->
      <button
        v-if="canAllowSession"
        type="button"
        class="approval-btn secondary"
        :title="t('approval.allowSessionHint')"
        @click="decide(true, 'session')"
      >
        {{ t('approval.allowSession') }}
      </button>
      <button type="button" class="approval-btn primary" @click="decide(true)">
        {{ t('approval.allow') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
/*
 * macOS 风格:半透明材质 + 大圆角 + 柔和投影(与 MentionPopover 的浮动面板同一套配方),
 * 分区用发丝线(0.5px),按钮为 macOS 形状(28px 高 / 7px 圆角 / 中性灰次级 + 主色主按钮)。
 * 分类色(--denial-tint)由 :class 的 category 决定,只作用于图标块与聚焦环。
 */
.approval-card {
  --denial-tint: var(--accent-text);
  box-sizing: border-box;
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px 14px 10px;
  /* 无外边框（按需求去掉；靠背景/投影区分层次） */
  border: none;
  /* 圆角与发送框 / 提问表单(ChatFormBar)一致,同一纵向栈内观感统一 */
  border-radius: 16px;
  background-color: color-mix(in srgb, var(--bg-2) 90%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  box-shadow:
    0 8px 28px rgba(0, 0, 0, 0.16),
    0 1px 2px rgba(0, 0, 0, 0.08);
  animation: approval-in 0.18s cubic-bezier(0.32, 0.72, 0, 1);
}

/* 分类色:图标块与聚焦环取该分类色调 */
.approval-card.file_read,
.approval-card.file_write {
  --denial-tint: var(--warning);
}

.approval-card.danger_command {
  --denial-tint: var(--danger);
}

.approval-card.resource {
  --denial-tint: var(--text-secondary);
}

@keyframes approval-in {
  from {
    opacity: 0;
    transform: translateY(5px) scale(0.99);
  }
}

@media (prefers-reduced-motion: reduce) {
  .approval-card {
    animation: none;
  }
}

.approval-head {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

/* 圆角方块图标(SF Symbols 风格的着色块) */
.approval-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex: none;
  border-radius: 6px;
  color: var(--denial-tint);
  background-color: color-mix(in srgb, var(--denial-tint) 16%, transparent);
}

.approval-title {
  flex: none;
  font-size: var(--font-size-sm);
  font-weight: 600;
  line-height: 1.4;
  color: var(--text-primary);
  letter-spacing: 0.01em;
}

/* 涉及目标:贴右,过长省略 */
.approval-targets {
  margin-left: auto;
  min-width: 0;
  font-size: var(--font-size-2xs);
  line-height: 1.4;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 被拦命令:代码块（CodeBlock 自带头部语言标识 + 复制;此处只需限高与观感对齐） */
.approval-card :deep(.code-block) {
  border-color: var(--border-light);
}

.approval-reason {
  margin: 0;
  padding: 0 2px;
  font-size: var(--font-size-xs);
  line-height: 1.55;
  color: var(--text-secondary);
  word-break: break-word;
}

/* 按钮组:发丝线分隔,macOS 右对齐(取消在左、默认按钮在右) */
.approval-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 2px;
  padding-top: 10px;
  border-top: 0.5px solid var(--border-base);
}

.approval-btn {
  min-width: 68px;
  height: 28px;
  padding: 0 14px;
  font-family: inherit;
  font-size: var(--font-size-sm);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  border: 0;
  border-radius: 7px;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    filter var(--transition-fast),
    box-shadow var(--transition-fast);
}

/* 次级:中性灰填充(macOS 非默认按钮) */
.approval-btn.secondary {
  color: var(--text-primary);
  background-color: color-mix(in srgb, var(--bg-5) 20%, transparent);
}

.approval-btn.secondary:hover {
  background-color: color-mix(in srgb, var(--bg-5) 32%, transparent);
}

/* 主按钮:主色实心 + 极轻投影 */
.approval-btn.primary {
  color: #fff;
  background-color: var(--accent-text);
  box-shadow: 0 1px 2.5px rgba(0, 0, 0, 0.18);
}

.approval-btn.primary:hover {
  filter: brightness(1.06);
}

.approval-btn:active {
  filter: brightness(0.94);
}

/* 键盘聚焦环(仅键盘可达时显示,取分类色) */
.approval-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3.5px color-mix(in srgb, var(--denial-tint) 32%, transparent);
}
</style>
