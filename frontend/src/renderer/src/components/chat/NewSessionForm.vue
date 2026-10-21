<script setup lang="ts">
/**
 * 新建会话表单：新建会话页专用（看板「新建待办」也跳到这一页）。
 *
 * 从 `views/NewChatView.vue` 抽出来，只保留与页面无关的部分（目录选择 + 分支展示 + 输入框 + 设置草稿）：
 * 页面外壳（欢迎标题、居中布局、背景）留在 `NewChatView`。
 *
 * 两条提交路径（对应后端 `create_session` 的 `auto_run`），由入口决定、表单不再提供切换：
 * - 立即执行（true）：普通「新建对话」入口；
 * - 添加待办（false）：看板「新建待办」跳转带 `?todo=1`，后端落 `idle`、生成标题、首条消息
 *   写进历史，但**不起循环**；卡片落在看板「待办」列，想跑时再启动。
 *
 * 初始档位来自 `defaultExecute`；状态仍随提交传给后端，只是这里不显示切换按钮。
 * 页面在 KeepAlive 里不会重挂载，故 watch 同步默认值。
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { useSessionsStore } from '@/stores/sessions'
import type { SessionSettingsView } from '@/stores/sessions'
import { useChatPrefsStore, type ChatMode } from '@/stores/chatPrefs'
import type { ChipMentionRef, QuoteRefPayload } from '@/components/mentions/types'
import type { MessageAttachment } from '@/types'
import ChatInput from '@/components/chat/ChatInput.vue'
import FileBranchBar from '@/components/chat/FileBranchBar.vue'

const props = withDefaults(
  defineProps<{
    /** 预填目录（新建会话页从 `?directory=` 带上来） */
    initialDirectory?: string
    /** 创建后是否跳到该会话 */
    navigateOnCreate?: boolean
    /**
     * 提交档位：true（默认）= 立即执行；false = 添加待办（看板「新建待办」跳转）。
     * 由入口决定，状态随提交带给后端；表单本身不显示切换控件。
     */
    defaultExecute?: boolean
  }>(),
  { initialDirectory: '', navigateOnCreate: true, defaultExecute: true }
)

const emit = defineEmits<{
  /** 创建完成（executed = 是否立即执行） */
  (e: 'created', id: string, executed: boolean): void
}>()

const { t } = useI18n()
const router = useRouter()
const sessions = useSessionsStore()
const chatPrefs = useChatPrefsStore()

// 待发送目录;分支只用于展示(FileBranchBar 回传实时 HEAD),不再随会话落库
const directory = ref(props.initialDirectory)
const branch = ref('')
watch(
  () => props.initialDirectory,
  (d) => {
    if (d) directory.value = d
  }
)

/**
 * 提交档位：true = 立即执行 / false = 添加待办。初始值来自 `defaultExecute`
 * （看板「新建待办」跳转为 false）；页面被 KeepAlive 缓存不会重挂载，
 * 所以默认值变化（再次从不同入口进入）时同步一次。
 */
const execute = ref(props.defaultExecute)
watch(
  () => props.defaultExecute,
  (v) => {
    execute.value = v
  }
)

/**
 * 新会话的初始设置(受控):草稿存在 chatPrefs(内存态)—— 会话还没创建,没有可落盘的对象。
 * 创建时随 create_session 一起落进会话记录;之后该会话的设置就由后端记录说了算。
 */
const draftSettings = computed<SessionSettingsView>(() => ({
  mode: chatPrefs.mode,
  permission: chatPrefs.permission,
  thinkingLevel: chatPrefs.thinkingLevel,
  provider: chatPrefs.selectedPath[0] ?? '',
  model: chatPrefs.selectedPath[1] ?? ''
}))
function onSettingsChange(patch: Partial<SessionSettingsView>): void {
  if (patch.mode) chatPrefs.mode = patch.mode
  if (patch.permission) chatPrefs.permission = patch.permission
  if (patch.thinkingLevel !== undefined) chatPrefs.thinkingLevel = patch.thinkingLevel
  if (patch.provider !== undefined || patch.model !== undefined) {
    chatPrefs.selectedPath = [
      patch.provider ?? draftSettings.value.provider,
      patch.model ?? draftSettings.value.model
    ]
  }
}

