<script setup lang="ts">
/**
 * 邮箱设置页：多账号管理（添加 / 编辑 / 切换 / 删除）。
 *
 * 视觉与交互对齐「模型设置」页：左侧账号列表 + 右侧账号配置表单（卡片式），
 * 账号与缓存由 Rust 后端 `astorm-mail` 维护，经 `backend:invoke` 的 mail_* 命令访问，
 * 与邮箱界面共用同一份数据（Pinia mail store 单例）。
 */
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { macConfirm } from '@/composables/useMacConfirm'
import { useMailStore, type MailAccount } from '@/stores/mail'

const { t } = useI18n()
const mail = useMailStore()

type Mode = 'edit' | 'add'
const mode = ref<Mode>('edit')
const selected = ref<string | null>(null)
const saving = ref(false)

/** 当前编辑中的账号（左侧选中项）。 */
const editing = computed<MailAccount | null>(
  () => mail.state.accounts.find((a) => a.name === selected.value) ?? null
)

interface AccountForm {
  name: string
  displayName: string
  username: string
  password: string
  imapHost: string
  imapPort: number
  imapTls: string
  smtpHost: string
  smtpPort: number
  smtpTls: string
}

/** 服务商预设默认值（镜像后端 astorm-mail::preset；新增与编辑均可套用）。 */
const PRESET_DEFAULTS: Record<
  string,
  { imapHost: string; imapPort: number; imapTls: string; smtpHost: string; smtpPort: number; smtpTls: string }
> = {
  qq: { imapHost: 'imap.qq.com', imapPort: 993, imapTls: 'ssl', smtpHost: 'smtp.qq.com', smtpPort: 465, smtpTls: 'ssl' },
  '163': { imapHost: 'imap.163.com', imapPort: 993, imapTls: 'ssl', smtpHost: 'smtp.163.com', smtpPort: 465, smtpTls: 'ssl' },
  '126': { imapHost: 'imap.126.com', imapPort: 993, imapTls: 'ssl', smtpHost: 'smtp.126.com', smtpPort: 465, smtpTls: 'ssl' },
  gmail: { imapHost: 'imap.gmail.com', imapPort: 993, imapTls: 'ssl', smtpHost: 'smtp.gmail.com', smtpPort: 465, smtpTls: 'ssl' },
  outlook: { imapHost: 'outlook.office365.com', imapPort: 993, imapTls: 'ssl', smtpHost: 'smtp.office365.com', smtpPort: 587, smtpTls: 'starttls' },
  icloud: { imapHost: 'imap.mail.me.com', imapPort: 993, imapTls: 'ssl', smtpHost: 'smtp.mail.me.com', smtpPort: 587, smtpTls: 'starttls' }
}

const TLS_OPTIONS = ['ssl', 'starttls', 'none']

function emptyForm(): AccountForm {
  return {
    name: '',
    displayName: '',
    username: '',
    password: '',
    imapHost: '',
    imapPort: 993,
    imapTls: 'ssl',
    smtpHost: '',
    smtpPort: 465,
    smtpTls: 'ssl'
  }
}

/** 统一表单模型：新增与编辑共用同一套字段与布局。 */
const form = reactive<AccountForm>(emptyForm())
/** 从统一表单识别预设；只有用户选择时才应用，回填/切换账号不会覆盖自定义配置。 */
const preset = computed({
  get: () => Object.entries(PRESET_DEFAULTS).find(([, p]) =>
    p.imapHost === form.imapHost && p.imapPort === form.imapPort && p.imapTls === form.imapTls &&
    p.smtpHost === form.smtpHost && p.smtpPort === form.smtpPort && p.smtpTls === form.smtpTls
  )?.[0] ?? '',
  set: (name: string) => applyPreset(name)
})
const isAdd = computed(() => mode.value === 'add')

interface AddDraft {
  preset: string
  form: AccountForm
}
/**
 * 表单草稿缓存：在左侧账号之间切换、或切到「添加账号」再切回时，
 * 保留用户已填但**尚未保存**的内容，避免切页丢输入。保存成功后清除对应草稿。
 */
const editDrafts = reactive<Record<string, AccountForm>>({})
const addDraft = ref<AddDraft | null>(null)

function applyPreset(name: string): void {
  const p = PRESET_DEFAULTS[name]
  if (!p) return
  form.imapHost = p.imapHost
  form.imapPort = p.imapPort
  form.imapTls = p.imapTls
  form.smtpHost = p.smtpHost
  form.smtpPort = p.smtpPort
  form.smtpTls = p.smtpTls
}

