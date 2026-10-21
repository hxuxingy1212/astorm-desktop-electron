<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useToolsStore } from '@/stores/tools'
import { useMcpStore } from '@/stores/mcp'
import JsonEditorPane from '@/components/common/JsonEditorPane.vue'
import ToolCard from '@/components/tools/ToolCard.vue'
import McpServerCard from '@/components/tools/McpServerCard.vue'
import type { McpServerSummary, ToolInfo } from '@/services/dto'

const { t } = useI18n()
const toolsStore = useToolsStore()
const mcpStore = useMcpStore()

const searchQuery = ref('')
const activeTab = ref<'builtin' | 'dynamic' | 'mcp'>('builtin')

// ── MCP config:默认隐藏,经右上角「编辑 / 导入」进入页内编辑页 ──
const editingConfig = ref(false)
const configDraft = ref('')
const configError = ref('')
const importingConfig = ref(false)

function validateConfigText(val: string): void {
  configError.value = ''
  try {
    const parsed = JSON.parse(val) as { mcpServers?: unknown }
    const servers = parsed?.mcpServers
    if (!servers || typeof servers !== 'object' || Array.isArray(servers)) {
      configError.value = t('settings.mcp.invalidConfig')
    }
  } catch (e) {
    configError.value = e instanceof Error ? e.message : String(e)
  }
}

watch(configDraft, validateConfigText)

function openConfigEditor(): void {
  configDraft.value = mcpStore.configText
  validateConfigText(configDraft.value)
  editingConfig.value = true
}

async function handleImportConfig(): Promise<void> {
  const openJson = window.api?.dialog?.openJson
  if (!openJson) {
    Message.error(t('settings.mcp.importUnavailable'))
    return
  }
  importingConfig.value = true
  try {
    const file = await openJson()
    if (!file) return
    configDraft.value = file.content
    validateConfigText(file.content)
    editingConfig.value = true
    const name = file.path.split(/[\\/]/).pop() || file.path
    Message.success(t('settings.mcp.importLoaded', { name }))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  } finally {
    importingConfig.value = false
  }
}

function formatConfig(): void {
  try {
    configDraft.value = JSON.stringify(JSON.parse(configDraft.value), null, 2)
  } catch {
    // 语法错误已由 watch 校验展示,这里保持原文
  }
}

function closeConfigEditor(): void {
  editingConfig.value = false
  configDraft.value = ''
  configError.value = ''
}

async function saveConfig(): Promise<void> {
  validateConfigText(configDraft.value)
  if (configError.value) return
  try {
    await mcpStore.saveConfig(configDraft.value)
    closeConfigEditor()
    Message.success(t('settings.mcp.saveSuccess'))
  } catch (e) {
    Message.error(e instanceof Error ? e.message : String(e))
  }
}

// ── Tool filtering ──
function filterTools(list: ToolInfo[]): ToolInfo[] {
  if (!searchQuery.value) return list
  const q = searchQuery.value.toLowerCase()
  return list.filter((tool) => tool.name.toLowerCase().includes(q) || tool.description.toLowerCase().includes(q))
}

const filteredBuiltin = computed(() => filterTools(toolsStore.builtinTools))
const filteredPlugin = computed(() => filterTools(toolsStore.pluginTools))
const filteredDynamic = computed(() => filterTools(toolsStore.dynamicTools))

/**
 * 内置页签 = 常规内置工具（剔除按需加载的动态工具）+ 插件工具合并：
 * 动态工具移到独立「动态工具」页签，每个工具只出现一处。
 */
const builtinToolsForTab = computed<ToolInfo[]>(() => [
  ...filteredBuiltin.value.filter((t) => !t.dynamic),
  ...filteredPlugin.value
])
const dynamicToolsForTab = computed<ToolInfo[]>(() => filteredDynamic.value)

/** 参数 Schema 弹窗：卡片只展示摘要，长 JSON 在弹窗里看 */
const schemaTool = ref<ToolInfo | null>(null)
const schemaVisible = ref(false)

const schemaText = computed(() =>
  schemaTool.value ? JSON.stringify(schemaTool.value.parametersSchema, null, 2) : ''
)

function openSchema(tool: ToolInfo): void {
  schemaTool.value = tool
  schemaVisible.value = true
}

