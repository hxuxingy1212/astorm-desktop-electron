<script setup lang="ts">
/**
 * 会话状态行:显示在对话框上方(脱离消息流,不随消息滚动/收纳),
 * 与「向下」按钮共用同一行——状态文字靠左(由 ChatPanel 的 .status-row 网格定位),
 * 「向下」胶囊居中。只描述一种态:
 *   提问被忽略 / 被取代 → 「本次提问已忽略,可继续填写表单」(恢复入口在收纳块下方)
 *   其余(用户打断 / 等待审批 / 等待填表 / 进行中 / 完成 / 出错)→ 不渲染,
 *   「工作已暂停,…」系列提示已按需求全部去掉(审批卡 / 表单卡 / 恢复入口已足够表达状态)。
 *
 * 只服务会话最新一轮(由 ChatPanel 传入该轮),历史轮次不显示。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { resumableForm, type TurnStep } from './turnGrouping'

const props = defineProps<{
  /** 最新一轮的步骤(用于识别被放弃、可继续填写的提问) */
  steps: TurnStep[]
  /** 该轮是否已结束 */
  ended: boolean
  /** 该轮起点(用户消息时间) */
  startedAt: number
  /** 该轮终点(历史记录回填;进行中不需要) */
  endedAt?: number
  /** 流式相位(进行中的过程状态):等待模型回复 / 思考中 / 等待工具结果 / 正在摘要压缩 */
  phase?: 'awaiting' | 'thinking' | 'tool' | 'summarizing'
}>()

const { t } = useI18n()

/** 该轮有被放弃的提问(忽略 / 被取代 / 被中断):暂停说明改成「可继续填写」 */
const resumable = computed(() => !!resumableForm(props.steps))

/**
 * 状态文案:空字符串表示不显示。
 * 「工作已暂停，…」系列提示(用户打断 / 等待审批 / 等待填表)已按需求全部去掉，
 * 只保留「本次提问已忽略，可继续填写表单」这一条(指向收纳块下方的恢复入口)。
 *
 * 进行中的相位文案(等待模型回复 / 思考中 / 等待工具结果)带流光动画,
 * 随事件相位切换显示;正文开始流式 / 中断等待 / 收尾时相位为空,自动让位。
 */
const text = computed(() => {
  if (resumable.value) return t('chat.turnIgnoredForm')
  if (props.ended) return ''
  if (props.phase === 'awaiting') return t('chat.turnPhaseAwaiting')
  if (props.phase === 'thinking') return t('chat.turnPhaseThinking')
  if (props.phase === 'tool') return t('chat.turnPhaseTool')
  if (props.phase === 'summarizing') return t('chat.turnPhaseSummarizing')
  return ''
})

/** 是否走流光样式(仅进行中的相位文案;「提问已忽略」是静态提示,不闪) */
const shimmer = computed(() => !props.ended && !resumable.value && !!props.phase)
</script>

<template>
  <div v-if="text" class="turn-status" :class="{ shimmer }" role="status">{{ text }}</div>
</template>

<style scoped>
/* 状态文字:与「向下」胶囊共用一行(ChatPanel 的 .status-row 提供网格与左对齐),
   只用文字不用卡片,避免和审批卡/表单卡抢视觉;
   纵向间距(与输入框的 8px)由那一行统一给,这里只留 2px 左缩进对齐文字列 */
.turn-status {
  padding-left: 2px;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-secondary);
}

/* 流光:渐变色文字横扫(进行中状态专用;颜色随主题,不新引入色板) */
.turn-status.shimmer {
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
  animation: turn-shimmer 1.8s linear infinite;
}

@keyframes turn-shimmer {
  from {
    background-position: 130% 0;
  }
  to {
    background-position: -130% 0;
  }
}

/* 降低动效偏好:保留文案,去掉横扫 */
@media (prefers-reduced-motion: reduce) {
  .turn-status.shimmer {
    animation: none;
    background: none;
    color: var(--text-secondary);
  }
}
</style>