function syncForm(a: MailAccount | null): void {
  if (!a) return
  form.name = a.name
  form.displayName = a.displayName ?? ''
  form.username = a.username
  form.password = ''
  form.imapHost = a.imapHost
  form.imapPort = a.imapPort
  form.imapTls = a.imapTls
  form.smtpHost = a.smtpHost
  form.smtpPort = a.smtpPort
  form.smtpTls = a.smtpTls
}

function resetAddForm(): void {
  Object.assign(form, emptyForm())
  applyPreset(mail.state.presets[0] ?? 'qq')
}

/** 把当前表单内容存入草稿（切页前调用）。 */
function persistCurrent(): void {
  if (mode.value === 'edit' && editing.value) {
    editDrafts[editing.value.name] = { ...form }
  } else if (mode.value === 'add') {
    addDraft.value = { preset: preset.value, form: { ...form } }
  }
}

/** 恢复某账号的编辑表单：有草稿用草稿，否则从账号数据回填。 */
function restoreEdit(name: string): void {
  const draft = editDrafts[name]
  if (draft) {
    Object.assign(form, draft)
  } else {
    syncForm(mail.state.accounts.find((a) => a.name === name) ?? null)
  }
}

/** 恢复「添加账号」表单：有草稿用草稿，否则重置。 */
function restoreAdd(): void {
  if (addDraft.value) {
    preset.value = addDraft.value.preset
    Object.assign(form, addDraft.value.form)
  } else {
    resetAddForm()
  }
}

/** 选中已有账号进入编辑；账号列表清空时回退到「添加账号」。 */
function pickDefault(): void {
  if (!mail.state.accounts.length) {
    mode.value = 'add'
    selected.value = null
    if (!addDraft.value) resetAddForm()
    return
  }
  if (mode.value === 'add') return
  if (!selected.value || !mail.state.accounts.some((a) => a.name === selected.value)) {
    selected.value = mail.state.active ?? mail.state.accounts[0].name
  }
  restoreEdit(selected.value)
}

function selectAccount(name: string): void {
  persistCurrent()
  mode.value = 'edit'
  selected.value = name
  restoreEdit(name)
}

function startAdd(): void {
  persistCurrent()
  mode.value = 'add'
  selected.value = null
  restoreAdd()
}

watch(() => mail.state.accounts, pickDefault, { deep: true })

async function loadAll(): Promise<void> {
  try {
    await mail.loadState()
  } catch {
    /* 后端未就绪时静默 */
  }
  if (!preset.value) resetAddForm()
  pickDefault()
}

async function onAdd(): Promise<void> {
  if (!form.name.trim() || !form.username.trim() || !form.password.trim()) {
    Message.warning(t('mail.needFields'))
    return
  }
  saving.value = true
  try {
    await mail.addAccount({
      preset: preset.value || undefined,
      name: form.name.trim(),
      username: form.username.trim(),
      password: form.password.trim(),
      displayName: form.displayName.trim() || undefined,
      imapHost: form.imapHost.trim() || undefined,
      imapPort: form.imapPort,
      imapTls: form.imapTls,
      smtpHost: form.smtpHost.trim() || undefined,
      smtpPort: form.smtpPort,
      smtpTls: form.smtpTls
    })
    Message.success(t('mail.accountAdded'))
    const name = form.name.trim()
    addDraft.value = null
    resetAddForm()
    selectAccount(name)
  } catch (e) {
    Message.error((e as Error).message)
  } finally {
    saving.value = false
  }
}

async function onSaveEdit(): Promise<void> {
  if (!editing.value) return
  saving.value = true
  try {
    const name = editing.value.name
    await mail.updateAccount(name, {
      displayName: form.displayName,
      username: form.username,
      password: form.password || undefined,
      imapHost: form.imapHost,
      imapPort: form.imapPort,
      imapTls: form.imapTls,
      smtpHost: form.smtpHost,
      smtpPort: form.smtpPort,
      smtpTls: form.smtpTls
    })
    // 保存成功：丢弃草稿并按最新账号数据回填（密码字段清空）
    delete editDrafts[name]
    syncForm(mail.state.accounts.find((a) => a.name === name) ?? null)
    Message.success(t('mail.saved'))
  } catch (e) {
    Message.error((e as Error).message)
  } finally {
    saving.value = false
  }
}

/** 统一保存入口：新增 → add；编辑 → update。 */
function onSave(): void {
  if (isAdd.value) void onAdd()
  else void onSaveEdit()
}

async function onActivate(): Promise<void> {
  if (!editing.value) return
  try {
    await mail.activateAccount(editing.value.name)
    Message.success(t('mail.activeSet'))
  } catch (e) {
    Message.error((e as Error).message)
  }
}

