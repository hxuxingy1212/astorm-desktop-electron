<script setup lang="ts">
import { computed, h, ref, type Component } from 'vue'
import { useI18n as useAppI18n } from 'vue-i18n'
import {
  NodeList,
  UI,
  useContext,
  useI18n as useMarkdownI18n,
  useSanitizers,
  type LinkNodeRendererProps
} from 'vue-stream-markdown'
import AppIcon from '@/components/common/AppIcon.vue'
import { UI_ICON_STROKE_WIDTH } from '@/utils/uiIconGallery'
import { useTabsStore } from '@/stores/tabs'

const props = defineProps<LinkNodeRendererProps>()

const { t } = useAppI18n()
const { t: md } = useMarkdownI18n()
const { uiComponents, linkOptions, hardenOptions } = useContext()
const tabs = useTabsStore()

const AlertComp = UI.Alert as Component
const ButtonComp = UI.Button as Component

const GlobeIcon = (iconProps: { width?: number; height?: number }): ReturnType<typeof h> =>
  h(AppIcon, { glyph: 'globe', size: iconProps.width ?? 14, strokeWidth: UI_ICON_STROKE_WIDTH })

const dialogOpen = ref(false)
const copied = ref(false)
let copyTimer: ReturnType<typeof setTimeout> | null = null

const rawUrl = computed(() => props.node.url ?? '')
const loading = computed(
  () => !!props.node.loading || !!props.markdownParser?.hasLoadingNode?.(props.node.children) || !rawUrl.value
)

const { transformedUrl, isHardenUrl } = useSanitizers({ url: rawUrl, hardenOptions, loading })

const showLink = computed(() => !isHardenUrl.value && typeof transformedUrl.value === 'string')

/** 站点图标：GitHub 系用品牌图标，其余用通用地球（与 dsh 的 siteGlyph 口径一致）。 */
const siteGlyph = computed(() => {
  const url = transformedUrl.value
  if (typeof url !== 'string') return 'globe'
  try {
    const host = new URL(url).hostname.toLowerCase()
    if (
      host === 'github.com' ||
      host.endsWith('.github.com') ||
      host === 'github.io' ||
      host.endsWith('.github.io') ||
      host === 'raw.githubusercontent.com'
    ) {
      return 'github'
    }
  } catch {
    /* 非法 URL 交给 harden 分支 */
  }
  return 'globe'
})
const ErrorComponent = computed<Component>(
  () =>
    (hardenOptions.value?.errorComponent as Component) ??
    (uiComponents.value.ErrorComponent as Component)
)

async function isTrusted(url: string): Promise<boolean> {
  if (linkOptions.value?.safetyCheck === false) return true
  const fn = linkOptions.value?.isTrusted
  return typeof fn === 'function' ? await Promise.resolve(fn(url)) : false
}

async function onClick(event: MouseEvent): Promise<void> {
  const url = transformedUrl.value
  if (!url) return
  event.preventDefault()
  if (await isTrusted(url)) {
    openInApp()
    return
  }
  dialogOpen.value = true
}

function openExternal(): void {
  const url = transformedUrl.value
  if (url) {
    // 有内置浏览器桥时 window.open 会被主进程再次转成内部页签；这里显式走系统浏览器。
    if (window.api?.shell?.openExternal) {
      void window.api.shell.openExternal(url).catch(() => window.open(url, '_blank'))
    } else {
      window.open(url, '_blank')
    }
  }
  dialogOpen.value = false
}

function openInApp(): void {
  const url = transformedUrl.value
  if (!url) return
  const res = tabs.applyTabCommand({ action: 'open', kind: 'browser', url, new_tab: true })
  if (!res.ok) window.open(url, '_blank')
  dialogOpen.value = false
}

async function onCopy(): Promise<void> {
  const url = transformedUrl.value
  if (!url) return
  try {
    await navigator.clipboard.writeText(url)
    copied.value = true
    if (copyTimer) clearTimeout(copyTimer)
    copyTimer = setTimeout(() => {
      copied.value = false
    }, 1500)
  } catch {
    copied.value = false
  }
}
</script>

<template>
  <span data-stream-markdown="link-container" class="inline">
    <a
      v-if="showLink"
      data-stream-markdown="link"
      :data-stream-markdown-loading="loading"
      class="text-primary underline cursor-pointer [overflow-wrap:anywhere] data-[stream-markdown-loading=true]:no-underline data-[stream-markdown-loading=true]:cursor-default data-[stream-markdown-loading=true]:pointer-events-none data-[stream-markdown-loading=true]:relative"
      rel="noreferrer"
      target="_blank"
      @click="onClick"
    >
      <span class="md-link-icon" aria-hidden="true">
        <AppIcon :glyph="siteGlyph" :size="14" :stroke-width="2" />
      </span>
      <NodeList :parent-node="node" :nodes="node.children" :deep="deep + 1" />
    </a>
    <component v-else :is="ErrorComponent" :variant="'harden-link'">
      <NodeList :parent-node="node" :nodes="node.children" :deep="deep + 1" />
    </component>

    <component
      :is="AlertComp"
      v-if="showLink"
      :open="dialogOpen"
      :title="md('link.title')"
      :description="md('link.description')"
      icon="externalLink"
      @confirm="openInApp"
      @update:open="(value: boolean) => (dialogOpen = value)"
    >
      <code class="text-sm font-mono p-3 rounded-lg bg-muted w-full inline-block overflow-x-auto">{{
        transformedUrl
      }}</code>
      <template #footer>
        <component
          :is="ButtonComp"
          variant="text"
          :name="copied ? md('button.copied') : md('link.copy')"
          :icon="copied ? 'check' : 'copy'"
          :icon-height="16"
          :icon-width="16"
          @click="onCopy"
        />
        <component
          :is="ButtonComp"
          variant="text"
          :name="md('link.open')"
          icon="externalLink"
          :icon-height="16"
          :icon-width="16"
          @click="openExternal"
        />
        <component
          :is="ButtonComp"
          variant="text"
          data-stream-markdown="open-in-app-button"
          :name="t('chat.openInBrowser')"
          :icon="GlobeIcon"
          :icon-height="16"
          :icon-width="16"
          @click="openInApp"
        />
      </template>
    </component>
  </span>
</template>

<style scoped>
/* 链接前缀站点图标：与文字同一水平线，不抬高行框 */
.md-link-icon {
  display: inline-flex;
  align-items: center;
  margin-right: 4px;
  vertical-align: -0.15em;
}
</style>
