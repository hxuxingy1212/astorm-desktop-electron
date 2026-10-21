<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import MentionInput from '../mentions/MentionInput.vue'
import type { ChipMentionRef, MentionItem } from '../mentions/types'
import { initPrompt, planPrompt } from '../mentions/initPrompt'
import { goalStartPrompt } from '../mentions/goalPrompt'
import ElementAttachment from './ElementAttachment.vue'
import SkillAttachment from './SkillAttachment.vue'
import QuoteAttachment from './QuoteAttachment.vue'
import AttachmentBar from './AttachmentBar.vue'
import ModelEffortSelect from './ModelEffortSelect.vue'
import SessionInfoCard from './SessionInfoCard.vue'
import SessionGoalButton from './SessionGoalButton.vue'
import TaskPlanButtons from './TaskPlanButtons.vue'
import type { MessageAttachment } from '@/types'
import { useModelsStore } from '@/stores/models'
import { useChatPrefsStore, type ChatMode } from '@/stores/chatPrefs'
import type { SessionSettingsView } from '@/stores/sessions'
import { useElementPickStore } from '@/stores/elementPick'
import { useOfficePickStore } from '@/stores/officePick'
import OfficeAttachment from './OfficeAttachment.vue'
import { useQuotePickStore, quotePreview } from '@/stores/quotePick'
import { elementChipLabel } from '@/utils/elementBlock'
import { officeRefChipLabel } from '@/utils/officeRef'
import type { QuoteRefPayload } from '@/components/mentions/types'

const props = defineProps<{
  streaming?: boolean
  /** 去掉输入栏容器左右留白,使胶囊铺满外层容器宽度 */
  noPadding?: boolean
  /** 去掉胶囊内边框(聊天页避免与外层边框叠成双层;新建会话页保留内边框) */
  noInnerBorder?: boolean
  /** 草稿态(新建会话页):尚无会话,隐藏会话级控件(上下文容量环)——
   *  不隐藏的话会把当前激活会话的占用百分比带到这张「空白」表单上 */
  noSessionInfo?: boolean
  /** 当前会话工作目录(绝对路径):@ 提及的文件列表数据源,缺省时弹层提示先选目录 */
  workspaceDir?: string
  /**
   * 会话设置(受控):模式 / 权限 / 思考强度 / 模型 / 供应商。
   * 这五项是**会话的持久化属性**,不在本组件内自持 —— 父组件接会话 store(ChatPanel)
   * 或新建草稿(NewChatView),改动经 `update:settings` 回传并由父组件下发。
   */
  settings: SessionSettingsView
}>()

/** 发送载荷:文本 + 附件。会话设置不随消息走;仅 `/plan` 显式携带 mode(原子切计划模式) */
interface ChatSendPayload {
  text: string
  /** 图片附件(staging 路径列表;后端复制进会话工作区) */
  attachments?: MessageAttachment[]
  /** 显式会话模式(仅 `/plan` 使用:随本条消息原子切到 plan,避免异步竞态) */
  mode?: ChatMode
  /** 目标文本(仅 `/goal` 使用:随本条消息原子设置会话目标循环) */
  goal?: string
  /** 目标语言(界面语言,如 zh-CN/en-US;决定判定提示词与续跑指令语言) */
  goalLang?: string
  /** 文件/智能体引用(原文之外的结构化标识,作为消息元数据下发) */
  mentions?: ChipMentionRef[]
  /** 浏览器拾取元素的围栏原文(作为消息元数据下发) */
  elements?: string[]
  /** office 预览拾取的 ```office-ref 围栏原文(作为消息元数据下发) */
  officeRefs?: string[]
  /** 对话正文引用(完整文本 + 来源,作为消息元数据下发) */
  quotes?: QuoteRefPayload[]
}

const emit = defineEmits<{
  (e: 'send', payload: ChatSendPayload): void
  (e: 'stop'): void
  /** 命令(整条输入恰为 `/xxx`):不经模型,由父组件执行对应动作(如 undo) */
  (e: 'command', id: string): void
  /** 会话设置改动(只带改动字段;父组件负责下发与持久化) */
  (e: 'update:settings', patch: Partial<SessionSettingsView>): void
}>()

/** 已识别的「控制型」命令:整条输入恰为其一时不发往模型(undo=撤回, fork=派生, compact=摘要)。 */
const ACTION_COMMANDS = new Set(['undo', 'fork', 'compact'])

const { t, locale } = useI18n()
const modelsStore = useModelsStore()
const chatPrefs = useChatPrefsStore()
const elementPick = useElementPickStore()
const officePick = useOfficePickStore()
const quotePick = useQuotePickStore()
const draft = ref('')
const isFocused = ref(false)

/** 通过 `$` 选中的技能:由输入框上方胶囊承载,不插入正文(发送时序列化为 `$id`) */
const selectedSkills = ref<MentionItem[]>([])

/** 对话框宽度档位:根据胶囊实际宽度隐藏溢出控件(优先级:先思考强度 → 再模型)。
 *  实测本 Electron 环境的 ResizeObserver 回调被节流(遮挡窗口不触发),
 *  改用 300ms 轻量轮询,仅在宽度档位变化时写一次 class */
