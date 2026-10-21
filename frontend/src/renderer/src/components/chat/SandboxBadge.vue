<script setup lang="ts">
/**
 * 沙箱运行事实徽标（无防护执行 / 被驱逐 / 输出截断）。
 *
 * 数据来源是后端 bash 结果元数据（`ToolCallSandboxFacts`：tool_result 的 `sandbox` /
 * 消息 `metadata.sandbox`）。与 denial-badge 的区别：这里标注的是**已跑完的执行事实**
 * （不是拦截分类，成功命令也可能无防护/被截断），因此不依赖 tool.status；
 * 一切正常时不渲染任何标记，避免工具卡变宽。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ToolCallSandboxFacts } from '@/types'

const props = defineProps<{ sandbox?: ToolCallSandboxFacts | null }>()

const { t } = useI18n()

interface Fact {
  key: string
  label: string
  /** 悬停说明（驱逐用后端给的原因原文） */
  tip: string
  cls: string
}

/** 需要标注的事实；正常执行 → 空数组 → 不渲染 */
const facts = computed<Fact[]>(() => {
  const s = props.sandbox
  if (!s) return []
  const out: Fact[] = []
  // protected 缺省不标注：缺席视为「无从判断」，只有显式 false 才是无防护
  if (s.protected === false) {
    out.push({
      key: 'unprotected',
      label: t('sandbox.unprotected'),
      tip: t('sandbox.unprotectedTip'),
      cls: 'unprotected'
    })
  }
  if (s.evicted?.length) {
    // 首条即摘要（后端已按条裁剪），全部原因进 tooltip
    out.push({
      key: 'evicted',
      label: t('sandbox.evicted'),
      tip: s.evicted.join('\n'),
      cls: 'evicted'
    })
  }
  if (s.output_truncated) {
    out.push({
      key: 'truncated',
      label: t('sandbox.truncated'),
      tip: t('sandbox.truncatedTip'),
      cls: 'truncated'
    })
  }
  return out
})
</script>

<template>
  <span v-for="f in facts" :key="f.key" class="sandbox-badge" :class="f.cls" :title="f.tip">
    {{ f.label }}
  </span>
</template>

<style scoped>
.sandbox-badge {
  display: inline-flex;
  align-items: center;
  flex: none;
  font-size: var(--font-size-2xs);
  line-height: 1.5;
  border-radius: 4px;
  padding: 0 5px;
  border: 1px solid transparent;
  white-space: nowrap;
}

/* 无防护：与「危险命令」同色——这条结果不能按沙箱语义解读 */
.sandbox-badge.unprotected {
  color: var(--danger, #f53f3f);
  border-color: var(--danger, #f53f3f);
}

/* 被驱逐：进程被沙箱主动终止（网络 / 资源），警告色 */
.sandbox-badge.evicted {
  color: var(--warning);
  border-color: var(--warning);
}

/* 输出截断：仅信息性（大输出被裁剪），中性色不抢注意力 */
.sandbox-badge.truncated {
  color: var(--text-secondary);
  border-color: var(--border-strong);
}
</style>
