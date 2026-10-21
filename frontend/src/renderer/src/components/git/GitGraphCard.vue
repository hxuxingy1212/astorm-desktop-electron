<script setup lang="ts">
/**
 * Git 图谱浮动卡片（顶栏分支菜单 →「Git 图谱」）。
 *
 * **只取当前分支**的历史 —— 该分支的提交序列恒为线性，所以图列就是「一条竖线 + 每行一个圆点」，
 * 不需要泳道分配算法。ref 徽章（HEAD / 本地分支 / 远程分支 / tag）来自 `%D`，
 * 并借 `local` / `remotes` 两个清单**精确区分**同名分支（`feat/x` 是本地、`origin/x` 是远程，
 * 光看有没有 `/` 分不出来）。
 *
 * 无遮罩浮层：卡片自己处理关闭（X / Esc / 点外部），与截图一致（背后界面不压暗）。
 */
import { onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  /** 仓库目录（绝对路径） */
  dir: string
  /** 当前分支（图谱只看它） */
  branch: string
  /** 本地分支清单（徽章分类用） */
  local: string[]
  /** 远程分支清单（徽章分类用） */
  remotes: string[]
}>()

const emit = defineEmits<{ close: [] }>()

const { t } = useI18n()

interface Commit {
  hash: string
  author: string
  dateISO: string
  dateRel: string
  subject: string
  decorations: string[]
}

type BadgeKind = 'head' | 'local' | 'remote' | 'tag'
interface Badge {
  kind: BadgeKind
  text: string
}

const commits = ref<Commit[]>([])
const loading = ref(false)
const error = ref('')

/** 图列行高（与 CSS 的 --gg-row-h 保持一致，SVG 要用像素值） */
const ROW_H = 30

