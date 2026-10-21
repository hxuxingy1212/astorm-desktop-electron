<script setup lang="ts">
/**
 * 目录选择器（浏览器部署专用；桌面端走原生对话框，见 useDirectoryPicker）。
 *
 * 服务端目录浏览：`/api/v1/fs/list-dirs` 单层列目录（仅目录，范围限定在数据根内），
 * `/api/v1/fs/mkdir` 新建文件夹。支持手输绝对路径（可越出数据根，与 --workspace 同语义）。
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { IconFolder, IconLeft, IconPlus } from '@arco-design/web-vue/es/icon'

const props = defineProps<{ title?: string; startPath?: string }>()
const emit = defineEmits<{ (e: 'confirm', path: string): void; (e: 'cancel'): void }>()

const { t } = useI18n()

const current = ref('')
const parent = ref<string | null>(null)
const dirs = ref<{ name: string; path: string }[]>([])
const selected = ref('')
const manual = ref('')
const loading = ref(false)
const newName = ref('')
const creating = ref(false)

/** 面包屑：数据根以内逐级（越界时只显示当前路径） */
const crumbs = computed(() => {
  const parts = current.value.split('/').filter(Boolean)
  const out: { name: string; path: string }[] = []
  let acc = ''
  parts.forEach((p) => {
    acc += '/' + p
    out.push({ name: p, path: acc })
  })
  return out
})

async function load(path: string): Promise<void> {
  loading.value = true
  try {
    const api = window.api?.fs as unknown as {
      listDirs?: (p?: string) => Promise<{ path: string; parent: string | null; root: string; dirs: { name: string; path: string }[] }>
    }
    const res = await api?.listDirs?.(path)
    if (!res) {
      Message.error(t('dirPicker.unavailable'))
      return
    }
    current.value = res.path
    parent.value = res.parent
    dirs.value = res.dirs ?? []
    selected.value = res.path
    manual.value = res.path
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    loading.value = false
  }
}

function enter(path: string): void {
  void load(path)
}

async function createFolder(): Promise<void> {
  const name = newName.value.trim()
  if (!name) return
  creating.value = true
  try {
    const api = window.api?.fs as unknown as {
      mkdir?: (parent: string, name: string) => Promise<{ path: string }>
    }
    const res = await api?.mkdir?.(current.value, name)
    newName.value = ''
    if (res?.path) await load(res.path)
    else await load(current.value)
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    creating.value = false
  }
}

function useManual(): void {
  const p = manual.value.trim()
  if (!p) return
  void load(p)
}

onMounted(() => {
  void load(props.startPath ?? '')
})
</script>

