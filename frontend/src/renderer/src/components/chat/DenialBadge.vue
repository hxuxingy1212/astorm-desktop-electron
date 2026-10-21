<script setup lang="ts">
/**
 * 安全拦截分类徽标（拒绝读 / 拒绝写 / 危险命令 / 目录切换 / 网络 / 资源限制 / 浏览器操作）。
 *
 * 数据来源是后端统一分类（`ToolCallDenial`）：审批卡取信封 `approval.category`，
 * 工具卡取 tool_result 的 `denial` / 消息 `metadata.denial`。来源为推断
 * （`stderr_attribution` / `exit_code`）时后缀「?」，鼠标悬停说明推断依据；
 * 来源缺失（如审计记录无 source 字段）时只展示分类，不显示来源提示。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ToolCallDenial } from '@/types'

const props = defineProps<{ denial?: ToolCallDenial | null }>()

const { t } = useI18n()

/** 推断来源（内核拒绝无结构化事件 / 仅退出码）——分类方向可能不准，需标注 */
const inferred = computed(
  () => props.denial?.source === 'stderr_attribution' || props.denial?.source === 'exit_code'
)
const label = computed(() => (props.denial ? t(`denial.${props.denial.category}`) : ''))
const tip = computed(() => {
  if (!props.denial) return ''
  const head = t(`denial.${props.denial.category}`)
  // 来源缺失时不追加（避免渲染出 `denial.source.undefined`）
  return props.denial.source ? `${head} · ${t(`denial.source.${props.denial.source}`)}` : head
})
</script>

<template>
  <span v-if="denial" class="denial-badge" :class="[denial.category, { inferred }]" :title="tip">
    {{ label }}<span v-if="inferred" class="denial-mark">?</span>
  </span>
</template>

<style scoped>
.denial-badge {
  display: inline-flex;
  align-items: center;
  gap: 1px;
  flex: none;
  font-size: var(--font-size-2xs);
  line-height: 1.5;
  border-radius: 4px;
  padding: 0 5px;
  border: 1px solid transparent;
  white-space: nowrap;
}

.denial-badge.file_read,
.denial-badge.file_write,
/* 目录切换：策略硬拒（不是危险命令），与文件类同用警告色 */
.denial-badge.cwd_change {
  color: var(--warning);
  border-color: var(--warning);
}

.denial-badge.danger_command {
  color: var(--danger, #f53f3f);
  border-color: var(--danger, #f53f3f);
}

.denial-badge.network {
  color: var(--accent-text);
  border-color: var(--accent-text);
}

.denial-badge.resource {
  color: var(--text-secondary);
  border-color: var(--border-strong);
}

/* 浏览器自动化：与网络同族的强调色（写操作审批 / Plan 拒写） */
.denial-badge.browser {
  color: var(--accent-text);
  border-color: var(--accent-text);
}

/* 邮件操作：与网络同族的强调色（发信确认 / 移动 / Plan 拒写） */
.denial-badge.mail {
  color: var(--accent-text);
  border-color: var(--accent-text);
}

/* 推断来源：虚线边框 + 「?」后缀，提示分类方向来自归因而非直接观测 */
.denial-badge.inferred {
  border-style: dashed;
}

.denial-mark {
  font-weight: 700;
  font-size: var(--font-size-2xs);
}
</style>
