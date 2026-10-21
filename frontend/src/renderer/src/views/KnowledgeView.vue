<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { invoke } from '@/services/backend'
import { getFileIconUrl } from '@/utils/fileIcons'
import FilePreviewPanel from '@/panels/FilePreviewPanel.vue'
import {
  ChevronDown,
  ChevronRight,
  ChevronsUpDown,
  Columns3,
  FolderPlus,
  Layers,
  List,
  Palette,
  Pencil,
  Search,
  Trash2,
  Upload
} from 'lucide-vue-next'
import MacFileIcon from '@/components/common/MacFileIcon.vue'
import DeliverablesPanel from '@/components/deliverables/DeliverablesPanel.vue'

const { t } = useI18n()

/** 页签:交付物(对话产物,按时间分组)/ 资料库(原文件管理器);名称=交付物/资料库 */
const activeTab = ref<'deliverables' | 'library'>('deliverables')

/** Mac(Finder) 风格文件夹图标配色 */
const COLOR_OPTIONS = [
  { key: 'blue', value: '#0a84ff' },
  { key: 'cyan', value: '#32ade6' },
  { key: 'teal', value: '#30b0c7' },
  { key: 'green', value: '#34c759' },
  { key: 'yellow', value: '#ffd60a' },
  { key: 'orange', value: '#ff9f0a' },
  { key: 'pink', value: '#ff375f' },
  { key: 'purple', value: '#bf5af2' },
  { key: 'indigo', value: '#5856d6' },
  { key: 'gray', value: '#8e8e93' },
  { key: 'brown', value: '#a2845e' }
]

function colorValue(key: string): string {
  return COLOR_OPTIONS.find((c) => c.key === key)?.value ?? '#34c759'
}

// ---------- 数据模型（真实后端：kb_* 系列 = 文件系统 + 元数据清单） ----------
type ItemType = 'folder' | 'document'
interface KbItem {
  id: string
  type: ItemType
  name: string
  color: string
  tags: string[]
  parentId: string | null
  createdAt: number
  updatedAt: number
  /** 文档大小(字节);文件夹为 0,展示时由子项汇总 */
  size: number
  /** 绝对路径(打开预览页签用) */
  absPath: string
}

const items = ref<KbItem[]>([])
const loading = ref(false)

/** napi KbItem(camelCase,nodeType)→ 视图模型 */
function toView(x: Record<string, unknown>): KbItem {
  return {
    id: String(x.id ?? ''),
    type: (x.type ?? x.nodeType) === 'folder' ? 'folder' : 'document',
    name: String(x.name ?? ''),
    color: String(x.color ?? 'blue'),
    tags: Array.isArray(x.tags) ? (x.tags as string[]) : [],
    parentId: x.parentId != null ? String(x.parentId) : null,
    createdAt: Number(x.createdAt ?? 0),
    updatedAt: Number(x.updatedAt ?? 0),
    size: Number(x.size ?? 0),
    absPath: String(x.absPath ?? '')
  }
}

// ---------- 文件预览(中心区;复用文件树页签预览体系 FilePreviewPanel) ----------
const previewItem = ref<KbItem | null>(null)

/** 点击文档:记录选中项,中心区由 FilePreviewPanel 渲染(与文件树页签同一套预览) */
function openPreview(item: KbItem): void {
  if (item.type !== 'document') return
  previewItem.value = item
}

function closePreview(): void {
  previewItem.value = null
}

/** markdown 相对图片内联的知识库相对目录(仅文档传入) */
function previewImageBaseRel(item: KbItem): string {
  return item.id.includes('/') ? item.id.slice(0, item.id.lastIndexOf('/')) : ''
}

/**
 * 预览列滚轮:横向滑动意图(触控板横扫 / Shift+滚轮)不进入查看器内部,
 * 而是驱动整个分栏条左右滑动——「整体滑动调整窗口视图」;纵向滚动仍留给内容。
 */
function onPreviewWheel(e: WheelEvent): void {
  const dx = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.shiftKey ? e.deltaY : 0
  if (!dx) return
  const el = columnsEl.value
  if (!el) return
  e.preventDefault()
  const max = el.scrollWidth - el.clientWidth
  el.scrollLeft = Math.min(max, Math.max(0, el.scrollLeft + dx))
}


/** 点击条目:选中 + 文档打开预览 */
function onItemClick(item: KbItem): void {
  onSelect(item.id)
  void openPreview(item)
}
/** 拉取整棵树(真实文件系统);失败toast后端原因 */
async function loadItems(): Promise<void> {
  loading.value = true
  try {
    const list = (await invoke('kb_list')) as Array<Record<string, unknown>>
    items.value = (Array.isArray(list) ? list : []).map(toView)
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    loading.value = false
  }
}

// ---------- 导航(Finder:面包屑 + 前进/后退/上级) ----------
const path = ref<string[]>([]) // 到达当前 folder 的 id 栈(空 = 根)
const viewMode = ref<ViewMode>('list') // list(列表) / columns(分栏)
const sortBy = ref<SortKey>('name')
const query = ref('')
const searchOpen = ref(false) // 搜索图标 → 圆角搜索框展开

type ViewMode = 'list' | 'columns'
/** 分栏视图下,当前每列选中的 folder id 链(不含根);空数组 = 只看根一层 */
const columnPath = ref<string[]>([])

type SortKey = 'name' | 'type' | 'updated' | 'created' | 'size'
const SORT_OPTIONS: { key: SortKey; label: string }[] = [
  { key: 'name', label: 'knowledge.sortName' },
  { key: 'type', label: 'knowledge.sortType' },
  { key: 'updated', label: 'knowledge.sortUpdated' },
  { key: 'created', label: 'knowledge.sortCreated' },
  { key: 'size', label: 'knowledge.sortSize' }
]

/** 各排序键的默认方向(true = 升序);切换排序键时回到默认方向 */
const NATURAL_ASC: Record<SortKey, boolean> = {
  name: true,
  type: true,
  size: false,
  updated: false,
  created: false
}
const sortAsc = ref(true)

function setSort(key: SortKey): void {
  sortBy.value = key
  sortAsc.value = NATURAL_ASC[key]
}
/** 列头点击:同键翻转方向,异键切换并回到默认方向 */
function toggleSort(key: SortKey): void {
  if (sortBy.value === key) sortAsc.value = !sortAsc.value
  else setSort(key)
}
function toggleSearch(): void {
  searchOpen.value = !searchOpen.value
  if (!searchOpen.value) query.value = ''
}
function closeSearch(): void {
  searchOpen.value = false
  query.value = ''
}

