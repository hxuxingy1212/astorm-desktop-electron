<script setup lang="ts">
/**
 * 一轮对话结束后的「交付物 + 文件变更」卡片（挂在助手最终答复下方）。
 *
 * 数据来自 sqlite 交付物表（`submit_deliverables` 工具逐轮上报，前端对话结束后拉取，
 * 见 stores/sessions.fetchDeliverables）；旧会话回退消息 metadata.artifacts（老数据）。
 * 展示口径：
 * - 交付物：图片优先排序，渲染为**正方形缩略图**（点击经 Arco 图片预览放大）；
 *   其余为图标 + 文件名 + 类型 / 体积卡片，点击进**文件预览页签**；
 * - 变更：一条可折叠汇总行（N 个文件 + 增删），展开列出文件，点击进**代码审查页签**并聚焦该文件；
 * - 交付物与变更各**最多展示 6 条**，超出显示「查看全部」入口；底部保留相关资料 / 相关记忆入口。
 */
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ArtifactChange, ArtifactDeliverable, MemorySource, TurnArtifacts, WebSource } from '@/types'
import { useTabsStore } from '@/stores/tabs'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'
import { getFileIconUrl } from '@/utils/fileIcons'
import FilePreviewModal from '@/components/common/FilePreviewModal.vue'

/** 交付物 / 变更各自最多展示条数（超出走「查看全部」） */
const MAX_VISIBLE = 6

const props = defineProps<{
  artifacts: TurnArtifacts
  /** 本轮网页检索结果（「相关资料」入口；为空则不显示） */
  sources?: WebSource[]
  /** 本轮记忆检索结果（「相关记忆」入口；为空则不显示） */
  memory?: MemorySource[]
  /** 会话工作区（会话记录里的值；相对路径经 resolveSessionWorkspaceDir 解析为绝对目录） */
  workspace?: string | null
}>()

const { t } = useI18n()
const tabs = useTabsStore()

const deliverables = computed(() => props.artifacts.deliverables ?? [])
const changes = computed(() => props.artifacts.changes ?? [])
/** 所有入口都为零时整块尾部 UI 不渲染（避免只剩一行 (0) 的空壳） */
const hasAnyEntry = computed(
  () =>
    deliverables.value.length > 0 ||
    changes.value.length > 0 ||
    (props.sources?.length ?? 0) > 0 ||
    (props.memory?.length ?? 0) > 0
)

/** 是否图片类交付物（按扩展名判定） */
function isImageDeliverable(d: ArtifactDeliverable): boolean {
  return IMAGE_EXT.includes(extOf(d.name))
}

/** 交付物排序：有图片内容的往前，其余保持模型给出的原顺序 */
const orderedDeliverables = computed(() => {
  const imgs = deliverables.value.filter((d) => isImageDeliverable(d))
  const files = deliverables.value.filter((d) => !isImageDeliverable(d))
  return [...imgs, ...files]
})
/**
 * 图片缩略图配额：右侧页签展开（对话区变窄）时最多 2 个，其余图片改用普通文件卡片呈现；
 * 页签收起（对话区宽）时按总上限 6 个展示。
 */
const imageThumbQuota = computed(() => (tabs.collapsed ? MAX_VISIBLE : 2))
const visibleImages = computed(() =>
  orderedDeliverables.value.filter((d) => isImageDeliverable(d)).slice(0, imageThumbQuota.value)
)
/** 卡片区：超出缩略图配额之外的图片 + 非图片交付物；与缩略图合计不超过 6 个 */
const visibleFiles = computed(() => {
  const thumbs = visibleImages.value
  const rest = orderedDeliverables.value.filter((d) => !thumbs.includes(d))
  return rest.slice(0, Math.max(0, MAX_VISIBLE - thumbs.length))
})
const visibleChanges = computed(() => changes.value.slice(0, MAX_VISIBLE))

/** 单文件变更时头部直接给「审查 / 打开」（免展开一步到位）；多文件走展开后的逐文件操作 */
const singleChange = computed(() => (changes.value.length === 1 ? changes.value[0] : null))

