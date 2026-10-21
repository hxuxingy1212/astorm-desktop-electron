<script setup lang="ts">
/**
 * 「相关资料」页签：列出本轮的网页搜索 / 记忆搜索结果卡片（参考搜索结果面板样式）。
 * 点击网页卡片 → 右侧浏览器页签打开该 URL；点击记忆卡片 → 记忆文档预览页签。
 */
import { onBeforeUnmount, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SourceItem } from '@/types'
import { useTabsStore } from '@/stores/tabs'
import { useSessionsStore } from '@/stores/sessions'
import { BROWSER_DESKTOP_1280 } from '@/utils/browserViewport'
import { hasIconProtocol, hostOfUrl, siteIconUrls } from '@/utils/siteIcons'

const props = defineProps<{ items: SourceItem[] }>()

const { t } = useI18n()
const tabs = useTabsStore()
const sessions = useSessionsStore()

// ── favicon：桌面端 `astorm-icon://` 由主进程按域名做 7 天磁盘缓存与回退
//    （站点 /favicon.ico → Google 服务，带超时、失败标记与并发去重），面板只负责展示。
//    Web 端没有该协议时，siteIconUrls 仍返回原来的直连候选链，逐项失败后回退通用图标。 ──
/** hostname → 当前尝试到第几个候选 */
const stage = new Map<string, number>()
/** hostname → 全部候选均失败（显示通用地球图标） */
const failed = reactive(new Set<string>())
/** 推进阶段 / 失败后触发重渲染 */
const tick = ref(0)

/** Web 端没有主进程超时，图片 neither load nor error 时靠看门狗推进候选 */
const HANG_MS = 2500
const timers = new Map<string, ReturnType<typeof setTimeout>>()

function clearTimer(host: string): void {
  const timer = timers.get(host)
  if (timer) {
    clearTimeout(timer)
    timers.delete(host)
  }
}

function advance(host: string): void {
  const urls = siteIconUrls(`https://${host}`, 64)
  const next = (stage.get(host) ?? 0) + 1
  if (next < urls.length) stage.set(host, next)
  else failed.add(host)
  tick.value += 1
  // 新候选可能同样挂起，继续挂表（桌面端不需要）
  if (!hasIconProtocol && !failed.has(host)) arm(host)
}

function arm(host: string): void {
  if (hasIconProtocol) return
  clearTimer(host)
  timers.set(
    host,
    setTimeout(() => {
      timers.delete(host)
      advance(host)
    }, HANG_MS)
  )
}

function faviconOf(item: SourceItem): string {
  void tick.value
  if (item.kind !== 'web') return ''
  const host = hostOfUrl(item.url)
  if (!host || failed.has(host)) return ''
  return siteIconUrls(item.url, 64)[stage.get(host) ?? 0] ?? ''
}

function onFaviconLoad(item: SourceItem): void {
  if (item.kind !== 'web') return
  clearTimer(hostOfUrl(item.url))
}

function onFaviconError(item: SourceItem): void {
  if (item.kind !== 'web') return
  const host = hostOfUrl(item.url)
  if (!host) return
  clearTimer(host)
  advance(host)
}

// Web 端挂载/清单变化即挂看门狗；桌面端由主进程协议保证有界超时，不需要定时器。
watch(
  () => props.items,
  (items) => {
    if (hasIconProtocol) return
    for (const it of items) {
      if (it.kind !== 'web') continue
      const host = hostOfUrl(it.url)
      if (host && !failed.has(host)) arm(host)
    }
  },
  { immediate: true }
)
onBeforeUnmount(() => {
  for (const timer of timers.values()) clearTimeout(timer)
  timers.clear()
})

function cardTitle(item: SourceItem): string {
  return item.title
}
function cardSubtitle(item: SourceItem): string {
  if (item.kind === 'web') return item.source
  return [t('panel.memoryConcept.title'), item.observedAt].filter(Boolean).join(' · ')
}

function openCard(item: SourceItem): void {
  if (item.kind === 'web') {
    tabs.openBrowserTabFor(sessions.activeId, item.url, BROWSER_DESKTOP_1280)
    return
  }
  tabs.openMemoryConceptTab(item.conceptId, item.title)
}
</script>

<template>
  <div class="sr-root">
    <div v-if="!props.items.length" class="sr-empty">{{ t('panel.sources.empty') }}</div>
    <div v-else class="sr-list">
      <button
        v-for="(item, i) in props.items"
        :key="`${item.kind}:${i}`"
        type="button"
        class="sr-card"
        :title="item.kind === 'web' ? item.url : item.conceptId"
        @click="openCard(item)"
      >
        <div class="sr-head">
          <span class="sr-source">
            <template v-if="item.kind === 'web'">
              <img
                v-if="faviconOf(item)"
                :key="faviconOf(item)"
                class="sr-favicon"
                :src="faviconOf(item)"
                width="16"
                height="16"
                alt=""
                decoding="async"
                @load="onFaviconLoad(item)"
                @error="onFaviconError(item)"
              />
              <icon-globe v-else :size="14" />
            </template>
            <icon-memory v-else :size="14" />
            <span class="sr-source-text">{{ cardSubtitle(item) }}</span>
          </span>
          <span class="sr-index">{{ i + 1 }}</span>
        </div>
        <div class="sr-title">{{ cardTitle(item) }}</div>
        <div v-if="item.snippet" class="sr-snippet">{{ item.snippet }}</div>
      </button>
    </div>
  </div>
</template>

<style scoped>
.sr-root {
  height: 100%;
  overflow-y: auto;
  padding: 5px 4px 16px;
}
.sr-empty {
  padding: 24px 12px;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}
.sr-list {
  display: flex;
  flex-direction: column;
}
.sr-card {
  display: block;
  width: 100%;
  padding: 12px 10px;
  border: none;
  border-bottom: 1px solid var(--border-light);
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color 0.15s;
}
.sr-card:hover {
  background: var(--bg-2);
}
.sr-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.sr-source {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}
.sr-source-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 站点 favicon（图2：每条显示对应网页 logo） */
.sr-favicon {
  display: block;
  flex: none;
  border-radius: 3px;
}
.sr-index {
  flex: none;
  margin-left: auto;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: var(--bg-3);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
  line-height: 20px;
  text-align: center;
}
.sr-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  line-height: 1.45;
  color: var(--text-primary);
  /* 最多两行 */
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.sr-snippet {
  margin-top: 5px;
  font-size: var(--font-size-code);
  line-height: 1.5;
  color: var(--text-tertiary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
</style>
