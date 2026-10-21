<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { CronExpressionParser } from 'cron-parser'
import { useSessionsStore } from '@/stores/sessions'
import { useAgentsStore } from '@/stores/agents'
import MultilineTextField from '@/components/common/MultilineTextField.vue'
import { invoke } from '@/services/backend'
import { useScheduledTasksVersion } from '@/services/schedulesSync'
import { pickDirectory } from '@/composables/useDirectoryPicker'
import type { ScheduledTask, ScheduledTaskRequest } from '@/services/dto'
import type { Session } from '@/types'
import { localizedSessionTitle } from '@/utils/sessionTitle'
import {
  MODE_TO_BACKEND,
  MODE_FROM_BACKEND,
  PERMISSION_TO_BACKEND,
  PERMISSION_FROM_BACKEND,
  type ChatMode,
  type ChatPermission
} from '@/stores/sessionSettings'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const sessions = useSessionsStore()
const agentsStore = useAgentsStore()

/** 会话级「执行模式 / 审批模式」选项（值 → i18n key；与聊天输入栏同一套文案） */
const SESSION_MODE_OPTIONS: { value: ChatMode; labelKey: string }[] = [
  { value: 'plan', labelKey: 'sessionInfo.modePlan' },
  { value: 'build', labelKey: 'sessionInfo.modeExecute' },
  { value: 'auto', labelKey: 'sessionInfo.modeAuto' }
]
const PERMISSION_OPTIONS: { value: ChatPermission; labelKey: string }[] = [
  { value: 'ask', labelKey: 'chat.permissionAsk' },
  { value: 'allow', labelKey: 'chat.permissionAllow' }
]
// 模型 / 思考强度不再是任务参数（定时会话一律用设置里的默认模型），
// 故不在此维护模型级联与强度阈值。
/**
 * 智能体下拉项：内置三个（默认主智能体 / General / Explore）在前，自定义卡片在后。
 * 默认选中**内置主智能体**（与后端「未指定 → 主智能体」同一语义，故不再单列「默认」项）。
 */
const MAIN_AGENT_ID = 'builtin-main'
const agentOptions = computed(() => {
  const list = [...agentsStore.agents]
  list.sort((a, b) => (a.agentId === MAIN_AGENT_ID ? -1 : b.agentId === MAIN_AGENT_ID ? 1 : 0))
  return list.map((a) => ({ value: a.agentId, label: a.name }))
})

type FrequencyType = 'daily' | 'weekly' | 'weekday' | 'cron'
type TaskMode = 'new_session' | 'continuous_session'

/**
 * 弹窗尺寸上限:宽 ≤ 75vw、高 ≤ 75vh —— 即最多占窗口 3/4。
 * 高度给**固定值**(而非 max-height)让弹窗体成为一列 flex 容器:编辑器 `flex: 1` 吃掉剩余高度
 * —— 打开即见完整编辑器,不必滚动(窗口过矮时仅栏内滚动)。
 * 105px = 实测的头部(46)+ 底部(59),即弹窗总高正好落在 75vh 内。
 */
const MODAL_WIDTH = 'min(960px, 75vw)'
const MODAL_TOP = '12.5vh'
const BODY_STYLE = { height: 'calc(75vh - 105px)', overflow: 'hidden' } as const

/** 视图任务:后端 ScheduledTask + parseCronToFrequency 推导的频率字段 */
interface Task {
  id: string
  name: string
  desc: string
  /** 运行方式：每次新建会话 / 持续复用同一会话 */
  mode: TaskMode
  instruction: string
  /** 会话目录 */
  workspacePath: string
  cron: string
  sessionId: string
  enabled: boolean
  createdAt: number
  /** 会话参数（前端枚举形态，便于表单双向绑定） */
  sessionMode: ChatMode
  permission: ChatPermission
  agentType: string
  frequencyType: FrequencyType
  hour: number
  minute: number
  weekDays: number[]
  cronExpression: string
}

const searchQuery = ref('')
const loading = ref(false)

const tasks = ref<Task[]>([])

async function loadTasks(): Promise<void> {
  try {
    loading.value = true
    const raw = (await invoke('list_scheduled_tasks')) as ScheduledTask[]
    tasks.value = (raw || []).map((t) => {
      const freq = parseCronToFrequency(t.cronExpr)
      return {
        id: t.id ?? '',
        name: t.name,
        desc: t.description,
        mode: (t.mode || 'new_session') as TaskMode,
        instruction: t.instruction || '',
        workspacePath: t.workspacePath || '',
        cron: t.cronExpr,
        sessionId: t.sessionId || '',
        enabled: t.enabled === 1,
        createdAt: t.createdAt,
        // 会话参数：后端枚举 → 前端枚举（缺省与聊天输入栏一致）
        sessionMode: MODE_FROM_BACKEND[t.sessionMode ?? ''] ?? 'build',
        permission: PERMISSION_FROM_BACKEND[t.permissionMode ?? ''] ?? 'ask',
        agentType: t.agentType || MAIN_AGENT_ID,
        ...freq
      }
    })
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  void loadTasks()
  // 智能体下拉（内置 + 自定义）；失败静默——表单仍可提交，只是选项只有「默认」
  void agentsStore.fetchAgents()
})

// 任务定义在别处被改（含本页操作触发的后端广播）→ 重拉列表：改名 / 启停 / 增删即时可见，
// 不必等下次进入本页。`list_scheduled_tasks` 是本地读，重拉无压力。
watch(useScheduledTasksVersion(), () => void loadTasks())

const WEEKDAY_NAMES = ['周日', '周一', '周二', '周三', '周四', '周五', '周六']
const WEEKDAY_OPTIONS: { label: string; value: number }[] = [
  { label: '周一', value: 1 },
  { label: '周二', value: 2 },
  { label: '周三', value: 3 },
  { label: '周四', value: 4 },
  { label: '周五', value: 5 },
  { label: '周六', value: 6 },
  { label: '周日', value: 0 }
]

function sortWeekDays(days: number[]): number[] {
  return [...days].sort((a, b) => {
    if (a === 0) return 1
    if (b === 0) return -1
    return a - b
  })
}

/** cron 5 段 → 表单友好频率(与源码一致) */
function parseCronToFrequency(cronExpr: string): {
  frequencyType: FrequencyType
  hour: number
  minute: number
  weekDays: number[]
  cronExpression: string
} {
  const def = { frequencyType: 'daily' as FrequencyType, hour: 9, minute: 0, weekDays: [] as number[], cronExpression: '' }
  if (!cronExpr) return def
  const parts = cronExpr.trim().split(/\s+/)
  if (parts.length !== 5) return { ...def, frequencyType: 'cron', cronExpression: cronExpr }
  const [minute, hour, dom, month, dow] = parts
  const hh = Number.isNaN(parseInt(hour)) ? 9 : parseInt(hour)
  const mm = Number.isNaN(parseInt(minute)) ? 0 : parseInt(minute)
  if (dow === '*' && dom === '*' && month === '*') return { ...def, frequencyType: 'daily', hour: hh, minute: mm }
  if (dow === '1-5' && dom === '*' && month === '*') return { ...def, frequencyType: 'weekday', hour: hh, minute: mm }
  if (dom === '*' && month === '*') {
    const days = dow.split(',').map(Number).filter((n) => !Number.isNaN(n))
    if (days.length) return { ...def, frequencyType: 'weekly', hour: hh, minute: mm, weekDays: days }
  }
  return { ...def, frequencyType: 'cron', cronExpression: cronExpr }
}