type CompactStage = 'wide' | 'hide-thinking' | 'hide-model'
const pillRef = ref<HTMLElement | null>(null)
const stage = ref<CompactStage>('wide')
let stageTimer: ReturnType<typeof setInterval> | null = null
let stageSeen: CompactStage | '' = ''
function applyStage(): void {
  const w = pillRef.value?.getBoundingClientRect().width ?? 0
  // 思考强度优先隐藏(≤600);再隐藏模型(≤560,在挤到自动授权之前);发送按钮始终保留
  const next: CompactStage = w <= 560 ? 'hide-model' : w <= 600 ? 'hide-thinking' : 'wide'
  if (next !== stageSeen) {
    stageSeen = next
    stage.value = next
  }
}
onMounted(() => {
  applyStage()
  stageTimer = setInterval(applyStage, 300)
})
onBeforeUnmount(() => {
  if (stageTimer) clearInterval(stageTimer)
  stageTimer = null
})

// 模型选择:数据源 = 设置中的供应商配置;无任何模型时输入栏显示「无模型配置」占位。
// 选择 UI 由 ModelEffortSelect 承担(模型 + 推理等级组合菜单),这里只判定是否有模型。
const hasModels = computed(() =>
  modelsStore.providers.some((p) => p.models.length > 0)
)

// 配置加载/变更后,把「新建会话的默认模型」回落/初始化到优先级(顺序)第一个供应商+模型;
// 已有会话的模型来自会话设置(props.settings),不在这里改写
watch(
  () => modelsStore.providers,
  () => chatPrefs.applyDefaultSelection(),
  { deep: true, immediate: true }
)

/** 模型/推理等级改动:作为会话设置补丁下发(父组件经 chat:update-session-settings 立即生效) */
function onModelEffortChange(patch: {
  provider?: string
  model?: string
  thinkingLevel?: number
}): void {
  emit('update:settings', patch)
}
// 会话模式(固定选项,后续接入真实 Agent 时按模式路由)
const mode = computed({
  get: () => props.settings.mode,
  set: (v: ChatMode) => emit('update:settings', { mode: v })
})
const MODE_OPTIONS = computed<{ value: ChatMode; label: string; iconKey: string }[]>(() => [
  { value: 'plan', label: t('chat.modePlan'), iconKey: 'icon-clipboard-list' },
  { value: 'build', label: t('chat.modeExecute'), iconKey: 'icon-build' },
  { value: 'auto', label: t('chat.modeAuto'), iconKey: 'icon-robot' }
])

// 当前模式的图标/文案(选中项前缀展示)
const currentModeIcon = computed(
  () => MODE_OPTIONS.value.find((m) => m.value === mode.value)?.iconKey ?? 'icon-clipboard-list'
)
const currentModeLabel = computed(
  () => MODE_OPTIONS.value.find((m) => m.value === mode.value)?.label ?? ''
)

// 模式下拉:使用项目统一菜单(非 Arco Select 默认弹层)
const modeMenuVisible = ref(false)
function onModeSelect(value: unknown): void {
  if (value === 'plan' || value === 'build' || value === 'auto') mode.value = value
}

// 权限模式:手动授权(绿盾) / 自动授权(橙盾),点击切换
const permission = computed(() => props.settings.permission)
function togglePermission(): void {
  emit('update:settings', { permission: props.settings.permission === 'ask' ? 'allow' : 'ask' })
}

/** 发送文本 = 输入内容 + 拾取的元素文本块(附件形式追加在末尾);两者皆空不可发送 */
function composeText(text: string): string {
  // 已选技能(`$` 由上方胶囊承载)序列化为 `$id` token;
  // 网页元素已作为**内联 chip** 存在于 text(序列化即 ```element 围栏),不再追加
  const skills = selectedSkills.value.length
    ? selectedSkills.value.map((s) => `$${s.id}`).join(' ')
    : ''
  return [text, skills].filter(Boolean).join('\n\n')
}

/** 把 store 里的网页元素同步为输入框内联 chip(增删与顶部胶囊一致) */
function syncElementChips(): void {
  mentionRef.value?.syncElementChips(
    elementPick.items.map((it) => ({
      key: it.ts,
      token: it.token,
      label: elementChipLabel(it.data)
    }))
  )
}

/** 用户删除内联 chip 后:把 store 中已不存在的元素一并移除(与顶部胶囊计数同步) */
function reconcileElementKeys(): void {
  const present = new Set(mentionRef.value?.elementChipKeys() ?? [])
  for (const it of [...elementPick.items]) {
    if (!present.has(it.ts)) elementPick.removeByKey(it.ts)
  }
}

/** 把 store 里的对话引用同步为输入框内联 chip(增删与顶部胶囊一致) */
function syncQuoteChips(): void {
  mentionRef.value?.syncQuoteChips(
    quotePick.items.map((it) => ({ key: it.ts, label: quotePreview(it.text) }))
  )
}

/** 用户删除内联 chip 后:把 store 中已不存在的引用一并移除 */
function reconcileQuoteKeys(): void {
  const present = new Set(mentionRef.value?.quoteChipKeys() ?? [])
  for (const it of [...quotePick.items]) {
    if (!present.has(it.ts)) quotePick.removeByKey(it.ts)
  }
}

/** 把 store 里的 office 路径引用同步为输入框内联 chip(office 预览拾取) */
function syncOfficeChips(): void {
  mentionRef.value?.syncOfficeChips(
    officePick.items.map((it) => ({
      key: it.ts,
      token: it.token,
      label: officeRefChipLabel(it.data)
    }))
  )
}