async function copySchema(tool: ToolInfo): Promise<void> {
  try {
    await navigator.clipboard.writeText(JSON.stringify(tool.parametersSchema, null, 2))
    Message.success(t('common.copied'))
  } catch {
    Message.error(t('common.copyFailed'))
  }
}

// 页签计数与列表同口径:内置页签不含动态工具,动态页签只含动态工具
const builtinCount = computed(
  () => toolsStore.builtinTools.filter((t) => !t.dynamic).length + toolsStore.pluginTools.length
)
const dynamicCount = computed(() => toolsStore.dynamicTools.length)

// 连接中标记：npx/uvx 系服务器首次连接要下载包（数十秒），按钮进加载态避免"点了没反应"
const connectingServers = ref(new Set<string>())

async function handleConnect(name: string): Promise<void> {
  connectingServers.value = new Set(connectingServers.value).add(name)
  try {
    const result = await mcpStore.connectServer(name)
    if (result) {
      toolsStore.clearServerToolsCache()
      Message.success(result)
      if (selectedMcpServer.value === name) void toolsStore.fetchServerTools(name)
    } else {
      Message.error(mcpStore.error || t('settings.mcp.connectFailed'))
    }
  } finally {
    const next = new Set(connectingServers.value)
    next.delete(name)
    connectingServers.value = next
  }
}

async function handleDisconnect(name: string): Promise<void> {
  await mcpStore.disconnectServer(name)
  toolsStore.clearServerToolsCache()
  Message.success(t('settings.mcp.disconnectSuccess'))
}

async function handleToggleEnabled(name: string, val: boolean | number | string): Promise<void> {
  await mcpStore.toggleEnabled(name, Boolean(val))
  toolsStore.clearServerToolsCache()
}

/* ---------- MCP 页内导航：服务器列表 → 工具卡片页 → 工具详情（均不使用弹窗） ---------- */
const selectedMcpServer = ref<string | null>(null)
const inlineMcpTool = ref<ToolInfo | null>(null)

const activeMcpServer = computed<McpServerSummary | null>(() =>
  selectedMcpServer.value
    ? mcpStore.servers.find((server) => server.name === selectedMcpServer.value) ?? null
    : null
)
const activeMcpServerTools = computed<ToolInfo[]>(() =>
  selectedMcpServer.value ? toolsStore.getServerTools(selectedMcpServer.value) ?? [] : []
)
const activeMcpServerLoading = computed(() =>
  selectedMcpServer.value ? !!toolsStore.serverToolsLoading[selectedMcpServer.value] : false
)

/* ---------- MCP 授权页（OAuth / 手动 Token，均页内） ---------- */
const authServerName = ref<string | null>(null)
const activeAuthServer = computed<McpServerSummary | null>(() =>
  authServerName.value
    ? mcpStore.servers.find((server) => server.name === authServerName.value) ?? null
    : null
)
const authToken = ref('')
const authHeaderName = ref('')
const authHeaderValue = ref('')
const authSaving = ref(false)
const oauthStarting = ref(false)
const oauthStatus = ref<'none' | 'pending' | 'authorized' | 'failed'>('none')
const oauthError = ref('')
let oauthPollTimer: ReturnType<typeof setInterval> | null = null

/** MCP 下钻页（工具 / 详情 / 授权）：收起分段切换与配置入口 */
const inMcpSubpage = computed(
  () =>
    activeTab.value === 'mcp' &&
    (!!activeMcpServer.value || !!inlineMcpTool.value || !!activeAuthServer.value)
)

function humanError(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e)
  return text.replace(/^Error invoking remote method '[^']+':\s*(?:Error:\s*)?/, '')
}

function stopOAuthPolling(): void {
  if (oauthPollTimer) {
    clearInterval(oauthPollTimer)
    oauthPollTimer = null
  }
}

function resetAuthForm(): void {
  stopOAuthPolling()
  authToken.value = ''
  authHeaderName.value = ''
  authHeaderValue.value = ''
  oauthStatus.value = 'none'
  oauthError.value = ''
}

function openAuthPage(server: McpServerSummary): void {
  authServerName.value = server.name
  selectedMcpServer.value = null
  inlineMcpTool.value = null
  resetAuthForm()
}

