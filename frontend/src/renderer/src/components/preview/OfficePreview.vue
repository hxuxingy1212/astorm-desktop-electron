<script setup lang="ts">
/**
 * Office 文件预览(docx / xlsx / pptx):读真实字节 → 专用渲染器。
 * - docx → docx-preview(HTML 渲染)
 * - xlsx/xls/csv → @stackline/xlsx 解析并渲染表格
 * - pptx → @aiden0z/pptx-renderer(幻灯片视图)
 * 失败/超限 → 提示外部打开。
 */
import { onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { renderAsync } from 'docx-preview'
import * as XLSX from '@stackline/xlsx'
import { PptxViewer } from '@aiden0z/pptx-renderer'

const props = defineProps<{ filePath: string; fileName: string }>()
const { t } = useI18n()

const container = ref<HTMLElement | null>(null)
const loading = ref(false)
const error = ref('')

let pptxViewer: PptxViewer | null = null

const ext = (): string => {
  const f = props.fileName.toLowerCase()
  return f.includes('.') ? f.slice(f.lastIndexOf('.') + 1) : ''
}

function isDocx(): boolean {
  return /^(docx|doc|docm)$/.test(ext())
}
function isXlsx(): boolean {
  return /^(xlsx|xls|xlsm|csv)$/.test(ext())
}
function isPptx(): boolean {
  return /^(pptx|ppt|pptm|pps|ppsx)$/.test(ext())
}

async function load(): Promise<void> {
  const el = container.value
  if (!el || !props.filePath) return
  loading.value = true
  error.value = ''
  el.innerHTML = ''
  try {
    if (isDocx()) {
      // docx-preview 需要一个空的 style 容器(它会在其中生成样式)
      const body = document.createElement('div')
      const style = document.createElement('div')
      el.append(body, style)
      const buf = await window.api.fs.readArrayBuffer(props.filePath)
      if (!buf) {
        error.value = t('panel.filePreview.binary')
        return
      }
      await renderAsync(new Blob([buf]), body, style)
    } else if (isXlsx()) {
      const buf = await window.api.fs.readArrayBuffer(props.filePath)
      if (!buf) {
        error.value = t('panel.filePreview.binary')
        return
      }
      renderSheets(el, buf)
    } else if (isPptx()) {
      const buf = await window.api.fs.readArrayBuffer(props.filePath)
      if (!buf) {
        error.value = t('panel.filePreview.binary')
        return
      }
      pptxViewer = new PptxViewer(el, { fitMode: 'contain' })
      await pptxViewer.open(buf)
    } else {
      error.value = t('panel.filePreview.binary')
    }
  } catch {
    error.value = t('panel.filePreview.binary')
  } finally {
    loading.value = false
  }
}

/** xlsx 多工作表 → 表格渲染 */
function renderSheets(el: HTMLElement, buf: ArrayBuffer | Uint8Array): void {
  const data = XLSX.read(new Uint8Array(buf), { type: 'array' })
  for (const sheetName of data.SheetNames) {
    const sheet = data.Sheets[sheetName]
    if (!sheet) continue
    const html = XLSX.utils.sheet_to_html(sheet, { header: '', footer: '' })
    const sec = document.createElement('section')
    sec.className = 'of-sheet'
    const h = document.createElement('h3')
    h.textContent = sheetName
    sec.append(h)
    const holder = document.createElement('div')
    holder.innerHTML = html
    sec.append(holder)
    el.append(sec)
  }
}

/** 外部打开(默认应用) */
async function openExternal(): Promise<void> {
  if (props.filePath) await window.api.fs.openFolder(props.filePath)
}

watch(
  () => [props.filePath, props.fileName] as const,
  async () => {
    pptxViewer?.destroy()
    pptxViewer = null
    await load()
  },
  { immediate: true }
)

onBeforeUnmount(() => {
  pptxViewer?.destroy()
  pptxViewer = null
})

defineExpose({ load })
</script>

<template>
  <div class="of-wrap">
    <div v-if="loading" class="of-empty">{{ t('common.loading') }}</div>
    <div v-else-if="error" class="of-empty">
      <span class="of-error-text">{{ error }}</span>
      <button type="button" class="of-open-btn" @click="openExternal">
        <icon-launch :size="13" />
        {{ t('panel.filePreview.openExternal') }}
      </button>
    </div>
    <div v-show="!loading && !error" ref="container" class="of-container" />
  </div>
</template>

<style scoped>
.of-wrap {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.of-container {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 12px 16px;
  background: var(--bg-1);
}
.of-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  text-align: center;
  padding: 24px;
}
.of-error-text {
  color: var(--text-tertiary);
}
.of-open-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--border-base);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    border-color 0.15s ease;
}
.of-open-btn:hover {
  background-color: var(--bg-hover);
  border-color: var(--border-strong);
}
/* xlsx 表格 */
.of-sheet {
  margin-bottom: 20px;
}
.of-sheet h3 {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  margin: 0 0 8px;
}
.of-sheet table {
  border-collapse: collapse;
  font-size: var(--font-size-xs);
  color: var(--text-primary);
}
.of-sheet td,
.of-sheet th {
  border: 1px solid var(--border-base);
  padding: 4px 8px;
  white-space: nowrap;
}
.of-sheet th {
  background: var(--bg-2);
  font-weight: 600;
}
.of-container :deep(.docx-wrapper),
.of-container :deep(.docx-preview) {
  background: transparent !important;
}
</style>