/** 用户删除内联 chip 后:把 store 中已不存在的 office 引用一并移除 */
function reconcileOfficeKeys(): void {
  const present = new Set(mentionRef.value?.officeChipKeys() ?? [])
  for (const it of [...officePick.items]) {
    if (!present.has(it.ts)) officePick.removeByKey(it.ts)
  }
}

/** 编辑器内容变化:元素 / 对话引用 / office 路径 内联 chip 与各自 store 对账 */
function reconcileRefChips(): void {
  reconcileElementKeys()
  reconcileQuoteKeys()
  reconcileOfficeKeys()
}

// store ↔ 编辑器内联 chip 同步:元素增删(浏览器拾取 / 胶囊 × / 删除 chip)保持一致
watch(
  () => elementPick.items.map((i) => i.ts).join(','),
  () => syncElementChips(),
  { flush: 'post' }
)
// office 路径引用同理
watch(
  () => officePick.items.map((i) => i.ts).join(','),
  () => syncOfficeChips(),
  { flush: 'post' }
)
// 对话引用同理
watch(
  () => quotePick.items.map((i) => i.ts).join(','),
  () => syncQuoteChips(),
  { flush: 'post' }
)
onMounted(() => {
  syncElementChips()
  syncQuoteChips()
})

/** `$` 选中技能:不插入正文,改由上方胶囊承载(去重) */
function onMentionPick(item: MentionItem): void {
  if (item.type !== 'skill') return
  if (selectedSkills.value.some((s) => s.id === item.id)) return
  selectedSkills.value = [...selectedSkills.value, item]
}

/** 选中条目是否插入 chip:技能交给上方胶囊,其余(命令/智能体/文件)照常内联 */
function shouldCommitMention(item: MentionItem): boolean {
  return item.type !== 'skill'
}

/** 单条消息**图片**上限（与后端一致；超出的由后端逐张忽略并提示） */
const MAX_IMAGES = 8
/** 单条消息**非图片**附件上限（与后端 `MAX_EXTERNAL_FILES` 一致；只做路径引用，不占上下文） */
const MAX_FILES = 20

/**
 * 待发送的附件：图片是 staging 副本路径（后端复制进会话工作区并作为图片上送）；
 * 其他文件是**用户原始路径**（后端只写进消息文本告知模型，不复制）。
 */
const attachments = ref<MessageAttachment[]>([])

/* ---------- 顶部附加胶囊的统一排序 ----------
 * 附件 / 技能 / 网页元素 / 对话引用默认分组固定顺序,会让「后选的技能 / 元素 / 引用」总排在
 * 附件后面。这里给每个**新出现**的条目发一个统一递增序号,分组顺序取组内最小序号,再以 CSS
 * `order` 施加到各分组容器上 —— 各组按「谁先加进来」横向排列。
 * 注意:必须放在 `attachments` 声明之后——watcher 的 getter 在注册时即求值,提前引用会 TDZ。 */
let topSeq = 0
const attachRank = ref<Record<string, number>>({})
const skillRank = ref<Record<string, number>>({})
const elementRank = ref<Record<string, number>>({})
const quoteRank = ref<Record<string, number>>({})
const officeRank = ref<Record<string, number>>({})

/** 保留已有条目的序号,新条目按出现顺序续号并剔除已移除项 */
function restamp(prev: Record<string, number>, keys: string[]): Record<string, number> {
  const out: Record<string, number> = {}
  for (const key of keys) out[key] = prev[key] ?? ++topSeq
  return out
}

/** 组序号 = 组内最小条目序号(组始终非空时才渲染,故不会用到哨兵值) */
function minRank(rank: Record<string, number>, keys: string[]): number {
  let min = Number.MAX_SAFE_INTEGER
  for (const key of keys) {
    const v = rank[key]
    if (v != null && v < min) min = v
  }
  return min
}

// 四类各自一个字符串源 watcher(新增条目续号,移除条目自然出局),可靠且互不干扰
watch(
  () => attachments.value.map((a) => a.path).join('|'),
  () => {
    attachRank.value = restamp(attachRank.value, attachments.value.map((a) => a.path))
  },
  { immediate: true }
)
watch(
  () => selectedSkills.value.map((s) => s.id).join('|'),
  () => {
    skillRank.value = restamp(skillRank.value, selectedSkills.value.map((s) => s.id))
  },
  { immediate: true }
)
watch(
  () => elementPick.items.map((i) => String(i.ts)).join('|'),
  () => {
    elementRank.value = restamp(
      elementRank.value,
      elementPick.items.map((i) => String(i.ts))
    )
  },
  { immediate: true }
)
watch(
  () => quotePick.items.map((i) => String(i.ts)).join('|'),
  () => {
    quoteRank.value = restamp(
      quoteRank.value,
      quotePick.items.map((i) => String(i.ts))
    )
  },
  { immediate: true }
)
watch(
  () => officePick.items.map((i) => String(i.ts)).join('|'),
  () => {
    officeRank.value = restamp(
      officeRank.value,
      officePick.items.map((i) => String(i.ts))
    )
  },
  { immediate: true }
)

