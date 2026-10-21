<script setup lang="ts">
/**
 * Office/PDF 高保真预览(astorm-office 的 @astorm/office-viewer):
 * CLI `unpack --inline` 产出单文件自包含 JSON(分片内联 + 媒体 data URI)→
 * `<OfficeViewer :data>` 直接渲染,悬浮元素高亮并弹出 CLI 路径卡片。
 *
 * 「选择路径」模式(对标浏览器拾取):开启后点击元素 → officePick store →
 * 发送框 chip + ```office-ref 围栏元数据,模型可按路径直接 office edit。
 *
 * 旧版格式(.doc/.xls/.ppt/csv)与解包失败回退:OfficePreview(JS 渲染库)/外部打开。
 */
import { computed, defineAsyncComponent, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useOfficePickStore } from '@/stores/officePick'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import { ZOOM_LEVELS } from '@/utils/browserViewport'
import { buildOfficeRefBlock, officeRefChipLabel } from '@/utils/officeRef'

/** office-viewer 惰性加载(组件 + 样式一起 static import 会进主 chunk;这里拆独立 chunk 按需加载) */
const OfficeViewer = defineAsyncComponent(() =>
  Promise.all([import('@astorm/office-viewer'), import('@astorm/office-viewer/style.css')]).then(
    ([m]) => m.OfficeViewer
  )
)

/** 旧渲染器(docx-preview / @stackline/xlsx / @aiden0z pptx-renderer,静态拖 echarts)同样惰性:
 *  仅旧版格式(.doc/.xls/.ppt/csv)兜底时才加载;echarts 实例与 office-viewer 共享 */
const OfficePreview = defineAsyncComponent(() => import('./OfficePreview.vue'))

const props = defineProps<{ filePath: string; fileName?: string }>()
const { t } = useI18n()

const officePick = useOfficePickStore()
const settings = useSettingsStore()
const isDark = computed(
  () => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark())
)

type ViewerKind = 'docx' | 'xlsx' | 'pptx' | 'pdf'
/** 新格式(office-viewer 渲染);其余(旧版 .doc/.xls/.ppt/csv)回退 OfficePreview */
const VIEWER_EXT_RE = /\.(docx|docm|xlsx|xlsm|pptx|pptm|ppsx|pps|pdf)$/i

const state = ref<{
  phase: 'loading' | 'ok' | 'failed' | 'unsupported'
  kind?: ViewerKind
  data?: unknown
  message?: string
}>({ phase: 'loading' })

/** 「选择路径」模式(默认关:阅读/滚动不受影响,开启后点击元素才入输入框;Esc 取消) */
const picking = ref(false)

/** Esc 取消拾取:仅拾取开启期间挂全局监听,关闭/切换文件/卸载即摘除 */
function cancelPickOnEsc(e: KeyboardEvent): void {
  if (e.key === 'Escape') picking.value = false
}
watch(picking, (on) => {
  if (on) window.addEventListener('keydown', cancelPickOnEsc)
  else window.removeEventListener('keydown', cancelPickOnEsc)
})
onBeforeUnmount(() => window.removeEventListener('keydown', cancelPickOnEsc))

/** 受控缩放(缩放控件外置):null = 适应(适配基准,倍率 1);数字 = 相对适配基准的用户倍率。
 *  viewer 永远处于受控模式,内置缩放滑杆不渲染。 */
const zoomUser = ref<number | null>(null)
/** viewer 汇报的绝对缩放与适配基准(显示 % 与档位换算都从这里来) */
const scaleInfo = ref({ scale: 1, fitScale: 1 })
function onZoomChange(v: { scale: number; fitScale: number }): void {
  scaleInfo.value = v
}
const zoomPct = computed(() => `${Math.round(scaleInfo.value.scale * 100)}%`)
/** 档位语义 = 绝对 %:换算成宿主传给 viewer 的用户倍率(除以适配基准) */
function setZoomAbs(abs: number): void {
  zoomUser.value = Math.min(3, Math.max(0.1, abs / scaleInfo.value.fitScale))
}
function onZoomMenu(v: string | number | Record<string, unknown> | undefined): void {
  if (v === undefined) return
  if (typeof v === 'string') {
    if (v === 'fit') zoomUser.value = null
    else if (v !== '') setZoomAbs(Number(v))
  }
}