async function startOAuth(): Promise<void> {
  const server = activeAuthServer.value
  if (!server) return
  oauthStarting.value = true
  oauthError.value = ''
  try {
    const start = await mcpStore.beginOAuth(server.name)
    const openExternal = window.api?.shell?.openExternal
    if (!openExternal) {
      oauthStatus.value = 'failed'
      oauthError.value = t('settings.mcp.oauthUnavailable')
      return
    }
    const openError = await openExternal(start.authorizationUrl)
    if (openError) {
      oauthStatus.value = 'failed'
      oauthError.value = openError
      return
    }
    oauthStatus.value = 'pending'
    stopOAuthPolling()
    oauthPollTimer = setInterval(() => {
      void pollOAuth(server.name)
    }, 1500)
  } catch (e) {
    oauthStatus.value = 'failed'
    oauthError.value = humanError(e)
  } finally {
    oauthStarting.value = false
  }
}

async function pollOAuth(name: string): Promise<void> {
  if (authServerName.value !== name) {
    stopOAuthPolling()
    return
  }
  try {
    const status = await mcpStore.oauthStatus(name)
    if (status.status === 'authorized') {
      stopOAuthPolling()
      oauthStatus.value = 'authorized'
      await finishOAuth(name)
    } else if (status.status === 'failed') {
      stopOAuthPolling()
      oauthStatus.value = 'failed'
      oauthError.value = status.error || t('settings.mcp.oauthFailed')
    }
  } catch {
    // 轮询偶发失败不终止，下一次继续
  }
}

async function finishOAuth(name: string): Promise<void> {
  const result = await mcpStore.connectServer(name)
  if (!result) {
    oauthStatus.value = 'failed'
    oauthError.value = mcpStore.error || t('settings.mcp.connectFailed')
    return
  }
  toolsStore.clearServerToolsCache()
  Message.success(result)
  void toolsStore.fetchTools()
  backToMcpServers()
}

async function saveManualAuth(): Promise<void> {
  const server = activeAuthServer.value
  if (!server) return
  const headers: Record<string, string> = {}
  const token = authToken.value.trim()
  if (token) {
    headers.Authorization = /^Bearer\s/i.test(token) ? token : `Bearer ${token}`
  }
  const headerName = authHeaderName.value.trim()
  const headerValue = authHeaderValue.value.trim()
  if (!!headerName !== !!headerValue) {
    Message.error(t('settings.mcp.authHeaderIncomplete'))
    return
  }
  if (headerName) headers[headerName] = headerValue
  if (!Object.keys(headers).length) {
    Message.error(t('settings.mcp.authEmpty'))
    return
  }
  authSaving.value = true
  try {
    const message = await mcpStore.setServerAuth(server.name, headers)
    toolsStore.clearServerToolsCache()
    Message.success(message || t('settings.mcp.authSaved'))
    void toolsStore.fetchTools()
    backToMcpServers()
  } catch (e) {
    Message.error(humanError(e))
  } finally {
    authSaving.value = false
  }
}

/** 工具详情页参数数量（JSON Schema properties） */
const inlineMcpParamCount = computed(() => {
  const properties = (inlineMcpTool.value?.parametersSchema as { properties?: unknown } | undefined)
    ?.properties
  return properties && typeof properties === 'object' ? Object.keys(properties).length : 0
})

function openMcpServer(server: McpServerSummary): void {
  // 需要授权的服务器：卡片点击直接进入授权页，避免打开一个空工具列表
  if (server.authRequired) {
    openAuthPage(server)
    return
  }
  selectedMcpServer.value = server.name
  inlineMcpTool.value = null
  authServerName.value = null
  if (server.enabled && server.connected) void toolsStore.fetchServerTools(server.name)
}

function openMcpTool(tool: ToolInfo): void {
  inlineMcpTool.value = tool
}

function backToMcpTools(): void {
  inlineMcpTool.value = null
}

function backToMcpServers(): void {
  selectedMcpServer.value = null
  inlineMcpTool.value = null
  authServerName.value = null
  resetAuthForm()
}

