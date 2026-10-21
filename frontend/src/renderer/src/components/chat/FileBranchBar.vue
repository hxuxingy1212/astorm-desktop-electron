<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { pickDirectory } from '@/composables/useDirectoryPicker'

/**
 * 欢迎页输入框上方的半圆角选择框:
 * - 目录选择器(经主进程系统对话框选目录,所选目录直接显示在按钮上)
 * - Git 分支:**只读展示** —— 仅当所选/预填目录是 git 仓库时显示,值为该目录的**实时 HEAD**。
 *
 * 早先这里是「给新会话指定基线分支」的下拉,该值会写进会话记录;分支字段已移除
 * (系统提示词每个用户消息自己探测工作区当前分支),下拉没有落点,故退化为只读展示。
 */
const props = defineProps<{
  directory: string
  branch: string
}>()

const emit = defineEmits<{
  (e: 'update:directory', directory: string): void
  /** 实时 HEAD 回传(仅用于展示;不再下发给后端) */
  (e: 'update:branch', branch: string): void
}>()

const { t } = useI18n()

/** 所选目录是否为 git 仓库(驱动分支栏显隐) */
const gitRepo = ref(false)
/** 正在选择目录(抑制 watcher 重复探测) */
const picking = ref(false)

/** 读取目录的当前分支:非仓库 / 分离 HEAD → 空串 */
async function readCurrentBranch(dir: string): Promise<{ rep: boolean; current: string }> {
  let current = ''
  try {
    current = ((await window.api.git?.currentBranch(dir)) ?? '').trim()
  } catch {
    current = ''
  }
  // 分离 HEAD 时 git 返回字面量 "HEAD"(并非真分支)→ 视为无分支
  if (current === 'HEAD') current = ''
  return { rep: !!current, current }
}

async function onSelectDirectory(): Promise<void> {
  // 桌面=原生目录对话框；浏览器部署=应用内模态（服务端目录浏览）
  const dir = await pickDirectory()
  if (!dir) return
  picking.value = true
  try {
    emit('update:directory', dir)
    const info = await readCurrentBranch(dir)
    gitRepo.value = info.rep
    emit('update:branch', info.current)
  } finally {
    picking.value = false
  }
}

/** 清除所选目录:回到「未选择目录」状态(走创建任务分支,不再归类为项目) */
function onClearDirectory(): void {
  emit('update:directory', '')
  emit('update:branch', '')
  gitRepo.value = false
}

/**
 * 目录变化(手动选择由 onSelectDirectory 处理,此处跳过;预填 / 切换目录时重新探测):
 * 是仓库 → 展示分支栏;非仓库 → 隐藏分支栏并清空分支。
 */
watch(
  () => props.directory,
  (dir) => {
    if (!dir || picking.value) return
    void (async () => {
      const info = await readCurrentBranch(dir)
      gitRepo.value = info.rep
      emit('update:branch', info.current)
    })()
  },
  { immediate: true }
)
</script>

<template>
  <div class="file-branch-bar">
    <!-- 目录选择器:常态显示文件夹图标;悬浮到按钮上时图标替换为清除 X(内嵌同一胶囊)。
         两者叠放于固定 18px 图标槽位,文字位置不因替换偏移;悬浮到 X 上其底色加深 -->
    <button
      type="button"
      class="pick-btn"
      :title="props.directory || t('newChat.selectDirectory')"
      @click="onSelectDirectory"
    >
      <span class="pick-icon-slot">
        <span class="pick-icon pick-icon-folder"><icon-folder :size="14" /></span>
        <span
          v-if="props.directory"
          class="pick-clear"
          :title="t('newChat.clearDirectory')"
          @click.stop="onClearDirectory"
        >
          <icon-close :size="12" />
        </span>
      </span>
      <span class="pick-label">
        {{ props.directory || t('newChat.selectDirectory') }}
      </span>
    </button>

    <!-- Git 分支(仅 git 仓库目录显示):只读展示该目录的实时 HEAD -->
    <span v-if="props.directory && gitRepo" class="branch-chip" :title="props.branch">
      <icon-branch :size="14" />
      <span class="branch-chip-text">{{ props.branch }}</span>
    </span>
  </div>
</template>

<style scoped>
/* 上半部分:内容排版(底由 ChatInput 顶部插槽区提供),小一号尺寸 */
.file-branch-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  padding: 4px 10px;
}

/* 目录选择按钮:圆角与输入框外边框(16px)弧度相称的小圆角,宽度自适应内容(选中目录超长时截断),上限 300px;
   与模式/思考下拉一致——常态透明背景,hover 才显示 */
.pick-btn {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 10px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  font-size: var(--font-size-xs);
  max-width: 300px;
  flex-shrink: 0;
  transition: background 0.15s;
}

.pick-btn:hover {
  background: var(--bg-3);
}

/* 图标槽位:固定 18px 宽,文件夹与清除 X 叠放;
   无论替换成哪种图标,文字位置都不偏移 */
.pick-icon-slot {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  flex-shrink: 0;
}

.pick-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

/* 清除 X(替代文件夹图标):默认隐藏,仅悬浮到按钮上时显示(替代文件夹);
   @click.stop 避免触发外层目录选择 */
.pick-clear {
  position: absolute;
  inset: 0;
  display: none;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 999px;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}

/* 悬浮到按钮上:仅当已选目录(存在清除 X)时,文件夹隐藏、清除出现在同一槽位;
   目录为空时保持文件夹图标,悬浮不消失 */
.pick-btn:has(.pick-clear):hover .pick-icon-folder {
  visibility: hidden;
}
.pick-btn:has(.pick-clear):hover .pick-clear {
  display: inline-flex;
}

/* 悬浮到 X 上:底色比按钮 hover(--bg-3)更深(--bg-4) */
.pick-clear:hover {
  background: var(--bg-4);
  color: var(--text-primary);
}

.pick-label {
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 分支只读展示:图标 + 实时 HEAD,超长省略;不可点击(本页不做 checkout) */
.branch-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 6px;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  max-width: 320px;
  flex-shrink: 0;
}

.branch-chip-text {
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
