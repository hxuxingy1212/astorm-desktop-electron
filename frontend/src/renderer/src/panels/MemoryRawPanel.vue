<script setup lang="ts">
/**
 * 记忆原文页签：读取并渲染 raw 会话底稿（`raw/sessions/<stem>.md`）的 Markdown。
 * 入口：记忆概念正文「相关」小节里的「原文」链接。
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Markdown } from 'vue-stream-markdown'
import { markdownNodeRenderers } from '@/components/markdown/renderers'
import { useMemoryStore } from '@/stores/memory'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'

const props = defineProps<{ stem: string }>()

const { t } = useI18n()
const memory = useMemoryStore()
const settings = useSettingsStore()
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

const markdown = ref('')
const loading = ref(false)
const error = ref('')

/** 底稿正文：剥掉 YAML frontmatter（元数据不属 Markdown，直接渲染会变成乱码表格）。 */
function stripFrontmatter(md: string): string {
  if (!md.startsWith('---')) return md
  const end = md.indexOf('\n---', 3)
  if (end === -1) return md
  return md.slice(end + 4).replace(/^\s*\n/, '')
}

const body = computed(() => stripFrontmatter(markdown.value))

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  markdown.value = ''
  try {
    markdown.value = await memory.readRawSource(props.stem)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}
watch(() => props.stem, () => void load(), { immediate: true })
</script>

<template>
  <div class="mr-root">
    <div v-if="loading" class="mr-status">{{ t('common.loading') }}</div>
    <div v-else-if="error" class="mr-status mr-error">{{ error }}</div>
    <Markdown
      v-else
      class="mr-md"
      :content="body"
      :is-dark="isDark"
      mode="static"
      :enable-animate="false"
      :node-renderers="markdownNodeRenderers"
    />
  </div>
</template>

<style scoped>
.mr-root {
  height: 100%;
  overflow-y: auto;
  padding: 5px 18px 24px;
}
.mr-status {
  padding: 16px 4px;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}
.mr-error {
  color: var(--danger);
}
</style>