// 切走 MCP 页签时回到服务器列表；选中服务器被删除时同样回退
watch(activeTab, (tab) => {
  if (tab !== 'mcp') {
    backToMcpServers()
    closeConfigEditor()
  }
})
watch(
  () => mcpStore.servers.map((server) => server.name).join('|'),
  () => {
    const missing = (name: string | null): boolean =>
      !!name && !mcpStore.servers.some((server) => server.name === name)
    if (missing(selectedMcpServer.value) || missing(authServerName.value)) {
      backToMcpServers()
    }
  }
)

watch(
  () => mcpStore.servers.map((server) => `${server.name}:${server.connected}:${server.enabled}`).join('|'),
  () => {
    for (const server of mcpStore.servers) {
      if (server.enabled && server.connected) void toolsStore.fetchServerTools(server.name)
    }
  },
  { immediate: true }
)

// ── Lifecycle ──
onMounted(() => {
  toolsStore.fetchTools()
  mcpStore.fetchServers()
})
onBeforeUnmount(stopOAuthPolling)
</script>

<template>
  <div class="page">
    <div class="page-header">
      <h2 class="page-title">{{ t('settings.tools.title') }}</h2>
      <div class="header-actions">
        <template v-if="activeTab === 'mcp' && !editingConfig && !inMcpSubpage">
          <a-button size="small" @click="openConfigEditor">
            <template #icon><icon-edit :size="13" /></template>
            {{ t('settings.mcp.editConfig') }}
          </a-button>
          <a-button size="small" :loading="importingConfig" @click="handleImportConfig">
            <template #icon><icon-upload :size="13" /></template>
            {{ t('settings.mcp.importConfig') }}
          </a-button>
        </template>
        <a-tooltip :content="t('common.refresh')">
          <a-button size="small" @click="toolsStore.fetchTools(); mcpStore.fetchServers()">
            <template #icon><icon-refresh :size="13" /></template>
          </a-button>
        </a-tooltip>
        <a-input-search
          v-model="searchQuery"
          :placeholder="t('settings.tools.searchPlaceholder')"
          allow-clear
          style="width: 260px"
        />
      </div>
    </div>

    <!-- MCP 下钻页（工具 / 详情 / 授权）收起分段切换，保持单页聚焦 -->
    <div v-if="!inMcpSubpage" class="filter-bar">
      <span class="filter-tag" :class="{ active: activeTab === 'builtin' }" @click="activeTab = 'builtin'">
        {{ t('settings.tools.builtin') }} ({{ builtinCount }})
      </span>
      <span class="filter-tag" :class="{ active: activeTab === 'dynamic' }" @click="activeTab = 'dynamic'">
        {{ t('settings.tools.dynamic') }} ({{ dynamicCount }})
      </span>
      <span class="filter-tag" :class="{ active: activeTab === 'mcp' }" @click="activeTab = 'mcp'">
        {{ t('settings.tools.mcp') }}
      </span>
    </div>

    <div class="page-body">
      <!-- Built-in tools -->
      <template v-if="activeTab === 'builtin'">
        <div v-if="builtinToolsForTab.length" class="tool-grid">
          <ToolCard
            v-for="tool in builtinToolsForTab"
            :key="tool.name"
            :tool="tool"
            @view="openSchema"
            @copy="copySchema"
          />
        </div>
        <a-empty v-else :description="t('settings.tools.noBuiltin')" />
      </template>

      <!-- Dynamic tools（按需加载）：独立页签 + 机制说明 -->
      <template v-if="activeTab === 'dynamic'">
        <p v-if="dynamicToolsForTab.length" class="dynamic-hint">
          {{ t('settings.tools.dynamicHint') }}
        </p>
        <div v-if="dynamicToolsForTab.length" class="tool-grid">
          <ToolCard
            v-for="tool in dynamicToolsForTab"
            :key="tool.name"
            :tool="tool"
            @view="openSchema"
            @copy="copySchema"
          />
        </div>
        <a-empty v-else :description="t('settings.tools.noDynamic')" />
      </template>

      <!-- MCP：服务器列表 → 工具卡片页 → 工具详情（全部页内，不弹窗） -->
      <template v-if="activeTab === 'mcp'">
        <!-- 工具详情：描述 + 参数 Schema 页内展开 -->
        <template v-if="inlineMcpTool">
          <div class="mcp-subpage">
            <div class="mcp-subpage-head">
              <a-button size="small" @click="backToMcpTools">
                <template #icon><icon-left :size="14" /></template>
                {{ t('settings.mcp.backToTools') }}
              </a-button>
              <span class="mcp-subpage-title">{{ inlineMcpTool.name }}</span>
              <span class="mcp-subpage-meta">
                <template v-if="activeMcpServer">{{ activeMcpServer.name }} · </template>
                {{ t('settings.tools.paramCount', { n: inlineMcpParamCount }) }}
              </span>
            </div>
            <p v-if="inlineMcpTool.description" class="mcp-tool-desc">
              {{ inlineMcpTool.description }}
            </p>
            <pre class="mcp-tool-schema">{{ JSON.stringify(inlineMcpTool.parametersSchema, null, 2) }}</pre>
            <div class="mcp-subpage-actions">
              <a-button size="small" @click="copySchema(inlineMcpTool)">
                {{ t('settings.tools.copySchema') }}
              </a-button>
            </div>
          </div>
        </template>

        <!-- 服务器工具卡片页 -->
        <template v-else-if="activeMcpServer">
          <div class="mcp-subpage">
            <div class="mcp-subpage-head">
              <a-button size="small" @click="backToMcpServers">
                <template #icon><icon-left :size="14" /></template>
                {{ t('settings.mcp.backToServers') }}
              </a-button>
              <span class="mcp-subpage-title">{{ activeMcpServer.name }}</span>
              <span class="server-path">{{ activeMcpServer.command || activeMcpServer.url }}</span>
            </div>

            <div v-if="activeMcpServerLoading" class="tools-loading">
              {{ t('common.loading') }}
            </div>
            <div v-else-if="activeMcpServerTools.length" class="tool-grid">
              <ToolCard
                v-for="tool in activeMcpServerTools"
                :key="tool.name"
                :tool="tool"
                selectable
                @select="openMcpTool"
                @copy="copySchema"
              />
            </div>
            <div v-else class="empty-state">
              <a-empty :description="t('settings.tools.noMcp')" />
            </div>
          </div>
        </template>

        <!-- 授权页：浏览器 OAuth / 手动填写 Token（全部页内，不弹窗） -->
        <template v-else-if="activeAuthServer">
          <div class="mcp-subpage">
            <div class="mcp-subpage-head">
              <a-button size="small" @click="backToMcpServers">
                <template #icon><icon-left :size="14" /></template>
                {{ t('settings.mcp.backToServers') }}
              </a-button>
              <span class="mcp-subpage-title">{{ activeAuthServer.name }}</span>
              <span class="server-path">{{ activeAuthServer.url }}</span>
            </div>

            <div class="mcp-auth">
              <section class="auth-section">
                <h3 class="auth-title">{{ t('settings.mcp.oauthTitle') }}</h3>
                <p class="auth-hint">{{ t('settings.mcp.oauthHint') }}</p>
                <div class="auth-row">
                  <a-button
                    size="small"
                    :loading="oauthStarting"
                    :disabled="oauthStatus === 'pending'"
                    @click="startOAuth"
                  >
                    <template #icon><icon-link :size="13" /></template>
                    {{ t('settings.mcp.openAuthPage') }}
                  </a-button>
                  <span v-if="oauthStatus === 'pending'" class="auth-status">
                    {{ t('settings.mcp.oauthWaiting') }}
                  </span>
                  <span v-else-if="oauthStatus === 'failed'" class="json-error auth-error">
                    {{ oauthError }}
                  </span>
                </div>
              </section>

              <div class="auth-divider" />

              <section class="auth-section">
                <h3 class="auth-title">{{ t('settings.mcp.manualTitle') }}</h3>
                <p class="auth-hint">{{ t('settings.mcp.manualHint') }}</p>
                <label class="auth-field">
                  <span>{{ t('settings.mcp.tokenLabel') }}</span>
                  <a-input-password
                    v-model="authToken"
                    :placeholder="t('settings.mcp.tokenPlaceholder')"
                    allow-clear
                  />
                </label>
                <div class="auth-header-row">
                  <label class="auth-field">
                    <span>{{ t('settings.mcp.headerName') }}</span>
                    <a-input
                      v-model="authHeaderName"
                      placeholder="X-MCP-Token"
                      allow-clear
                    />
                  </label>
                  <label class="auth-field auth-field-grow">
                    <span>{{ t('settings.mcp.headerValue') }}</span>
                    <a-input-password
                      v-model="authHeaderValue"
                      :placeholder="t('settings.mcp.headerValuePlaceholder')"
                      allow-clear
                    />
                  </label>
                </div>
                <div class="auth-row">
                  <a-button
                    type="primary"
                    size="small"
                    :loading="authSaving"
                    @click="saveManualAuth"
                  >
                    {{ t('settings.mcp.authSave') }}
                  </a-button>
                  <span class="auth-status">{{ t('settings.mcp.authWriteHint') }}</span>
                </div>
              </section>
            </div>
          </div>
        </template>

        <!-- 服务器列表 / 配置编辑（配置文件默认隐藏，编辑时整页进入） -->
        <template v-else>
          <div v-if="editingConfig" class="mcp-config-editor">
            <div class="mcp-config-editor-head">
              <span class="mcp-subpage-title">{{ t('settings.mcp.editConfigTitle') }}</span>
              <span class="mcp-subpage-meta">{{ t('settings.mcp.configFile') }}</span>
            </div>
            <div class="mcp-config-editor-body">
              <JsonEditorPane v-model="configDraft" />
            </div>
            <div class="mcp-config-editor-actions">
              <span v-if="configError" class="json-error">{{ configError }}</span>
              <a-button size="small" :disabled="!!configError" @click="formatConfig">
                {{ t('settings.mcp.formatConfig') }}
              </a-button>
              <a-button size="small" @click="closeConfigEditor">{{ t('common.cancel') }}</a-button>
              <a-button type="primary" size="small" :disabled="!!configError" @click="saveConfig">
                {{ t('common.save') }}
              </a-button>
            </div>
          </div>

          <template v-else>
            <div v-if="mcpStore.servers.length" class="tool-grid">
              <McpServerCard
                v-for="server in mcpStore.servers"
                :key="server.name"
                :server="server"
                :connecting="connectingServers.has(server.name)"
                :tool-count="toolsStore.getServerTools(server.name)?.length ?? 0"
                @open="openMcpServer(server)"
                @connect="handleConnect(server.name)"
                @disconnect="handleDisconnect(server.name)"
                @auth="openAuthPage(server)"
                @toggle="(val) => handleToggleEnabled(server.name, val)"
              />
            </div>
            <div v-else class="empty-state">
              <a-empty :description="t('settings.mcp.noServers')" />
            </div>
          </template>
        </template>
      </template>
    </div>

    <!-- 参数 Schema 弹窗：卡片只展示摘要，长 JSON 在这里查看/复制 -->
    <a-modal
      v-model:visible="schemaVisible"
      modal-class="mac-modal"
      :title="schemaTool?.name ?? ''"
      width="720px"
    >
      <pre class="schema-modal">{{ schemaText }}</pre>
      <template #footer>
        <a-button v-if="schemaTool" type="primary" @click="copySchema(schemaTool)">
          {{ t('settings.tools.copySchema') }}
        </a-button>
      </template>
    </a-modal>
  </div>