function generateCron(f: FormState): string {
  switch (f.frequencyType) {
    case 'daily':
      return `${f.minute} ${f.hour} * * *`
    case 'weekly':
      return `${f.minute} ${f.hour} * * ${sortWeekDays(f.weekDays).join(',')}`
    case 'weekday':
      return `${f.minute} ${f.hour} * * 1-5`
    case 'cron':
      return f.cronExpression
    default:
      return ''
  }
}

function computeNextRun(cron: string): Date | null {
  try {
    const interval = CronExpressionParser.parse(cron)
    return interval.next().toDate()
  } catch {
    return null
  }
}

function formatDate(d: Date | null): string {
  if (!d) return ''
  const yy = d.getFullYear()
  const mm = String(d.getMonth() + 1).padStart(2, '0')
  const dd = String(d.getDate()).padStart(2, '0')
  const hh = String(d.getHours()).padStart(2, '0')
  const mi = String(d.getMinutes()).padStart(2, '0')
  return `${yy}-${mm}-${dd} ${hh}:${mi}`
}

function getHumanFrequency(task: Task): string {
  const time = `${String(task.hour).padStart(2, '0')}:${String(task.minute).padStart(2, '0')}`
  switch (task.frequencyType) {
    case 'daily':
      return `每天 ${time}`
    case 'weekly': {
      const days = sortWeekDays(task.weekDays)
        .map((d) => WEEKDAY_NAMES[d])
        .join('、')
      return `每周 ${days} ${time}`
    }
    case 'weekday':
      return `工作日 ${time}`
    case 'cron':
      return task.cron || '--'
    default:
      return task.cron || '--'
  }
}

/**
 * 看板列表 = 搜索过滤 + 排序。
 * 排序:启用的在前、禁用的在后;组内按「下次执行时间」升序(马上要跑的最靠前),
 * 算不出下次执行的(非法 cron)与同名项按名称兜底,保证顺序稳定。
 * 启用/禁用切换、创建或编辑后列表会重排 —— 这是预期:一眼看到谁先跑、谁被停了。
 */
const filteredTasks = computed(() => {
  const q = searchQuery.value.trim().toLowerCase()
  const list = q
    ? tasks.value.filter((task) => task.name.toLowerCase().includes(q) || task.desc.toLowerCase().includes(q))
    : [...tasks.value]
  // 先算一遍下次执行时间:比较函数会被调用 O(n log n) 次,cron 解析不该放进去
  const nextAt = new Map(list.map((t) => [t.id, computeNextRun(t.cron)?.getTime() ?? Number.MAX_SAFE_INTEGER]))
  return list.sort((a, b) => {
    if (a.enabled !== b.enabled) return a.enabled ? -1 : 1
    const na = nextAt.get(a.id) ?? Number.MAX_SAFE_INTEGER
    const nb = nextAt.get(b.id) ?? Number.MAX_SAFE_INTEGER
    return na === nb ? a.name.localeCompare(b.name) : na - nb
  })
})

/** 详情页任务 id（路由参数 `/scheduled/:taskId`；空 = 列表态） */
const detailTaskId = computed(() => (typeof route.params.taskId === 'string' ? route.params.taskId : ''))
const detailTask = computed(() => tasks.value.find((x) => x.id === detailTaskId.value) ?? null)

/** 定时任务会话 → 打开对话：会话可能在 store 里以本地 id 或后端 id 存在，两种都认 */
function openTaskSession(sessionId: string): void {
  if (!sessionId) return
  const s = sessions.sessions.find((x) => x.backendId === sessionId || x.id === sessionId)
  if (!s) {
    Message.warning(t('scheduled.sessionMissing'))
    return
  }
  sessions.setActive(s.id)
  void router.push(`/chat/${s.id}`)
}

/**
 * 卡片点击：
 * - 持续复用 → 直接打开那条会话（没有则提示尚未执行）；
 * - 每次新建 → 进入该任务的**详情页**（列出每次触发的会话卡片）。
 *
 * 为什么不再就地展开：展开会把卡片撑满整行、挤动网格里其它卡片（破坏列表观感），
 * 详情页则在原布局之外呈现同一份执行记录，返回后网格原样恢复。
 * 卡片内的按钮/开关一律 `@click.stop`，不会走到这里。
 */
function onCardClick(task: Task): void {
  if (task.mode === 'continuous_session') {
    if (task.sessionId) openTaskSession(task.sessionId)
    else Message.info(t('scheduled.notRunYet'))
    return
  }
  void router.push(`/scheduled/${task.id}`)
}

/** 返回列表（保留滚动位置由 KeepAlive 保证） */
function closeDetail(): void {
  void router.push('/scheduled')
}

/** 该任务产生的会话（按 scheduledTaskId 归集，最新在前） */
function taskSessions(task: Task): Session[] {
  return sessions.sessionsOfTask(task.id)
}

interface DayGroup {
  key: string
  label: string
  items: Session[]
}

/** 按自然日分组（今天 / 昨天 / YYYY-MM-DD），组内保持传入顺序（时间倒序） */
function groupByDay(list: Session[]): DayGroup[] {
  const groups: DayGroup[] = []
  for (const s of list) {
    const d = new Date(s.createdAt)
    const key = `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`
    let g = groups.find((x) => x.key === key)
    if (!g) {
      g = { key, label: dayLabel(d), items: [] }
      groups.push(g)
    }
    g.items.push(s)
  }
  return groups
}

function dayLabel(d: Date): string {
  const today = new Date()
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime()
  const ts = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
  if (ts === startOfToday) return t('scheduled.history.today')
  if (ts === startOfToday - 86400000) return t('scheduled.history.yesterday')
  const mm = String(d.getMonth() + 1).padStart(2, '0')
  const dd = String(d.getDate()).padStart(2, '0')
  return `${d.getFullYear()}-${mm}-${dd}`
}

