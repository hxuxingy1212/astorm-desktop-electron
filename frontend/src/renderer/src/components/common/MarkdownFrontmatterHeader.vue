<script setup lang="ts">
/**
 * Markdown frontmatter 元数据块（文件预览 / 记忆页共用）。
 *
 * 观感与记忆页详情一致：标题行 + 类型/状态/时间小徽标 + 标签 chips + 来源/依据/其余字段细行 +
 * 生成信息尾行。字段名沿用记忆页的 i18n 键（`memory.detail.*`），中英一致。
 *
 * 用法：数据由 `parseFrontmatter()`（文件预览）或记忆后端 DTO 映射而来；
 * `interactive` 打开时来源/依据可点（emit 给调用方决定跳转）。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FrontmatterMeta } from '@/utils/frontmatter'

const props = withDefaults(
  defineProps<{
    meta: FrontmatterMeta
    /** 来源 / 依据可点（点击 emit，跳转由调用方决定） */
    interactive?: boolean
  }>(),
  { interactive: false }
)

const emit = defineEmits<{
  (e: 'source-click', resource: string): void
  (e: 'frag-click', frag: string): void
}>()

const { t } = useI18n()

interface Badge {
  text: string
  color?: string
}

/** 徽标：类型 / 状态 / 观察于 / 更新于（缺省字段不出徽标） */
const badges = computed<Badge[]>(() => {
  const m = props.meta
  const out: Badge[] = []
  if (m.type) out.push({ text: m.type, color: 'arcoblue' })
  if (m.status) out.push({ text: m.status, color: 'green' })
  if (m.observedAt) out.push({ text: `${t('memory.detail.observedAt')} ${fmtWhen(m.observedAt)}` })
  if (m.updatedAt) out.push({ text: `${t('memory.detail.updatedAt')} ${fmtWhen(m.updatedAt)}` })
  return out
})

/** 时间：unix 秒 → 本地时间；其余（ISO / YYYY-MM-DD / 已格式化串）原样 */
function fmtWhen(v: string): string {
  return /^\d{9,}$/.test(v) ? new Date(Number(v) * 1000).toLocaleString() : v
}

/** 尾行：生成者 / 模型 / 生成时间 */
const genLine = computed<string>(() => {
  const m = props.meta
  const parts: string[] = []
  if (m.generatedBy) parts.push(`${t('memory.detail.generatedBy')}: ${m.generatedBy}`)
  if (m.model) parts.push(`${t('memory.detail.model')}: ${m.model}`)
  if (m.generatedAt) parts.push(fmtWhen(m.generatedAt))
  return parts.join(' · ')
})
</script>

<template>
  <div class="mf-root">
    <div v-if="meta.title || $slots.actions" class="mf-head">
      <div v-if="meta.title" class="mf-title">{{ meta.title }}</div>
      <div v-if="$slots.actions" class="mf-actions">
        <slot name="actions" />
      </div>
    </div>
    <slot name="subtitle" />

    <div v-if="badges.length" class="mf-badges">
      <a-tag v-for="b in badges" :key="b.text" size="small" :color="b.color">{{ b.text }}</a-tag>
    </div>

    <div v-if="meta.description" class="mf-desc">{{ meta.description }}</div>

    <div v-if="meta.tags.length" class="mf-tags">
      <span v-for="tag in meta.tags" :key="tag" class="mf-tag">#{{ tag }}</span>
    </div>

    <div v-if="meta.sources.length" class="mf-line">
      <span class="mf-label">{{ t('memory.detail.sources') }}</span>
      <template v-for="s in meta.sources" :key="s.resource">
        <button
          v-if="interactive"
          type="button"
          class="mf-chip mf-chip-btn"
          :title="`${t('memory.detail.openSource')}：${s.resource}`"
          @click="emit('source-click', s.resource)"
        >
          {{ s.title || s.resource }}
          <em v-if="s.lastModified">{{ fmtWhen(s.lastModified) }}</em>
        </button>
        <span v-else class="mf-chip" :title="s.resource">
          {{ s.title || s.resource }}
          <em v-if="s.lastModified">{{ fmtWhen(s.lastModified) }}</em>
        </span>
      </template>
    </div>

    <div v-if="meta.fragIds.length" class="mf-line">
      <span class="mf-label">{{ t('memory.detail.evidence') }}</span>
      <template v-for="f in meta.fragIds" :key="f">
        <button
          v-if="interactive"
          type="button"
          class="mf-frag mf-frag-btn"
          :title="t('memory.detail.evidenceTitle')"
          @click="emit('frag-click', f)"
        >
          {{ f }}
        </button>
        <span v-else class="mf-frag">{{ f }}</span>
      </template>
    </div>

    <!-- 未映射字段：标量（含串行化的数组/嵌套值）走细行，标量数组走 chips，避免信息静默丢失 -->
    <div v-for="e in meta.extras" :key="e.key" class="mf-line">
      <span class="mf-label">{{ e.key }}</span>
      <template v-if="e.items.length > 0">
        <span v-for="it in e.items" :key="it" class="mf-chip" :title="it">{{ it }}</span>
      </template>
      <span v-else class="mf-extra">{{ e.value }}</span>
    </div>

    <div v-if="genLine" class="mf-gen">{{ genLine }}</div>
  </div>
</template>

<style scoped>
/* 元数据块：底部细分割线与正文分开（记忆页详情头 / 文件预览正文上方同一套观感） */
.mf-root {
  padding: 0 0 10px;
  border-bottom: 1px solid var(--border-light);
  margin-bottom: 12px;
}

.mf-head {
  display: flex;
  align-items: flex-start;
  gap: 10px;
}

.mf-title {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-lg);
  font-weight: 600;
  line-height: 1.35;
  color: var(--text-primary);
  word-break: break-word;
}

.mf-actions {
  flex: none;
  margin-left: auto;
  display: flex;
  align-items: center;
}

.mf-badges {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 8px;
}

/* 徽标观感沿用记忆页：统一成半透明小丸，只有首个（类型）用主色 */
.mf-badges :deep(.arco-tag) {
  border: none;
  border-radius: 6px;
  background-color: var(--fill-1);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
}

.mf-badges :deep(.arco-tag:first-child) {
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 15%, transparent);
}

.mf-desc {
  margin-top: 8px;
  font-size: var(--font-size-sm);
  line-height: 1.6;
  color: var(--text-secondary);
}

.mf-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 8px;
}

.mf-tag {
  font-size: var(--font-size-xs);
  color: var(--accent-text);
}

.mf-line {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px;
  margin-top: 6px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.mf-label {
  font-weight: 600;
}

.mf-chip {
  padding: 1px 7px;
  border: none;
  border-radius: 4px;
  font-family: inherit;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  background-color: var(--fill-1);
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mf-chip em {
  margin-left: 4px;
  font-style: normal;
  opacity: 0.7;
}

.mf-chip-btn {
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.mf-chip-btn:hover {
  color: var(--text-primary);
  background-color: var(--fill-2);
}

.mf-frag {
  padding: 1px 7px;
  border: none;
  border-radius: 999px;
  font-family: monospace;
  font-size: var(--font-size-2xs);
  color: var(--accent-text);
  background-color: color-mix(in srgb, var(--accent-text) 14%, transparent);
}

.mf-frag-btn {
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    transform var(--transition-fast);
}

.mf-frag-btn:hover {
  background-color: color-mix(in srgb, var(--accent-text) 24%, transparent);
}

.mf-frag-btn:active {
  transform: scale(0.96);
}

.mf-extra {
  color: var(--text-secondary);
  word-break: break-word;
}

.mf-gen {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  margin-top: 6px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}
</style>