<template>
  <div class="dp-mask" @click.self="emit('cancel')">
    <div class="dp-card">
      <div class="dp-head">
        <span class="dp-title">{{ title || t('dirPicker.title') }}</span>
        <span class="dp-hint">{{ t('dirPicker.scopeHint') }}</span>
      </div>

      <!-- 路径行：上级 + 面包屑 -->
      <div class="dp-path">
        <button
          type="button"
          class="dp-icon-btn"
          :disabled="!parent"
          :title="t('dirPicker.up')"
          @click="parent && enter(parent)"
        >
          <IconLeft :size="14" />
        </button>
        <div class="dp-crumbs">
          <template v-for="(c, i) in crumbs" :key="c.path">
            <span v-if="i > 0" class="dp-sep">/</span>
            <button type="button" class="dp-crumb" @click="enter(c.path)">{{ c.name }}</button>
          </template>
          <span v-if="!crumbs.length" class="dp-crumb muted">{{ current || '—' }}</span>
        </div>
      </div>

      <!-- 目录列表 -->
      <div class="dp-list">
        <div v-if="loading" class="dp-empty">{{ t('common.loading') }}</div>
        <div v-else-if="!dirs.length" class="dp-empty">{{ t('dirPicker.empty') }}</div>
        <button
          v-for="d in dirs"
          v-else
          :key="d.path"
          type="button"
          class="dp-item"
          :class="selected === d.path && 'dp-item-active'"
          @click="selected = d.path"
          @dblclick="enter(d.path)"
        >
          <IconFolder :size="15" class="dp-item-icon" />
          <span class="dp-item-name">{{ d.name }}</span>
        </button>
      </div>

      <!-- 新建文件夹 -->
      <div class="dp-new">
        <input v-model="newName" class="dp-input" :placeholder="t('dirPicker.newFolderPlaceholder')" @keydown.enter.prevent="createFolder" />
        <button type="button" class="dp-btn" :disabled="creating || !newName.trim()" @click="createFolder">
          <IconPlus :size="13" />
          {{ t('dirPicker.newFolder') }}
        </button>
      </div>

      <!-- 手输路径（可越出数据根） -->
      <div class="dp-manual">
        <input v-model="manual" class="dp-input mono" :placeholder="t('dirPicker.manualPlaceholder')" @keydown.enter.prevent="useManual" />
        <button type="button" class="dp-btn" @click="useManual">{{ t('dirPicker.go') }}</button>
      </div>

      <div class="dp-foot">
        <span class="dp-selected" :title="selected">{{ t('dirPicker.selected') }}：{{ selected || '—' }}</span>
        <div class="flex items-center gap-2">
          <button type="button" class="dp-btn" @click="emit('cancel')">{{ t('common.cancel') }}</button>
          <button
            type="button"
            class="dp-btn primary"
            :disabled="!selected"
            @click="emit('confirm', selected)"
          >
            {{ t('dirPicker.confirm') }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.dp-mask {
  position: fixed;
  inset: 0;
  z-index: 2000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.42);
}
.dp-card {
  width: 560px;
  max-width: calc(100vw - 32px);
  max-height: calc(100vh - 80px);
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 16px;
  border-radius: 14px;
  background: var(--color-bg-2, #fff);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.22);
  color: var(--text-primary);
}
.dp-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.dp-title {
  font-size: var(--font-size-title);
  font-weight: 600;
}
.dp-hint {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.dp-path {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 28px;
  padding: 2px 6px;
  border-radius: 8px;
  background: var(--bg-hover, rgba(0, 0, 0, 0.03));
  overflow: hidden;
}
.dp-crumbs {
  display: flex;
  align-items: center;
  gap: 2px;
  overflow-x: auto;
  white-space: nowrap;
}
.dp-crumb {
  border: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 5px;
}
.dp-crumb:hover {
  background: var(--bg-active, rgba(0, 0, 0, 0.06));
}
.dp-crumb.muted {
  color: var(--text-tertiary);
}
.dp-sep {
  color: var(--text-tertiary);
  font-size: var(--font-size-xs);
}
.dp-list {
  flex: 1;
  min-height: 180px;
  max-height: 320px;
  overflow-y: auto;
  border: 1px solid var(--panel-border, rgba(0, 0, 0, 0.08));
  border-radius: 10px;
  padding: 4px;
}
.dp-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 8px;
  border: none;
  background: transparent;
  border-radius: 7px;
  cursor: pointer;
  text-align: left;
  color: var(--text-primary);
  font-size: var(--font-size-sm);
}
.dp-item:hover {
  background: var(--bg-hover, rgba(0, 0, 0, 0.04));
}
.dp-item-active {
  background: var(--bg-active, rgba(0, 0, 0, 0.07));
}
.dp-item-icon {
  color: var(--primary, #165dff);
  flex-shrink: 0;
}
.dp-item-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dp-empty {
  padding: 24px;
  text-align: center;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.dp-new,
.dp-manual {
  display: flex;
  gap: 6px;
}
.dp-input {
  flex: 1;
  min-width: 0;
  height: 30px;
  padding: 0 8px;
  border: 1px solid var(--panel-border, rgba(0, 0, 0, 0.1));
  border-radius: 7px;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  outline: none;
}
.dp-input.mono {
  font-family: Menlo, Monaco, Consolas, monospace;
}
.dp-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 30px;
  padding: 0 10px;
  border: 1px solid var(--panel-border, rgba(0, 0, 0, 0.1));
  border-radius: 7px;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  white-space: nowrap;
}
.dp-btn:hover:not(:disabled) {
  background: var(--bg-hover, rgba(0, 0, 0, 0.04));
}
.dp-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.dp-btn.primary {
  background: var(--primary, #165dff);
  border-color: var(--primary, #165dff);
  color: #fff;
}
.dp-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
}
.dp-icon-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
.dp-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding-top: 2px;
}
.dp-selected {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
</style>
