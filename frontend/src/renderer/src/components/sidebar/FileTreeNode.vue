<script setup lang="ts">
import { computed, inject, type Ref } from 'vue'
import type { FileNode } from '@/types'
import { getFileIconUrl } from '@/utils/fileIcons'

const props = defineProps<{
  /** 当前节点 */
  node: FileNode
  /** 递归深度(缩进 = depth * 16 + 4) */
  depth: number
  /** 相对路径(父路径 + 节点名,「/」拼接;点击打开时传给页签) */
  path: string
  /** 直接子节点的路径前缀:根目录包裹节点传 ''(Workspace 名不进子节点路径) */
  childBasePath?: string
}>()

const emit = defineEmits<{
  (e: 'open', payload: { path: string; name: string; type: FileNode['type'] }): void
}>()

/** 展开/折叠状态:由 FileExplorerPanel provide 的响应式 Set(路径集合,跨重渲染存活) */
const expandedPaths = inject<Ref<Set<string>> | null>('expandedPaths', null)
/** 已打开文件判定(相对路径 → 高亮) */
const isOpenFile = inject<(rel: string) => boolean>('isOpenFile', () => false)
/** git 变更状态字(相对路径 → A/M/D…;「查看变更」模式下有效,否则空) */
const gitStatusOf = inject<(rel: string) => string>('gitStatusOf', () => '')
/** 是否被 git 忽略(相对路径自身或某级祖先命中 .gitignore → 淡色区分) */
const isIgnoredPath = inject<(rel: string) => boolean>('isIgnoredPath', () => false)
const isFolder = computed(() => props.node.type === 'folder')
const expanded = computed(() => !!expandedPaths?.value.has(props.path))
/** 文件行高亮:已作为预览页签打开(文件夹不高亮) */
const active = computed(() => !isFolder.value && isOpenFile(props.path))
/** git 状态徽标:文件且存在变更时显示(忽略的文件不出现在变更里) */
const statusBadge = computed(() => (isFolder.value ? '' : gitStatusOf(props.path)))
/** 忽略态:自身或祖先命中忽略列表 → 整行淡色 */
const ignored = computed(() => isIgnoredPath(props.path))
/** 状态类名:按状态字映射(未跟踪 '?' 显示为 U/Untracked,类名统一到 st-u) */
const statusClass = computed(() => (statusBadge.value ? `st-${statusBadge.value === '?' ? 'u' : statusBadge.value.toLowerCase()}` : ''))

function toggleFolder(): void {
  if (!expandedPaths) return
  const s = expandedPaths.value
  const next = new Set(s)
  if (next.has(props.path)) next.delete(props.path)
  else next.add(props.path)
  ;(expandedPaths as Ref<Set<string>>).value = next
}

/** 图标 URL:文件按扩展名映射手写 SVG(文件夹行只显示箭头,不用图标) */
const iconUrl = computed(() => (isFolder.value ? '' : getFileIconUrl(props.node.name)))

/** 子节点路径基座(根包裹节点 → '';其余 → 自身 path) */
const childrenPrefix = computed(() => (props.childBasePath !== undefined ? props.childBasePath : props.path))

function handleClick(): void {
  if (isFolder.value) {
    toggleFolder()
    return
  }
  emit('open', {
    path: props.path,
    name: props.node.name,
    type: props.node.type
  })
}
</script>

<template>
  <div>
    <button
      type="button"
      class="ft-row"
      :class="[active && 'active', ignored && 'ignored']"
      :style="{ paddingLeft: `${depth * 16 + 4}px` }"
      @click="handleClick"
    >
      <span class="ft-arrow" :class="isFolder && expanded && 'expanded'">
        <svg v-if="isFolder" class="ft-arrow-svg" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="9 18 15 12 9 6" />
        </svg>
      </span>
      <span class="ft-icon" v-if="!isFolder">
        <!-- 文件图标:手写 SVG(16 网格实色,自带明暗可读性,无需滤镜) -->
        <img class="ft-glyph" :src="iconUrl" width="16" height="16" alt="" />
      </span>
      <span class="ft-file-info">
        <span class="ft-name">{{ node.name }}</span>
      </span>
      <!-- git 变更状态徽标:右对齐(A/M/D/R/C…) -->
      <span v-if="statusBadge" class="ft-status" :class="statusClass">{{ statusBadge === '?' ? 'U' : statusBadge }}</span>
    </button>

    <!-- 递归子节点:文件夹展开且有子节点时渲染 -->
    <template v-if="isFolder && expanded && node.children?.length">
      <FileTreeNode
        v-for="child in node.children"
        :key="child.name"
        :node="child"
        :depth="depth + 1"
        :path="childrenPrefix ? `${childrenPrefix}/${child.name}` : child.name"
        :child-base-path="childrenPrefix ? `${childrenPrefix}/${child.name}` : child.name"
        @open="(p) => emit('open', p)"
      />
    </template>
  </div>
</template>

<style scoped>
/* 树行:14px 文字 / min-height 26px(紧凑;图标恒 16px,见下方 .ft-glyph 的尺寸约束) */
.ft-row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  min-height: 26px;
  padding: 1px 6px 1px 0;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-md);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--transition-fast);
}

.ft-row:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}

/* 已打开预览的行:仅文字主色,不加背景高亮(悬停反馈保留) */
.ft-row.active {
  color: var(--text-primary);
}

/* git 忽略节点:整行淡色区分(名称/图标/箭头同步降透明度) */
.ft-row.ignored {
  opacity: 0.45;
}

.ft-row.ignored:hover {
  background-color: var(--bg-hover);
  opacity: 0.75;
}

/* 折叠箭头:16px,展开旋转 90° */
.ft-arrow {
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: var(--text-tertiary);
  transition: transform 0.15s ease;
}

.ft-arrow-svg {
  display: block;
}

.ft-arrow.expanded {
  transform: rotate(90deg);
}

/* 图标容器:文件行 16px 槽位(文件夹行仅箭头,无图标) */
.ft-icon {
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

/* 文件图标:手写 SVG 16px(与图标集设计尺寸一致,15px 会糊)。
   实色品牌填充自带明暗可读性,不再需要 --icon-filter 对比度增强 */
.ft-glyph {
  display: block;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

.ft-file-info {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  display: flex;
  align-items: center;
}

.ft-name {
  font-size: var(--font-size-md);
  font-weight: 500;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* git 变更状态徽标:等宽小字,右对齐(位于 flex:1 名称之后) */
.ft-status {
  font-family: var(--font-mono);
  font-size: var(--font-size-2xs);
  font-weight: 600;
  line-height: 1;
  padding: 1px 4px;
  border-radius: 4px;
  flex-shrink: 0;
  background: var(--bg-3);
}

/* 状态字配色:随主题的品牌色(A 绿 / M 琥珀 / D 红 / R 蓝 / C 青 / U 未跟踪=中性灰),亮暗各有对应值(--ficon-*) */
.ft-status.st-a {
  color: var(--ficon-shell);
}

.ft-status.st-m {
  color: var(--ficon-data);
}

.ft-status.st-d {
  color: var(--ficon-media);
}

.ft-status.st-r {
  color: var(--ficon-code);
}

.ft-status.st-c {
  color: var(--ficon-sheet);
}

.ft-status.st-u {
  color: var(--text-secondary);
}
</style>