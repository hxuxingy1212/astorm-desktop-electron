<script setup lang="ts">
/**
 * 用户消息快捷轨（对话区左侧留白里的消息导航）。
 *
 * 每条用户消息一根短横线：常驻淡显，鼠标移到轨上加深、悬停那条变长；悬停弹出卡片
 * 显示该条提问原文 + 本轮最终正文预览；点击通知父级滚到那条消息；父级按滚动位置回传
 * `activeIndex`，高亮「当前视口所在的那条」。
 *
 * **渲染隔离（务必遵守，见 turnGrouping.ts 文件头）**：轨线数量只依赖 `turns` 的结构性
 * 字段（每条是否有 user 消息），逐 token 追加的正文不参与；卡片文本走 `preview` 懒计算
 * ——只有 `hovered != null` 时才去读 `content`，因此不悬停时轨线不会随流式正文重渲染。
 *
 * 宽度不够时由 CSS 容器查询（global.css 的 `.chat-section`）整体隐藏，本组件不做分支。
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { TurnItem } from './turnGrouping'

const props = defineProps<{
  /** 有条目即有轨线：只传「带用户消息的轮次」（顺序 = 会话顺序） */
  turns: TurnItem[]
  /** 当前视口所在的那条（父级按滚动位置算；-1 = 无） */
  activeIndex: number
}>()

const emit = defineEmits<{ (e: 'jump', id: string): void }>()

const { t } = useI18n()

/** 悬停中的轨线序号（null = 没悬停，此时不读任何正文） */
const hovered = ref<number | null>(null)
const ticksRef = ref<HTMLElement | null>(null)

const anchors = computed(() => props.turns.map((turn) => turn.user?.id ?? ''))

/**
 * 悬停卡片文案（**懒读正文**：不悬停直接返回 null，不建立内容依赖）。
 * 副行只认「本轮最终正文」（`turn.final`）：以工具调用收尾 / 被忽略 / 被打断的轮次
 * 没有最终正文 → 显示「暂无助手正文」，与参考设计一致。
 */
const preview = computed<{ question: string; reply: string } | null>(() => {
  const i = hovered.value
  if (i == null) return null
  const turn = props.turns[i]
  if (!turn) return null
  const question = (turn.user?.content ?? '').trim()
  const reply = (turn.final?.content ?? '').trim()
  return {
    // 纯图片消息没有文本：给一个占位，卡片不至于空白
    question: question || (turn.user?.attachments?.length ? t('chat.railImage') : ''),
    reply: reply || t('chat.railNoReply')
  }
})

/** 卡片纵向对齐悬停那条的中线（相对本组件根节点；轨线内滚时按 scrollTop 修正） */
const cardTop = computed<number>(() => {
  const i = hovered.value
  const ticks = ticksRef.value
  if (i == null || !ticks) return 0
  const tick = ticks.children[i] as HTMLElement | undefined
  if (!tick) return 0
  return tick.offsetTop - ticks.scrollTop + tick.offsetHeight / 2
})

function onJump(index: number): void {
  const id = anchors.value[index]
  if (id) emit('jump', id)
}
</script>

<template>
  <!-- 只要有用户消息就渲染（再窄的对话区由容器查询整体隐藏，不做第二重条件——
       否则会出现「窗口明明有留白但轨线不见」的隐性状态） -->
  <nav
    v-if="turns.length"
    class="msg-rail"
    :aria-label="t('chat.railAria')"
    @mouseleave="hovered = null"
  >
    <div ref="ticksRef" class="rail-ticks">
      <button
        v-for="(turn, i) in turns"
        :key="turn.id"
        type="button"
        class="rail-tick"
        :class="{ active: i === activeIndex, on: i === hovered }"
        :aria-label="t('chat.railJump', { n: i + 1 })"
        @mouseenter="hovered = i"
        @focus="hovered = i"
        @blur="hovered = null"
        @click="onJump(i)"
      />
    </div>

    <!-- 悬停卡片：主行 = 用户提问原文、副行 = 本轮最终正文 / 暂无助手正文，各至多两行 -->
    <div
      v-if="preview"
      class="rail-card"
      role="status"
      :style="{ transform: `translateY(calc(${cardTop}px - 50%))` }"
    >
      <div class="rail-card-question">{{ preview.question }}</div>
      <div class="rail-card-reply">{{ preview.reply }}</div>
    </div>
  </nav>
</template>

<style scoped>
/*
 * macOS 观感：轨线是安静的中性短线（常驻淡显，悬停/当前加深加长），
 * 卡片沿用仓内既有材质配方（半透明底 + 毛玻璃 + 发丝边 + 大圆角 + 软投影），
 * 与审批卡 / 提问表单卡 / 错误条同一套配方（见 ChatFormBar.vue 注释）。
 * 水平位置与显示条件由 global.css 的容器查询给（必须落在左侧留白里）。
 */
