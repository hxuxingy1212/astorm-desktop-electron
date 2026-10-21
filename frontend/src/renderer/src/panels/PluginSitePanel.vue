<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { usePluginsStore } from '@/stores/plugins'
import { useTabsStore } from '@/stores/tabs'
import { useSessionsStore } from '@/stores/sessions'
import { useChatPrefsStore } from '@/stores/chatPrefs'
import { useModelsStore } from '@/stores/models'
import { usePluginSessionStore } from '@/stores/pluginSession'
import { askLlm, type LlmChunkEvent } from '@/services/llm'
import { resolveSessionWorkspaceDir } from '@/utils/workspace'

/**
 * 插件网站页签：iframe 托管插件页面 + postMessage SDK 协议桥。
 *
 * 消息协议（宿主侧实现；插件侧经 astorm-plugin://__sdk__/astorm-sdk.js）：
 * - 插件 → 应用：{ astorm, kind:'request', id, method, params }，method ⊂
 *   platform.info / config.get / chat.appendInput / chat.send / llm.chat / plugin.ping
 * - 应用 → 插件：响应 { astorm, kind:'response', id, ok, result|error }
 *   / 流式 { astorm, kind:'stream', id, chunk:{type:'chunk'|'reasoning'|'end'|'error'} }
 *   / 推送 { astorm, kind:'event', event:'app.message', data }
 * origin 校验：仅接受 event.source === 本插件 iframe && origin === astorm-plugin://<id> 的消息。
 */

const props = defineProps<{ pluginId: string }>()

const { t } = useI18n()
const plugins = usePluginsStore()
const tabs = useTabsStore()
const sessions = useSessionsStore()
const chatPrefs = useChatPrefsStore()
const models = useModelsStore()
const pluginSession = usePluginSessionStore()

const view = computed(() => plugins.byId.get(props.pluginId))
const enabled = computed(() => view.value?.enabled !== false && !!view.value?.entryUrl)
const src = computed(() => view.value?.entryUrl ?? '')
const origin = computed(() => `astorm-plugin://${props.pluginId}`)

const iframeRef = ref<HTMLIFrameElement | null>(null)
const loaded = ref(false)
const loadFailed = ref(false)

// 发给插件的消息弹窗
const sendVisible = ref(false)
const sendText = ref('')
// 插件主动发给应用的留言（最后 N 条展示在工具栏下拉/气泡里）
const fromPluginMessages = ref<Array<{ text: string; ts: number }>>([])

// 进行中的 LLM 流取消函数（页面卸载时统一取消）
const llmCancels = new Set<() => void>()

/* ---------- postMessage 工具 ---------- */

/**
 * 向插件 iframe 发送消息。payload 统一深拷贝为纯 JSON 后再投递：
 * store 数据（如 config.get 的配置）是 Vue reactive Proxy，
 * 结构拷贝不能克隆 Proxy（DataCloneError），协议载荷本就约定为可 JSON 序列化数据。
 *
 * targetOrigin 用 '*'：安全边界在接收侧（onWindowMessage 校验 e.source === 本 iframe
 * 且 e.origin === astorm-plugin://<id>）。iframe 处于初始 about:blank / 重载中间态时
 * 其临时窗口 origin 不是协议 origin，写死 targetOrigin 会抛 DOMException 并打断后续
 * 推送；'*' 配合接收侧校验保持等价安全。
 */
function post(message: unknown): void {
  const win = iframeRef.value?.contentWindow
  if (!win) return
  win.postMessage(JSON.parse(JSON.stringify(message)), '*')
}

function respond(id: string, ok: boolean, result?: unknown, error?: { code: string; message: string }): void {
  post({ astorm: true, pluginId: props.pluginId, kind: 'response', id, ok, result, error })
}

function streamChunk(id: string, chunk: LlmChunkEvent): void {
  post({ astorm: true, pluginId: props.pluginId, kind: 'stream', id, chunk })
}

function hasPerm(perm: string): boolean {
  return (view.value?.manifest.permissions ?? []).includes(perm)
}