/** 文件夹大小 = 其下所有后代文档的 size 之和(递归);文档取自身 size */
function itemSize(id: string): number {
  const self = items.value.find((i) => i.id === id)
  if (!self) return 0
  if (self.type === 'document') return self.size || 0
  return items.value.filter((i) => i.parentId === id).reduce((sum, c) => sum + itemSize(c.id), 0)
}

/** 字节 → 展示文本(0/空 显示 --,与 Finder 一致) */
function formatSize(bytes: number): string {
  if (!bytes) return '--'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

const currentId = computed<string | null>(() => path.value[path.value.length - 1] ?? null)
const currentFolder = computed<KbItem | null>(() =>
  currentId.value ? (items.value.find((i) => i.id === currentId.value) ?? null) : null
)

const crumbStack = computed<KbItem[]>(() => {
  const res: KbItem[] = []
  let cur = currentFolder.value
  while (cur) {
    res.unshift(cur)
    const parent = items.value.find((i) => i.id === cur?.parentId)
    cur = parent ?? null
  }
  return res
})

const children = computed<KbItem[]>(() => {
  const list = items.value.filter((i) => i.parentId === currentId.value)
  const q = query.value.trim().toLowerCase()
  if (q) return list.filter((i) => i.name.toLowerCase().includes(q) || i.tags.some((tag) => tag.toLowerCase().includes(q)))
  return list
})

/** 按当前排序键 + 方向排序 */
function sortList(list: KbItem[]): KbItem[] {
  const dir = sortAsc.value ? 1 : -1
  const byName = (a: KbItem, b: KbItem): number => a.name.localeCompare(b.name, 'zh-Hans-CN')
  const arr = [...list]
  arr.sort((a, b) => {
    // 类型排序:文件夹/文档各自成组,方向决定哪组在前(组内按名称)
    if (sortBy.value === 'type') {
      if (a.type !== b.type) return dir * (a.type === 'folder' ? -1 : 1)
      return dir * byName(a, b)
    }
    // 其余排序键:文件夹恒在前,文档在后
    if (a.type !== b.type) return a.type === 'folder' ? -1 : 1
    switch (sortBy.value) {
      case 'updated':
        return dir * (a.updatedAt - b.updatedAt)
      case 'created':
        return dir * (a.createdAt - b.createdAt)
      case 'size':
        return dir * (itemSize(a.id) - itemSize(b.id))
      default:
        return dir * byName(a, b)
    }
  })
  return arr
}

const sortedChildren = computed<KbItem[]>(() => sortList(children.value))

// ---------- 分栏(column)视图:每列显示上一列选中 folder 的子项 ----------
/** 分栏中某一列的子项列表。index 0 = 根层;index k = columnPath[k-1] 的子项。
 *  搜索词同样应用于分栏。 */
function columnChildren(colIndex: number): KbItem[] {
  const parentId = colIndex === 0 ? null : (columnPath.value[colIndex - 1] ?? null)
  const list = items.value.filter((i) => i.parentId === parentId)
  const q = query.value.trim().toLowerCase()
  const filtered = q
    ? list.filter((i) => i.name.toLowerCase().includes(q) || i.tags.some((tag) => tag.toLowerCase().includes(q)))
    : list
  return sortList(filtered)
}

/** 分栏视图的列:第 0 列恒为根层;其后每列 = 上一列选中 folder 的子项。
 *  与 Finder 一致:选中的文件夹若为空(或搜索后无可见子项),不追加空列。 */
const columns = computed<KbItem[][]>(() => {
  const cols: KbItem[][] = [columnChildren(0)]
  for (let k = 0; k < columnPath.value.length; k++) {
    const selItem = items.value.find((x) => x.id === columnPath.value[k])
    if (!selItem || selItem.type !== 'folder') break
    const col = columnChildren(k + 1)
    if (!col.length) break
    cols.push(col)
  }
  return cols
})

/** 分栏某列被点击:文件夹 → 高亮并向右展开下一列;文档 → 仅选中 */
function onColumnClick(item: KbItem, colIndex: number): void {
  selectedId.value = item.id
  if (item.type !== 'folder') {
    // 上层文档:截断更深层的列与面包屑(Finder 行为:点上层文件即收起下级)
    columnPath.value = columnPath.value.slice(0, colIndex)
    path.value = path.value.slice(0, colIndex)
    void openPreview(item)
    return
  }
  columnPath.value = columnPath.value.slice(0, colIndex)
  columnPath.value.push(item.id)
  // 分栏导航时同步面包屑 path,保持一致
  path.value = buildPathTo(item.id)
}

/** 行高亮(Finder 语义):
 *  active = 当前点选行(最后点击的文件夹或文档)→ 灰底;
 *  in-path = 下钻路径上更早选中的文件夹 → 灰底 */
function isRowActive(item: KbItem): boolean {
  return selectedId.value === item.id
}
function isRowInPath(item: KbItem, colIndex: number): boolean {
  return columnPath.value[colIndex] === item.id && selectedId.value !== item.id
}
function isRowHighlighted(item: KbItem, colIndex: number): boolean {
  return isRowActive(item) || isRowInPath(item, colIndex)
}

// 新列出现时自动横向滚动到最右,保证最新一列可见(Finder 行为)
const columnsEl = ref<HTMLElement | null>(null)
watch(
  () => columns.value.length,
  async () => {
    await nextTick()
    const el = columnsEl.value
    if (el) el.scrollTo({ left: el.scrollWidth, behavior: 'smooth' })
  }
)
// 分栏视图:预览作为「下一栏」出现时,同样滚到最右
watch(previewItem, async () => {
  if (viewMode.value !== 'columns') return
  await nextTick()
  const el = columnsEl.value
  if (el) el.scrollTo({ left: el.scrollWidth, behavior: 'smooth' })
})


function buildPathTo(id: string): string[] {
  const chain: string[] = []
  let cur = items.value.find((i) => i.id === id) ?? null
  while (cur) {
    chain.unshift(cur.id)
    const pid: string | null = cur.parentId
    const parent = pid ? items.value.find((i) => i.id === pid) : null
    cur = parent ?? null
  }
  return chain
}

function goTo(id: string | null): void {
  path.value = id ? buildPathTo(id) : []
  selectedId.value = id
}

function goHome(): void {
  goTo(null)
  selectedId.value = null
}

/** 打开条目:文件夹 → 进入该层;文档 → 不响应(仅选中) */
function openItem(item: KbItem): void {
  if (item.type === 'folder') {
    goTo(item.id)
  }
}

// ---------- 选中 & 操作 ----------
const selectedId = ref<string | null>(null)
const selected = computed<KbItem | null>(() => items.value.find((i) => i.id === selectedId.value) ?? null)

// 分栏视图与面包屑 path 保持同步:path 变化时重建 columnPath(仅限该路径上有 folder 的链)。
// 选中态不在此同步——由 goTo / 列点击各自维护(点文档时面包屑截断但文档保持高亮)。
// 注:需放在 selectedId 声明之后(immediate 回调会立即执行一次)。
watch(
  path,
  (p) => {
    columnPath.value = p.filter((id) => items.value.find((i) => i.id === id)?.type === 'folder')
  },
  { immediate: true }
)

// 预览跟随选中态:当前选中不是文档(文件夹/空)时关闭预览
watch(selectedId, (id) => {
  if (!id) {
    closePreview()
    return
  }
  const it = items.value.find((i) => i.id === id)
  if (it?.type !== 'document') closePreview()
})

function onSelect(id: string): void {
  selectedId.value = id
}

// 新建 / 编辑弹窗
const modalVisible = ref(false)
const editingId = ref<string | null>(null)
const form = reactive({
  name: '',
  type: 'folder' as ItemType,
  color: 'blue',
  tags: [] as string[]
})
const tagInput = ref('')

function addTag(): void {
  const val = tagInput.value.trim().replace(/\s*[,，]\s*$/, '').trim()
  if (val && !form.tags.includes(val)) form.tags.push(val)
  tagInput.value = ''
}

/** 标签使用统计(key → 次数),由当前全部条目派生;新建页回显最高的 5 个做快捷方式 */
const tagStats = computed<Array<{ tag: string; count: number }>>(() => {
  const counts = new Map<string, number>()
  for (const it of items.value) {
    for (const t of it.tags) counts.set(t, (counts.get(t) ?? 0) + 1)
  }
  return [...counts.entries()]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag, 'zh-Hans-CN'))
    .slice(0, 5)
})