/** 会话触发时刻（本地 HH:mm） */
function timeOf(ts: number): string {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** 会话运行状态（后端唯一真源；缺省按空闲处理） */
function statusOf(s: Session): string {
  return sessions.sessionInfo[s.id]?.status ?? 'idle'
}

/** 后端状态（snake_case）→ i18n key（session.status.*；两处多词状态是驼峰） */
const STATUS_I18N_KEY: Record<string, string> = {
  running: 'running',
  awaiting_approval: 'awaitingApproval',
  awaiting_form: 'awaitingForm',
  done: 'done',
  error: 'error',
  cancelled: 'cancelled',
  interrupted: 'interrupted',
  ignored: 'ignored',
  idle: 'idle'
}

/** 状态文案：复用会话状态文案（session.status.*），未知状态不显示 */
function statusLabel(s: Session): string {
  const key = STATUS_I18N_KEY[statusOf(s)]
  return key ? t(`session.status.${key}`) : ''
}

/** 会话轮次（0 轮不显示） */
function roundOf(s: Session): number {
  return sessions.sessionInfo[s.id]?.currentRound ?? 0
}

/** 详情页参数摘要：执行模式 / 审批 / 智能体 / 模型 / 思考强度 */
function modeLabel(task: Task): string {
  return task.sessionMode === 'plan'
    ? t('sessionInfo.modePlan')
    : task.sessionMode === 'auto'
      ? t('sessionInfo.modeAuto')
      : t('sessionInfo.modeExecute')
}

function permissionText(task: Task): string {
  return t(task.permission === 'ask' ? 'chat.permissionAsk' : 'chat.permissionAllow')
}

function agentText(task: Task): string {
  return agentsStore.agents.find((a) => a.agentId === task.agentType)?.name ?? task.agentType
}

async function runTaskNow(task: Task): Promise<void> {
  try {
    await invoke('run_scheduled_task_now', { id: task.id })
    Message.success(`"${task.name}" ${t('scheduled.runNow')}`)
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

const createVisible = ref(false)
const editingId = ref<string | null>(null)
const preGeneratedId = ref<string | null>(null)

interface FormState {
  name: string
  desc: string
  /** 运行方式 */
  mode: TaskMode
  instruction: string
  /** 会话目录 */
  workspacePath: string
  /** 会话参数 */
  sessionMode: ChatMode
  permission: ChatPermission
  agentType: string
  frequencyType: FrequencyType
  hour: number
  minute: number
  weekDays: number[]
  cronExpression: string
}

const form = ref<FormState>(createDefaultForm())

function createDefaultForm(): FormState {
  return {
    name: '',
    desc: '',
    mode: 'new_session',
    instruction: '',
    workspacePath: '',
    // 会话参数缺省：执行 / 手动授权 / 默认主智能体（模型与思考强度用设置里的默认值）
    sessionMode: 'build',
    permission: 'ask',
    agentType: MAIN_AGENT_ID,
    frequencyType: 'daily',
    hour: 9,
    minute: 0,
    weekDays: [],
    cronExpression: ''
  }
}

/** 表单当前频率对应的 cron:每周未勾选任何一天时视为「尚无有效表达式」（否则会算出误导性的下次执行） */
const formCron = computed(() => {
  const f = form.value
  if (f.frequencyType === 'weekly' && f.weekDays.length === 0) return ''
  return f.frequencyType === 'cron' ? f.cronExpression : generateCron(f)
})
const formNextRun = computed(() => computeNextRun(formCron.value))
const formCronValid = computed(() => {
  const f = form.value
  if (!f.frequencyType) return false
  if (f.frequencyType === 'daily' || f.frequencyType === 'weekday') return true
  if (f.frequencyType === 'weekly') return f.weekDays.length > 0
  if (f.frequencyType === 'cron') {
    try {
      CronExpressionParser.parse(f.cronExpression)
      return true
    } catch {
      return false
    }
  }
  return false
})

/** 可提交:任务名非空 + cron 合法（与智能体弹窗 canSave 同款约束） */
const canSubmit = computed(() => form.value.name.trim().length > 0 && formCronValid.value)

async function openCreate(): Promise<void> {
  editingId.value = null
  preGeneratedId.value = null
  form.value = createDefaultForm()
  try {
    const { id, path } = (await invoke('generate_scheduler_workspace')) as { id: string; path: string }
    preGeneratedId.value = id
    form.value.workspacePath = path
  } catch (e) {
    console.error('Failed to generate scheduler workspace:', e)
  }
  createVisible.value = true
}

function openEdit(task: Task): void {
  editingId.value = task.id
  form.value = {
    name: task.name,
    desc: task.desc,
    mode: task.mode,
    instruction: task.instruction,
    workspacePath: task.workspacePath || '',
    sessionMode: task.sessionMode ?? 'build',
    permission: task.permission ?? 'ask',

    agentType: task.agentType || MAIN_AGENT_ID,

    frequencyType: task.frequencyType || 'daily',
    hour: task.hour ?? 9,
    minute: task.minute ?? 0,
    weekDays: task.weekDays || [],
    cronExpression: task.cronExpression || ''
  }
  createVisible.value = true
}

/**
 * 关闭:本页对 a-modal 是全受控用法(`:visible` 恒为布尔),关闭必须由这里推回本地 ref。
 * 三条路径收敛到此:点 X / ESC / 点遮罩(@cancel)、离场动画结束(@close)、底部按钮(直调)。
 */
function handleCancel(): void {
  createVisible.value = false
}

async function confirmSubmit(): Promise<void> {
  if (!canSubmit.value) return
  const payload: ScheduledTaskRequest = {
    id: (editingId.value || preGeneratedId.value || undefined) as string | undefined,
    name: form.value.name.trim(),
    description: form.value.desc,
    cronExpr: formCron.value,
    mode: form.value.mode,
    instruction: form.value.instruction,
    workspacePath: form.value.workspacePath,
    // 会话参数：前端枚举 → 后端枚举；模型/供应商/智能体未选则不下发（后端按缺省处理）
    sessionMode: MODE_TO_BACKEND[form.value.sessionMode],
    permissionMode: PERMISSION_TO_BACKEND[form.value.permission],

    agentType: form.value.agentType || MAIN_AGENT_ID,
  }
  try {
    if (editingId.value) {
      await invoke('update_scheduled_task', { ...payload })
    } else {
      await invoke('add_scheduled_task', { ...payload })
      // 后端新建任务一律**停用**起步：明确告知，否则卡片是灰的容易被当成没保存成功
      Message.info(t('scheduled.createdDisabled'))
    }
    createVisible.value = false
    await loadTasks()
  } catch (e) {
    // 后端会在保存前校验 cron（段数 / 取值），非法表达式在此给出确切原因
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

async function deleteTask(task: Task): Promise<void> {
  try {
    await invoke('delete_scheduled_task', { id: task.id })
    await loadTasks()
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

async function toggleEnabled(task: Task): Promise<void> {
  try {
    await invoke('toggle_scheduled_task', { id: task.id, enabled: !task.enabled })
    await loadTasks()
  } catch (e) {
    // 注册失败（cron 非法等）会走到这里，提示后由 loadTasks 把开关状态拉回后端真值
    Message.error(e instanceof Error ? e.message : String(e))
    await loadTasks()
  }
}

async function selectWorkspace(): Promise<void> {
  try {
    // 桌面=原生目录对话框；浏览器部署=应用内模态（服务端目录浏览）
    const dir = await pickDirectory()
    if (dir) form.value.workspacePath = dir
  } catch (e) {
    console.error('Failed to select workspace:', e)
  }
}
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div class="header-left">
        <h2 class="page-title">{{ t('scheduled.title') }}</h2>
        <span v-if="tasks.length" class="page-count">{{ tasks.length }}</span>
      </div>
      <div class="header-actions">
        <a-input
          v-model="searchQuery"
          class="board-search"
          :placeholder="t('scheduled.searchPlaceholder')"
          allow-clear
          size="small"
          style="width: 220px"
        >
          <template #prefix><icon-search :size="14" /></template>
        </a-input>
        <!-- 刷新:重新拉取任务列表(触发次数/启停状态等可能被后台或其它窗口改动) -->
        <a-tooltip :content="t('common.refresh')">
          <a-button
            type="text"
            size="small"
            class="scheduled-tool-btn board-refresh"
            :loading="loading"
            @click="loadTasks"
          >
            <template #icon><icon-refresh :size="15" /></template>
          </a-button>
        </a-tooltip>
        <a-button class="scheduled-default-btn" type="primary" size="small" @click="openCreate">
          <template #icon><icon-plus :size="13" /></template>
          {{ t('scheduled.create') }}
        </a-button>
      </div>
    </div>
    <p class="page-desc">{{ t('scheduled.description') }}</p>

    <div v-if="filteredTasks.length" v-show="!detailTask" class="card-grid">
      <div
        v-for="task in filteredTasks"
        :key="task.id"
        class="task-card"
        :class="{ disabled: !task.enabled, clickable: true }"
        @click="onCardClick(task)"
      >
        <!-- 第一行:标题 + 启用开关(开关不触发展开/打开会话) -->
        <div class="card-top">
          <span class="card-name">{{ task.name }}</span>
          <a-switch :model-value="task.enabled" size="small" @click.stop @change="toggleEnabled(task)" />
        </div>
        <!-- 第二行:运行方式 / 执行模式 / 审批模式三个标签 -->
        <div class="card-tags">
          <a-tag size="small" class="info-tag">
            {{ task.mode === 'new_session' ? t('scheduled.mode.newSession') : t('scheduled.mode.continuousSession') }}
          </a-tag>
          <a-tag size="small" class="info-tag">{{ modeLabel(task) }}</a-tag>
          <a-tag size="small" class="info-tag">{{ permissionText(task) }}</a-tag>
        </div>
        <p class="card-desc">{{ task.desc || t('scheduled.noDescription') }}</p>
        <div class="card-meta">
          <span class="card-freq">{{ getHumanFrequency(task) }}</span>
          <span class="meta-sep">·</span>
          <span v-if="task.enabled" class="card-next">
            {{ t('scheduled.card.nextRun', { time: formatDate(computeNextRun(task.cron)) }) }}
          </span>
          <span v-else class="card-next paused">{{ t('scheduled.card.noNextRun') }}</span>
        </div>
        <div class="card-footer">
          <a-button type="text" size="mini" @click.stop="runTaskNow(task)">
            <template #icon><icon-play :size="13" /></template>
            {{ t('scheduled.runNow') }}
          </a-button>
          <a-button type="text" size="mini" @click.stop="openEdit(task)">
            <template #icon><icon-edit :size="13" /></template>
            {{ t('scheduled.edit') }}
          </a-button>
          <a-popconfirm :content="t('scheduled.deleteWarning', { name: task.name })" @ok="deleteTask(task)">
            <a-button type="text" size="mini" status="danger" @click.stop>
              <template #icon><icon-delete :size="13" /></template>
              {{ t('scheduled.delete') }}
            </a-button>
          </a-popconfirm>
        </div>

      </div>
    </div>

    <!-- 空态（详情态下不显示） -->
    <div v-else-if="!detailTask" class="empty-state">
      <component :is="tasks.length ? 'icon-search' : 'icon-schedule'" :size="36" class="empty-icon" />
      <p class="empty-title">
        {{ tasks.length ? t('scheduled.empty.noMatch') : t('scheduled.empty.noTasks') }}
      </p>
      <!-- 空态不放「新建任务」按钮:入口统一在页头右上角（避免同一动作两个入口） -->
      <p class="empty-desc">
        {{ tasks.length ? t('scheduled.empty.noMatchDesc') : t('scheduled.empty.createHint') }}
      </p>
    </div>

    <!-- 任务详情：该任务每次触发的会话卡片（点卡片进入；返回后列表布局原样） -->
    <div v-if="detailTask" class="detail">
      <div class="detail-toolbar">
        <button type="button" class="back-btn scheduled-tool-btn" @click="closeDetail">
          <icon-left :size="15" />
          {{ t('scheduled.detail.back') }}
        </button>
        <div class="detail-toolbar-actions">
          <a-button class="scheduled-default-btn" size="small" @click="runTaskNow(detailTask)">
            <template #icon><icon-play :size="13" /></template>
            {{ t('scheduled.runNow') }}
          </a-button>
          <a-button class="scheduled-plain-btn" size="small" @click="openEdit(detailTask)">
            <template #icon><icon-edit :size="13" /></template>
            {{ t('scheduled.edit') }}
          </a-button>
          <a-popconfirm :content="t('scheduled.deleteWarning', { name: detailTask.name })" @ok="deleteTask(detailTask)">
            <a-button class="scheduled-danger-btn" size="small">
              <template #icon><icon-delete :size="13" /></template>
              {{ t('scheduled.delete') }}
            </a-button>
          </a-popconfirm>
        </div>
      </div>

      <div class="detail-head">
        <!-- 名称后一排三个标签:运行方式 / 执行模式 / 审批模式(任务级 + 会话级设置一眼看全) -->
        <div class="detail-title-row">
          <h3 class="detail-name">{{ detailTask.name }}</h3>
          <a-tag size="small" class="info-tag">
            {{ detailTask.mode === 'new_session' ? t('scheduled.mode.newSession') : t('scheduled.mode.continuousSession') }}
          </a-tag>
          <a-tag size="small" class="info-tag">{{ modeLabel(detailTask) }}</a-tag>
          <a-tag size="small" class="info-tag">{{ permissionText(detailTask) }}</a-tag>
        </div>
        <p class="detail-desc">{{ detailTask.desc || t('scheduled.noDescription') }}</p>
        <div class="card-meta">
          <span class="card-freq">{{ getHumanFrequency(detailTask) }}</span>
          <span class="meta-sep">·</span>
          <span v-if="detailTask.enabled" class="card-next">
            {{ t('scheduled.card.nextRun', { time: formatDate(computeNextRun(detailTask.cron)) }) }}
          </span>
          <span v-else class="card-next paused">{{ t('scheduled.card.noNextRun') }}</span>
        </div>
        <div class="detail-params">
          <div class="param">
            <span class="param-label">{{ t('scheduled.session.agent') }}</span>
            <span class="param-value">{{ agentText(detailTask) }}</span>
          </div>
          <div class="param" :title="detailTask.workspacePath">
            <span class="param-label">{{ t('scheduled.form.workspace') }}</span>
            <span class="param-value param-path">{{ detailTask.workspacePath || '--' }}</span>
          </div>
        </div>
      </div>

      <div class="detail-history">
        <div class="history-header">
          <span class="history-title">{{ t('scheduled.history.title') }}</span>
          <span class="history-count">{{ taskSessions(detailTask).length }}</span>
          <a-button
            v-if="detailTask.sessionId"
            type="text"
            size="small"
            class="history-open scheduled-tool-btn"
            @click="openTaskSession(detailTask.sessionId)"
          >
            <template #icon><icon-link :size="13" /></template>
            {{ t('scheduled.detail.openLatest') }}
          </a-button>
        </div>
        <template v-if="taskSessions(detailTask).length">
          <div v-for="group in groupByDay(taskSessions(detailTask))" :key="group.key" class="history-day">
            <div class="history-day-label">{{ group.label }}</div>
            <div class="history-cards">
              <button
                v-for="s in group.items"
                :key="s.id"
                type="button"
                class="history-card"
                @click="openTaskSession(s.id)"
              >
                <span class="history-time">{{ timeOf(s.createdAt) }}</span>
                <span class="history-name">{{ localizedSessionTitle(s) }}</span>
                <span class="history-meta">
                  <span class="status-dot" :class="`status-${statusOf(s)}`" />
                  <span>{{ statusLabel(s) }}</span>
                  <template v-if="roundOf(s) > 0">
                    <span class="meta-sep">·</span>
                    <span>{{ t('scheduled.history.round', { n: roundOf(s) }) }}</span>
                  </template>
                </span>
              </button>
            </div>
          </div>
        </template>
        <p v-else class="history-empty">
          <icon-schedule :size="15" />
          {{ t('scheduled.history.empty') }}
        </p>
      </div>
    </div>

    <a-modal
      :visible="createVisible"
      :title="editingId ? t('scheduled.edit') : t('scheduled.create')"
      title-align="start"
      :width="MODAL_WIDTH"
      :top="MODAL_TOP"
      :body-style="BODY_STYLE"
      modal-class="scheduled-modal"
      unmount-on-close
      @cancel="handleCancel"
      @close="handleCancel"
    >
      <div class="form">
        <!-- 左栏:任务字段(窄窗口下不够宽时回落单栏,见 .form 的媒体查询) -->
        <div class="form-col form-col-fields">
          <div class="form-row field-name">
            <label class="form-label">
              {{ t('scheduled.form.name') }} <span class="required">*</span>
            </label>
            <a-input
              v-model="form.name"
              :max-length="64"
              allow-clear
              :placeholder="t('scheduled.form.namePlaceholder')"
            />
          </div>

          <div class="form-row field-desc">
            <label class="form-label">{{ t('scheduled.form.desc') }}</label>
            <a-textarea
              v-model="form.desc"
              :max-length="1024"
              allow-clear
              :placeholder="t('scheduled.form.descPlaceholder')"
              :auto-size="{ minRows: 2, maxRows: 4 }"
            />
          </div>

          <div class="form-row">
            <label class="form-label">{{ t('scheduled.mode.label') }}</label>
            <a-radio-group v-model="form.mode" type="button" size="small">
              <a-radio value="new_session">{{ t('scheduled.mode.newSession') }}</a-radio>
              <a-radio value="continuous_session">{{ t('scheduled.mode.continuousSession') }}</a-radio>
            </a-radio-group>
            <span class="field-hint">{{ t('scheduled.mode.hint') }}</span>
          </div>

          <!-- ↓↓↓ 创建会话的参数（触发时按这些参数新建/复用会话） ↓↓↓ -->
          <div class="form-row-inline">
            <div class="form-row">
              <label class="form-label">{{ t('scheduled.session.mode') }}</label>
              <a-radio-group v-model="form.sessionMode" type="button" size="small">
                <a-radio v-for="opt in SESSION_MODE_OPTIONS" :key="opt.value" :value="opt.value">
                  {{ t(opt.labelKey) }}
                </a-radio>
              </a-radio-group>
            </div>
            <div class="form-row">
              <label class="form-label">{{ t('scheduled.session.permission') }}</label>
              <a-radio-group v-model="form.permission" type="button" size="small">
                <a-radio v-for="opt in PERMISSION_OPTIONS" :key="opt.value" :value="opt.value">
                  {{ t(opt.labelKey) }}
                </a-radio>
              </a-radio-group>
            </div>
          </div>

          <div class="form-row">
            <label class="form-label">{{ t('scheduled.session.agent') }}</label>
            <a-select v-model="form.agentType" size="small" allow-search>
              <a-option v-for="opt in agentOptions" :key="opt.value" :value="opt.value">
                {{ opt.label }}
              </a-option>
            </a-select>
            <span class="field-hint">{{ t('scheduled.session.agentHint') }}</span>
          </div>

          <div class="form-row">
            <label class="form-label">{{ t('scheduled.form.workspace') }}</label>
            <!-- 只读路径在窄栏里会被截断 → 整行加 title,悬浮可看全路径 -->
            <div class="workspace-row" :title="form.workspacePath">
              <a-input
                v-model="form.workspacePath"
                :placeholder="t('scheduled.form.workspaceAutoHint')"
                readonly
              />
              <a-button size="small" @click="selectWorkspace">
                {{ form.workspacePath ? t('scheduled.form.workspaceChangeDir') : t('scheduled.form.workspaceSelectDir') }}
              </a-button>
            </div>
          </div>

          <!-- 脚注:模型 / 思考强度不再是任务参数(避免用户找不到模型选项) -->
          <div class="form-note">
            <icon-info :size="13" />
            <span>{{ t('scheduled.session.modelFromSettings') }}</span>
          </div>
        </div>

        <!-- 右栏:调度规则 + 执行指令;编辑器吃满本栏剩余高度 → 打开即见完整编辑区 -->
        <div class="form-col form-col-right">
          <div class="form-row">
            <label class="form-label">{{ t('scheduled.form.frequency') }}</label>
            <div class="frequency-panel">
              <a-radio-group v-model="form.frequencyType" type="button" size="small">
                <a-radio value="daily">{{ t('scheduled.form.daily') }}</a-radio>
                <a-radio value="weekly">{{ t('scheduled.form.weekly') }}</a-radio>
                <a-radio value="weekday">{{ t('scheduled.form.weekday') }}</a-radio>
                <a-radio value="cron">Cron</a-radio>
              </a-radio-group>

              <div v-if="form.frequencyType === 'daily' || form.frequencyType === 'weekday'" class="freq-config">
                <div class="time-picker">
                  <a-select v-model="form.hour" style="width: 72px" size="small">
                    <a-option v-for="h in 24" :key="h - 1" :value="h - 1">{{ String(h - 1).padStart(2, '0') }}</a-option>
                  </a-select>
                  <span class="time-sep">:</span>
                  <a-select v-model="form.minute" style="width: 72px" size="small">
                    <a-option v-for="m in 60" :key="m - 1" :value="m - 1">{{ String(m - 1).padStart(2, '0') }}</a-option>
                  </a-select>
                </div>
              </div>

              <div v-if="form.frequencyType === 'weekly'" class="freq-config">
                <div class="weekly-row">
                  <a-checkbox-group v-model="form.weekDays">
                    <a-checkbox v-for="opt in WEEKDAY_OPTIONS" :key="opt.value" :value="opt.value">
                      {{ opt.label }}
                    </a-checkbox>
                  </a-checkbox-group>
                  <div class="time-picker">
                    <a-select v-model="form.hour" style="width: 72px" size="small">
                      <a-option v-for="h in 24" :key="h - 1" :value="h - 1">{{ String(h - 1).padStart(2, '0') }}</a-option>
                    </a-select>
                    <span class="time-sep">:</span>
                    <a-select v-model="form.minute" style="width: 72px" size="small">
                      <a-option v-for="m in 60" :key="m - 1" :value="m - 1">{{ String(m - 1).padStart(2, '0') }}</a-option>
                    </a-select>
                  </div>
                </div>
              </div>

              <div v-if="form.frequencyType === 'cron'" class="freq-config">
                <div class="cron-row">
                  <a-input
                    v-model="form.cronExpression"
                    :placeholder="t('scheduled.form.cronPlaceholder')"
                    size="small"
                    style="width: 200px"
                  />
                  <span class="form-hint">{{ t('scheduled.form.cronHint') }}</span>
                </div>
              </div>

              <div v-if="form.frequencyType" class="next-run-preview">
                <span v-if="formCron && formNextRun">
                  {{ t('scheduled.form.nextRun', { time: formatDate(formNextRun) }) }}
                </span>
                <span v-else class="error-text">
                  {{ t('scheduled.form.computeError') }}
                </span>
              </div>
            </div>
          </div>

          <!-- 执行指令:多行输入框吃掉本栏剩余高度(内容仍是 Markdown 文本) -->
          <div class="form-row form-row-editor">
            <label class="form-label">{{ t('scheduled.form.instruction') }}</label>
            <MultilineTextField
              v-model="form.instruction"
              class="instruction-editor"
              fill
              :min-height="200"
              :placeholder="t('scheduled.form.instructionPlaceholder')"
            />
          </div>
        </div>
      </div>

      <!-- 底部操作:固定弹窗底部(macOS 风格,与智能体 / 技能弹窗一致) -->
      <template #footer>
        <div class="modal-footer">
          <a-button @click="handleCancel">{{ t('common.cancel') }}</a-button>
          <a-button type="primary" :disabled="!canSubmit" @click="confirmSubmit">
            {{ editingId ? t('common.save') : t('scheduled.create') }}
          </a-button>
        </div>
      </template>
    </a-modal>
  </div>
</template>

<style scoped>
.page {
  height: 100%;
  /* 路由视图直接子项:撑满剩余宽度(右侧页签工具栏除外) */
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  padding: 28px 56px 16px;
  overflow: hidden;
}

.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
  flex-shrink: 0;
  flex-wrap: wrap;
  gap: 12px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 10px;
}

.page-title {
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}

.page-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 20px;
  height: 20px;
  box-sizing: border-box;
  font-size: var(--font-size-2xs);
  font-weight: 500;
  line-height: 1;
  color: var(--text-tertiary);
  background: var(--bg-3);
  border-radius: 50%;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

/* ---------- 看板工具栏:macOS 观感 ----------
   搜索框 = 填充胶囊(默认无描边,聚焦才出现主色光环),放大镜在框内左侧;
   新建 = 主色默认按钮(与设置页保存 / 聊天提交同一套配方:实心主色 + 极轻投影)。
   搜索改用 a-input + 前置图标而非 a-input-search:macOS 搜索框没有尾巴按钮,
   输入即筛选;顺带避开全局 `.arco-btn` 的 !important 覆盖与尾巴按钮的拼接圆角。 */
.header-actions :deep(.arco-input-wrapper) {
  height: 28px;
  padding: 0 10px;
  border: 0.5px solid transparent;
  border-radius: 999px;
  background-color: color-mix(in srgb, var(--bg-2) 92%, transparent);
  transition:
    background-color var(--transition-fast),
    border-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

.header-actions :deep(.arco-input-wrapper:hover) {
  background-color: var(--bg-hover);
}

/* 聚焦态用 :focus-within + .arco-input-focus(Arco 的两套聚焦钩子;后者只在其
   内部状态机加,名称与 modal 里的 -focus 不同),否则 Arco 自带的主色描边会漏出来 */
.header-actions :deep(.arco-input-wrapper:focus-within),
.header-actions :deep(.arco-input-wrapper.arco-input-focus) {
  background-color: var(--bg-base);
  border-color: var(--accent-text);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-text) 20%, transparent);
}

.header-actions :deep(.arco-input-prefix) {
  margin-right: 6px;
  color: var(--text-tertiary);
}

.header-actions :deep(.arco-input) {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}

.header-actions :deep(.arco-input::placeholder) {
  color: var(--text-tertiary);
}

.header-actions :deep(.arco-input-clear-btn) {
  color: var(--text-tertiary);
}

/* 「新建任务」用非 scoped 的 .scheduled-default-btn(与详情页「立即执行」同一套材质与尺寸) */

/* 刷新:icon-only 工具按钮 —— 与文字工具按钮同高(28),方形热区、无文字内边距 */
.header-actions :deep(.board-refresh.arco-btn) {
  width: 28px;
  padding: 0;
  color: var(--text-secondary);
}

.header-actions :deep(.board-refresh.arco-btn:hover) {
  color: var(--text-primary);
}

.page-desc {
  margin: 0 0 18px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  line-height: 1.6;
  flex-shrink: 0;
}

.card-grid {
  flex: 1;
  display: grid;
  /* auto-fit:空轨道折叠,最后一行卡片拉伸铺满,避免右侧留白 */
  --card-w: 280px;
  grid-template-columns: repeat(auto-fit, minmax(var(--card-w), 1fr));
  gap: 12px;
  align-content: start;
  overflow-y: auto;
  /* 预占滚动条位置,内容增删时视图不晃动 */
  scrollbar-gutter: stable;
  padding: 0 0 16px;
}

/* 空态:图标 + 标题 + 说明(+ 新建按钮),与侧栏 .side-empty 同一套观感 */
.empty-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 12px 24px 48px;
  text-align: center;
  color: var(--text-tertiary);
}

.empty-icon {
  color: var(--text-tertiary);
  opacity: 0.5;
  margin-bottom: 2px;
}

.empty-title {
  margin: 0;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.empty-desc {
  margin: 0 0 8px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  line-height: 1.5;
}

/* macOS 卡片:白底 + 细边框 + 大圆角(与智能体卡片同款) */
.task-card {
  /* 宽度上限 = 2 × 基准宽:卡片少(筛选后 1~2 张)时不再被 auto-fit 拉满整行,
     最多长到 2 倍宽,多余空间留在行尾(整行铺满时单卡 ≈ 基准宽,故 2 倍基准 ≈ 2 倍原先宽度)。
     注意:不能把上限写进 grid-template-columns 的 minmax(max) —— auto-fit 的轨道数
     按“确定最大值”算,会退化成一列;改成限制卡片自身宽度,轨道照旧折叠。
     不设 justify-self:start(那会让卡片收缩成内容宽),靠 stretch 被 max-width 截断,
     截断后自动左对齐。 */
  max-width: calc(var(--card-w) * 2);
  background: var(--bg-base);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-lg);
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  transition:
    border-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

.task-card:hover {
  border-color: var(--accent-text);
  box-shadow: var(--shadow-sm);
}

.task-card.disabled {
  opacity: 0.6;
}

/* 整卡可点：持续复用=打开对话，每次新建=展开执行记录 */
.task-card.clickable {
  cursor: pointer;
}

/* ---------- 任务详情页（点卡片进入；列表布局不受影响） ---------- */
.detail {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 16px;
  overflow-y: auto;
  scrollbar-gutter: stable;
  padding-bottom: 16px;
}

.detail-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
}

/* 返回按钮:材质走 .scheduled-tool-btn(透明 + 悬浮浮现),这里只管与页面左缘对齐
   (负左边距抵消按钮内边距,让图标与下方卡片左缘齐平) */
.back-btn {
  margin-left: -10px;
}

.detail-toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.detail-head {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 16px;
  /* macOS 材质卡:发丝边 + 极轻投影(与聊天区错误条 / 提示卡同一套观感) */
  background: var(--bg-base);
  border: 0.5px solid color-mix(in srgb, var(--border-strong) 55%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.06);
  flex-shrink: 0;
}

.detail-title-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  min-width: 0;
}

.detail-name {
  margin: 0;
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.detail-desc {
  margin: 0;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.5;
}

/* 参数摘要:标签在上、值在下的「系统设置」式信息网格(macOS 信息区惯例)。
   固定两列 —— 剩四项(智能体 / 模型 / 思考强度 / 会话目录),两列正好两排,不留孤项;
   长路径在格内省略号 + title 悬浮看全。 */
.detail-params {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px 20px;
  margin-top: 4px;
  padding-top: 12px;
  border-top: 0.5px solid var(--border-light);
}

.param {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  min-width: 0;
}

.param-label {
  flex-shrink: 0;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.param-value {
  max-width: 100%;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.param-path {
  font-family: monospace;
  font-size: var(--font-size-2xs);
  font-weight: 400;
  color: var(--text-secondary);
}

.detail-history {
  display: flex;
  flex-direction: column;
  gap: 12px;
  flex-shrink: 0;
}

.history-open {
  margin-left: auto;
}

.history-header {
  display: flex;
  align-items: center;
  gap: 8px;
}

.history-title {
  font-size: var(--font-size-xs);
  font-weight: 600;
  color: var(--text-secondary);
}

.history-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  background: var(--bg-3);
  border-radius: 9px;
}

.history-day {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.history-day-label {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.history-cards {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

/* 单条执行记录：时间 + 标题 + 状态，点击进入该会话 */
.history-card {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 168px;
  max-width: 240px;
  padding: 8px 10px;
  text-align: left;
  background: var(--bg-2);
  border: 0.5px solid color-mix(in srgb, var(--border-strong) 45%, transparent);
  border-radius: 10px;
  cursor: pointer;
  transition:
    border-color var(--transition-fast),
    background-color var(--transition-fast),
    box-shadow var(--transition-fast);
}

.history-card:hover {
  background: var(--bg-hover);
  border-color: var(--accent-text);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.10);
}

.history-time {
  font-size: var(--font-size-2xs);
  font-variant-numeric: tabular-nums;
  color: var(--text-tertiary);
}

.history-name {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.history-meta {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-disabled);
  flex-shrink: 0;
}

.status-running {
  background: var(--accent-text);
}

.status-awaiting_approval,
.status-awaiting_form {
  background: #f59e0b;
}

.status-done {
  background: #22c55e;
}

.status-error {
  background: var(--danger);
}

.status-cancelled,
.status-ignored,
.status-interrupted {
  background: var(--text-disabled);
}

/* 无执行记录:内嵌式空态砖(macOS 里空区不写一句裸文字,给一块凹底区域) */
.history-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  margin: 0;
  padding: 20px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  background: color-mix(in srgb, var(--bg-2) 55%, transparent);
  border: 0.5px solid color-mix(in srgb, var(--border-strong) 45%, transparent);
  border-radius: 10px;
}

/* 第一行:标题 + 开关(标题吃剩余宽度,过长省略号) */
.card-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.card-name {
  min-width: 0;
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 第二行:运行方式 / 执行模式 / 审批模式(窄卡片放不下时换行) */
.card-tags {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

/* 信息标签:macOS 胶囊(无描边,灰底) —— 标签比按钮轻,尺寸不参与按钮统一 */
.info-tag.arco-tag {
  flex-shrink: 0;
  height: 20px;
  padding: 0 8px;
  border: none;
  border-radius: 999px;
  background: var(--bg-3);
  color: var(--text-secondary);
  font-size: var(--font-size-2xs);
  font-weight: 500;
  line-height: 20px;
}

/* 描述固定占三行(第三~五行):卡片等高,多行文本不把下面的行推来推去 */
.card-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.5;
  margin: 0;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  min-height: calc(1.5em * 3);
}

.card-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.meta-sep {
  color: var(--border-base);
}

.card-freq {
  color: var(--accent-text);
  font-weight: 500;
  white-space: nowrap;
}

.card-next {
  color: var(--text-tertiary);
  white-space: nowrap;
}

.card-next.paused {
  color: var(--text-disabled);
}

.card-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  margin-top: auto;
  padding-top: 4px;
  border-top: 1px solid var(--border-light);
}

.card-footer :deep(.arco-btn) {
  font-size: var(--font-size-xs);
  border-radius: 6px;
}

.card-footer :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}

/* ---------- 弹窗表单:两栏(左字段 / 右编辑器),编辑器吃满整列高度 ---------- */
.form {
  height: 100%;
  min-height: 0;
  display: flex;
  gap: 20px;
}

.form-col {
  display: flex;
  flex-direction: column;
  min-height: 0;
}

/* 左栏:任务字段(名称 / 模式 / 描述 / 工作空间);窗口过矮时本栏内滚动 */
.form-col-fields {
  flex: 0 0 52%;
  gap: 12px;
  overflow-y: auto;
  padding-right: 2px;
}

/* 右栏:调度规则 + 执行指令;编辑器撑满剩余高度(窗口过矮 / 每周勾选换行时本栏内滚动,不被裁掉) */
.form-col-right {
  flex: 1 1 auto;
  min-width: 0;
  gap: 12px;
  overflow-y: auto;
  padding-right: 2px;
}

/* 执行指令:吃掉右栏剩余高度(不再固定 300px) */
.form-row-editor {
  flex: 1 1 auto;
  min-height: 120px;
  min-width: 0;
}

/*
 * 窄窗口:弹窗被 75vw 压到放不下两栏 → 回落单栏(编辑器仍有最小高度)。
 * 阈值取 820px:再窄左栏就容不下「工作空间 + 更改目录」了。
 */
@media (max-width: 820px) {
  .form {
    flex-direction: column;
  }

  .form-col-fields {
    flex: none;
    overflow: visible;
  }

  .form-col-right {
    flex: 1 1 auto;
  }

  .form-row-editor {
    min-height: 200px;
  }
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 6px;
  /* 高度不足时只压缩编辑器,字段本身按内容排 */
  flex-shrink: 0;
}

/* 并排两个字段（执行模式 / 审批模式）：各占一半，内部控件仍按内容宽度 */
/* 表单脚注:模型跟随设置等整表说明(图标 + 次级文案,与字段说明区分开) */
.form-note {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin-top: 2px;
  font-size: var(--font-size-2xs);
  line-height: 1.5;
  color: var(--text-tertiary);
}

.form-note :deep(svg) {
  flex-shrink: 0;
  margin-top: 2px;
  opacity: 0.8;
}

.form-row-inline {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
}

.form-row-inline > .form-row {
  flex: 1 1 auto;
  min-width: 0;
}

/*
 * 字段宽度按内容需要给（macOS 表单不把输入框一律拉满整行）：
 * 任务名给固定上限;描述 / 工作空间随栏宽;只读路径与分组面板占满本栏。
 */
.field-name {
  max-width: 320px;
}

.field-desc {
  max-width: 100%;
}

.form-label {
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
}

.required {
  color: var(--danger);
}

.field-hint {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
  line-height: 1.6;
}

.workspace-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.workspace-row :deep(.arco-input-wrapper) {
  flex: 1;
}

/* 执行指令输入框:撑满右栏剩余高度 */
.instruction-editor {
  flex: 1;
  min-height: 0;
}

/* 频率分组:macOS 分组面板(中性填充底,不再用嵌套边框) */
.frequency-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
  background: var(--bg-2);
  border-radius: 10px;
  padding: 14px 16px;
}

/* 分段控件按内容宽度(macOS 分段控件不拉满整行);其余控件仍撑满本栏 */
.form-row :deep(.arco-radio-group-button) {
  align-self: flex-start;
}

.freq-config {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.time-picker {
  display: inline-flex;
  align-items: center;
  gap: 2px;
}

.time-sep {
  color: var(--text-tertiary);
  font-weight: 600;
  padding: 0 2px;
}

.weekly-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.cron-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.form-hint {
  font-size: var(--font-size-2xs);
  color: var(--text-disabled);
  font-family: monospace;
}

.next-run-preview {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  padding-top: 8px;
  border-top: 1px solid var(--border-light);
  font-family: monospace;
}

.error-text {
  color: var(--danger);
}

/* ---------- 弹窗底部:macOS 风格 ---------- */
.modal-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding-top: 4px;
}

.modal-footer :deep(.arco-btn) {
  border-radius: 6px;
}

/* 主按钮实心强调色(全局 .arco-btn-primary 被改成中性灰,弹窗内要压回来) */
.modal-footer :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* 弹窗内控件圆角与分段控件/勾选框配色跟随应用 token */
.form :deep(.arco-radio-group-button) {
  border-radius: 8px;
}

.form :deep(.arco-checkbox-label) {
  color: var(--text-primary);
}

.frequency-panel :deep(.arco-checkbox-label) {
  color: var(--text-primary);
}
</style>

<style>
/* ===== macOS 按钮三件套(页面内共用;scheduled- 前缀防冲突) =====
   尺寸统一:高 28 / 圆角 8 / 字号 13(文字按钮同为高 28,只换材质)——
   同一行里的「文字按钮 + 动作按钮」不再一大一小。
   全局 css 用 !important 把 .arco-btn-primary 压成灰色,主色默认按钮必须同权重 !important
   才能拿回主色(与设置页保存 / 聊天提交同一套配方)。 */
.scheduled-default-btn.arco-btn,
.scheduled-plain-btn.arco-btn,
.scheduled-danger-btn.arco-btn {
  height: 28px;
  padding: 0 12px;
  border-radius: 8px;
  border-width: 0.5px;
  border-style: solid;
  font-size: var(--font-size-sm);
  font-weight: 500;
  transition:
    background-color var(--transition-fast),
    border-color var(--transition-fast),
    filter var(--transition-fast);
}

/* 主色默认按钮:实心 + 白字 + 极轻投影 */
.scheduled-default-btn.arco-btn {
  border-color: transparent !important;
  color: #fff !important;
  background-color: var(--accent-text) !important;
  box-shadow: 0 1px 2.5px rgba(0, 0, 0, 0.18);
}

.scheduled-default-btn.arco-btn:hover {
  background-color: var(--accent-text) !important;
  filter: brightness(1.06);
}

.scheduled-default-btn.arco-btn:active {
  filter: brightness(0.94);
}

/* 非默认按钮:填充材质 + 发丝边(macOS 中性按钮) */
.scheduled-plain-btn.arco-btn {
  border-color: var(--border-base);
  background-color: var(--bg-2);
  color: var(--text-primary);
}

.scheduled-plain-btn.arco-btn:hover {
  border-color: var(--border-strong);
  background-color: var(--bg-hover);
  color: var(--text-primary) !important;
}

/* 破坏性按钮:中性材质 + 危险色文字(macOS 里删除不铺满红底,只在悬浮时透出红) */
.scheduled-danger-btn.arco-btn {
  border-color: var(--border-base);
  background-color: var(--bg-2);
  color: var(--danger) !important;
}

.scheduled-danger-btn.arco-btn:hover {
  border-color: color-mix(in srgb, var(--danger) 40%, var(--border-base)) !important;
  background-color: color-mix(in srgb, var(--danger) 12%, var(--bg-2)) !important;
  color: var(--danger) !important;
}

/* 文字/工具按钮:透明底,悬浮才浮现(与上面三件套同尺寸,不描边不占视觉重量) */
.scheduled-tool-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 28px;
  padding: 0 10px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  font-weight: 400;
  line-height: 1;
  cursor: pointer;
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.scheduled-tool-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.scheduled-tool-btn:active {
  background: var(--bg-active);
}

/* ===== macOS 风格弹窗骨架(Arco Modal 传送至 body,必须非 scoped;scheduled- 前缀防冲突) =====
   与知识库(.fm-modal) / 智能体(.agent-modal)同一套:颜色全走主题变量,明暗与强调色自动适配 */
.scheduled-modal.arco-modal,
.scheduled-modal .arco-modal {
  border-radius: 12px;
  overflow: hidden;
  background: var(--panel-bg);
  box-shadow: 0 18px 50px rgba(0, 0, 0, 0.22);
}

.scheduled-modal .arco-modal-header {
  padding: 14px 16px 10px;
  background: transparent;
  border-bottom: none;
}

.scheduled-modal .arco-modal-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
  text-align: left;
}

/* 关闭按钮:方形热区圆角,悬浮才浮现底色(macOS 工具按钮观感) */
.scheduled-modal .arco-modal-close-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: 6px;
  color: var(--text-tertiary);
  transition:
    background-color var(--transition-fast),
    color var(--transition-fast);
}

.scheduled-modal .arco-modal-close-btn:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}

.scheduled-modal .arco-modal-body {
  padding: 4px 16px 12px;
  background: transparent;
}

.scheduled-modal .arco-modal-footer {
  padding: 10px 16px 14px;
  background: transparent;
  border-top: 1px solid var(--border-light);
}

/* 表单控件:填充式圆角 + 聚焦主题色描边(macOS 表单观感) */
.scheduled-modal .arco-input-wrapper,
.scheduled-modal .arco-textarea-wrapper,
.scheduled-modal .arco-select-view {
  background: var(--bg-2);
  border: 1px solid transparent;
  border-radius: 6px;
  transition:
    border-color 0.15s ease,
    background-color 0.15s ease;
}

.scheduled-modal .arco-input-wrapper:hover,
.scheduled-modal .arco-textarea-wrapper:hover,
.scheduled-modal .arco-select-view:hover {
  background: var(--bg-hover);
  border-color: transparent;
}

.scheduled-modal .arco-input-wrapper:focus-within,
.scheduled-modal .arco-textarea-wrapper:focus-within,
.scheduled-modal .arco-select-view-focus {
  background: var(--bg-base);
  border-color: var(--accent-text);
}

.scheduled-modal .arco-input,
.scheduled-modal .arco-textarea,
.scheduled-modal .arco-select-view-value {
  color: var(--text-primary);
}

.scheduled-modal .arco-input::placeholder,
.scheduled-modal .arco-textarea::placeholder,
.scheduled-modal .arco-select-view-value-input::placeholder {
  color: var(--text-disabled);
}

/* 频率分组面板的底色就是 --bg-2(与控件填充色相同) → 组内控件改用 --bg-base + 描边,
   否则时分选择 / Cron 输入框在面板里"隐形"(看不出来是可输入的框) */
.scheduled-modal .frequency-panel .arco-select-view,
.scheduled-modal .frequency-panel .arco-input-wrapper {
  background: var(--bg-base);
  border-color: var(--border-base);
}

.scheduled-modal .frequency-panel .arco-select-view:hover,
.scheduled-modal .frequency-panel .arco-input-wrapper:hover {
  background: var(--bg-base);
  border-color: var(--border-strong);
}

.scheduled-modal .frequency-panel .arco-select-view-focus,
.scheduled-modal .frequency-panel .arco-input-wrapper:focus-within {
  background: var(--bg-base);
  border-color: var(--accent-text);
}

/* 只读输入(工作空间)：保留填充底,但不要聚焦描边误导成可编辑 */
.scheduled-modal .arco-input-wrapper:has(input[readonly]):focus-within {
  background: var(--bg-2);
  border-color: transparent;
}

/* 分段控件(执行模式 / 执行频率):macOS 分段观感 —— 灰轨道 + 白色选中胶囊 */
.scheduled-modal .arco-radio-group-button {
  background: var(--bg-3);
  border-radius: 8px;
  padding: 2px;
}

.scheduled-modal .arco-radio-group-button .arco-radio-button {
  margin: 0;
  border-radius: 6px;
  color: var(--text-secondary);
}

/* 段间竖分隔线不符合 macOS 分段控件观感(Arco 用 ::before 画的分隔) */
.scheduled-modal .arco-radio-group-button .arco-radio-button:not(:first-of-type)::before {
  display: none;
}

.scheduled-modal .arco-radio-group-button .arco-radio-button:hover {
  background: transparent;
}

.scheduled-modal .arco-radio-group-button .arco-radio-button.arco-radio-checked {
  background: var(--bg-base) !important;
  color: var(--text-primary) !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
}

/* markdown 编辑器容器:与外框同圆角,消掉 md-editor 自带边框造成的双线 */
.scheduled-modal .md-editor {
  border: none !important;
}
</style>