function kindOf(name: string): ViewerKind {
  const ext = name.toLowerCase().split('.').pop() ?? ''
  if (ext === 'pdf') return 'pdf'
  if (ext.startsWith('xls')) return 'xlsx'
  if (ext.startsWith('pp')) return 'pptx'
  return 'docx'
}

async function load(): Promise<void> {
  const p = props.filePath
  if (!p) return
  const name = props.fileName ?? p.split(/[/\\]/).pop() ?? ''
  if (!VIEWER_EXT_RE.test(name)) {
    state.value = { phase: 'unsupported' }
    return
  }
  state.value = { phase: 'loading', kind: kindOf(name) }
  const res = await window.api.office.preview(p)
  if (!res.ok || res.data === undefined) {
    state.value = { phase: 'failed', kind: kindOf(name), message: res.message }
    return
  }
  state.value = { phase: 'ok', kind: res.kind ?? kindOf(name), data: res.data }
}

/** 拾取 → officePick store(文件 + CLI 路径 + 元素摘要;序列化为 ```office-ref 围栏) */
function onSelect(
  path: string,
  info?: { type?: string; name?: string; text?: string; meta?: Record<string, string | number | undefined> }
): void {
  if (!picking.value) return
  const data = {
    file: props.filePath,
    kind: state.value.kind ?? 'docx',
    path,
    type: info?.type,
    name: info?.name,
    text: info?.text,
    meta: info?.meta
  }
  officePick.push({ data, token: buildOfficeRefBlock(data) })
  Message.success(t('panel.filePreview.officePick.added', { label: officeRefChipLabel(data) }))
}

watch(
  () => [props.filePath, props.fileName] as const,
  () => {
    picking.value = false
    zoomUser.value = null
    scaleInfo.value = { scale: 1, fitScale: 1 }
    load()
  },
  { immediate: true }
)

/** 外部打开(默认应用) */
async function openExternal(): Promise<void> {
  if (props.filePath) await window.api.fs.openFolder(props.filePath)
}
</script>

<template>
  <div class="ovr-wrap">
    <template v-if="state.phase === 'ok'">
      <!-- 顶部工具条(参考浏览器面板 .bw-bar):右侧无边框图标按钮 = 选择路径开关 -->
      <div class="ovr-bar">
        <!-- 缩放 chip:浏览器同款胶囊档位(absolute %,viewer 回报实际值),位置规划在拾取按钮左侧 -->
        <a-dropdown trigger="click" @select="onZoomMenu">
          <button type="button" class="ovr-chip" :title="t('panel.filePreview.zoom.label')">
            <span class="ovr-chip-val">{{ zoomPct }}</span>
            <icon-chevron-down :size="12" />
          </button>
          <template #content>
            <a-doption value="fit">{{ t('panel.filePreview.zoom.fit') }}</a-doption>
            <a-doption v-for="z in ZOOM_LEVELS" :key="z" :value="String(z)">
              {{ Math.round(z * 100) }}%
            </a-doption>
          </template>
        </a-dropdown>
        <button
          type="button"
          class="ovr-btn"
          :class="{ active: picking }"
          :title="t('panel.filePreview.officePick.toggle')"
          @click="picking = !picking"
        >
          <icon-pick-element :size="16" />
        </button>
      </div>
      <div class="ovr-content" :class="{ 'ovr-picking': picking }">
        <div v-if="picking" class="ovr-hint">{{ t('panel.filePreview.officePick.hint') }}</div>
        <OfficeViewer
          :class="{ 'ov-dark': isDark }"
          :kind="(state.kind as any)"
          :data="(state.data as any)"
          :zoom="zoomUser ?? 1"
          :pick-mode="picking"
          @select="onSelect"
          @zoom-change="onZoomChange"
        />
      </div>
    </template>
    <!-- 旧版格式(.doc/.xls/.ppt/csv):回退旧渲染器 -->
    <div v-else-if="state.phase === 'unsupported'" class="ovr-fallback">
      <OfficePreview :file-path="filePath" :file-name="fileName ?? ''" />
    </div>
    <div v-else-if="state.phase === 'failed'" class="ovr-empty">
      <span class="ovr-error-text">{{ state.message || t('panel.filePreview.officePick.failed') }}</span>
      <button type="button" class="ovr-open-btn" @click="openExternal">
        <icon-launch :size="13" />
        {{ t('panel.filePreview.openExternal') }}
      </button>
    </div>
    <div v-else class="ovr-empty">{{ t('common.loading') }}</div>
  </div>