const attachOrder = computed(() =>
  minRank(attachRank.value, attachments.value.map((a) => a.path))
)
const skillOrder = computed(() =>
  minRank(skillRank.value, selectedSkills.value.map((s) => s.id))
)
const elementOrder = computed(() =>
  minRank(elementRank.value, elementPick.items.map((i) => String(i.ts)))
)
const officeOrder = computed(() =>
  minRank(officeRank.value, officePick.items.map((i) => String(i.ts)))
)
const quoteOrder = computed(() =>
  minRank(quoteRank.value, quotePick.items.map((i) => String(i.ts)))
)

/** 是否图片附件（非图片不取字节、不占图片配额） */
function isImageAttachment(item: MessageAttachment): boolean {
  return !!item.mime?.startsWith('image/')
}

function countImages(): number {
  return attachments.value.filter(isImageAttachment).length
}

function countFiles(): number {
  return attachments.value.filter((a) => !isImageAttachment(a)).length
}

/** 把处理结果并入附件列表（按类型各自限数 + 去重），失败项即时提示 */
function addStaged(res: { ok: MessageAttachment[]; failed: { name: string; reason: string }[] }): void {
  for (const item of res.ok) {
    if (isImageAttachment(item)) {
      if (countImages() >= MAX_IMAGES) break
    } else if (countFiles() >= MAX_FILES) break
    if (attachments.value.some((x) => x.path === item.path)) continue
    attachments.value.push({ path: item.path, name: item.name, size: item.size, mime: item.mime })
  }
  if (res.failed.length) {
    Message.warning(res.failed.map((f) => `${f.name}：${f.reason}`).join('；'))
  }
}

function removeAttachment(index: number): void {
  attachments.value.splice(index, 1)
}