const inputRef = ref<InstanceType<typeof ChatInput> | null>(null)

/** 目录拼进首条消息（与 workspace 字段双轨：模型也从正文里读到路径） */
function composedText(text: string): string {
  const attachLines = directory.value ? [`${t('newChat.attachPrefix')}: ${directory.value}`] : []
  return attachLines.length ? `${attachLines.join('\n')}\n\n${text}` : text
}

/**
 * 提交。`execute=false` 时只创建（后端不起循环）。返回会话 id（失败 / 空输入为 null）。
 *
 * `payload` 为输入框 `@send` 事件携带的载荷（文本 / 附件 / 显式模式）：优先采用它，
 * 否则回退 `currentDraft()`（直接读取编辑框）——保证 `/init`、`/plan` 这类文本宏在新建页也生效。
 */
async function submit(
  execute: boolean,
  payload?: {
    text: string
    attachments?: MessageAttachment[]
    mode?: ChatMode
    goal?: string
    goalLang?: string
    mentions?: ChipMentionRef[]
    elements?: string[]
    quotes?: QuoteRefPayload[]
  }
): Promise<string | null> {
  const draft = inputRef.value?.currentDraft()
  const text = (payload?.text ?? draft?.text ?? '').trim()
  if (!text) {
    Message.warning(t('newChat.emptyHint'))
    return null
  }
  const attachments = payload?.attachments ?? draft?.attachments
  // 选择目录的会话归类为「项目」,未选择目录按「任务」(放入侧栏对应分组)
  const kind = directory.value ? 'project' : 'task'
  if (!execute) {
    const id = await sessions.createParkedSession({
      text: composedText(text),
      kind,
      workspace: directory.value || undefined,
      attachments,
      settings: draftSettings.value
    })
    if (id) {
      emit('created', id, false)
      // 页面入口留在本页（待办卡片出现在看板），但要有「已创建」的反馈并清空输入，
      // 否则原文还留在框里，回车就重复建一条 —— 等价于旧弹窗的「创建完关闭」
      if (props.navigateOnCreate) {
        inputRef.value?.setDraft('')
        Message.success(t('newChat.todoCreated'))
      }
    }
    return id
  }

  const id = sessions.createSession(kind, {
    workspace: directory.value || undefined
  })
  // 输入框选的设置先记为该会话的草稿,随下面的 create_session 一次性落库
  sessions.setSessionSettings(id, draftSettings.value)
  sessions.sendMessage(composedText(text), {
    // 首条消息的图片附件(staging 路径):后端建会话后复制进该会话工作区
    attachments,
    // 目录随首条消息传给后端:后端会话记录存绝对路径,重启回填才能继续归类「项目」
    workspace: directory.value || undefined,
    // 显式模式(/plan)、目标(/goal)、引用元数据(mentions/elements):随本条消息下发
    mode: payload?.mode,
    goal: payload?.goal,
    goalLang: payload?.goalLang,
    mentions: payload?.mentions,
    elements: payload?.elements,
    quotes: payload?.quotes
  })
  emit('created', id, true)
  if (props.navigateOnCreate) router.push(`/chat/${id}`)
  return id
}

/** 输入框发送：按当前档位决定「创建并执行」还是「仅创建（待办）」 */
function onSend(payload: {
  text: string
  attachments?: MessageAttachment[]
  mode?: ChatMode
  goal?: string
  goalLang?: string
}): void {
  void submit(execute.value, payload)
}

</script>

<template>
  <div class="new-session-form">
    <ChatInput
      ref="inputRef"
      no-padding
      no-session-info
      :settings="draftSettings"
      :workspace-dir="directory"
      @send="onSend"
      @update:settings="onSettingsChange"
    >
      <template #top>
        <div class="top-row">
          <FileBranchBar v-model:directory="directory" v-model:branch="branch" />
        </div>
      </template>
    </ChatInput>
  </div>
</template>

<style scoped>
.new-session-form {
  width: 100%;
}

.top-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.top-row > :first-child {
  flex: 1;
  min-width: 0;
}
</style>