</template>

<style scoped>
.page {
  height: 100%;
  display: flex;
  flex-direction: column;
  padding: 28px 56px 16px;
}

.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 20px;
  flex-shrink: 0;
}

.header-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  flex-wrap: wrap;
  gap: 8px;
}

.page-title {
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
}

/* 页签筛选:macOS 分段控件(浅灰胶囊容器 + 选中白色胶囊) */
.filter-bar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  padding: 4px 6px;
  margin-bottom: 16px;
  flex-shrink: 0;
  background: var(--bg-2);
  border-radius: 10px;
  width: fit-content;
  max-width: 100%;
}

.filter-tag {
  font-size: var(--font-size-sm);
  padding: 5px 14px;
  line-height: 1.4;
  cursor: pointer;
  border-radius: 6px;
  color: var(--text-secondary);
  transition: all 0.15s ease;
  user-select: none;
}

.filter-tag:hover {
  color: var(--text-primary);
  background: var(--bg-3);
}

.filter-tag.active {
  color: var(--accent-text);
  background: var(--bg-base);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

.page-body {
  flex: 1;
  overflow: auto;
  /* 预占滚动条位置,内容增删时视图不晃动 */
  scrollbar-gutter: stable;
}

/* ── Tool grid (builtin/dynamic):与技能页同一套卡片网格 ── */
.tool-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 12px;
}