/* 显隐与水平位置在 global.css（依赖对话区宽度，用容器查询）：这里只给尺寸与内部排布，
   刻意不写 display —— 否则本作用域选择器（带 data-v 属性）会盖掉那边的 display:none */
.msg-rail {
  position: absolute;
  /* 纵向基准 = 对话区高度 − 底部栈（状态行 + 错误条 + 输入框）的**常驻**高度。
     这里刻意用常量而不是量出来的实时高度：状态行里「向下」按钮的一出一收就能让它
     8px→37px，错误条 / 排队条出现同样会改变消息区高度；若跟着消息区居中，
     轨线就会随滚动（按钮出现/消失）上下轻微跳动（实测 15px）。
     常量换来的是「滚到哪都一样」。--chat-rail-reserve 定义见 global.css。 */
  top: 0;
  bottom: var(--chat-rail-reserve);
  /* 定宽 30px 让卡片 x 恒定（悬停时轨线加长不推动卡片） */
  flex-direction: column;
  width: 30px;
  z-index: 20;
}

/* 轨线列：长会话超出时内部滚动；margin:auto 居中且溢出端仍可滚到。
   滚动条走全局那套（空闲隐藏/滚动时出现，4px 预留位）——轨线左对齐，预留位不吃轨线宽度 */
.rail-ticks {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 7px;
  max-height: 100%;
  margin: auto 0;
  overflow-y: auto;
  /* 常驻淡显；鼠标进入轨线区后加重（淡显只管轨线，不含卡片） */
  opacity: 0.45;
  transition: opacity 0.22s cubic-bezier(0.32, 0.72, 0, 1);
}

.msg-rail:hover .rail-ticks,
.msg-rail:focus-within .rail-ticks {
  opacity: 1;
}

/*
 * 单根轨线：3px 高圆角短线。三档状态都做宽度/底色过渡，鼠标扫过时是「滑出来」而不是跳变：
 *   默认 18px → 当前视口所在 24px（细一档的标记，颜色也重一点）→ 悬停 30px（最重）
 * 高度恒为 3px：卡片纵向定位依赖轨线中线，高度一变卡片就会跟着漂。
 */
.rail-tick {
  width: 18px;
  height: 3px;
  flex: none;
  padding: 0;
  border: 0;
  border-radius: 2px;
  background-color: var(--text-disabled);
  cursor: pointer;
  transition:
    width 0.24s cubic-bezier(0.32, 0.72, 0, 1),
    background-color 0.2s cubic-bezier(0.32, 0.72, 0, 1);
}

/* 当前滚动位置所在的那条：常驻标记（比默认长一档、深一档，比悬停轻） */
.rail-tick.active {
  width: 24px;
  background-color: var(--text-tertiary);
}

/* 悬停 / 键盘聚焦那条：最长最深（写在 .active 之后，同特异度下盖住它的宽度） */
.rail-tick:hover,
.rail-tick.on,
.rail-tick:focus-visible {
  width: 30px;
  background-color: var(--text-secondary);
}

.rail-tick.active:hover,
.rail-tick.active.on,
.rail-tick.active:focus-visible {
  background-color: var(--text-primary);
}

.rail-tick:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-text) 32%, transparent);
}

/* 悬停卡片：贴在轨线右侧、纵向对齐那条的中线。
   位置由内联 transform 给（见模板）：换轨线时是合成层位移过渡，丝滑且不触发重排 */
.rail-card {
  position: absolute;
  top: 0;
  left: calc(100% + 10px);
  width: max-content;
  max-width: 320px;
  padding: 10px 13px;
  border: 0.5px solid color-mix(in srgb, var(--border-strong) 70%, transparent);
  border-radius: 14px;
  background-color: color-mix(in srgb, var(--bg-2) 92%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  box-shadow:
    0 8px 28px rgba(0, 0, 0, 0.16),
    0 1px 2px rgba(0, 0, 0, 0.08);
  /* 卡片只读展示，不吃鼠标（否则轨迹 mouseleave 抖动） */
  pointer-events: none;
  will-change: transform;
  /* 位移过渡 + 首次出现时的淡入（淡入只动 opacity：动 transform 会与内联定位打架） */
  transition: transform 0.22s cubic-bezier(0.32, 0.72, 0, 1);
  animation: rail-card-in 0.16s ease-out;
}

@keyframes rail-card-in {
  from {
    opacity: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .rail-card,
  .rail-tick,
  .rail-ticks {
    transition: none;
  }

  .rail-card {
    animation: none;
  }
}

/* 主行：提问原文（最多两行，超出省略） */
.rail-card-question {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  font-size: var(--font-size-sm);
  font-weight: 600;
  line-height: 1.5;
  color: var(--text-primary);
  white-space: pre-wrap;
  word-break: break-word;
}

/* 副行：本轮最终正文预览（最多两行，超出省略；没有则「暂无助手正文」） */
.rail-card-reply {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  margin-top: 5px;
  font-size: var(--font-size-xs);
  line-height: 1.5;
  color: var(--text-tertiary);
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