async function load(): Promise<void> {
  if (!props.dir || !props.branch) return
  loading.value = true
  error.value = ''
  try {
    const r = await window.api.git.log(props.dir, props.branch)
    commits.value = r.commits
    error.value = r.error || ''
  } catch (e) {
    commits.value = []
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

function badgeKind(name: string): BadgeKind {
  if (props.local.includes(name)) return 'local'
  if (props.remotes.includes(name)) return 'remote'
  return 'tag'
}

/** `%D` 装饰 → 徽章列表（`HEAD -> X` 拆成 HEAD + X 两个） */
function badgesOf(decorations: string[]): Badge[] {
  const out: Badge[] = []
  for (const d of decorations) {
    if (d.startsWith('HEAD ->')) {
      out.push({ kind: 'head', text: 'HEAD' })
      const target = d.slice('HEAD ->'.length).trim()
      if (target) out.push({ kind: badgeKind(target), text: target })
    } else if (d.startsWith('tag:')) {
      out.push({ kind: 'tag', text: d.slice(4).trim() })
    } else {
      out.push({ kind: badgeKind(d), text: d })
    }
  }
  return out
}

/** ISO 时间 → `MM/DD HH:mm`（本地时区） */
function formatDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  const p = (n: number): string => String(n).padStart(2, '0')
  return `${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

function onDocMouseDown(e: MouseEvent): void {
  const target = e.target as Element | null
  if (target?.closest('.gg-card')) return
  emit('close')
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape') emit('close')
}

watch(
  () => [props.dir, props.branch] as const,
  () => void load(),
  { immediate: true }
)

document.addEventListener('mousedown', onDocMouseDown, true)
document.addEventListener('keydown', onKeydown)
onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocMouseDown, true)
  document.removeEventListener('keydown', onKeydown)
})

const isFirst = (i: number): boolean => i === 0
const isLast = (i: number): boolean => i === commits.value.length - 1
/** HEAD 行画成空心圈（与后续提交点区分） */
function isHeadRow(decorations: string[]): boolean {
  return decorations.some((d) => d.startsWith('HEAD'))
}
</script>

<template>
  <Teleport to="body">
    <div class="gg-card" role="dialog" :aria-label="t('git.graphTitle')">
      <!-- 头部：标题 + 分支 + 刷新 + 关闭 -->
      <div class="gg-head">
        <icon-mind-mapping :size="15" class="gg-head-icon" />
        <span class="gg-head-title">{{ t('git.graphTitle') }}</span>
        <span v-if="branch" class="gg-head-branch">{{ branch }}</span>
        <span class="gg-spacer" />
        <button type="button" class="gg-btn" :disabled="loading" :title="t('common.refresh')" @click="load">
          <icon-refresh :size="14" :class="{ 'gg-spin': loading }" />
        </button>
        <button type="button" class="gg-btn" :title="t('common.close')" @click="emit('close')">
          <icon-close :size="14" />
        </button>
      </div>

      <!-- 表头 -->
      <div class="gg-grid gg-head-row">
        <span>{{ t('git.colGraph') }}</span>
        <span>{{ t('git.colDesc') }}</span>
        <span>{{ t('git.colDate') }}</span>
        <span>{{ t('git.colAuthor') }}</span>
        <span>{{ t('git.colCommit') }}</span>
      </div>

      <!-- 行 -->
      <div class="gg-body">
        <div v-if="loading && !commits.length" class="gg-hint">{{ t('common.loading') }}</div>
        <div v-else-if="error" class="gg-hint gg-err">{{ t('git.loadFailed') }}：{{ error }}</div>
        <div v-else-if="!commits.length" class="gg-hint">{{ t('git.noCommits') }}</div>
        <template v-else>
          <div
            v-for="(c, i) in commits"
            :key="c.hash"
            class="gg-grid gg-row"
            :title="`${c.hash} · ${c.dateRel}`"
          >
            <!-- 图列：线性历史 = 竖线 + 圆点（首行不画上半段、末行不画下半段） -->
            <svg class="gg-graph" :width="40" :height="ROW_H" viewBox="0 0 40 30" aria-hidden="true">
              <line v-if="!isFirst(i)" x1="20" y1="0" x2="20" y2="15" />
              <line v-if="!isLast(i)" x1="20" y1="15" x2="20" y2="30" />
              <template v-if="isHeadRow(c.decorations)">
                <circle cx="20" cy="15" r="5" class="gg-ring" />
                <circle cx="20" cy="15" r="2" class="gg-dot" />
              </template>
              <circle v-else cx="20" cy="15" r="4" class="gg-dot" />
            </svg>

            <!-- 描述：subject + ref 徽章（徽章类名用 gg-b-*，避免与头部 .gg-head 撞名） -->
            <span class="gg-desc">
              <span class="gg-subject">{{ c.subject }}</span>
              <span
                v-for="(b, bi) in badgesOf(c.decorations)"
                :key="bi"
                class="gg-badge"
                :class="`gg-b-${b.kind}`"
              >
                <icon-branch v-if="b.kind === 'local' || b.kind === 'remote'" :size="10" />
                <icon-check v-else-if="b.kind === 'head'" :size="10" />
                <icon-tags v-else :size="10" />
                {{ b.text }}
              </span>
            </span>

            <span class="gg-date">{{ formatDate(c.dateISO) }}</span>
            <span class="gg-author">{{ c.author }}</span>
            <span class="gg-hash">{{ c.hash }}</span>
          </div>
        </template>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
/* 浮层无遮罩：半透底 + backdrop-blur + 细边框 + 阴影（材料同 TaskPlanPanel） */
.gg-card {
  position: fixed;
  top: 72px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 2000;
  display: flex;
  flex-direction: column;
  width: min(1080px, calc(100vw - 96px));
  max-height: calc(100vh - 140px);
  overflow: hidden;
  color: var(--text-primary);
  background: color-mix(in srgb, var(--panel-bg) 94%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  border: 0.5px solid var(--border-strong);
  border-radius: 14px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.22), 0 1px 3px rgba(0, 0, 0, 0.08);
}

/* ---------- 头部 ---------- */
.gg-head {
  display: flex;
  flex: none;
  align-items: center;
  gap: 7px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border-light);
}

.gg-head-icon {
  color: var(--text-secondary);
}

.gg-head-title {
  font-size: var(--font-size-md);
  font-weight: 600;
}

/* 分支名：次要色小字，说明这张图是哪个分支的历史 */
.gg-head-branch {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.gg-spacer {
  flex: 1;
  min-width: 0;
}

.gg-btn {
  display: inline-flex;
  width: 26px;
  height: 26px;
  flex: none;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background-color 0.15s, color 0.15s;
}

.gg-btn:hover:not(:disabled) {
  background: var(--bg-3);
  color: var(--text-primary);
}

.gg-btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.gg-spin {
  animation: gg-rot 0.9s linear infinite;
}

@keyframes gg-rot {
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 表格：Grid 共享列轨道，表头与数据行严格对齐 ---------- */
.gg-grid {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr) 118px 96px 76px;
  align-items: center;
  column-gap: 8px;
  padding: 0 12px;
}

.gg-head-row {
  flex: none;
  height: 30px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  border-bottom: 1px solid var(--border-light);
}

.gg-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  /* 面板内自滚；滚动条走全局那套（空闲隐藏/滚动时出现，同文件树口径） */
}

.gg-row {
  height: 30px;
  font-size: var(--font-size-code);
  transition: background-color 0.12s;
}

.gg-row:hover {
  background: var(--bg-hover);
}

/* ---------- 图列 ---------- */
.gg-graph {
  display: block;
  /* 竖线与圆点用同一个琥珀色（同截图） */
  color: rgb(var(--warning-6));
}

.gg-graph line {
  stroke: currentColor;
  stroke-width: 1.5;
  opacity: 0.55;
}

.gg-dot {
  fill: currentColor;
}

.gg-ring {
  fill: none;
  stroke: currentColor;
  stroke-width: 2;
}

/* ---------- 描述列 ---------- */
.gg-desc {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.gg-subject {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* ref 徽章：outline 胶囊；远程/tag 弱化 */
.gg-badge {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 3px;
  max-width: 210px;
  padding: 1px 6px;
  border-radius: 5px;
  border: 1px solid var(--border-base);
  font-size: var(--font-size-2xs);
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gg-badge.gg-b-head,
.gg-badge.gg-b-local {
  color: var(--text-primary);
}

.gg-badge.gg-b-remote,
.gg-badge.gg-b-tag {
  color: var(--text-tertiary);
}

/* ---------- 其余列 ---------- */
.gg-date,
.gg-author {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
}

.gg-hash {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.gg-hint {
  padding: 14px 12px;
  font-size: var(--font-size-code);
  color: var(--text-tertiary);
}

.gg-err {
  color: rgb(var(--danger-6));
  overflow-wrap: anywhere;
}
</style>