/** 「+」:系统文件选择器（**不限类型**；图片由主进程按内容识别后走图片逻辑） */
async function onAttach(): Promise<void> {
  const api = window.api
  if (!api?.dialog?.openAttachments || !api.attachment) return
  if (countImages() >= MAX_IMAGES && countFiles() >= MAX_FILES) {
    Message.warning(t('chat.maxAttachments', { images: MAX_IMAGES, files: MAX_FILES }))
    return
  }
  try {
    const paths = await api.dialog.openAttachments()
    if (!paths.length) return
    addStaged(await api.attachment.stagePaths(paths))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

/**
 * 粘贴:剪贴板含图片时添加附件(截图粘贴的常见路径)。
 * MentionInput 自身的 paste 处理只认 text/plain(图片粘贴时拿不到文本会直接返回),
 * 事件冒泡到此层再取二进制,故无需改动编辑器。
 */
async function onPaste(event: ClipboardEvent): Promise<void> {
  const api = window.api
  if (!api?.attachment) return
  const files = Array.from(event.clipboardData?.files ?? []).filter((f) => f.type.startsWith('image/'))
  if (!files.length) {
    // 少数平台只给类型不给 File → 让主进程直接读系统剪贴板图片
    const types = Array.from(event.clipboardData?.types ?? [])
    if (!types.includes('Files') && !types.some((ty) => ty.startsWith('image/'))) return
    if (countImages() >= MAX_IMAGES) return
    addStaged(await api.attachment.stageClipboard())
    return
  }
  event.preventDefault()
  if (countImages() >= MAX_IMAGES) {
    Message.warning(t('chat.maxImages', { count: MAX_IMAGES }))
    return
  }
  for (const file of files) {
    if (countImages() >= MAX_IMAGES) break
    const bytes = new Uint8Array(await file.arrayBuffer())
    addStaged(await api.attachment.stageBytes(bytes, file.name || 'image.png'))
  }
}

/**
 * 拖拽文件到输入框：有真实路径的一律收（图片与任意文件都走同一入口，由主进程按内容分类），
 * 无路径的（如从浏览器拖出的图片）才退回字节 staging。
 */
async function onDrop(event: DragEvent): Promise<void> {
  const api = window.api
  dropActive.value = false
  const files = Array.from(event.dataTransfer?.files ?? [])
  if (!api?.attachment || !files.length) return
  if (countImages() >= MAX_IMAGES && countFiles() >= MAX_FILES) {
    Message.warning(t('chat.maxAttachments', { images: MAX_IMAGES, files: MAX_FILES }))
    return
  }
  const paths: string[] = []
  for (const file of files) {
    const path = api.attachment.pathForFile(file)
    if (path) paths.push(path)
  }
  if (paths.length) {
    addStaged(await api.attachment.stagePaths(paths))
  }
  for (const file of files) {
    if (api.attachment.pathForFile(file) || !file.type.startsWith('image/')) continue
    if (countImages() >= MAX_IMAGES) break
    const bytes = new Uint8Array(await file.arrayBuffer())
    addStaged(await api.attachment.stageBytes(bytes, file.name || 'image.png'))
  }
}

/** 拖拽悬停高亮:仅文件拖拽(而非文本/选区拖拽)才提示可放置 */
const dropActive = ref(false)
function onDragOver(event: DragEvent): void {
  if (Array.from(event.dataTransfer?.types ?? []).includes('Files')) dropActive.value = true
}

/** 移入子元素也会触发 dragleave,仅在真正离开胶囊时复位 */
function onDragLeave(event: DragEvent): void {
  const next = event.relatedTarget as Node | null
  if (next && pillRef.value?.contains(next)) return
  dropActive.value = false
}

/** 发送载荷组装:文本 + 附件 + 引用元数据(会话设置随会话走;mode/goal 仅 /plan、/goal 显式给出) */
function sendPayload(
  text: string,
  opts: { mode?: ChatMode; goal?: string; goalLang?: string } = {}
): ChatSendPayload {
  const pending = attachments.value.length ? [...attachments.value] : undefined
  const refs = mentionRef.value?.collectMentions()
  const quotes: QuoteRefPayload[] = quotePick.items.map((it) => ({
    text: it.text,
    seq: it.seq,
    role: it.role
  }))
  return {
    text,
    attachments: pending,
    ...opts,
    mentions: refs?.mentions.length ? refs.mentions : undefined,
    elements: refs?.elements.length ? refs.elements : undefined,
    officeRefs: refs?.officeRefs.length ? refs.officeRefs : undefined,
    quotes: quotes.length ? quotes : undefined
  }
}

/** 发送后清空两类「引用」选取(附件 / 技能另清) */
function clearRefPicks(): void {
  elementPick.remove()
  officePick.remove()
  quotePick.remove()
}

const canSend = computed(
  () =>
    !props.streaming &&
    (draft.value.trim().length > 0 ||
      elementPick.count > 0 ||
      officePick.count > 0 ||
      quotePick.count > 0 ||
      selectedSkills.value.length > 0 ||
      attachments.value.length > 0)
)
// 流式中:发送按钮切换为红色停止按钮
// 流式中:按钮恒为红色停止(不再因聚焦+有草稿切成发送,避免误触)。
// streaming 时的发送走 Enter(onSubmit),其提交的消息正常入队
const showStop = computed(() => props.streaming)

function onAction(): void {
  if (showStop.value) {
    emit('stop')
  } else {
    submit()
  }
}

/** 整条输入恰为一个控制命令(`/undo`)时返回其 id,否则 null */
function actionCommandOf(raw: string): string | null {
  const m = raw.trim().match(/^\/([a-z-]+)$/)
  return m && ACTION_COMMANDS.has(m[1]) ? m[1] : null
}

/**
 * 文本宏命令（非控制命令，整条输入匹配触发）：
 *   `/init [附加约束]` → 替换为初始化提示词；
 *   `/plan <需求>` → 替换为计划提示词并切「规划」模式（需求为空拒绝）；
 *   `/goal <目标>` → 替换为启动提示词并随消息设置会话目标循环（目标为空拒绝）。
 */
const INIT_RE = /^\/init(?:\s+([\s\S]*))?$/
const PLAN_RE = /^\/plan(?:\s+([\s\S]*))?$/
const GOAL_RE = /^\/goal(?:\s+([\s\S]*))?$/

function resolveText(raw: string): string {
  const m = raw.match(INIT_RE)
  return m ? initPrompt(locale.value, m[1]) : raw
}

function submit(): void {
  const raw = draft.value.trim()
  const command = actionCommandOf(raw)
  if (command) {
    emit('command', command)
    draft.value = ''
    attachments.value = []
    clearRefPicks()
    selectedSkills.value = []
    return
  }
  const plan = raw.match(PLAN_RE)
  if (plan) {
    const req = (plan[1] ?? '').trim()
    if (!req) {
      Message.warning(t('chat.planNeedRequirement'))
      return // 保留草稿,便于用户补充需求
    }
    emit('update:settings', { mode: 'plan' })
    emit('send', sendPayload(composeText(planPrompt(locale.value, req)), { mode: 'plan' }))
    draft.value = ''
    attachments.value = []
    clearRefPicks()
    selectedSkills.value = []
    return
  }
  const goal = raw.match(GOAL_RE)
  if (goal) {
    const text = (goal[1] ?? '').trim()
    if (!text) {
      Message.warning(t('chat.goalNeedText'))
      return // 保留草稿
    }
    emit('send', sendPayload(composeText(goalStartPrompt(locale.value, text)), { goal: text, goalLang: locale.value }))
    draft.value = ''
    attachments.value = []
    clearRefPicks()
    selectedSkills.value = []
    return
  }
  const text = composeText(resolveText(raw))
  // 允许「只有附件没有文字」的消息
  if (!text && !attachments.value.length) return
  emit('send', sendPayload(text))
  draft.value = ''
  attachments.value = []
  clearRefPicks()
  selectedSkills.value = []
}

// 提及编辑器的 Enter 提交:富文本已由 MentionInput 序列化为纯文本(chip → token)
function onSubmit(text: string): void {
  const command = actionCommandOf(text)
  if (command) {
    // 控制命令不发往模型;编辑器 DOM 由 MentionInput 在该事件后自行清空
    emit('command', command)
    attachments.value = []
    clearRefPicks()
    selectedSkills.value = []
    return
  }
  const raw = text.trim()
  const plan = raw.match(PLAN_RE)
  if (plan) {
    const req = (plan[1] ?? '').trim()
    if (!req) {
      Message.warning(t('chat.planNeedRequirement'))
      nextTick(() => mentionRef.value?.setContent(raw))
      return
    }
    emit('update:settings', { mode: 'plan' })
    emit('send', sendPayload(composeText(planPrompt(locale.value, req)), { mode: 'plan' }))
    attachments.value = []
    clearRefPicks()
    selectedSkills.value = []
    return
  }
  const goal = raw.match(GOAL_RE)
  if (goal) {
    const gtext = (goal[1] ?? '').trim()
    if (!gtext) {
      Message.warning(t('chat.goalNeedText'))
      nextTick(() => mentionRef.value?.setContent(raw))
      return
    }
    emit('send', sendPayload(composeText(goalStartPrompt(locale.value, gtext)), { goal: gtext, goalLang: locale.value }))
    attachments.value = []
    clearRefPicks()
    selectedSkills.value = []
    return
  }
  const payload = composeText(resolveText(raw))
  if (!payload && !attachments.value.length) return
  emit('send', sendPayload(payload))
  attachments.value = []
  clearRefPicks()
  selectedSkills.value = []
}

// 外部回显(编辑消息队列):把文本写入编辑器,由父组件(消息队列编辑)调用
const mentionRef = ref<InstanceType<typeof MentionInput> | null>(null)
function setDraft(text: string): void {
  mentionRef.value?.setContent(text)
  // setContent 会重写编辑器 DOM,按 store 重新补回元素 / 对话引用内联 chip
  syncElementChips()
  syncQuoteChips()
}

/** 外部回填附件(撤回消息后恢复发送框):先清空再灌入(按 path 去重) */
function setAttachments(items: MessageAttachment[]): void {
  const seen = new Set<string>()
  attachments.value = (items ?? []).filter((it) => {
    if (!it?.path || seen.has(it.path)) return false
    seen.add(it.path)
    return true
  })
}

/**
 * 读出当前草稿（合成元素引用后的文本 + 图片附件），**不清空输入框**。
 * 给「不走 send 事件」的提交路径用：看板「新建任务」弹窗中的「仅创建」（后端只建不跑）。
 * 用同一份 `composeText` / `sendPayload`，避免两处各拼一遍文本后行为分叉。
 */
function currentDraft(): ChatSendPayload {
  return sendPayload(composeText(draft.value.trim()))
}
defineExpose({ setDraft, setAttachments, currentDraft })
</script>

<template>
  <!-- 输入栏:与消息列共享宽度体系 -->
  <div class="chat-input-bar chat-column" :class="noPadding && 'no-padding'">
    <!-- 顶部附加区(文件选择 + Git 分支 / 消息队列 / 图片 / 技能 / 网页元素附件,独立一层) -->
    <div
      v-if="$slots.top || elementPick.count || selectedSkills.length || attachments.length"
      class="chat-input-top"
    >
      <slot name="top" />
      <!-- 附件/技能/网页元素胶囊:横向排列,放不下时换行 -->
      <div
        v-if="
          attachments.length || selectedSkills.length || elementPick.count ||
          officePick.count || quotePick.count
        "
        class="chat-input-attachments"
      >
        <AttachmentBar
          v-if="attachments.length"
          :items="attachments"
          :style="{ order: attachOrder }"
          @remove="removeAttachment"
        />
        <SkillAttachment
          v-if="selectedSkills.length"
          :skills="selectedSkills"
          :style="{ order: skillOrder }"
          @remove="selectedSkills = []"
        />
        <ElementAttachment v-if="elementPick.count" :style="{ order: elementOrder }" />
        <OfficeAttachment v-if="officePick.count" :style="{ order: officeOrder }" />
        <QuoteAttachment v-if="quotePick.count" :style="{ order: quoteOrder }" />
      </div>
    </div>

    <!-- 对话框:完整的原样边框(圆角 24px),只包 textarea + footer -->
    <div
      ref="pillRef"
      class="chat-input-pill"
      :class="[stage, { 'no-inner-border': noInnerBorder, 'drop-active': dropActive }]"
      @paste="onPaste"
      @dragover.prevent="onDragOver"
      @dragleave="onDragLeave"
      @drop.prevent="onDrop"
    >
      <div class="chat-textarea">
        <MentionInput
          ref="mentionRef"
          v-model="draft"
          :placeholder="t('chat.placeholder')"
          :session-dir="workspaceDir"
          :should-commit="shouldCommitMention"
          @submit="onSubmit"
          @pick="onMentionPick"
          @change="reconcileRefChips"
          @focus="isFocused = true"
          @blur="isFocused = false"
        />
      </div>

      <div class="chat-input-footer">
        <!-- 左侧控件簇(附件/模式/授权):窄时优先压缩裁剪,不为挤掉发送按钮 -->
        <div class="footer-left">
          <!-- 附件:选择图片(+ 也可拖拽 / 粘贴截图) -->
          <button type="button" class="attach-btn" :title="t('chat.attachFile')" @click="onAttach">
            <icon-plus :size="16" />
          </button>
          <!-- 会话模式下拉:计划 / 执行 / 自动(带对应图标)，使用全局菜单配方。
               会话属性:改动立即下发到当前会话,下一个工具调用即生效(不必等发消息) -->
          <a-dropdown
            v-model:popup-visible="modeMenuVisible"
            trigger="click"
            position="bl"
            :popup-max-height="false"
            class="mode-dropdown"
            @select="onModeSelect"
          >
            <button
              type="button"
              class="mode-trigger"
              :aria-label="t('chat.mode')"
              :title="t('chat.settingsApplyLive')"
            >
              <component :is="currentModeIcon" :size="14" />
              <span>{{ currentModeLabel }}</span>
              <icon-down :size="12" class="mode-chevron" :class="{ open: modeMenuVisible }" />
            </button>
            <template #content>
              <a-doption v-for="m in MODE_OPTIONS" :key="m.value" :value="m.value">
                <span class="mode-option">
                  <component :is="m.iconKey" :size="14" />
                  <span>{{ m.label }}</span>
                  <icon-check v-if="mode === m.value" :size="14" class="mode-check" />
                </span>
              </a-doption>
            </template>
          </a-dropdown>
          <!-- 权限模式:绿=手动授权,橙=自动授权,图标+文字,点击切换。
               会话属性:改动立即下发;已在等待的审批卡不受影响,只作用于后续工具调用 -->
          <button
            type="button"
            class="perm-btn"
            :class="permission === 'allow' && 'perm-btn-allow'"
            :aria-label="t('chat.permission')"
            :title="t('chat.settingsApplyLive')"
            @click="togglePermission"
          >
            <icon-safe :size="14" />
            <span class="perm-label">{{ permission === 'ask' ? t('chat.permissionAsk') : t('chat.permissionAllow') }}</span>
          </button>
        </div>

        <div class="flex-1" />

        <!-- 计划 / 目标 / 会话信息：三个图标紧邻无间隔；各自有内容才显示。
             目标位于计划右侧、上下文用量左侧；草稿态没有"当前会话"，均不显示 -->
        <div class="footer-icons">
          <TaskPlanButtons />
          <SessionGoalButton />
          <SessionInfoCard v-if="!noSessionInfo" />
        </div>

        <!-- 模型 + 推理等级:组合选择(参考 DeepSeek Harness ui-model-selection) -->
        <ModelEffortSelect
          v-if="hasModels"
          class="model-effort"
          :provider="props.settings.provider"
          :model="props.settings.model"
          :thinking-level="props.settings.thinkingLevel"
          @change="onModelEffortChange"
        />
        <span v-else class="model-empty-trigger">{{ t('chat.noModels') }}</span>

        <button
          type="button"
          class="send-btn"
          :class="[showStop && 'send-btn-stop', !canSend && !showStop && 'send-btn-disabled']"
          :disabled="!canSend && !showStop"
          :title="showStop ? t('chat.stop') : t('chat.send')"
          @click="onAction"
        >
          <!-- 发送：手绘导航箭头（实心，带底部缺口；圆角线帽/连接让转折更柔和） -->
          <svg
            v-if="!showStop"
            class="send-arrow"
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="currentColor"
            stroke="currentColor"
            stroke-width="2"
            stroke-linejoin="round"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path d="M12 3.5 5 20.2l.6.6L12 17.6l6.4 3.2.6-.6z" />
          </svg>
          <icon-close v-else :size="16" />
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 输入栏容器:宽度/边距由共享的 .chat-column 提供 */
.chat-input-bar {
  flex-shrink: 0;
  box-sizing: border-box;
  padding-bottom: 12px;
}

/* no-padding 模式:铺满外层容器宽度(覆盖 65% 上限);整体为双层对话框面板——
   最外层细边框(0.5px,更轻),圆角 16px。
   灰色底( --bg-2 )承载白色输入胶囊:pill 上圆角缺口透出灰色背景,
   形成白色胶囊叠加在灰色区域上的视觉层次 */
.chat-input-bar.no-padding {
  border: 0.5px solid var(--border-base);
  padding: 0;
  max-width: none;
  border-radius: 16px;
  overflow: hidden;
  background: var(--bg-2);
  /* 外侧阴影:将对话框与页面背景区分开(多层柔投影,明暗主题均适用) */
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.06),
    0 6px 16px rgba(0, 0, 0, 0.1);
}

/* 顶部附加区(文件选择 + Git 分支 / 消息队列):浅灰底,与下方白色输入区形成对比;
   底部细分隔线分隔两层 */
.chat-input-top {
  background: var(--bg-2);
  border-bottom: 1px solid var(--border-light);
}

/* 附件 / 技能 / 网页元素胶囊:同一行横向排列,放不下时换行;
   各子件自身的外边距/内边距归零,间距统一由容器 gap 控制 */
.chat-input-attachments {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
}

.chat-input-attachments :deep(.attach-bar) {
  padding: 0;
}

.chat-input-attachments :deep(.skill-attach),
.chat-input-attachments :deep(.element-attach-wrap) {
  margin: 0;
  max-width: 100%;
}

/* 胶囊外壳:默认带细内边框;聊天页传 no-inner-border 去掉,
   避免与外层 0.5px 边框叠加成双层(新建会话页保留内边框成完整双框)。
   白色底色,与上方浅灰附加条拉开对比 */
.chat-input-pill {
  border: 0.5px solid var(--border-strong);
  background-color: var(--surface-bg);
  border-radius: 16px;
  overflow: hidden;
}

.chat-input-pill.no-inner-border {
  border: none;
}

/* 拖拽图片悬停:高亮提示可放置(虚线圈 + 淡色底) */
.chat-input-pill.drop-active {
  border-color: var(--primary);
  box-shadow: inset 0 0 0 1px var(--primary);
}

/* 变窄:先隐藏推理等级(模型仍显示) */
.chat-input-pill.hide-thinking :deep(.me-trigger-effort) {
  display: none;
}

/* 更窄:隐藏整个「模型+推理等级」控件(含无模型占位) */
.chat-input-pill.hide-model .model-effort,
.chat-input-pill.hide-model :deep(.model-empty-trigger) {
  display: none;
}

/* 编辑区:contenteditable 编辑器(MentionInput)自带内边距,本层仅作占位容器 */
.chat-textarea {
  position: relative;
}

/* 底部工具行:溢出裁剪防极窄时元素互相重叠;发送按钮 flex-shrink:0 始终可见 */
.chat-input-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px 10px;
  overflow: hidden;
}