function onRemove(): void {
  const a = editing.value
  if (!a) return
  // 原为 Modal.warning(hideCancel: false)：是「移除账号」的二选一确认，必须保留取消
  // （macAlert 只有一个按钮，点下即执行删除），故用 warning 色调的 macConfirm
  void macConfirm({
    title: t('mail.removeAccount'),
    content: t('mail.removeConfirm', { name: a.name }),
    tone: 'warning',
    onConfirm: async () => {
      try {
        await mail.removeAccount(a.name)
        delete editDrafts[a.name]
        Message.success(t('mail.removed'))
      } catch (e) {
        Message.error((e as Error).message)
      }
    }
  })
}

onMounted(loadAll)
</script>

<template>
  <div class="mail-page settings-editor-page">
    <div class="settings-editor-heading">
      <h2 class="settings-editor-title">{{ t('mail.manage') }}</h2>
      <p class="settings-editor-desc">{{ t('mail.settingsDesc') }}</p>
    </div>

    <div class="settings-editor-card">
      <!-- 左：账号列表 + 添加账号（与模型页一致） -->
      <div class="settings-editor-sidebar">

        <div class="settings-editor-list">
          <button
            v-for="a in mail.state.accounts"
            :key="a.name"
            type="button"
            class="settings-editor-item"
            :class="{ 'settings-editor-item-active': mode === 'edit' && selected === a.name }"
            @click="selectAccount(a.name)"
          >
            <span class="settings-editor-icon"><icon-mail :size="16" /></span>
            <span class="settings-editor-name">
              <span class="truncate">{{ a.displayName || a.name }}</span>
              <span v-if="a.name === mail.state.active" class="settings-editor-tag">{{ t('mail.active') }}</span>
            </span>
            <span v-if="a.name === mail.state.active" class="settings-editor-dot" />
          </button>
        </div>
        <button
          type="button"
          class="settings-editor-add"
          :class="{ 'settings-editor-add-active': mode === 'add' }"
          @click="startAdd"
        >
          <icon-plus :size="14" />
          {{ t('mail.addAccount') }}
        </button>
      </div>

      <!-- 右：账号配置（新增与编辑共用同一页面） -->
      <div class="settings-editor-detail">
        <div class="settings-editor-header">
          <div class="settings-editor-label">{{ t('mail.accountConfig') }}</div>
          <div class="settings-editor-actions">
            <a-button
              v-if="!isAdd && editing && editing.name !== mail.state.active"
              size="small"
              @click="onActivate"
            >
              <template #icon><icon-check-circle :size="13" /></template>
              {{ t('mail.setActive') }}
            </a-button>
            <a-button type="primary" size="small" :loading="saving" @click="onSave">
              <template #icon><icon-save :size="13" /></template>
              {{ t('mail.save') }}
            </a-button>
            <a-button v-if="!isAdd" size="small" @click="onRemove">
              <template #icon><icon-delete :size="13" /></template>
              {{ t('mail.remove') }}
            </a-button>
          </div>
        </div>

        <div class="form">
          <!-- 服务商预设：新增与编辑共用，已保存的自定义服务器原样保留。 -->
          <section class="form-group">
            <div class="settings-editor-field">
              <span class="settings-editor-field-label">
                {{ t('mail.preset') }}
                <a-tooltip :content="t('mail.fieldTips.preset')" position="top">
                  <icon-info :size="15" class="field-help" />
                </a-tooltip>
              </span>
              <a-select v-model="preset" size="medium" :placeholder="t('mail.customPreset')">
                <template #prefix><icon-config :size="14" /></template>
                <a-option v-for="p in mail.state.presets" :key="p" :value="p">{{ p }}</a-option>
              </a-select>
            </div>
          </section>

          <!-- 账号信息 -->
          <section class="form-group">
            <div class="grid2">
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  <span class="settings-editor-required">*</span>{{ t('mail.accountName') }}
                  <a-tooltip :content="t('mail.fieldTips.alias')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-input v-model="form.name" size="medium" :disabled="!isAdd" :placeholder="t('mail.ph.alias')" allow-clear>
                  <template #prefix><icon-idcard :size="14" /></template>
                </a-input>
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  {{ t('mail.displayName') }}
                  <a-tooltip :content="t('mail.fieldTips.displayName')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-input v-model="form.displayName" size="medium" :placeholder="t('mail.ph.displayName')" allow-clear>
                  <template #prefix><icon-edit :size="14" /></template>
                </a-input>
              </div>
            </div>
            <div class="settings-editor-field">
              <span class="settings-editor-field-label">
                <span class="settings-editor-required">*</span>{{ t('mail.username') }}
                <a-tooltip :content="t('mail.fieldTips.username')" position="top">
                  <icon-info :size="15" class="field-help" />
                </a-tooltip>
              </span>
              <a-input v-model="form.username" size="medium" :placeholder="t('mail.ph.username')" allow-clear>
                <template #prefix><icon-message :size="14" /></template>
              </a-input>
            </div>
            <div class="settings-editor-field">
              <span class="settings-editor-field-label">
                <span class="settings-editor-required">*</span>{{ isAdd ? t('mail.password') : t('mail.passwordKeep') }}
                <a-tooltip
                  :content="isAdd ? t('mail.fieldTips.password') : t('mail.fieldTips.passwordKeep')"
                  position="top"
                >
                  <icon-info :size="15" class="field-help" />
                </a-tooltip>
              </span>
              <a-input-password
                v-model="form.password"
                size="medium"
                :placeholder="isAdd ? t('mail.ph.password') : t('mail.ph.passwordKeep')"
                allow-clear
              >
                <template #prefix><icon-safe :size="14" /></template>
              </a-input-password>
            </div>
          </section>

          <!-- 接收服务器 -->
          <section class="form-group">
            <h3 class="form-group-title">{{ t('mail.groupImap') }}</h3>
            <div class="grid3">
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  IMAP Host
                  <a-tooltip :content="t('mail.fieldTips.imapHost')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-input v-model="form.imapHost" size="medium" :placeholder="t('mail.ph.imapHost')" allow-clear>
                  <template #prefix><icon-globe :size="14" /></template>
                </a-input>
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  IMAP Port
                  <a-tooltip :content="t('mail.fieldTips.imapPort')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-input-number v-model="form.imapPort" size="medium" :min="1" :max="65535" placeholder="993" />
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  IMAP TLS
                  <a-tooltip :content="t('mail.fieldTips.imapTls')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-select v-model="form.imapTls" size="medium">
                  <template #prefix><icon-lock :size="14" /></template>
                  <a-option v-for="o in TLS_OPTIONS" :key="o" :value="o">{{ o }}</a-option>
                </a-select>
              </div>
            </div>
          </section>

          <!-- 发送服务器 -->
          <section class="form-group">
            <h3 class="form-group-title">{{ t('mail.groupSmtp') }}</h3>
            <div class="grid3">
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  SMTP Host
                  <a-tooltip :content="t('mail.fieldTips.smtpHost')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-input v-model="form.smtpHost" size="medium" :placeholder="t('mail.ph.smtpHost')" allow-clear>
                  <template #prefix><icon-globe :size="14" /></template>
                </a-input>
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  SMTP Port
                  <a-tooltip :content="t('mail.fieldTips.smtpPort')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-input-number v-model="form.smtpPort" size="medium" :min="1" :max="65535" placeholder="465" />
              </div>
              <div class="settings-editor-field">
                <span class="settings-editor-field-label">
                  SMTP TLS
                  <a-tooltip :content="t('mail.fieldTips.smtpTls')" position="top">
                    <icon-info :size="15" class="field-help" />
                  </a-tooltip>
                </span>
                <a-select v-model="form.smtpTls" size="medium">
                  <template #prefix><icon-lock :size="14" /></template>
                  <a-option v-for="o in TLS_OPTIONS" :key="o" :value="o">{{ o }}</a-option>
                </a-select>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 表单：限制阅读宽度，避免输入框被过度拉伸、行长过长 */