/* 动态工具页签顶部的机制说明（灰字小行） */
.dynamic-hint {
  margin: 0 0 12px;
  font-size: var(--font-size-code);
  color: var(--text-tertiary);
}

/* 参数 Schema 弹窗：等宽、限高滚动；长行横向滚动 */
.schema-modal {
  max-height: 58vh;
  margin: 0;
  padding: 12px 14px;
  overflow: auto;
  border-radius: 8px;
  background: var(--bg-2);
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: var(--font-size-code);
  line-height: 1.55;
  white-space: pre;
}

/* ── MCP 授权页：OAuth + 手动 Token ── */
.mcp-auth {
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-width: 680px;
  padding: 16px;
  border-radius: 12px;
  background: var(--fill-1);
}

.auth-title {
  margin: 0 0 6px;
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-title);
}

.auth-hint {
  margin: 0 0 12px;
  font-size: var(--font-size-sm);
  line-height: 1.6;
  color: var(--text-secondary);
}

.auth-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
}

.auth-status {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.auth-divider {
  height: 1px;
  background: var(--border-light);
}

.auth-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 12px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
}

.auth-field :deep(.arco-input-wrapper) {
  width: 100%;
}

.auth-header-row {
  display: flex;
  gap: 10px;
}

.auth-header-row .auth-field:first-child {
  width: 200px;
  flex-shrink: 0;
}