</template>

<style scoped>
.ovr-wrap {
  position: relative;
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--bg-1);
}
.ovr-wrap :deep(.ov-dark) {
  background: var(--bg-1);
}

/* 顶部工具条:与浏览器面板同一套组件形态(无边框 28px 图标按钮,悬停灰底,激活亮底) */
.ovr-bar {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  flex-shrink: 0;
  padding: 5px 12px 2px;
}
.ovr-btn {
  width: 28px;
  height: 28px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}
.ovr-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.ovr-btn.active {
  background: var(--bg-active);
  color: var(--text-primary);
}
.ovr-chip {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 26px;
  min-width: 52px;
  padding: 0 9px;
  border: none;
  border-radius: 7px;
  background-color: color-mix(in srgb, var(--bg-3) 72%, transparent);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition: background-color 0.15s ease;
}
.ovr-chip:hover {
  background-color: var(--bg-3);
}

/* 内容区:viewer 占满,舞台由 viewer 内部 margin:auto 居中;压平卡片外观(去圆角/阴影) */
.ovr-content {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.ovr-content.ovr-picking {
  cursor: crosshair;
}
/* 统一入口自带的 .ov-root 包装是布局断点:不接链它就是 auto 高,内部 100% 全部失真
 * (表现为 viewer 只到内容高、面板底部大段空白)——把它接成弹性列填满内容区 */
.ovr-content :deep(.ov-root) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.ovr-content :deep(.ov-pptx),
.ovr-content :deep(.ov-docx),
.ovr-content :deep(.ov-pdf),
.ovr-content :deep(.ov-xlsx) {
  flex: 1;
  min-height: 0;
  height: 100%;
  border-radius: 0;
  box-shadow: none;
}
/* 侧栏的 72vh 上限同样来自浏览器演示场景;面板内放开,跟随面板高度 */
.ovr-content :deep(.ov-pptx-side),
.ovr-content :deep(.ov-pdf-side),
.ovr-content :deep(.ov-docx-side) {
  max-height: none;
}
/* 舞台画布从固定 72vh 高改为填满面板剩余高度:页面 margin:auto 双向居中(浏览器内容区同款) */
.ovr-content :deep(.ov-stage-wrap),
.ovr-content :deep(.ov-docx-canvas) {
  flex: 1;
  height: auto;
  min-height: 0;
}
.ovr-hint {
  position: absolute;
  left: 50%;
  bottom: 20px;
  transform: translateX(-50%);
  z-index: 30;
  padding: 5px 12px;
  border-radius: var(--radius-lg, 10px);
  background: rgb(0 0 0 / 78%);
  color: #fff;
  font-size: var(--font-size-xs);
  pointer-events: none;
  white-space: nowrap;
}
.ovr-fallback {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.ovr-empty {
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
.ovr-error-text {
  color: var(--text-tertiary);
  max-width: 520px;
  white-space: pre-wrap;
}
.ovr-open-btn {
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
}
.ovr-open-btn:hover {
  background-color: var(--bg-hover);
  border-color: var(--border-strong);
}
</style>