const totalAdditions = computed(() => changes.value.reduce((s, c) => s + (c.additions || 0), 0))
const totalDeletions = computed(() => changes.value.reduce((s, c) => s + (c.deletions || 0), 0))

/** 变更状态分类统计（新增 / 修改 / 删除，用于「10 个文件已更改」后的拆分说明） */
const statusCounts = computed(() => {
  let added = 0
  let modified = 0
  let deleted = 0
  for (const c of changes.value) {
    const s = (c.status || 'M').toUpperCase()
    if (s === 'A' || s === '?') added += 1
    else if (s === 'D') deleted += 1
    else modified += 1
  }
  return { added, modified, deleted }
})

/** 变更列表默认收起（点击汇总行展开） */
const changesExpanded = ref(false)

/** 体积文案（B / KB / MB） */
function sizeText(size?: number): string {
  if (!size) return ''
  if (size >= 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`
  if (size >= 1024) return `${Math.round(size / 1024)} KB`
  return `${size} B`
}

const DOC_EXT = ['md', 'markdown', 'mdx', 'txt']
const IMAGE_EXT = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico', 'avif']
const OFFICE_EXT = ['xls', 'xlsx', 'csv', 'doc', 'docx', 'ppt', 'pptx', 'pdf']
const DATA_EXT = ['json', 'yaml', 'yml', 'toml', 'xml']
const ARCHIVE_EXT = ['zip', 'tar', 'gz', 'rar', '7z']

function extOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i < 0 ? '' : name.slice(i + 1).toLowerCase()
}

/** 扩展名 → 类型文案（用于「文档 · MD」这类副标题） */
function kindLabel(name: string): string {
  const ext = extOf(name)
  if (DOC_EXT.includes(ext)) return '文档'
  if (IMAGE_EXT.includes(ext)) return '图片'
  if (OFFICE_EXT.includes(ext)) return '办公文档'
  if (DATA_EXT.includes(ext)) return '数据'
  if (ARCHIVE_EXT.includes(ext)) return '压缩包'
  return '文件'
}

/** 交付物副标题：类型 · 扩展名（有体积时再附加体积） */
function deliverableMeta(d: ArtifactDeliverable): string {
  const parts = [kindLabel(d.name)]
  const ext = extOf(d.name)
  if (ext) parts.push(ext.toUpperCase())
  const size = sizeText(d.size)
  if (size) parts.push(size)
  return parts.join(' · ')
}

/** git 状态字 → 徽章配色类（A/新增 绿、M/修改 橙、D/删除 红、R/C 紫、其余灰） */
function statusClass(status?: string): string {
  const s = (status || 'M').toUpperCase()
  if (s === 'A' || s === '?') return 'st-a'
  if (s === 'D') return 'st-d'
  if (s === 'R' || s === 'C') return 'st-r'
  return 'st-m'
}

/** 状态字展示：未跟踪/新增统一显示 A（与表头「新增 N」口径一致） */
function statusLabel(status?: string): string {
  const s = (status || 'M').toUpperCase()
  return s === '?' ? 'A' : s
}

function isAbsolute(p: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(p) || /^\//.test(p)
}

/** 相对工作区路径 → 绝对路径 */
async function absolutePath(path: string): Promise<string> {
  if (isAbsolute(path)) return path
  const base = (await resolveSessionWorkspaceDir(props.workspace)).replace(/[\\/]+$/, '')
  const rel = path.replace(/^[\\/]+/, '')
  return base ? `${base}/${rel}` : rel
}

/** 图片扩展名 → MIME（缩略图 data URL 用） */
function imageMime(name: string): string {
  const m: Record<string, string> = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
    webp: 'image/webp', bmp: 'image/bmp', svg: 'image/svg+xml', ico: 'image/x-icon', avif: 'image/avif'
  }
  return m[extOf(name)] ?? 'application/octet-stream'
}

/** 图片交付物缩略图缓存（path → data URL；空串 = 加载中/失败） */
const imageUrls = reactive<Record<string, string>>({})
/** 图片原始宽高（path → {w,h}），用于按自身宽高比渲染：宽度固定、高度自适应 */
const imageSizes = reactive<Record<string, { w: number; h: number }>>({})

/** 图片预览固定宽度（容器宽度即此值；高度 = 宽 × 原图高宽比） */
const IMAGE_W = 280

/** 量取图片原始尺寸（data URL）；失败回退 1×1（渲染层再兜底） */
function measureImage(url: string): Promise<{ w: number; h: number }> {
  return new Promise((resolve) => {
    const img = new Image()
    img.onload = () => resolve({ w: img.naturalWidth || 1, h: img.naturalHeight || 1 })
    img.onerror = () => resolve({ w: 1, h: 1 })
    img.src = url
  })
}

/** 图片展示高度：按原图高宽比缩放；尺寸未知时先给固定宽度（正方形占位，量到后回正） */
function imageHeight(d: ArtifactDeliverable): number {
  const s = imageSizes[d.path]
  if (!s || !s.w) return IMAGE_W
  return Math.max(40, Math.round((IMAGE_W * s.h) / s.w))
}

/** 图片副标题：原始宽×高 · 文件大小（量取/体积缺失时省略对应项） */
function imageMeta(d: ArtifactDeliverable): string {
  const parts: string[] = []
  const s = imageSizes[d.path]
  if (s && s.w && s.h) parts.push(`${s.w}×${s.h}`)
  const size = sizeText(d.size)
  if (size) parts.push(size)
  return parts.join(' · ')
}

/** 确保单张图片的 data URL 已加载（缩略图懒加载与预览共用；'' = 加载中 / 失败） */
async function ensureImageUrl(d: ArtifactDeliverable): Promise<string> {
  const cached = imageUrls[d.path]
  if (cached !== undefined) return cached
  imageUrls[d.path] = ''
  try {
    const abs = await absolutePath(d.path)
    const b64 = await window.api.fs.readFileBase64(abs)
    if (!b64) return ''
    const url = `data:${imageMime(d.name)};base64,${b64}`
    imageUrls[d.path] = url
    imageSizes[d.path] = await measureImage(url)
    return url
  } catch {
    /* 读取失败：保留空串占位 */
    return ''
  }
}

/** 懒加载可见图片交付物的缩略图（只读一次；失败留空走占位图标） */
async function loadImageThumbs(): Promise<void> {
  for (const d of visibleImages.value) {
    await ensureImageUrl(d)
  }
}
watch(visibleImages, () => void loadImageThumbs(), { immediate: true })

/** 图片预览：统一文件预览弹窗（与邮件附件同款，FilePreviewPanel 直连模式） */
const previewFile = ref<{ name: string; path: string } | null>(null)

/** 预览：解析绝对路径后打开弹窗（内容由 FilePreviewPanel 读取渲染） */
async function previewDeliverable(d: ArtifactDeliverable): Promise<void> {
  previewFile.value = { name: d.name, path: await absolutePath(d.path) }
}

/** 打开交付物 → 文件预览页签 */
async function openDeliverable(d: ArtifactDeliverable): Promise<void> {
  const abs = await absolutePath(d.path)
  // origin=deliverable:交付物预览按宽档打开(panels/index.ts 的 widthModeOfTab)
  tabs.openFileTab({ path: abs, name: d.name }, { origin: 'deliverable' })
}

/** 「审查」：打开代码审查页签并聚焦该文件（diff 由面板按需懒加载） */
function reviewChange(c: ArtifactChange): void {
  tabs.openReviewTab(c.path)
}

/** 「打开」：打开文件预览页签 */
async function openChangeFile(c: ArtifactChange): Promise<void> {
  const abs = await absolutePath(c.path)
  tabs.openFileTab({ path: abs, name: c.name })
}

/** 查看全部交付物：打开「工作区文件」页签（浏览产物所在目录），不是系统文件管理器 */
function openAllDeliverables(): void {
  tabs.openTab('workspace-files')
}

/** 查看全部变更：打开代码审查页签（完整列表 + 逐文件懒加载 diff） */
function openAllChanges(): void {
  tabs.openReviewTab()
}

/** 相关资料：打开「相关资料」页签，列出本轮网页检索结果卡片 */
function openSources(): void {
  tabs.openSourcesTab(props.sources ?? [], t('panel.sources.title'))
}

/** 相关记忆：打开「相关记忆」页签，列出本轮记忆检索结果卡片 */
function openMemoryResults(): void {
  tabs.openMemoryResultsTab(props.memory ?? [], t('panel.memoryResults.title'))
}
</script>

<template>
  <div v-if="hasAnyEntry" class="ta-root">
    <!-- 图片类交付物：正方形缩略图，点击 Arco 预览放大（排在最前） -->
    <div v-if="visibleImages.length" class="ta-images">
      <div
        v-for="d in visibleImages"
        :key="d.path"
        class="ta-image-tile"
        :title="d.path"
        @click="openDeliverable(d)"
      >
        <div class="ta-image-frame">
          <a-image
            v-if="imageUrls[d.path]"
            :src="imageUrls[d.path]"
            :width="IMAGE_W"
            :height="imageHeight(d)"
            fit="contain"
            :preview="false"
            :alt="d.name"
            class="ta-image-thumb"
          />
          <span v-else class="ta-image-ph">
            <img :src="getFileIconUrl(d.name)" width="22" height="22" alt="" />
          </span>
          <!-- 悬浮操作层：预览（弹窗大图）/ 打开（内置页签），两枚按钮居中；
               隐藏时不拦截点击（点击图块本体 = 打开页签） -->
          <div class="ta-image-hover" @click.stop>
            <button type="button" class="ta-img-act" @click="previewDeliverable(d)">
              {{ t('chat.artifacts.preview') }}
            </button>
            <button type="button" class="ta-img-act" @click="openDeliverable(d)">
              {{ t('chat.artifacts.open') }}
            </button>
          </div>
        </div>
        <!-- 文件名 + 尺寸/大小同一行，溢出省略 -->
        <span class="ta-image-caption">
          <span class="ta-image-name">{{ d.name }}</span>
          <span v-if="imageMeta(d)" class="ta-image-meta">{{ imageMeta(d) }}</span>
        </span>
      </div>
    </div>

    <!-- 其他交付物卡片（图片之后，最多合计 6 个） -->
    <div v-if="visibleFiles.length" class="ta-deliverables">
      <button
        v-for="d in visibleFiles"
        :key="d.path"
        type="button"
        class="ta-deliverable"
        :title="d.path"
        @click="openDeliverable(d)"
      >
        <img class="ta-deliverable-icon" :src="getFileIconUrl(d.name)" alt="" />
        <span class="ta-deliverable-main">
          <span class="ta-deliverable-name">{{ d.name }}</span>
          <span class="ta-deliverable-meta">{{ deliverableMeta(d) }}</span>
        </span>
        <span class="ta-deliverable-open"><icon-right :size="14" /></span>
      </button>
    </div>

    <!-- 变更汇总行（可折叠）；单文件时头部直接给「审查 / 打开」，不必展开 -->
    <div v-if="changes.length" class="ta-changes">
      <div class="ta-changes-head-row" :class="{ open: changesExpanded }">
        <button
          type="button"
          class="ta-changes-head"
          @click="changesExpanded = !changesExpanded"
        >
          <icon-right :size="12" class="ta-chev" :class="{ open: changesExpanded }" />
          <span class="ta-changes-title">
            {{ t('chat.artifacts.filesChanged', { n: changes.length }) }}
          </span>
          <span class="ta-changes-spacer" />
          <!-- 状态拆分（新增 x · 修改 y · 删除 z）与增删行数同簇：紧挨 +N / -N 左侧 -->
          <span v-if="statusCounts.added" class="ta-bd">
            {{ t('chat.artifacts.added', { n: statusCounts.added }) }}
          </span>
          <span v-if="statusCounts.modified" class="ta-bd">
            {{ t('chat.artifacts.modified', { n: statusCounts.modified }) }}
          </span>
          <span v-if="statusCounts.deleted" class="ta-bd">
            {{ t('chat.artifacts.deleted', { n: statusCounts.deleted }) }}
          </span>
          <span v-if="totalAdditions" class="ta-add">+{{ totalAdditions }}</span>
          <span v-if="totalDeletions" class="ta-del">-{{ totalDeletions }}</span>
        </button>
        <!-- 单文件：审查（代码审查页签并聚焦）/ 打开（文件预览页签），与展开行同款 -->
        <span v-if="singleChange" class="ta-change-actions ta-head-actions">
          <button type="button" class="ta-act" @click="reviewChange(singleChange)">
            {{ t('chat.artifacts.review') }}
          </button>
          <button type="button" class="ta-act" @click="openChangeFile(singleChange)">
            {{ t('chat.artifacts.open') }}
          </button>
        </span>
      </div>

      <div v-if="changesExpanded" class="ta-changes-body">
        <div v-for="c in visibleChanges" :key="c.path" class="ta-change" :title="c.path">
          <img class="ta-change-icon" :src="getFileIconUrl(c.name)" alt="" />
          <!-- git 状态徽标（A 新增 / M 修改 / D 删除 / R·C 重命名复制） -->
          <span class="ta-git-status" :class="statusClass(c.status)">{{ statusLabel(c.status) }}</span>
          <span class="ta-change-name">{{ c.name }}</span>
          <span class="ta-change-dir">{{ c.dir }}</span>
          <!-- 行内操作在数字左侧；数字列定宽右对齐，保证数字变大时按钮不移动 -->
          <span class="ta-change-actions">
            <button type="button" class="ta-act" @click="reviewChange(c)">
              {{ t('chat.artifacts.review') }}
            </button>
            <button type="button" class="ta-act" @click="openChangeFile(c)">
              {{ t('chat.artifacts.open') }}
            </button>
          </span>
          <span class="ta-change-stat">
            <span v-if="c.additions" class="ta-add">+{{ c.additions }}</span>
            <span v-if="c.deletions" class="ta-del">-{{ c.deletions }}</span>
            <!-- 二进制 / 未统计到增删时也给一个占位，避免数字区忽有忽无 -->
            <span v-if="!c.additions && !c.deletions" class="ta-stat-empty">±0</span>
          </span>
        </div>
      </div>
    </div>

    <!-- 底部一行：产物 / 变更 / 相关资料 / 相关记忆（各自有内容时才出现） -->
    <div class="ta-footer">
      <button v-if="deliverables.length" type="button" class="ta-more" @click="openAllDeliverables">
        {{ t('chat.artifacts.viewAllDeliverables', { n: deliverables.length }) }}
        <icon-right :size="11" />
      </button>
      <button v-if="changes.length" type="button" class="ta-more" @click="openAllChanges">
        {{ t('chat.artifacts.viewAllChanges', { n: changes.length }) }}
        <icon-right :size="11" />
      </button>
      <button
        v-if="(props.sources?.length ?? 0) > 0"
        type="button"
        class="ta-more"
        @click="openSources"
      >
        {{ t('chat.artifacts.viewAllSources', { n: props.sources?.length ?? 0 }) }}
        <icon-right :size="11" />
      </button>
      <button
        v-if="(props.memory?.length ?? 0) > 0"
        type="button"
        class="ta-more"
        @click="openMemoryResults"
      >
        {{ t('chat.artifacts.viewAllMemory', { n: props.memory?.length ?? 0 }) }}
        <icon-right :size="11" />
      </button>
    </div>

    <!-- 图片预览弹窗：全项目统一的文件预览弹窗（与邮件附件同款） -->
    <FilePreviewModal
      :open="!!previewFile"
      :name="previewFile?.name ?? ''"
      :path="previewFile?.path ?? ''"
      @close="previewFile = null"
    />
  </div>
</template>

<style scoped>
.ta-root {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 10px 0 6px;
}

/* ---------- 图片交付物：正方形缩略图 ---------- */
.ta-images {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}

.ta-image-tile {
  display: flex;
  width: 280px;
  min-width: 0;
  flex-direction: column;
  gap: 4px;
}

/* 正方形预览：圆角 + 发丝描边，悬停轻微浮起（class 落在 a-image 根元素上） */
.ta-image-thumb {
  display: block;
  border-radius: 12px;
  overflow: hidden;
  border: 1px solid var(--border-light);
  /* 图片完整显示（fit=contain）时的留白底色，避免透明露底 */
  background: var(--bg-1);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.05);
  transition: box-shadow 0.15s, border-color 0.15s;
}

.ta-image-tile {
  cursor: pointer;
}

.ta-image-frame {
  position: relative;
}

/* 悬浮操作层：只盖图片区（不含下方文件名行），两枚按钮水平垂直居中；
   隐藏时不拦截点击（点击落在图块本体 = 打开页签），悬浮时才显示并可交互 */
.ta-image-hover {
  position: absolute;
  inset: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  border-radius: 12px;
  background: rgba(0, 0, 0, 0.35);
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.15s;
}

.ta-image-frame:hover .ta-image-hover {
  opacity: 1;
  pointer-events: auto;
}

.ta-img-act {
  height: 30px;
  padding: 0 16px;
  border: none;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.94);
  color: #1d1d1f;
  font-family: inherit;
  font-size: var(--font-size-code);
  font-weight: 500;
  cursor: pointer;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.28);
  transition: background-color 0.15s, transform 0.15s;
}

.ta-img-act:hover {
  background: #ffffff;
}

.ta-img-act:active {
  transform: scale(0.97);
}

.ta-image-tile:hover .ta-image-thumb {
  border-color: var(--border-base);
  box-shadow: 0 3px 10px rgba(0, 0, 0, 0.12);
}

/* 加载中 / 读取失败的占位：正方形虚线块 + 文件图标 */
.ta-image-ph {
  display: flex;
  width: 280px;
  height: 280px;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border-light);
  border-radius: 12px;
  background: var(--bg-2);
}

/* 文件名 + 尺寸/大小共用一行，超宽整体省略（ellipsis 需落在容器上） */
.ta-image-caption {
  display: block;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  /* 文件名 + 尺寸/大小 居中于图片下方 */
  text-align: center;
}

.ta-image-name {
  color: var(--text-secondary);
}

.ta-image-meta {
  margin-left: 6px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

/* ---------- 交付物：自适应网格（窄屏自动单列，最多两列） ---------- */
.ta-deliverables {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 8px;
}

.ta-deliverable {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border: 1px solid var(--border-light);
  border-radius: 11px;
  background: var(--bg-2);
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  box-shadow: 0 1px 1.5px rgba(0, 0, 0, 0.04);
  transition: background-color 0.15s, border-color 0.15s, box-shadow 0.15s;
}

.ta-deliverable:hover {
  background: var(--bg-3);
  border-color: var(--border-base);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.06);
}

.ta-deliverable:active {
  background: var(--bg-hover);
}

.ta-deliverable-icon {
  display: block;
  width: 22px;
  height: 22px;
  flex: none;
}

.ta-deliverable-main {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 2px;
}

.ta-deliverable-name {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
}

.ta-deliverable-meta {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.ta-deliverable-open {
  display: inline-flex;
  flex: none;
  color: var(--text-tertiary);
  transition: color 0.15s;
}

.ta-deliverable:hover .ta-deliverable-open {
  /* 中性强调（不用蓝色）：明暗自适应的文字色阶 */
  color: var(--text-primary);
}

/* ---------- 底部「查看所有」一行入口（macOS 强调色链接；固定在对话区底部） ---------- */
/* 底部入口：普通文档流，只有文字（无背景、不固定），随对话区滚动 */
.ta-footer {
  display: flex;
  align-items: center;
  gap: 18px;
  padding: 2px 0;
}

.ta-more {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 1px 0;
  border: none;
  background: transparent;
  /* 中性文字色（不用蓝色）：亮/暗主题各自适配，避免 --color-primary-6 解析失败的不可见回退 */
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-xs);
  font-weight: 500;
  cursor: pointer;
  transition: color 0.15s;
}

.ta-more:hover {
  color: var(--text-primary);
  text-decoration: underline;
}

/* ---------- 变更汇总（macOS 分组卡片） ---------- */
.ta-changes {
  border: 1px solid var(--border-light);
  border-radius: 12px;
  background: var(--bg-2);
  box-shadow: 0 1px 1.5px rgba(0, 0, 0, 0.04);
  overflow: hidden;
}

/* 头部行：折叠按钮占满剩余宽度，单文件的审查/打开按钮排在其右（与行内 padding 对齐）；
   hover / 展开态的高亮落在整行上（按钮区也一起变色） */
.ta-changes-head-row {
  display: flex;
  align-items: center;
  transition: background-color 0.15s;
}

.ta-changes-head-row:hover,
.ta-changes-head-row.open {
  background: var(--bg-3);
}

.ta-changes-head-row .ta-changes-head {
  flex: 1;
  min-width: 0;
}

.ta-head-actions {
  flex: none;
  margin: 0 13px 0 4px;
}

.ta-changes-head {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 8px;
  padding: 10px 13px;
  border: none;
  background: transparent;
  color: var(--text-primary);
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.ta-chev {
  flex: none;
  color: var(--text-tertiary);
  transition: transform 0.15s;
}

.ta-chev.open {
  transform: rotate(90deg);
}

.ta-changes-title {
  font-size: var(--font-size-sm);
  font-weight: 500;
}

/* 状态拆分（新增 x · 修改 y · 删除 z）：次要色，紧随文件数 */
.ta-bd {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.ta-bd + .ta-bd::before {
  content: '·';
  margin: 0 5px;
}

.ta-changes-spacer {
  flex: 1;
  min-width: 0;
}

.ta-stat-empty {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.ta-add {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: #34c759;
}

.ta-del {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: #ff3b30;
}

.ta-changes-body {
  border-top: 1px solid var(--border-light);
}

.ta-change {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 8px;
  padding: 8px 13px;
  background: transparent;
  transition: background-color 0.12s;
}

.ta-change + .ta-change {
  border-top: 1px solid var(--border-light);
}

.ta-change:hover {
  background: var(--bg-3);
}

/* 行动作区：审查 / 打开两个小按钮 */
.ta-change-actions {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 6px;
  margin-left: 4px;
}

/* macOS 紧凑按钮：细描边 + 轻投影，悬停浮起 */
.ta-act {
  height: 22px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: 7px;
  background: var(--bg-1);
  color: var(--text-secondary);
  font-family: inherit;
  font-size: var(--font-size-xs);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  user-select: none;
  box-shadow: 0 1px 1px rgba(0, 0, 0, 0.04);
  transition: background-color 0.15s, color 0.15s, border-color 0.15s, box-shadow 0.15s;
}

.ta-act:hover {
  background: var(--bg-3);
  color: var(--text-primary);
  border-color: var(--text-tertiary);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}

.ta-act:active {
  background: var(--bg-hover);
  box-shadow: none;
}

.ta-change-icon {
  display: block;
  width: 16px;
  height: 16px;
  flex: none;
}

/* git 状态徽标：小圆角方块 + 系统色（与代码审查页签同款） */
.ta-git-status {
  flex: none;
  width: 15px;
  height: 15px;
  line-height: 15px;
  text-align: center;
  border-radius: 4px;
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  font-weight: 700;
}

.st-a {
  color: #34c759;
  background: rgba(52, 199, 89, 0.16);
}

.st-m {
  color: #ff9500;
  background: rgba(255, 149, 0, 0.16);
}

.st-d {
  color: #ff3b30;
  background: rgba(255, 59, 48, 0.15);
}

.st-r {
  color: #af52de;
  background: rgba(175, 82, 222, 0.16);
}

.ta-change-name {
  flex: 0 1 auto;
  max-width: 46%;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-code);
  color: var(--text-primary);
}

.ta-change-dir {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 数字列：定宽 + 右对齐（宽度按「4 位数 + 符号」取值，保证数字增大不挤动左侧按钮）。
   两数并存时（+N -M）可能超出定宽向左溢出，属可接受边界，不为此放大常态间隔 */
.ta-change-stat {
  display: inline-flex;
  flex: none;
  min-width: 44px;
  align-items: center;
  justify-content: flex-end;
  gap: 5px;
  font-variant-numeric: tabular-nums;
}
</style>
