<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import type { MentionItem } from '@/components/mentions/types'

/**
 * 输入框上方的「已选技能」胶囊（样式对齐 ElementAttachment）：`$` 选中的技能不再插入正文，
 * 而是汇总成一枚「N 个技能」胶囊；悬浮/点击弹出技能卡片（图标 + 名称 + 描述）。发送时并入消息。
 */
defineProps<{ skills: MentionItem[] }>()
const emit = defineEmits<{ (e: 'remove'): void }>()

const { t } = useI18n()
</script>

<template>
  <div v-if="skills.length" class="skill-attach">
    <a-popover
      :trigger="['hover', 'click']"
      position="tl"
      content-class="skill-popover"
      :arrow-style="{ display: 'none' }"
    >
      <span class="skill-pill-body">
        <icon-sparkles :size="14" />
        <span class="skill-pill-text">{{ t('chat.skillCount', { count: skills.length }) }}</span>
      </span>
      <template #content>
        <div class="skill-cards">
          <div v-for="s in skills" :key="s.id" class="skill-card">
            <div class="skill-card-head">
              <span class="skill-card-icon">
                <component :is="s.icon || 'icon-thunderbolt'" :size="14" />
              </span>
              <span class="skill-card-name">{{ s.label || s.id }}</span>
            </div>
            <div v-if="s.description" class="skill-card-desc">{{ s.description }}</div>
          </div>
        </div>
      </template>
    </a-popover>

    <button
      type="button"
      class="skill-remove"
      :title="t('chat.skillRemove')"
      @click="emit('remove')"
    >
      <icon-close :size="12" />
    </button>
  </div>
</template>

<style scoped>
/* 胶囊:浅色圆角 + 细边框,左侧悬停/点击触发区,右侧移除按钮(与 ElementAttachment 一致) */
.skill-attach {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  height: 32px;
  padding-right: 3px;
  border: 0.5px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-1);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  max-width: 100%;
}

.skill-pill-body {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 100%;
  padding: 0 6px 0 10px;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  cursor: pointer;
}

.skill-pill-text {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.skill-remove {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  padding: 0;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  transition:
    background-color 0.15s,
    color 0.15s;
}

.skill-remove:hover {
  background: var(--bg-3);
  color: var(--text-primary);
}
</style>

<style>
/* 弹层内容渲染在 body 层(Arco portal),需非 scoped 命中。
   Arco 是 content-class(.skill-popover)包 .arco-popover-content 两层,默认各有一套白底/内边距/
   圆角/阴影,叠起来像套两层壳。这里抹掉**外层**壳,只让内层呈现一张卡片。 */
.skill-popover.skill-popover {
  padding: 0;
  border: none;
  border-radius: 0;
  background: transparent;
  box-shadow: none;
}

.skill-popover .arco-popover-content {
  padding: 0;
  border-radius: 10px;
  overflow: hidden;
  background: var(--bg-1);
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.08),
    0 10px 28px rgba(0, 0, 0, 0.16);
}

.skill-popover .skill-cards {
  /* 篇幅过多时滚动:限制最大高度,预留滚动条位置,细滚动条 */
  max-height: min(360px, 55vh);
  overflow-y: auto;
  padding: 6px 4px;
  scrollbar-gutter: stable;
}

.skill-popover .skill-card {
  min-width: 240px;
  max-width: 420px;
  padding: 8px 12px;
}

.skill-popover .skill-card + .skill-card {
  border-top: 1px solid var(--border-light);
}

.skill-popover .skill-card-head {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 3px;
}

.skill-popover .skill-card-icon {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  color: var(--text-primary);
}

.skill-popover .skill-card-name {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.skill-popover .skill-card-desc {
  font-size: var(--font-size-xs);
  line-height: 1.6;
  color: var(--text-tertiary);
  word-break: break-word;
  white-space: pre-wrap;
}
</style>
