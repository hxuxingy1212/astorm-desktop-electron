<script setup lang="ts">
/**
 * 记忆概念页签：拉取该记忆文档，顶部渲染元数据（frontmatter），下方渲染 Markdown 正文。
 * 入口：对话区「相关记忆」里的记忆卡片点击。
 */
import { computed, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Markdown } from 'vue-stream-markdown'
import MarkdownFrontmatterHeader from '@/components/common/MarkdownFrontmatterHeader.vue'
import FileRefCode from '@/components/markdown/FileRefCode.vue'
import MemoryConceptLink from '@/components/markdown/MemoryConceptLink.vue'
import { invoke } from '@/services/backend'
import type { MemoryConceptDetail } from '@/services/dto'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import { emptyFrontmatterMeta, stripFrontmatter, type FrontmatterMeta } from '@/utils/frontmatter'

const props = defineProps<{ conceptId: string }>()

const { t } = useI18n()
const settings = useSettingsStore()
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

const detail = ref<MemoryConceptDetail | null>(null)
const loading = ref(false)
const error = ref('')

/**
 * 节点渲染器：链接走概念互链 / 原文（MemoryConceptLink），行内代码里的**文件引用**
 * 与对话正文同一套 chip（本面板没有注入基准目录，故只有绝对路径 / `file://` 引用可点）。
 */
const nodeRenderers = { link: MemoryConceptLink, inlineCode: FileRefCode }

/** 详情正文：剥掉 YAML frontmatter（元数据由顶部元数据块呈现，直接渲染会变成乱码表格）。 */
const body = computed(() => stripFrontmatter(detail.value?.markdown ?? ''))

/** 元数据块数据：后端解析好的 frontmatter 字段 → 与文件预览/记忆页共用的组件 */
const meta = computed<FrontmatterMeta>(() => {
  const d = detail.value
  if (!d) return emptyFrontmatterMeta()
  return {
    ...emptyFrontmatterMeta(),
    title: d.title || d.conceptId,
    type: d.type,
    status: d.status ?? '',
    description: d.description,
    tags: d.tags,
    observedAt: d.observedAt ?? '',
    updatedAt: fmtTime(d.updatedAt),
    sources: d.sources.map((s) => ({
      resource: s.resource,
      title: s.title,
      lastModified: s.lastModified ? fmtIso(s.lastModified) : null
    })),
    fragIds: d.fragIds,
    generatedBy: d.generatedBy ?? '',
    generatedAt: d.generatedAt ? fmtIso(d.generatedAt) : '',
    model: d.model ?? ''
  }
})

/** 供 MemoryConceptLink 折算相对链接的基准目录 */
const dir = computed(() => detail.value?.dir ?? '')
provide('memoryConceptDir', dir)

function fmtIso(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

function fmtTime(secs: number): string {
  if (!secs) return '—'
  return new Date(secs * 1000).toLocaleString()
}

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  detail.value = null
  try {
    detail.value = (await invoke('memory_show_concept', {
      concept_id: props.conceptId
    })) as MemoryConceptDetail | null
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
watch(() => props.conceptId, () => void load(), { immediate: true })
</script>

<template>
  <div class="mc-root">
    <div v-if="loading" class="mc-status">{{ t('common.loading') }}</div>
    <div v-else-if="error" class="mc-status mc-error">{{ error }}</div>
    <template v-else-if="detail">
      <MarkdownFrontmatterHeader :meta="meta">
        <template #subtitle>
          <div class="mc-id">{{ detail.conceptId }}</div>
        </template>
      </MarkdownFrontmatterHeader>
      <Markdown
        class="mc-md"
        :content="body"
        :is-dark="isDark"
        mode="static"
        :enable-animate="false"
        :node-renderers="nodeRenderers"
      />
    </template>
  </div>
</template>

<style scoped>
.mc-root {
  height: 100%;
  overflow-y: auto;
  padding: 5px 18px 24px;
}
.mc-status {
  padding: 16px 4px;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}
.mc-error {
  color: var(--danger);
}
.mc-id {
  margin-top: 3px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  word-break: break-all;
}
</style>
