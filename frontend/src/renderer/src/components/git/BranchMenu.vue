<script setup lang="ts">
/**
 * 分支下拉菜单（顶栏分支胶囊点开）。
 *
 * 数据一次拉齐：`git:branch-state` → 当前分支 / 本地分支 / 远程分支 / 未提交文件数。
 * 点分支 = **切换**（会改动工作区，由用户显式点击触发）；失败时原样透出 git 的报错
 * （如「本地改动会被覆盖」）。「创建并检出…」**就地**把列表换成一行输入框，不另弹模态。
 *
 * 组件常驻（作为下拉内容），打开期间的自持状态（搜索词、创建态）靠 `watch(open)` 复位，
 * 不能指望 unmount。
 */
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useGitStore } from '@/stores/git'

const props = defineProps<{
  /** 仓库目录（绝对路径） */
  dir: string
  /** 下拉是否展开（由父级控制，用于复位状态） */
  open: boolean
}>()

const emit = defineEmits<{
  close: []
  /** 分支已切换 / 已新建：携带新分支名（父级据此重探 + 写回会话记录） */
  changed: [branch: string]
  'open-graph': []
}>()

const { t } = useI18n()
const git = useGitStore()

const current = ref('')
const local = ref<string[]>([])
const uncommitted = ref(0)
const loading = ref(false)
const keyword = ref('')

/** 「创建并检出」输入态 */
const creating = ref(false)
const newName = ref('')
/** 切换 / 创建进行中：忽略重复点击 */
const busy = ref(false)

const searchRef = ref<HTMLInputElement | null>(null)
const nameRef = ref<HTMLInputElement | null>(null)

const filtered = computed(() => {
  const k = keyword.value.trim().toLowerCase()
  return k ? local.value.filter((b) => b.toLowerCase().includes(k)) : local.value
})

async function load(): Promise<void> {
  if (!props.dir) return
  loading.value = true
  try {
    const s = await window.api.git.branchState(props.dir)
    current.value = s.current
    local.value = s.local
    uncommitted.value = s.uncommitted
  } catch {
    // 读不到就按空仓库呈现（菜单显示空态即可，不打扰用户）
    current.value = ''
    local.value = []
    uncommitted.value = 0
  } finally {
    loading.value = false
  }
}

/** 每次打开复位：清搜索、退出创建态、重拉分支 */
async function resetAndLoad(): Promise<void> {
  keyword.value = ''
  creating.value = false
  newName.value = ''
  await load()
  await nextTick()
  searchRef.value?.focus()
}

function startCreate(): void {
  creating.value = true
  // 顺手把已输入的搜索词带过去：用户多半是先搜到「没有这个分支」才要新建
  newName.value = keyword.value.trim()
  void nextTick(() => nameRef.value?.focus())
}

async function switchTo(branch: string): Promise<void> {
  if (busy.value) return
  if (branch === current.value) {
    emit('close')
    return
  }
  busy.value = true
  try {
    const r = await window.api.git.checkout(props.dir, branch)
    if (!r.ok) {
      Message.error(r.error || t('git.checkoutFailed'))
      return
    }
    git.bumpRev()
    emit('changed', branch)
    emit('close')
  } finally {
    busy.value = false
  }
}

async function createAndCheckout(): Promise<void> {
  const name = newName.value.trim()
  if (!name || busy.value) return
  // 以 `-` 开头会被 git 当成选项（主进程亦会再拦一道）
  if (name.startsWith('-')) {
    Message.error(t('git.createFailed'))
    return
  }
  busy.value = true
  try {
    const r = await window.api.git.createBranch(props.dir, name)
    if (!r.ok) {
      Message.error(r.error || t('git.createFailed'))
      return
    }
    git.bumpRev()
    emit('changed', name)
    emit('close')
  } finally {
    busy.value = false
  }
}

// `immediate` 是必需的：Arco 的下拉内容是**首次展开时才挂载**的，那时 `open` 已经是 true，
// 不 immediate 的话初次打开不会触发（组件一挂载就已处于目标状态）。
watch(
  () => props.open,
  (o) => {
    if (o) void resetAndLoad()
  },
  { immediate: true }
)
</script>