/* 计划 / 子会话 / 会话信息 三个图标:紧邻排列，无间隔 */
.footer-icons {
  display: flex;
  align-items: center;
  gap: 0;
  flex-shrink: 0;
}

/* 左侧控件簇:窄时优先压缩(裁剪),保证右侧模型/思考/发送不被挤出 */
.footer-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 0 1 auto;
  overflow: hidden;
}

/* 右侧控件(模型+推理等级/发送)不参与压缩——宁可先藏(由 stage 控制),也不挤掉发送按钮 */
.chat-input-footer .model-effort,
.chat-input-footer :deep(.model-empty-trigger) {
  flex-shrink: 0;
}

/* 附件按钮:固定圆形,无外边框,常态透明背景、hover 才显示(与模式/思考下拉一致);
   hover 不再用失效的 var(--primary) 改色(暗色下会变黑) */
.attach-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  flex-shrink: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  padding: 0;
  line-height: 0;
  transition:
    background 0.15s,
    color 0.15s;
}

.attach-btn:hover {
  background: var(--bg-3);
}

/* 权限模式按钮:图标+文字,绿盾=手动授权,橙盾=自动授权;
   与模式/思考下拉一致——常态透明背景,hover 才显示;
   字号 14px 与旁边模式下拉的选项值(Arco size-small 默认)保持一致;
   line-height:1 让文字行盒与 14px 图标等高,图标文字水平居中对齐 */
