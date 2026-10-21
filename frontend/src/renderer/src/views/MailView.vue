<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { useMailStore } from '@/stores/mail'
import { useResizeDrag } from '@/composables/useResizeDrag'
import MailSidebar from '@/components/mail/MailSidebar.vue'
import MailList from '@/components/mail/MailList.vue'
import MailReader from '@/components/mail/MailReader.vue'
import MailCompose from '@/components/mail/MailCompose.vue'
import FilePreviewModal from '@/components/common/FilePreviewModal.vue'

const { t } = useI18n()
const router = useRouter()
const mail = useMailStore()

const composeOpen = ref(false)
const composeInitial = ref<{ to?: string; subject?: string; body?: string } | undefined>()
const noAccounts = computed(() => mail.state.accounts.length === 0)

const narrow = ref(false)
let mql: MediaQueryList | null = null
function onMql(e: MediaQueryListEvent | MediaQueryList): void {
  narrow.value = e.matches
}

const reading = computed(() => narrow.value && mail.curUid !== null)

// 三栏可拖动调宽：最左账号/文件夹栏 与 中间邮件列表栏（右侧阅读栏自动占满剩余）
const sidebarWidth = ref(188)
const listWidth = ref(300)
const sidebarDrag = useResizeDrag({
  min: 150,
  max: 300,
  get: () => sidebarWidth.value,
  set: (w) => (sidebarWidth.value = w)
})
const listDrag = useResizeDrag({
  min: 220,
  max: 640,
  get: () => listWidth.value,
  set: (w) => (listWidth.value = w)
})

onMounted(async () => {
  mql = window.matchMedia('(max-width: 820px)')
  onMql(mql)
  mql.addEventListener('change', onMql)

  try {
    await mail.loadState()
  } catch {
    /* 后端未就绪时静默，界面显示空状态 */
  }
  if (mail.state.accounts.length === 0) return
  if (!mail.curAccount) {
    mail.curAccount = mail.state.active ?? mail.state.accounts[0]?.name ?? null
  }
  if (mail.curAccount) {
    // 缓存优先：先渲染列表，侧栏随后台刷新
    await mail.loadMessages(true)
    void mail.refreshSidebar()
  }
})

onBeforeUnmount(() => {
  mql?.removeEventListener('change', onMql)
})

function openCompose(): void {
  composeInitial.value = undefined
  composeOpen.value = true
}

function openReply(payload: { to: string; subject: string; body: string }): void {
  composeInitial.value = payload
  composeOpen.value = true
}

function backToList(): void {
  mail.curUid = null
  mail.curDetail = null
}

function openMailSettings(): void {
  router.push('/settings/mail')
}

// 附件预览：全项目统一的文件预览弹窗（FilePreviewModal，FilePreviewPanel 直连模式）
const preview = ref<{ open: boolean; name: string; path: string }>({ open: false, name: '', path: '' })
function onPreview(payload: { name: string; path: string }): void {
  preview.value = { open: true, name: payload.name, path: payload.path }
}
</script>

<template>
  <div
    class="mail-view"
    :class="{ reading }"
    :style="{ '--mail-sidebar-w': sidebarWidth + 'px', '--mail-list-w': listWidth + 'px' }"
  >
    <div class="mail-sidebar-col">
      <MailSidebar @compose="openCompose" />
    </div>

    <!-- 账号栏 / 列表栏 的拖动调宽手柄（窄屏单栏时隐藏） -->
    <div
      v-if="!narrow"
      class="col-resizer"
      :style="{ left: sidebarWidth + 'px' }"
      @pointerdown="sidebarDrag.onPointerDown"
      @pointermove="sidebarDrag.onPointerMove"
      @pointerup="sidebarDrag.onPointerUp"
    />
    <div
      v-if="!narrow"
      class="col-resizer"
      :style="{ left: sidebarWidth + listWidth + 'px' }"
      @pointerdown="listDrag.onPointerDown"
      @pointermove="listDrag.onPointerMove"
      @pointerup="listDrag.onPointerUp"
    />

    <div class="mail-mobile-bar">
      <button v-if="reading" type="button" class="mobile-btn" @click="backToList">
        <icon-left :size="18" />
      </button>
      <span class="mobile-title">{{ t('mail.title') }}</span>
      <span class="mobile-spacer" />
      <button type="button" class="mobile-btn" @click="openCompose"><icon-edit :size="18" /></button>
    </div>

    <div class="mail-list-col">
      <MailList />
    </div>
    <div class="mail-reader-col">
      <MailReader @compose="openReply" @preview="onPreview" />
    </div>

    <!-- 未配置账号：引导到「设置 → 邮箱」添加（账号管理不在邮箱界面内） -->
    <div v-if="noAccounts" class="mail-no-account">
      <icon-mail :size="52" />
      <p class="no-account-title">{{ t('mail.noAccount') }}</p>
      <p class="no-account-desc">{{ t('mail.noAccountHint') }}</p>
      <a-button type="primary" @click="openMailSettings">{{ t('mail.goSettings') }}</a-button>
    </div>

    <MailCompose v-model:open="composeOpen" :initial="composeInitial" />

    <!-- 附件预览：统一文件预览弹窗（占视口 3/4，FilePreviewPanel 直连模式） -->
    <FilePreviewModal v-model:open="preview.open" :name="preview.name" :path="preview.path" />
  </div>
</template>

<style scoped>
.mail-view {
  position: relative;
  display: grid;
  /* 前两栏宽度由拖动状态驱动（--mail-sidebar-w / --mail-list-w），第三栏占满剩余 */
  grid-template-columns: var(--mail-sidebar-w, 188px) var(--mail-list-w, 300px) minmax(0, 1fr);
  height: 100%;
  width: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--panel-bg);
}
/* 列宽拖动手柄：贴在列边界上，悬停显示强调色竖线 */
.col-resizer {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 7px;
  margin-left: -3px;
  z-index: 4;
  cursor: col-resize;
}
.col-resizer::after {
  content: '';
  position: absolute;
  left: 3px;
  top: 0;
  bottom: 0;
  width: 1px;
  background: var(--accent-text);
  opacity: 0;
  transition: opacity 0.15s ease;
}
.col-resizer:hover::after {
  opacity: 0.4;
}
.mail-no-account {
  position: absolute;
  inset: 0;
  z-index: 5;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  background: var(--panel-bg);
  color: var(--text-tertiary);
}
.no-account-title {
  margin: 6px 0 0;
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}
.no-account-desc {
  margin: 0 0 10px;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}
.mail-sidebar-col,
.mail-list-col,
.mail-reader-col {
  min-width: 0;
  min-height: 0;
  height: 100%;
}
.mail-mobile-bar {
  display: none;
}
@media (max-width: 820px) {
  .mail-view {
    grid-template-columns: 1fr;
  }
  .mail-sidebar-col {
    display: none;
  }
  .mail-mobile-bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 44px;
    padding: 0 8px;
    border-bottom: 1px solid var(--panel-border);
    grid-row: 1;
  }
  .mail-list-col {
    grid-row: 2;
  }
  .mail-reader-col {
    grid-row: 2;
  }
  .mail-view.reading .mail-list-col {
    display: none;
  }
  .mail-view:not(.reading) .mail-reader-col {
    display: none;
  }
  .mobile-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }
  .mobile-btn:hover {
    background: var(--bg-hover);
  }
  .mobile-title {
    font-size: var(--font-size-md);
    font-weight: 600;
    color: var(--text-primary);
  }
  .mobile-spacer {
    flex: 1;
  }
}
</style>