/** 快捷标签:已选则移除,未选则加入 */
function toggleQuickTag(tag: string): void {
  const idx = form.tags.indexOf(tag)
  if (idx >= 0) form.tags.splice(idx, 1)
  else form.tags.push(tag)
}

function presetNew(): void {
  editingId.value = null
  form.name = ''
  form.type = 'folder'
  form.color = 'blue'
  form.tags = []
  tagInput.value = ''
  modalVisible.value = true
}

function presetEdit(item: KbItem): void {
  editingId.value = item.id
  form.name = item.name
  form.color = item.color
  form.tags = [...item.tags]
  tagInput.value = ''
  modalVisible.value = true
}

async function confirmSave(): Promise<void> {
  const name = form.name.trim()
  if (!name) {
    Message.warning(t('knowledge.nameRequired'))
    return
  }
  try {
    if (editingId.value) {
      // 编辑:名称变更 → kb_rename(后端同步迁移后代元数据);图标/颜色/标签 → kb_update_meta
      let currentId = editingId.value
      const original = items.value.find((i) => i.id === editingId.value)
      if (original && original.name !== name) {
        const renamed = (await invoke('kb_rename', { path: editingId.value, new_name: name })) as Record<string, unknown>
        currentId = toView(renamed).id
        if (previewItem.value?.id === editingId.value) closePreview()
      }
      await invoke('kb_update_meta', {
        meta: { path: currentId, color: form.color, tags: [...form.tags] }
      })
    } else {
      // 新建:父目录 = 当前所在文件夹(空串 = 根层)
      await invoke('kb_create', {
        req: {
          parent: currentId.value ?? '',
          name,
          kind: form.type,
          color: form.color,
          tags: [...form.tags]
        }
      })
    }
    modalVisible.value = false
    await loadItems()
    Message.success(t('common.saveSuccess'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

function confirmDelete(): void {
  if (!selected.value) return
  const it = selected.value
  void macConfirm({
    title: t('knowledge.deleteTitle'),
    content: t('knowledge.deleteContent', { name: it.name }),
    tone: 'danger',
    confirmText: t('common.delete'),
    onConfirm: async () => {
      try {
        await invoke('kb_delete', { path: it.id })
        if (previewItem.value?.id === it.id) closePreview()
        await loadItems()
        // 当前所在目录可能已被删除 → 回根;选中态同步清理
        if (currentId.value && !items.value.find((i) => i.id === currentId.value)) goTo(null)
        if (selectedId.value === it.id) selectedId.value = null
      } catch (e) {
        Message.error(e instanceof Error ? e.message : String(e))
      }
    }
  })
}

// ---------- 导入(系统对话框多选本地文件/目录 → 真实拷入当前文件夹) ----------
async function handleImport(): Promise<void> {
  try {
    const paths = await window.api.dialog.openFiles()
    if (!paths?.length) return
    const imported = (await invoke('kb_import', { parent: currentId.value ?? '', paths })) as unknown[]
    await loadItems()
    Message.success(t('knowledge.importSuccess', { count: imported.length }))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

// ---------- 右键菜单 ----------
const ctxMenu = reactive<{ visible: boolean; x: number; y: number; item: KbItem | null }>({
  visible: false,
  x: 0,
  y: 0,
  item: null
})
function onContext(e: MouseEvent, item: KbItem): void {
  selectedId.value = item.id
  const W = 168
  const H = 3 * 32 + 8
  ctxMenu.x = Math.max(4, Math.min(e.clientX, window.innerWidth - W - 4))
  ctxMenu.y = Math.max(4, Math.min(e.clientY, window.innerHeight - H - 4))
  ctxMenu.item = item
  ctxMenu.visible = true
}
function hideCtx(): void {
  ctxMenu.visible = false
  ctxMenu.item = null
}
function onCtxAction(action: 'open' | 'edit' | 'delete'): void {
  const item = ctxMenu.item
  hideCtx()
  if (!item) return
  if (action === 'open') openItem(item)
  else if (action === 'edit') presetEdit(item)
  else {
    selectedId.value = item.id
    confirmDelete()
  }
}

/** 资料库懒加载:默认页签是交付物,首次切到「资料库」才扫文件系统(避免每次进页面都全量扫盘) */
const libraryLoaded = ref(false)

async function ensureLibrary(): Promise<void> {
  if (libraryLoaded.value) return
  libraryLoaded.value = true
  await loadItems()
}

watch(
  activeTab,
  (tab) => {
    if (tab === 'library') void ensureLibrary()
  },
  { immediate: true }
)

onMounted(() => {
  window.addEventListener('contextmenu', hideCtx, true)
})
onUnmounted(() => {
  window.removeEventListener('contextmenu', hideCtx, true)
})

defineOptions({ name: 'KnowledgeView' })
</script>

<template>
  <div class="page">
    <!-- 页签:交付物 / 资料库(macOS 分段胶囊;交付物=对话产物分组列表,资料库=原文件管理器) -->
    <div class="page-tabs">
      <button
        type="button"
        class="page-tab"
        :class="{ active: activeTab === 'deliverables' }"
        @click="activeTab = 'deliverables'"
      >
        {{ t('knowledge.tabDeliverables') }}
      </button>
      <button
        type="button"
        class="page-tab"
        :class="{ active: activeTab === 'library' }"
        @click="activeTab = 'library'"
      >
        {{ t('knowledge.tabLibrary') }}
      </button>
    </div>

    <!-- ── 资料库页签:原 Finder 风格文件管理器(v-show 保持挂载,页签往返不重扫/不丢滚动位) ── -->
    <div v-show="activeTab === 'library'" class="tab-host">
    <!-- Finder 风格工具栏:面包屑 + 视图切换 / 搜索 / 新建 -->
    <div class="fm-toolbar">
      <div class="fm-breadcrumb">
        <a-button class="crumb-root-btn" type="text" size="small" @click="goHome">
          {{ t('knowledge.title') }}
        </a-button>
        <template v-for="(crumb, idx) in crumbStack" :key="crumb.id">
          <span class="crumb-sep">›</span>
          <a-button
            class="crumb-btn"
            :class="{ 'crumb-active': idx === crumbStack.length - 1 }"
            type="text"
            size="small"
            @click="goTo(crumb.id)"
          >
            {{ crumb.name }}
          </a-button>
        </template>
        <!-- 列表路径:文件名追加到面包屑末尾(预览无独立标题头部,文件名在路径上呈现) -->
        <template v-if="previewItem">
          <span class="crumb-sep">›</span>
          <span class="crumb-file" :title="previewItem.name">{{ previewItem.name }}</span>
        </template>
        <!-- 条目数量:紧跟文件夹名称之后 -->
        <span class="fm-count">{{ t('knowledge.items', { count: children.length }) }}</span>
      </div>

      <div class="fm-actions">
        <!-- Finder 分段胶囊:图标切换,无文字标签(图标与 macOS 一致) -->
        <div class="fm-view-seg">
          <a-tooltip :content="t('knowledge.viewList')" position="bottom">
            <button class="fm-view-btn" :class="{ active: viewMode === 'list' }" @click="viewMode = 'list'">
              <List :size="15" :stroke-width="1.8" />
            </button>
          </a-tooltip>
          <a-tooltip :content="t('knowledge.viewColumns')" position="bottom">
            <button class="fm-view-btn" :class="{ active: viewMode === 'columns' }" @click="viewMode = 'columns'">
              <Columns3 :size="15" :stroke-width="1.8" />
            </button>
          </a-tooltip>
        </div>

        <!-- 排序:mac 样式图标下拉(名称/修改日期/创建日期/大小/标签分类) -->
        <a-dropdown class="fm-sort-dropdown" trigger="click" position="br" @select="(v) => setSort(String(v) as SortKey)">
          <button class="fm-sort-btn" :title="t('knowledge.sort')">
            <ChevronsUpDown :size="16" />
          </button>
          <template #content>
            <a-doption
              v-for="opt in SORT_OPTIONS"
              :key="opt.key"
              :value="opt.key"
              :active="sortBy === opt.key"
            >
              {{ t(opt.label) }}
            </a-doption>
          </template>
        </a-dropdown>

        <!-- 搜索:图标按钮,点击后向左弹开圆角搜索框 -->
        <div class="fm-search">
          <transition name="fm-search">
            <div v-if="searchOpen" class="fm-search-box">
              <input
                v-model="query"
                type="text"
                class="fm-search-input"
                :placeholder="t('knowledge.searchPlaceholder')"
                spellcheck="false"
                autofocus
                @keydown.esc="closeSearch"
              />
            </div>
          </transition>
          <button class="fm-search-toggle" :class="{ active: searchOpen }" :title="t('knowledge.search')" @click="toggleSearch">
            <Search :size="16" />
          </button>
        </div>

        <a-button class="fm-tool-btn" @click="presetNew">
          <template #icon><FolderPlus :size="14" /></template>
          {{ t('knowledge.newFolder') }}
        </a-button>
        <a-button class="fm-tool-btn" @click="handleImport">
          <template #icon><Upload :size="14" /></template>
          {{ t('knowledge.import') }}
        </a-button>
      </div>
    </div>

    <!-- 主体:卡片网格(grid) / 列表(list) / 分栏(columns) -->
    <div
      class="fm-body"
      :class="{ 'fm-body-flush': viewMode === 'columns', 'fm-body-preview': !!previewItem && viewMode !== 'columns' }"
    >
      <div v-if="!(previewItem && viewMode !== 'columns')" class="fm-browser" :class="{ 'fm-browser-flush': viewMode === 'columns' }">
      <!-- 空态:分栏模式下根层为空时由末尾分支处理;此处仅管 grid/list -->
      <div v-if="viewMode !== 'columns' && !sortedChildren.length" class="fm-empty">
        <Layers :size="44" :stroke-width="1.2" />
        <p>{{ t('knowledge.empty') }}</p>
      </div>

      <div v-else-if="viewMode === 'list'" class="fm-list">
        <div class="fm-list-head">
          <button class="l-head l-name" :class="{ sorted: sortBy === 'name' }" @click="toggleSort('name')">
            <span class="l-head-text">{{ t('knowledge.colName') }}</span>
            <ChevronDown v-if="sortBy === 'name'" class="l-head-arrow" :class="{ asc: sortAsc }" :size="11" :stroke-width="2.2" />
          </button>
          <span class="l-tags">{{ t('knowledge.colTags') }}</span>
          <button class="l-head l-type" :class="{ sorted: sortBy === 'type' }" @click="toggleSort('type')">
            <span class="l-head-text">{{ t('knowledge.colType') }}</span>
            <ChevronDown v-if="sortBy === 'type'" class="l-head-arrow" :class="{ asc: sortAsc }" :size="11" :stroke-width="2.2" />
          </button>
          <button class="l-head l-size" :class="{ sorted: sortBy === 'size' }" @click="toggleSort('size')">
            <span class="l-head-text">{{ t('knowledge.colSize') }}</span>
            <ChevronDown v-if="sortBy === 'size'" class="l-head-arrow" :class="{ asc: sortAsc }" :size="11" :stroke-width="2.2" />
          </button>
          <button class="l-head l-time" :class="{ sorted: sortBy === 'updated' }" @click="toggleSort('updated')">
            <span class="l-head-text">{{ t('knowledge.colUpdated') }}</span>
            <ChevronDown v-if="sortBy === 'updated'" class="l-head-arrow" :class="{ asc: sortAsc }" :size="11" :stroke-width="2.2" />
          </button>
          <span class="l-path">{{ t('knowledge.colPath') }}</span>
        </div>
        <div
          v-for="item in sortedChildren"
          :key="item.id"
          class="fm-list-row"
          :class="{ selected: selectedId === item.id }"
          @click="onItemClick(item)"
          @dblclick="openItem(item)"
          @contextmenu.prevent="onContext($event, item)"
        >
          <span class="l-name">
            <img v-if="item.type === 'document'" class="fm-file-glyph" :src="getFileIconUrl(item.name)" width="18" height="18" alt="" />
            <MacFileIcon v-else :type="item.type" :color="colorValue(item.color)" :size="20" />
            <span class="l-name-text">{{ item.name }}</span>
          </span>
          <span class="l-tags">
            <span v-for="tag in item.tags.slice(0, 3)" :key="tag" class="fm-tag">{{ tag }}</span>
            <span v-if="item.tags.length > 3" class="fm-tag fm-tag-more">+{{ item.tags.length - 3 }}</span>
          </span>
          <span class="l-type">{{ item.type === 'folder' ? t('knowledge.folder') : t('knowledge.document') }}</span>
          <span class="l-size">{{ formatSize(item.type === 'folder' ? itemSize(item.id) : item.size) }}</span>
          <span class="l-time">{{ new Date(item.updatedAt).toLocaleString() }}</span>
          <span class="l-path" :title="item.id">{{ item.id }}</span>
        </div>
      </div>

      <!-- 分栏视图(Finder Column View):无列标题;点击 folder → 右侧展开下一列;
           选中行统一灰底;超出宽度横向滚动并自动滚到最新列。
           进入空文件夹时列结构保留(仅高亮选中行,不追加空列) -->
      <div v-else-if="columnChildren(0).length" ref="columnsEl" class="fm-columns">
        <div
          v-for="(col, cIdx) in columns"
          :key="cIdx === 0 ? 'root' : (columnPath[cIdx - 1] ?? cIdx)"
          class="fm-column"
        >
          <div class="fm-column-body">
            <div
              v-for="item in col"
              :key="item.id"
              class="fm-col-row"
              :class="{ active: isRowActive(item), 'in-path': isRowInPath(item, cIdx) }"
              @click="onColumnClick(item, cIdx)"
              @contextmenu.prevent="onContext($event, item)"
            >
              <img v-if="item.type === 'document'" class="fm-file-glyph" :src="getFileIconUrl(item.name)" width="16" height="16" alt="" />
              <MacFileIcon v-else :type="item.type" :color="colorValue(item.color)" :size="16" />
              <span class="fm-col-name" :title="item.name">{{ item.name }}</span>
              <!-- 访达:仅高亮的文件夹行在右缘显示下级箭头 -->
              <span v-if="item.type === 'folder' && isRowHighlighted(item, cIdx)" class="fm-col-caret">
                <ChevronRight :size="14" :stroke-width="2.2" />
              </span>
            </div>
          </div>
        </div>

        <!-- 预览「下一栏」:像打开下一栏一样参与横向滚动,整页宽度 -->
        <div v-if="previewItem" class="fm-preview-column" @wheel.capture="onPreviewWheel">
          <div class="fm-preview-body">
            <FilePreviewPanel
              :file-path="previewItem.absPath"
              :file-name="previewItem.name"
              :image-base-rel="previewImageBaseRel(previewItem)"
            />
          </div>
        </div>
      </div>
      <!-- 分栏模式下根层为空:整片空态 -->
      <div v-else class="fm-empty">
        <Layers :size="44" :stroke-width="1.2" />
        <p>{{ t('knowledge.empty') }}</p>
      </div>
      </div>

      <!-- 中心区:文件预览(FilePreviewPanel;grid/list 整页形态) -->
      <div v-if="previewItem && viewMode !== 'columns'" class="fm-preview">
        <div class="fm-preview-body">
          <FilePreviewPanel
            :file-path="previewItem.absPath"
            :file-name="previewItem.name"
            :image-base-rel="previewImageBaseRel(previewItem)"
          />
        </div>
      </div>
    </div>

    <!-- 新建 / 编辑弹窗(Mac sheet 风格;主题色经 --accent-text/--bg-* 适配明暗) -->
    <a-modal
      v-model:visible="modalVisible"
      :title="editingId ? t('knowledge.editTitle') : t('knowledge.createTitle')"
      width="440px"
      modal-class="fm-modal"
      title-align="start"
      :mask-closable="false"
      :hide-cancel="true"
    >
      <div class="fm-form">
        <div class="fm-form-item">
          <label class="fm-form-label">{{ t('knowledge.name') }}</label>
          <a-input v-model="form.name" :placeholder="t('knowledge.namePlaceholder')" />
        </div>

        <div class="fm-form-item">
          <label class="fm-form-label">{{ t('knowledge.color') }}</label>
          <div class="fm-colors">
            <button
              v-for="c in COLOR_OPTIONS"
              :key="c.key"
              type="button"
              class="fm-color"
              :class="{ active: form.color === c.key }"
              :style="{ background: c.value }"
              :title="c.key"
              @click="form.color = c.key"
            />
          </div>
        </div>

        <div class="fm-form-item">
          <label class="fm-form-label">{{ t('knowledge.tags') }}</label>
          <div v-if="tagStats.length" class="fm-tag-quick">
            <button
              v-for="s2 in tagStats"
              :key="s2.tag"
              type="button"
              class="fm-tag fm-tag-quick"
              :class="{ active: form.tags.includes(s2.tag) }"
              @click="toggleQuickTag(s2.tag)"
            >
              {{ s2.tag }}
              <span class="fm-tag-count">{{ s2.count }}</span>
            </button>
          </div>
          <div class="fm-tag-editor">
            <span v-for="(tag, idx) in form.tags" :key="tag" class="fm-tag fm-tag-removable">
              {{ tag }}
              <button class="fm-tag-remove" @click="form.tags.splice(idx, 1)">×</button>
            </span>
            <a-input
              v-model="tagInput"
              :placeholder="t('knowledge.tagsPlaceholder')"
              size="small"
              class="fm-tag-input"
              @press-enter="addTag"
              @blur="addTag"
            />
          </div>
        </div>
      </div>

      <template #footer>
        <div class="fm-modal-footer">
          <button type="button" class="fm-btn" @click="modalVisible = false">{{ t('common.cancel') }}</button>
          <button type="button" class="fm-btn primary" @click="confirmSave">{{ t('common.save') }}</button>
        </div>
      </template>
    </a-modal>

    <!-- 右键菜单 -->
    <Teleport to="body">
      <div
        v-if="ctxMenu.visible"
        class="fm-ctx"
        :style="{ left: `${ctxMenu.x}px`, top: `${ctxMenu.y}px` }"
        @contextmenu.prevent
      >
        <button type="button" class="fm-ctx-item" @click="onCtxAction('open')">
          <Palette :size="15" />
          <span>{{ t('knowledge.open') }}</span>
        </button>
        <button type="button" class="fm-ctx-item" @click="onCtxAction('edit')">
          <Pencil :size="15" />
          <span>{{ t('knowledge.edit') }}</span>
        </button>
        <div class="fm-ctx-divider" />
        <button type="button" class="fm-ctx-item danger" @click="onCtxAction('delete')">
          <Trash2 :size="15" />
          <span>{{ t('knowledge.delete') }}</span>
        </button>
      </div>
    </Teleport>
    </div>

    <!-- ── 交付物页签:跨会话对话产物,按 今天/昨天/近一周/近一月 分组 ── -->
    <DeliverablesPanel v-show="activeTab === 'deliverables'" class="dv-host" />
  </div>
</template>

<style scoped>
.page {
  height: 100%;
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  padding: 16px 24px;
  overflow: hidden;
  background-color: var(--panel-bg);
}

/* ===== 页签:交付物 / 资料库(macOS 分段胶囊,同设置页工具筛选) ===== */
.page-tabs {
  display: flex;
  align-items: center;
  gap: 4px;
  width: fit-content;
  max-width: 100%;
  padding: 3px 6px;
  margin-bottom: 12px;
  flex-shrink: 0;
  background: var(--bg-2);
  border-radius: 10px;
}

.page-tab {
  font-size: var(--font-size-sm);
  padding: 5px 14px;
  line-height: 1.4;
  border: none;
  background: transparent;
  border-radius: 6px;
  color: var(--text-secondary);
  font-family: inherit;
  cursor: pointer;
  transition: all 0.15s ease;
}

.page-tab:hover {
  color: var(--text-primary);
  background: var(--bg-3);
}

.page-tab.active {
  color: var(--accent-text);
  background: var(--bg-base);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

/* 交付物页签宿主:占满页签下方剩余空间(面板内部自带滚动) */
.dv-host {
  flex: 1;
  min-height: 0;
}

/* 资料库页签宿主:v-show 常挂载,内部还原原 flex 纵向布局 */
.tab-host {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* ===== 工具栏 ===== */
.fm-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 8px 4px 12px;
  border-bottom: 1px solid var(--border-light);
}

/* 面包屑 */
.fm-breadcrumb {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 2px;
  overflow: hidden;
  white-space: nowrap;
}
.crumb-root-btn,
.crumb-btn {
  /* 整体缩小:矮按钮 + 小字号(覆盖 Arco small 的 28px/14px) */
  height: 24px;
  font-size: var(--font-size-xs);
  padding: 0 6px;
}
.crumb-root-btn {
  font-weight: 500;
}
.crumb-btn {
  max-width: 160px;
  overflow: hidden;
}
.crumb-active {
  color: var(--text-primary) !important;
}
.crumb-sep {
  color: var(--text-tertiary);
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
}
/* 面包屑路径末尾的预览文件名:与目录行同级,不可点击 */
.crumb-file {
  max-width: 160px;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-primary);
}

/* 右侧操作组(视图切换 / 搜索 / 新建):flex 行,整体右对齐,不随内容换行 */
.fm-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  margin-left: auto;
}

/* 视图切换 seg:Finder 分段胶囊,激活段白底浮起 */
.fm-view-seg {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 2px;
  background-color: var(--bg-2);
  border-radius: 10px;
  flex-shrink: 0;
}
.fm-view-btn {
  width: 30px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast),
    box-shadow var(--transition-fast);
}
.fm-view-btn:hover {
  color: var(--text-primary);
}
.fm-view-btn.active {
  background-color: var(--bg-base);
  color: var(--text-primary);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.1);
}

/* 搜索:图标按钮 + 点击后向左弹开的圆角搜索框 */
.fm-search {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}
.fm-search-toggle {
  width: 30px;
  height: 30px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: var(--radius-md, 8px);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}
.fm-search-toggle:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}
.fm-search-toggle.active {
  background-color: var(--bg-active);
}
.fm-search-box {
  display: flex;
  align-items: center;
  /* 30px(含边框,border-box)= 与工具栏其他控件同高:打开/收起时工具栏高度不变,内容不抖动 */
  height: 30px;
  width: 200px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: 999px;
  background-color: var(--bg-2);
  transition: border-color 0.15s ease;
}
.fm-search-box:focus-within {
  border-color: var(--border-strong);
}
.fm-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
}
.fm-search-input::placeholder {
  color: var(--text-tertiary);
}
/* 弹开过渡:宽度 + 透明度 */
.fm-search-enter-active,
.fm-search-leave-active {
  transition:
    width 0.2s ease,
    opacity 0.2s ease;
}
.fm-search-enter-from,
.fm-search-leave-to {
  width: 0;
  opacity: 0;
  padding: 0;
  border-width: 0;
}
.fm-search-enter-to,
.fm-search-leave-from {
  width: 200px;
  opacity: 1;
}

/* 排序:mac 样式图标下拉按钮 */
.fm-sort-btn {
  width: 30px;
  height: 30px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: var(--radius-md, 8px);
  background-color: var(--bg-2);
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}
.fm-sort-btn:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

/* 条目数量:紧跟文件夹名称之后 */
.fm-count {
  flex-shrink: 0;
  margin-left: 6px;
  padding-left: 6px;
  border-left: 1px solid var(--border-base);
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  white-space: nowrap;
}

/* 工具按钮(新建文件夹 / 导入):统一的浅色中性按钮,贴合 Finder 工具栏。
   高度/字号与左侧面包屑按钮对齐 */
.fm-tool-btn {
  height: 24px;
  border-radius: 8px;
  background-color: var(--bg-2) !important;
  border: 1px solid var(--border-base) !important;
  color: var(--text-primary) !important;
  font-size: var(--font-size-xs);
  padding: 0 8px;
}
.fm-tool-btn:hover {
  background-color: var(--bg-hover) !important;
}

/* ===== 主体 ===== */
.fm-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 14px 4px 4px;
  scrollbar-gutter: stable;
}