.perm-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 28px;
  flex-shrink: 0;
  border: none;
  border-radius: 6px;
  padding: 0 8px;
  background: transparent;
  color: var(--success);
  cursor: pointer;
  font-size: var(--font-size-md);
  line-height: 1;
  transition:
    background 0.15s,
    color 0.15s;
}

/* 图标显式块级,配合 flex 双轴居中 */
.perm-btn svg {
  display: block;
}

.perm-btn:hover {
  background: var(--bg-3);
}

.perm-btn-allow {
  color: var(--warning);
}

/* 会话模式触发器:与「手动授权」同款图标+文字按钮 */
.mode-trigger {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  flex-shrink: 0;
  border: none;
  border-radius: 6px;
  padding: 0 8px;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  font-size: var(--font-size-md);
  line-height: 1;
  white-space: nowrap;
  transition: background 0.15s;
}

.mode-trigger svg {
  display: block;
}

.mode-trigger:hover {
  background: var(--bg-3);
}

.mode-chevron {
  margin-left: 2px;
  color: var(--text-tertiary);
  transition: transform 0.15s;
}

.mode-chevron.open {
  transform: rotate(180deg);
}

/* 发送按钮 */
.send-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 50%;
  background: var(--bg-3);
  border: none;
  color: var(--text-primary);
  cursor: pointer;
  outline: none;
  flex-shrink: 0;
  padding: 0;
  line-height: 0;
  transition:
    background 0.15s,
    color 0.15s,
    opacity 0.15s,
    transform 0.15s;
}