<template>
  <div class="bm">
    <!-- 搜索 -->
    <div class="bm-search">
      <icon-search :size="14" class="bm-search-icon" />
      <input
        ref="searchRef"
        v-model="keyword"
        class="bm-search-input"
        :placeholder="t('git.searchBranch')"
        @keydown.esc.stop="emit('close')"
      />
    </div>

    <!-- 创建态：就地换成输入框（Enter 确认 / Esc 返回列表） -->
    <template v-if="creating">
      <div class="bm-group-title">{{ t('git.createTitle') }}</div>
      <div class="bm-create">
        <icon-branch :size="14" class="bm-icon" />
        <input
          ref="nameRef"
          v-model="newName"
          class="bm-create-input"
          :placeholder="t('git.branchNamePlaceholder')"
          @keydown.enter="createAndCheckout"
          @keydown.esc.stop="creating = false"
        />
      </div>
      <div class="bm-hint">{{ t('git.createHint') }}</div>
    </template>

    <!-- 分支列表 -->
    <template v-else>
      <div class="bm-group-title">
        {{ t('git.branches') }}
        <span v-if="filtered.length" class="bm-group-count">{{ filtered.length }}</span>
      </div>
      <div v-if="loading" class="bm-hint">{{ t('common.loading') }}</div>
      <div v-else-if="!filtered.length" class="bm-hint">{{ t('git.noBranches') }}</div>
      <div v-else class="bm-list">
        <button
          v-for="b in filtered"
          :key="b"
          type="button"
          class="bm-item"
          :class="{ current: b === current }"
          :disabled="busy"
          @click="switchTo(b)"
        >
          <icon-branch :size="14" class="bm-icon" />
          <span class="bm-item-main">
            <span class="bm-item-name">{{ b }}</span>
            <!-- 当前分支：附一行未提交计数（git 的「N 个文件」口径） -->
            <span v-if="b === current && uncommitted > 0" class="bm-item-sub">
              {{ t('git.uncommitted', { n: uncommitted }) }}
            </span>
          </span>
          <icon-check v-if="b === current" :size="14" class="bm-check" />
        </button>
      </div>
    </template>

    <div class="bm-sep" />
    <button type="button" class="bm-action" @click="startCreate">
      <icon-plus :size="14" class="bm-icon" />
      {{ t('git.createAndCheckout') }}
    </button>
    <button type="button" class="bm-action" @click="emit('open-graph')">
      <icon-mind-mapping :size="14" class="bm-icon" />
      {{ t('git.graph') }}
    </button>
  </div>
</template>

<style scoped>
.bm {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 280px;
  padding: 6px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

/* ---------- 搜索 ---------- */
.bm-search {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 8px;
  border-radius: 8px;
  background: var(--bg-2);
}

.bm-search-icon {
  flex-shrink: 0;
  color: var(--text-tertiary);
}

.bm-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: inherit;
  font: inherit;
}

.bm-search-input::placeholder {
  color: var(--text-tertiary);
}

/* ---------- 分组标题 ---------- */
.bm-group-title {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 6px 8px 2px;
  font-size: var(--font-size-xs);
  font-weight: 600;
  color: var(--text-tertiary);
}

.bm-group-count {
  font-weight: 400;
  opacity: 0.75;
}

.bm-hint {
  padding: 6px 8px 8px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* ---------- 分支列表 ---------- */
.bm-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  /* 长分支列表在本面板内滚动（下拉本身不限高，故这里自己封顶）；滚动条走全局那套 */
  max-height: 260px;
  overflow-y: auto;
}

.bm-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color 0.12s;
}

.bm-item:hover:not(:disabled) {
  background: var(--bg-hover);
}

.bm-item:disabled {
  opacity: 0.6;
  cursor: default;
}

/* 当前分支：常驻浅底，与悬停区分开 */
.bm-item.current {
  background: var(--bg-active);
}

.bm-icon {
  flex-shrink: 0;
  margin-top: 1px;
  color: var(--text-secondary);
}

.bm-item-main {
  display: flex;
  flex-direction: column;
  gap: 1px;
  flex: 1;
  min-width: 0;
}

.bm-item-name {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-weight: 500;
}

.bm-item-sub {
  font-size: var(--font-size-xs);
  font-weight: 400;
  color: var(--text-tertiary);
}

.bm-check {
  flex-shrink: 0;
  margin-top: 1px;
  color: var(--text-secondary);
}

/* ---------- 创建态 ---------- */
.bm-create {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
}

.bm-create-input {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 8px;
  border: 1px solid var(--border-base);
  border-radius: 6px;
  outline: none;
  background: var(--bg-2);
  color: inherit;
  font: inherit;
}

.bm-create-input:focus {
  border-color: var(--accent-text);
}

/* ---------- 底部动作 ---------- */
.bm-sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--border-light);
}

.bm-action {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color 0.12s;
}

.bm-action:hover {
  background: var(--bg-hover);
}
</style>