.auth-field-grow {
  flex: 1;
  min-width: 0;
}

.auth-error {
  max-width: 100%;
  white-space: normal;
  overflow-wrap: anywhere;
}

/* ── MCP 配置编辑页（配置文件默认不展示，编辑时整页进入） ── */
.mcp-config-editor {
  height: 100%;
  min-height: 360px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.mcp-config-editor-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.mcp-config-editor-body {
  flex: 1;
  min-height: 0;
  display: flex;
}

.mcp-config-editor-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  flex-shrink: 0;
}

.mcp-config-editor-actions .json-error {
  margin-right: auto;
}

.json-error {
  overflow: hidden;
  max-width: 70%;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--font-size-xs);
  color: var(--danger);
  font-family: var(--font-mono);
}

/* ---------- MCP 页内子页面：工具卡片页 / 工具详情 ---------- */
.mcp-subpage {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.mcp-subpage-head {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
  min-width: 0;
}

.mcp-subpage-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-title);
  white-space: nowrap;
}

.mcp-subpage-meta {
  font-size: var(--font-size-2xs);
  color: var(--text-tertiary);
}

.mcp-tool-desc {
  margin: 0;
  font-size: var(--font-size-sm);
  line-height: 1.6;
  color: var(--text-secondary);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.mcp-tool-schema {
  max-height: 55vh;
  margin: 0;
  padding: 12px 14px;
  overflow: auto;
  border-radius: 8px;
  background: var(--bg-2);
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: var(--font-size-code);
  line-height: 1.55;
  white-space: pre;
}

.mcp-subpage-actions {
  display: flex;
  justify-content: flex-end;
}

.server-path {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  font-family: monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex-shrink: 1;
  min-width: 0;
}

.tools-loading {
  padding: 20px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

.empty-state {
  padding: 40px 0;
}

/* ---------- macOS 风格控件覆盖 ---------- */

/* 主按钮(保存配置):macOS 系统蓝填充 + 白字 */
.page :deep(.arco-btn-primary) {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}

/* 按钮:macOS 圆角 */
.page :deep(.arco-btn) {
  border-radius: 6px;
}

/* 按钮图标:垂直居中 */
.page :deep(.arco-btn-icon) {
  display: inline-flex;
  align-items: center;
}

/* 搜索框:更柔和的圆角 */
.page :deep(.arco-input-wrapper),
.page :deep(.arco-input-outer) {
  border-radius: 8px;
}
</style>