/**
 * 当前会话工作区绝对目录（与 sessions.openSessionFolder 同口径：
 * 绝对路径直接用；相对路径归入 `<用户目录>/workspace/` 下解析，
 * 见 utils/workspace.resolveSessionWorkspaceDir）。
 */
async function resolveSessionWorkspaceDirOfActiveSession(): Promise<string> {
  const sid = sessions.activeId
  const rec = sid
    ? (sessions.sessionInfo[sid] ??
      sessions.sessions.find((s) => s.id === sid) ??
      undefined)
    : undefined
  return resolveSessionWorkspaceDir(rec?.workspace)
}

/* ---------- 请求分发 ---------- */

async function handleRequest(id: string, method: string, params: Record<string, unknown>): Promise<void> {
  switch (method) {
    case 'platform.info':
      respond(id, true, {
        app: 'astorm-agent',
        sdkVersion: view.value?.manifest.sdkVersion ?? '1',
        protoVersion: 1,
        providers: models.providers.length,
        sessionActive: !!sessions.activeId,
        sessionId: pluginSession.currentSessionId,
        locale: document.documentElement.lang
      })
      break
    case 'config.get':
      respond(id, true, { config: view.value?.config ?? {}, schema: view.value?.manifest.configSchema ?? [] })
      break
    case 'state.get':
      // 会话级插件状态：按 会话×插件 作用域读取（传 key 取单值，不传返回整表）
      respond(id, true, {
        sessionId: pluginSession.currentSessionId,
        value: pluginSession.get(props.pluginId, typeof params.key === 'string' && params.key ? params.key : undefined)
      })
      break
    case 'state.set':
      if (typeof params.key !== 'string' || !params.key) {
        respond(id, false, undefined, { code: 'BAD_REQUEST', message: 'state.set 需要非空 key 参数' })
        break
      }
      pluginSession.set(props.pluginId, params.key, params.value)
      respond(id, true, { ok: true, sessionId: pluginSession.currentSessionId })
      break
    case 'chat.appendInput':
      plugins.setInjectedInput(String(params.text ?? ''))
      respond(id, true, { ok: true })
      break
    case 'file.openPptx': {
      // 内置插件 ppt-inspector：宿主代开系统对话框(默认落在当前会话空间目录)，
      // 选中 .pptx 后由主进程 json2pptx-cli 解包为产物 deck JSON 回传(取消回 canceled)。
      const dir = await resolveSessionWorkspaceDirOfActiveSession()
      const file = await window.api.dialog.openPptx(dir || undefined)
      if (!file) {
        respond(id, true, { canceled: true })
        break
      }
      const r = await window.api.pptx.unpack(file)
      if (r?.ok) respond(id, true, { canceled: false, file: r.file, deck: r.deck })
      else respond(id, false, undefined, { code: 'UNPACK_FAILED', message: r?.message ?? 'PPT 解包失败' })
      break
    }
    case 'chat.send':
      if (!hasPerm('chat')) {
        respond(id, false, undefined, { code: 'FORBIDDEN', message: `缺少 chat 权限：清单 permissions 需包含 "chat"` })
        break
      }
      sessions.sendMessage(String(params.text ?? ''), runtimeContext())
      respond(id, true, { ok: true })
      break
    case 'llm.chat':
      if (!hasPerm('llm')) {
        respond(id, false, undefined, { code: 'FORBIDDEN', message: `缺少 llm 权限：清单 permissions 需包含 "llm"` })
        break
      }
      runLlmChat(id, params)
      break
    case 'plugin.ping':
      respond(id, true, { ok: true, ts: Date.now() })
      break
    case 'plugin.messageToApp':
      handlePluginMessage(params)
      respond(id, true, { ok: true })
      break
    default:
      respond(id, false, undefined, { code: 'UNKNOWN_METHOD', message: `未知方法: ${method}` })
  }
}