.send-btn:hover:not(.send-btn-disabled) {
  transform: scale(1.08);
}

/* 手绘发送箭头：实心导航箭头（带底部缺口），块级以配合 flex 居中 */
.send-arrow {
  display: block;
}

.send-btn:active:not(.send-btn-disabled) {
  background: color-mix(in srgb, var(--primary) 20%, var(--bg-1));
}

.send-btn-disabled {
  opacity: 0.3;
  cursor: not-allowed;
}

/* 流式时的停止按钮 */
.send-btn-stop {
  background: color-mix(in srgb, var(--danger) 15%, var(--bg-2)) !important;
  color: var(--danger) !important;
}

.send-btn-stop:hover {
  background: color-mix(in srgb, var(--danger) 22%, var(--bg-2)) !important;
}

/* 模式/下拉项:图标 + 文字,flex 对齐,间距 4px */
.mode-option {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  white-space: nowrap;
  color: var(--text-primary);
}

/* 无模型配置:禁用态占位(点击无响应) */
.model-empty-trigger {
  display: flex;
  align-items: center;
  height: 28px;
  padding: 0 8px;
  border-radius: 6px;
  flex-shrink: 0;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  user-select: none;
}
</style>

<style>
/* 非 scoped:下拉弹层 teleport 到 body，沿用项目统一菜单配方（同 .chat-more-dropdown） */
.mode-dropdown .arco-dropdown {
  min-width: 160px;
  width: max-content;
  padding: 4px;
  border: 1px solid var(--border-base);
  border-radius: 8px;
  background: var(--bg-2);
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
}

.mode-dropdown .arco-dropdown-list {
  padding: 0;
}

.mode-dropdown .arco-dropdown-option {
  height: var(--menu-item-height);
  line-height: var(--line-height-ui);
  padding: 0 10px;
  border-radius: 6px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

.mode-dropdown .arco-dropdown-option-content {
  width: 100%;
}

.mode-dropdown .arco-dropdown-option:hover,
.mode-dropdown .arco-dropdown-option.arco-dropdown-option-active {
  background: var(--bg-3);
  color: var(--text-primary);
}

.mode-option {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
}

.mode-check {
  margin-left: auto;
  color: var(--accent-text);
}
</style>