.form {
  display: flex;
  flex-direction: column;
  gap: 28px;
  max-width: 720px;
}
/* 分组：用留白 + 小标题分隔，降低密集感（Apple「按关系分组」） */
.form-group {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.form-group-title {
  margin: 0;
  font-size: var(--font-size-xs);
  font-weight: 600;
  letter-spacing: 0.05em;
  color: var(--text-tertiary);
}
/* 字段说明图标：悬浮显示含义（cursor: help 提示可交互） */
.field-help {
  display: inline-flex;
  align-items: center;
  color: var(--text-tertiary);
  cursor: help;
  transition: color 0.15s ease;
}
.field-help:hover {
  color: var(--text-secondary);
}
.grid2 {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
}
/* 服务器组：主机 / 端口 / TLS 左右三列（主机更宽） */
.grid3 {
  display: grid;
  grid-template-columns: minmax(0, 1.6fr) minmax(0, 0.8fr) minmax(0, 1fr);
  gap: 16px;
}
/* 按表单的实际可用宽度响应，而非窗口宽度（侧栏展开也会压缩内容区）。 */
@container settings-editor-form (max-width: 460px) {
  .grid3 {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .grid3 > .settings-editor-field:first-child {
    grid-column: 1 / -1;
  }
}

@container settings-editor-form (max-width: 300px) {
  .grid3 > .settings-editor-field:first-child { grid-column: auto; }
  .grid2,
  .grid3 {
    grid-template-columns: minmax(0, 1fr);
  }
}

/* 尊重系统「减弱动态效果」：去掉位移/缩放，只用即时状态变化 */
</style>