/** 由当前聊天偏好组装发送上下文（模式/权限/思考强度随会话输入框） */
function runtimeContext(): Record<string, unknown> {
  chatPrefs.applyDefaultSelection()
  const [provider = '', model = ''] = chatPrefs.selectedPath
  return {
    provider,
    model,
    mode: chatPrefs.mode,
    permission: chatPrefs.permission,
    thinkingLevel: chatPrefs.thinkingLevel
  }
}

/** llm.chat：经 window.api.llm（→ Rust ask_llm）流式补全，chunk 事件转发给插件。 */
function runLlmChat(id: string, params: Record<string, unknown>): void {
  const messages = (Array.isArray(params.messages) ? params.messages : [])
    .filter(
      (m): m is { role: 'system' | 'user' | 'assistant'; content: string } =>
        !!m &&
        typeof (m as { content?: unknown }).content === 'string' &&
        ['system', 'user', 'assistant'].includes((m as { role?: string }).role ?? '')
    )
    .map((m) => ({ role: m.role, content: m.content }))
  if (!messages.length) {
    respond(id, false, undefined, { code: 'BAD_REQUEST', message: 'messages 必须为非空 {role, content} 数组' })
    return
  }
  const cancel = askLlm(
    {
      provider: typeof params.provider === 'string' ? params.provider : undefined,
      model: typeof params.model === 'string' ? params.model : undefined,
      messages,
      reasoningEffort: typeof params.reasoningEffort === 'string' ? params.reasoningEffort : undefined
    },
    (ev) => streamChunk(id, ev)
  )
  llmCancels.add(cancel)
}

/* ---------- 消息监听 ---------- */

function onWindowMessage(e: MessageEvent): void {
  if (e.source !== iframeRef.value?.contentWindow) return
  if (e.origin !== origin.value) return
  const msg = e.data as { astorm?: boolean; kind?: string; id?: unknown; method?: string; params?: unknown; pluginId?: string }
  if (!msg || msg.astorm !== true || msg.kind !== 'request') return
  if (msg.pluginId && msg.pluginId !== props.pluginId) return
  const id = typeof msg.id === 'number' || typeof msg.id === 'string' ? String(msg.id) : ''
  if (!id) return
  const params = (msg.params ?? {}) as Record<string, unknown>
  void handleRequest(id, String(msg.method ?? ''), params)
}

/** 收到插件的「发给应用」留言（SDK plugin.sendMessageToApp） */
function handlePluginMessage(data: Record<string, unknown>): void {
  const text = String(data.text ?? data.message ?? '')
  if (!text) return
  fromPluginMessages.value = [{ text, ts: Date.now() }, ...fromPluginMessages.value].slice(0, 5)
}

/* ---------- 工具栏动作 ---------- */

function reload(): void {
  const iframe = iframeRef.value
  if (!iframe) return
  loaded.value = false
  loadFailed.value = false
  const url = iframe.src
  iframe.src = 'about:blank'
  void nextTick(() => {
    iframe.src = url
  })
}

function onFrameLoad(): void {
  loaded.value = true
  loadFailed.value = false
  // 应用 → 插件：加载完成即推送一条欢迎消息，演示 app.message 事件通道
  post({
    astorm: true,
    pluginId: props.pluginId,
    kind: 'event',
    event: 'app.message',
    data: { from: 'app', text: t('plugins.siteWelcome', { name: view.value?.manifest.name ?? props.pluginId }), ts: Date.now() }
  })
}

function confirmSendToPlugin(): void {
  const text = sendText.value.trim()
  if (!text) return
  post({
    astorm: true,
    pluginId: props.pluginId,
    kind: 'event',
    event: 'app.message',
    data: { from: 'app', text, ts: Date.now() }
  })
  tabs.expand()
  sendVisible.value = false
  sendText.value = ''
}

// 插件卸载/重装（视图占位）时清理进行中的 LLM 流
watch(view, () => {
  if (!enabled.value) {
    for (const off of llmCancels) off()
    llmCancels.clear()
  }
})