/* 分栏模式:内容区贴边铺满(与访达一致),滚动交给 .fm-columns */
.fm-body-flush {
  padding: 0;
  overflow: hidden;
  scrollbar-gutter: auto;
}

/* 浏览器包裹层:撑满高度,保证 columns/预览列的百分比高度链成立 */
.fm-browser {
  height: 100%;
}

/* grid/list 点击文档:预览整页铺满(浏览器隐藏) */
.fm-body-preview {
  padding: 0;
  overflow: hidden;
  display: flex;
}
.fm-preview {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel-bg);
}
.fm-preview-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 分栏视图内的预览「下一栏」:参与横向滚动,宽度比常规列更宽便于阅读 */
.fm-columns > .fm-preview-column {
  flex: 0 0 auto;
  /* 宽度 = 中心区域宽度:打开预览即整页滑入,左右滑动在「列浏览 ↔ 整页预览」间切换 */
  width: 100%;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel-bg);
  overflow: hidden;
}

.fm-empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

.fm-tag {
  display: inline-flex;
  align-items: center;
  padding: 0 7px;
  height: 18px;
  font-size: var(--font-size-2xs);
  line-height: 1;
  color: var(--text-secondary);
  background-color: var(--bg-3);
  border-radius: 9px;
  white-space: nowrap;
}
.fm-tag-more {
  display: inline-flex;
  align-items: center;
  padding: 0 5px;
  height: 18px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

/* List 视图:CSS Grid + subgrid 共享列轨道——表头与数据行的列边界严格一致 */
.fm-list {
  display: grid;
  grid-template-columns:
    200px                /* 名称:固定宽,超长省略 */
    160px                /* 标签:固定宽(最多 3 个 + 溢出数) */
    64px                 /* 类型:固定宽 */
    72px                 /* 大小:固定宽 */
    128px                /* 更新时间:固定宽 */
    minmax(120px, 1fr);  /* 路径:占满剩余宽度,省略收尾 */
  column-gap: 8px;
}
.fm-list-head,
.fm-list-row {
  display: grid;
  grid-template-columns: subgrid;
  grid-column: 1 / -1;
  align-items: center;
  padding: 0 12px;
  height: 36px;
  border-radius: 8px;
}
.fm-list-head {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  height: 30px;
}
/* 单元格左内边距:表头/数据一致,保证列对齐 */
.fm-list-head > *,
.fm-list-row > * {
  padding-left: 6px;
}
/* 表头单元格分隔线(Finder 细竖线;inset 阴影实现,不与按钮的 border 重置冲突) */
.fm-list-head > * + * {
  box-shadow: inset 1px 0 0 var(--border-base);
}
.fm-list-row {
  cursor: default;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  transition: background-color var(--transition-fast);
}
.fm-list-row:hover {
  background-color: var(--bg-hover);
}
.fm-list-row.selected {
  background-color: var(--bg-active);
}
/* 单元格即子网格项:min-width 0 允许收缩省略 */
.l-name {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.l-name-text {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.l-type {
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.l-size {
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.l-tags {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 4px;
  overflow: hidden;
}
.l-time {
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 路径列(末列):占满剩余宽度,超长省略 */
.l-path {
  min-width: 0;
  font-size: var(--font-size-2xs);
  letter-spacing: -0.1px;
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 排序表头按钮(Finder 样式:激活列加深加粗 + 方向箭头;须位于 l-* 之后以覆盖同名列样式) */
.l-head {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  min-width: 0;
  border: none;
  background: none;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: color var(--transition-fast);
}
.l-head:hover {
  color: var(--text-primary);
}
.l-head.sorted {
  color: var(--text-primary);
  font-weight: 600;
}
.l-head-text {
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.l-head-arrow {
  flex-shrink: 0;
  transition: transform 0.15s ease;
}
.l-head-arrow.asc {
  transform: rotate(180deg);
}

/* 分栏视图(Finder Column View):整片统一底色,列间细分隔线,无列标题 */
.fm-columns {
  display: flex;
  align-items: stretch;
  height: 100%;
  overflow-x: auto;
  overflow-y: hidden;
  background-color: var(--panel-bg);
}
.fm-column {
  display: flex;
  flex-direction: column;
  /* 宽度 = 容器的 1/4:默认正好 4 栏铺满、无滚动条;第 5 栏起横向滚动。
     百分比宽度随容器(窗口/侧栏)自适应;min-width 为窄容器的可读下限 */
  flex: 0 0 auto;
  width: 25%;
  min-width: 200px;
  border-right: 1px solid var(--border-base);
  overflow: hidden;
}
.fm-column-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 4px;
}
.fm-col-row {
  display: flex;
  align-items: center;
  gap: 8px;
  /* 行高/字号与列表视图名称列一致(36px 行 / 13px 字) */
  height: 36px;
  padding: 0 8px;
  border-radius: 6px;
  color: var(--text-primary);
  cursor: default;
  font-size: var(--font-size-sm);
  transition: background-color var(--transition-fast);
}
.fm-col-row:hover {
  background-color: var(--bg-hover);
}
/* 选中行(当前点选/下钻路径上更早选中):统一灰底,不用蓝色强调 */
.fm-col-row.in-path,
.fm-col-row.active {
  background-color: var(--bg-active);
}
.fm-col-name {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 仅高亮的文件夹行在右缘显示下级箭头 */
.fm-col-caret {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--text-tertiary);
}

/* 文件树同款文件图标(手写 SVG,尺寸由 img 的 width/height 属性给定) */
.fm-file-glyph {
  display: block;
  flex-shrink: 0;
}

/* ===== 弹窗表单(Mac sheet 风格,主题变量适配明暗) ===== */
.fm-form {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.fm-form-item {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.fm-form-label {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-secondary);
}
.fm-colors {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
/* 色板:圆形;选中 = 主题色描边圆环 */
.fm-color {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: none;
  cursor: pointer;
  box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
  transition:
    transform 0.12s ease,
    box-shadow 0.12s ease;
}
.fm-color:hover {
  transform: scale(1.12);
}
.fm-color.active {
  box-shadow:
    0 0 0 2px var(--panel-bg),
    0 0 0 4px var(--accent-text);
}

/* 常用标签快捷行:药丸 + 次数徽标;选中 = 主题色 12% 底 + 主题色文字 */
.fm-tag-quick {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
button.fm-tag-quick {
  border: none;
  background: var(--bg-2);
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}
button.fm-tag-quick:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
button.fm-tag-quick.active {
  background: color-mix(in srgb, var(--accent-text) 12%, transparent);
  color: var(--accent-text);
}
.fm-tag-count {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  line-height: 1;
}
button.fm-tag-quick.active .fm-tag-count {
  color: color-mix(in srgb, var(--accent-text) 70%, transparent);
}

/* 标签编辑器:填充式输入,聚焦主题色描边 */
.fm-tag-editor {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border: 1px solid transparent;
  border-radius: 6px;
  background-color: var(--bg-2);
  transition:
    border-color var(--transition-fast),
    background-color var(--transition-fast);
}
.fm-tag-editor:focus-within {
  border-color: var(--accent-text);
  background-color: var(--bg-base);
}
.fm-tag-removable {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background-color: var(--bg-3);
}
.fm-tag-remove {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  font-size: var(--font-size-xs);
  line-height: 1;
}
.fm-tag-remove:hover {
  color: var(--danger);
}
.fm-tag-input {
  min-width: 120px;
  flex: 1;
}
.fm-tag-input :deep(.arco-input) {
  background: transparent;
  border: none;
  box-shadow: none;
}

/* ===== 弹窗底部按钮(Mac sheet:取消中性 / 保存主题色) ===== */
.fm-modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px 16px;
}
.fm-btn {
  height: 28px;
  padding: 0 14px;
  border: 1px solid var(--border-base);
  border-radius: 6px;
  background: var(--bg-2);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    filter var(--transition-fast);
}
.fm-btn:hover {
  background: var(--bg-hover);
}
.fm-btn.primary {
  background: var(--accent-text);
  border-color: var(--accent-text);
  color: #ffffff;
}
.fm-btn.primary:hover {
  background: color-mix(in srgb, var(--accent-text) 86%, #ffffff);
}

/* ===== 右键菜单 ===== */
.fm-ctx {
  position: fixed;
  z-index: 3000;
  min-width: 168px;
  padding: 4px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-2);
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
  user-select: none;
}
.fm-ctx-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: var(--menu-item-height);
  line-height: var(--line-height-ui);
  padding: 0 10px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--transition-fast);
}
.fm-ctx-item:hover {
  background-color: var(--bg-hover);
}
.fm-ctx-item.danger {
  color: var(--danger);
}
.fm-ctx-item.danger:hover {
  background-color: color-mix(in srgb, var(--danger) 10%, var(--bg-2));
}
.fm-ctx-divider {
  height: 1px;
  margin: 4px 6px;
  background-color: var(--border-base);
}
</style>

<style>
/* 新建/编辑弹窗 Mac 风格骨架(Arco Modal 传送至 body,需全局作用域;fm- 前缀防冲突)。
   颜色全部走主题变量:明暗主题与强调色自动适配 */
.fm-modal .arco-modal,
.fm-modal.arco-modal {
  border-radius: 12px;
  overflow: hidden;
  background: var(--panel-bg);
  box-shadow: 0 18px 50px rgba(0, 0, 0, 0.22);
}
.fm-modal .arco-modal-header {
  padding: 14px 16px 10px;
  background: transparent;
  border-bottom: none;
}
.fm-modal .arco-modal-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
  text-align: left;
}
.fm-modal .arco-modal-close-btn {
  color: var(--text-tertiary);
}
.fm-modal .arco-modal-close-btn:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}
.fm-modal .arco-modal-body {
  padding: 0 16px 4px;
  background: transparent;
}
.fm-modal .arco-modal-footer {
  padding: 0;
  border-top: none;
  background: transparent;
}
/* 输入框:填充式圆角,聚焦主题色描边 */
.fm-modal .arco-input-wrapper {
  background: var(--bg-2);
  border: 1px solid transparent;
  border-radius: 6px;
  transition:
    border-color 0.15s ease,
    background-color 0.15s ease;
}
.fm-modal .arco-input-wrapper:hover {
  background: var(--bg-hover);
}
.fm-modal .arco-input-wrapper:focus-within {
  background: var(--bg-base);
  border-color: var(--accent-text);
}
.fm-modal .arco-input {
  color: var(--text-primary);
}
.fm-modal .arco-input::placeholder {
  color: var(--text-disabled);
}
/* 标签编辑器内的输入框:去掉自身边框/底色,只保留编辑器外框(避免双框) */
.fm-modal .fm-tag-editor .arco-input-wrapper,
.fm-modal .fm-tag-editor .arco-input-wrapper:hover,
.fm-modal .fm-tag-editor .arco-input-wrapper:focus-within {
  background: transparent;
  border: none;
  box-shadow: none;
}

/* ===== 排序下拉:mac Finder 菜单风格(非 scoped:弹层 teleport 到 body) ===== */
.fm-sort-dropdown .arco-dropdown {
  padding: 4px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background-color: var(--bg-2);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  min-width: max-content;
}
.fm-sort-dropdown .arco-dropdown-list {
  background: transparent;
  padding: 0;
}
.fm-sort-dropdown .arco-dropdown-option {
  height: 28px;
  padding: 0 10px;
  border-radius: 6px;
  font-size: var(--font-size-sm);
  line-height: 1;
  color: var(--text-primary);
  transition: background-color var(--transition-fast), color var(--transition-fast);
}
.fm-sort-dropdown .arco-dropdown-option:hover,
.fm-sort-dropdown .arco-dropdown-option-active {
  background-color: var(--bg-active);
  color: var(--text-primary);
}
.fm-sort-dropdown .arco-dropdown-option-content {
  display: inline-flex;
  align-items: center;
  line-height: 1;
}
.fm-sort-dropdown .arco-dropdown-option-suffix {
  margin-left: auto;
  color: var(--text-tertiary);
}
</style>