// 页签全局共享:切换会话时插件 iframe 不销毁;推送 app.session.change 供插件按会话重新取状态
watch(
  () => sessions.activeId,
  (sid) => {
    post({
      astorm: true,
      pluginId: props.pluginId,
      kind: 'event',
      event: 'app.session.change',
      data: { sessionId: sid ?? '', ts: Date.now() }
    })
  }
)

onMounted(() => {
  window.addEventListener('message', onWindowMessage)
})

onBeforeUnmount(() => {
  window.removeEventListener('message', onWindowMessage)
  for (const off of llmCancels) off()
  llmCancels.clear()
})
</script>

<template>
  <div :key="props.pluginId" class="flex h-full min-h-0 flex-col">
    <!-- 工具条：插件名 + 状态 / 刷新 / 发送消息给插件 -->
    <div class="plugin-bar flex shrink-0 items-center gap-2 px-3 py-1.5">
      <span class="plugin-dot" :class="loaded ? 'is-loaded' : 'is-loading'" />
      <span class="min-w-0 flex-1 truncate text-[13px] font-semibold text-[var(--text-primary)]">
        {{ view?.manifest.name ?? props.pluginId }}
      </span>
      <span v-if="view?.manifest.version" class="text-[11px] text-[var(--text-tertiary)]">
        v{{ view.manifest.version }}
      </span>
      <a-tooltip :content="t('plugins.siteReload')">
        <button type="button" class="plugin-bar-btn" :aria-label="t('plugins.siteReload')" @click="reload">
          <icon-refresh :size="15" />
        </button>
      </a-tooltip>
      <a-tooltip :content="t('plugins.siteSendToPlugin')">
        <button type="button" class="plugin-bar-btn" :aria-label="t('plugins.siteSendToPlugin')" @click="sendVisible = true">
          <icon-send :size="15" />
        </button>
      </a-tooltip>
    </div>

    <!-- 插件已禁用 / 已卸载占位 -->
    <div v-if="!enabled" class="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-[var(--text-tertiary)]">
      <icon-puzzle :size="28" class="opacity-50" />
      <span>{{ t('plugins.siteDisabled') }}</span>
      <a-button size="small" type="text" @click="plugins.setEnabled(props.pluginId, true)">
        {{ t('plugins.enable') }}
      </a-button>
    </div>

    <!-- iframe：sandbox 隔离，插件页面仅能经 postMessage 与宿主通信 -->
    <div v-else class="relative min-h-0 flex-1">
      <div v-if="!loaded && !loadFailed" class="absolute inset-0 flex items-center justify-center bg-[var(--panel-bg)]">
        <a-spin :tip="t('plugins.siteLoading')" />
      </div>
      <iframe
        v-show="loaded"
        ref="iframeRef"
        class="h-full w-full border-0 bg-white"
        :src="src"
        sandbox="allow-scripts allow-same-origin allow-forms"
        title="plugin-site"
        @load="onFrameLoad"
      />
    </div>

    <!-- 应用 → 插件 消息弹窗 -->
    <a-modal
      v-model:visible="sendVisible"
      width="420px"
      modal-class="mac-modal"
      :title="t('plugins.siteSendToPlugin')"
      :ok-text="t('chat.send')"
      :cancel-text="t('common.cancel')"
      @ok="confirmSendToPlugin"
    >
      <a-textarea
        v-model="sendText"
        :auto-size="{ minRows: 3, maxRows: 8 }"
        :placeholder="t('plugins.siteSendToPluginPlaceholder')"
      />
    </a-modal>
  </div>
</template>

<style scoped>
.plugin-bar {
  border-bottom: 1px solid var(--panel-border);
}

.plugin-dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 50%;
}

.plugin-dot.is-loaded {
  background-color: var(--color-success-6, #00b42a);
}

.plugin-dot.is-loading {
  background-color: var(--color-warning-6, #ff7d00);
}

.plugin-bar-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  padding: 0;
  border: 1px solid var(--border-base);
  border-radius: 6px;
  background-color: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
}

.plugin-bar-btn:hover {
  background-color: var(--bg-hover);
  color: var(--text-primary);
}
</style>