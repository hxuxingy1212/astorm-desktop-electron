/**
 * 后端统一命令入口（backend:invoke）。
 *
 * 渲染层 store 经 services/backend 的 invoke(command, args) 调用这里：
 * - 技能 / 工具 / 定时任务 → 转发到 Rust napi 后端（binding methods，入参 snake_case）
 * - 应用设置（get/save_settings）→ 本地 backend-settings.json（安全配置独立 security.json）
 * - MCP 服务器管理（list / 配置读写 / 启停 / 连接）→ Rust 后端（astorm-core 的 mcp 模块，
 *   stdio 子进程常驻 + HTTP JSON-RPC；配置仍落 config/mcp_servers.json）
 *
 * 命令返回形状与 astorm-desktop 后端一致（camelCase），后端未加载时抛
 * `BACKEND_UNAVAILABLE` 前缀错误，渲染层据此降级 mock。
 */
import { ipcMain, shell } from 'electron'
import { join } from 'path'
import { readFile } from 'fs/promises'
import { napiCall } from './napi'
import { writeFileAtomic } from './atomicWrite'
import { clearBrowserCache, clearBrowserData, importChromeCookies } from './browser-data'
import { configDir } from './systemPaths'
import {
  readSecurity, readCompact, readGoal, readMemory, readSummary, readWorkspace, readInstructions,
  readWebSearch, readGlobalAgents, writeGlobalAgents, syncBackendConfig,
  securityFile, writeSecurityConfig, ensureSecurityConfigFile } from './settings-sync'
import {
  installPlugin,
  pluginDirFor,
  removePlugin,
  savePluginConfig,
  scanPlugins,
  setPluginEnabled
} from './plugins'

type Args = Record<string, unknown>

/* ---------- 应用路径(系统目录 config/,与 systemPaths 同源) ---------- */

function settingsFile(): string {
  return join(configDir(), 'backend-settings.json')
}

/* ---------- 应用设置（settings.json,Node 侧持久化） ---------- */

const DEFAULT_SETTINGS = {
  locale: 'zh-CN'
}

function readJsonFile(file: string): Promise<unknown | null> {
  return readFile(file, 'utf8')
    .then((text) => {
      const parsed = JSON.parse(text.replace(/^\uFEFF/, '')) as unknown
      return parsed && typeof parsed === 'object' ? parsed : null
    })
    .catch(() => null)
}

async function writeJsonFile(file: string, value: unknown): Promise<void> {
  await writeFileAtomic(file, JSON.stringify(value, null, 2))
}

/**
 * 读取设置:会话摘要 / 默认会话空间按后端配置同步(backend-settings.json 即上次推给后端的来源;
 * 安全配置单独落在 security.json);语言为前端偏好,仅本地回显,不进后端配置。主题色在前端 settings store。
 */
async function getSettings(): Promise<unknown> {
  const raw = ((await readJsonFile(settingsFile())) ?? {}) as Record<string, unknown>
  return {
    locale: typeof raw.locale === 'string' ? raw.locale : DEFAULT_SETTINGS.locale,
    summary: await readSummary(),
    compact: await readCompact(),
    goal: await readGoal(),
    webSearch: await readWebSearch(),
    workspace: await readWorkspace(),
    security: await readSecurity(),
    // 必须回显：各设置页用「读全量 → 改自己那段 → 写全量」的方式保存，
    // 这里漏掉哪段，别处保存一次就会把它从 settings.json 里静默抹掉
    memory: await readMemory(),
    instructions: await readInstructions()
  }
}

/** 保存设置:落盘后端配置来源 + 推给后端热更新(刷新后端);主题/语言不在推送内 */
async function saveSettings(args: Args): Promise<{ ok: boolean }> {
  const payload = { ...((args.settings ?? {}) as Record<string, unknown>) }
  // 安全配置独立成 security.json:各设置页「读全量 → 改自己那段 → 写全量」时,
  // 这里把 security 段拆走,避免它又被塞回 backend-settings.json 形成两份来源
  if (payload.security && typeof payload.security === 'object') {
    await writeSecurityConfig(payload.security)
  }
  delete payload.security
  await writeJsonFile(settingsFile(), payload)
  await syncBackendConfig()
  return { ok: true }
}

/** 打开安全配置的真实持久化文件;首次打开补齐默认值,不覆盖已配置名单。 */
async function openSecurityConfigFile(): Promise<void> {
  const path = await ensureSecurityConfigFile()
  const error = await shell.openPath(path)
  if (error) throw new Error(error)
}

/** 读取安全配置文件原文（文件不存在时先按当前生效值创建）。 */
async function readSecurityConfigFile(): Promise<{ path: string; content: string }> {
  const path = await ensureSecurityConfigFile()
  return { path, content: await readFile(path, 'utf8') }
}

/** 保存安全配置文件原文：必须是 JSON 对象；写盘后立即热更后端。 */
async function writeSecurityConfigFile(content: string): Promise<{ ok: true }> {
  let parsed: unknown
  try {
    parsed = JSON.parse(content.replace(/^\uFEFF/, ''))
  } catch (e) {
    throw new Error(`JSON 格式错误：${e instanceof Error ? e.message : String(e)}`)
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('安全配置必须是 JSON 对象')
  }
  await writeFileAtomic(securityFile(), content)
  await syncBackendConfig()
  return { ok: true }
}

/* ---------- MCP 服务器（实现在 Rust 后端：astorm-core 的 mcp 模块） ---------- */

// 客户端（stdio 子进程常驻 / HTTP JSON-RPC + 工具发现与调用）在 core：桌面与云容器同一实现，
// 工具连接后直接进入会话工具集。本进程只转发命令，不再自建握手（历史实现握手后即杀进程，
// 工具只展示不可调用）。
async function mcpServers(): Promise<unknown> {
  return napiCall('mcpListServers')
}

async function readMcpConfigFile(): Promise<unknown> {
  return napiCall('mcpReadConfigFile')
}

async function writeMcpConfigFile(args: Args): Promise<unknown> {
  await napiCall('mcpWriteConfigFile', String(args.content ?? ''))
  return { ok: true }
}

async function toggleMcpServer(args: Args): Promise<unknown> {
  await napiCall('mcpToggleServer', String(args.name ?? ''), Boolean(args.enabled))
  return { ok: true }
}

async function connectMcpServer(args: Args): Promise<unknown> {
  return napiCall('mcpConnectServer', String(args.name ?? ''))
}

async function disconnectMcpServer(args: Args): Promise<unknown> {
  await napiCall('mcpDisconnectServer', String(args.name ?? ''))
  return { ok: true }
}

async function setMcpServerAuth(args: Args): Promise<unknown> {
  return napiCall('mcpSetServerAuth', String(args.name ?? ''), String(args.headers ?? '{}'))
}

async function beginMcpOAuth(args: Args): Promise<unknown> {
  return napiCall('mcpBeginOauth', String(args.name ?? ''))
}

async function mcpOAuthStatus(args: Args): Promise<unknown> {
  return napiCall('mcpOauthStatus', String(args.name ?? ''))
}

/* ---------- 命令路由 ---------- */

const NAPI_COMMANDS: Record<string, (a: Args) => unknown> = {
  // 本地文件浏览:目录树(list_directory 递归) + 文件内容读取(1MB/二进制守卫)
  list_directory: (a) => napiCall('listDirectory', a.dir),
  builtin_danger_patterns: () => napiCall('builtinDangerPatterns'),
  /** 安全配置的**后端默认值**（凭据黑名单等动态默认，渲染层取它做初值/重置） */
  security_defaults: () => napiCall('securityDefaults'),
  read_audit_log: (a) => napiCall('readAuditLog', a.session_id),
  // 使用统计:按天×模型聚合 token 用量(since_ts 为 unix 秒下限;返回 JSON 字符串)
  usage_stats: (a) => napiCall('usageStats', a.since_ts),
  // 会话交付物:submit_deliverables 工具逐轮落库的记录(按轮次升序;返回 JSON 字符串)
  session_deliverables: (a) => napiCall('listSessionDeliverables', a.session_id),
  // 跨会话交付物(按更新时间倒序限量;「交付物」页跨会话分组展示;返回 JSON 字符串)
  all_deliverables: (a) => napiCall('listAllDeliverables', a.limit ?? undefined),
  // 本地编程环境探测(设置页「本地环境」;无会话、纯函数)
  detect_local_env: () => napiCall('detectLocalEnv'),
  // 沙箱自动注入白名单报告(本地环境注入开关 + 安全配置只读展示)
  toolchain_allowances: () => napiCall('toolchainAllowances'),
  read_file_content: (a) => napiCall('readFileContent', a.path),
  // 终端:交互式 PTY shell(前端 xterm.js;输出经 napi:event 流回传)
  pty_open: (a) => napiCall('ptyOpen', a.session_id, a.cols ?? 80, a.rows ?? 24),
  pty_write: (a) => napiCall('ptyWrite', a.session_id, a.data),
  pty_resize: (a) => napiCall('ptyResize', a.session_id, a.cols, a.rows),
  pty_close: (a) => napiCall('ptyClose', a.session_id),
  // git 工作区状态(文件树「查看变更」过滤用)
  git_status: (a) => napiCall('gitStatus', a.dir),
  // git 初始化仓库(空状态「创建 Git 仓库」按钮)
  git_init: (a) => napiCall('gitInit', a.dir),
  // git 忽略路径列表(文件树淡色渲染用)
  git_ignored: (a) => napiCall('gitIgnored', a.dir),
  // git 差异摘要(代码审查页签的文件列表:状态字 + 增删计数;staged 区分暂存/未暂存)
  git_diff_summary: (a) => napiCall('getGitDiffSummary', a.dir, a.staged),
  // git 单文件差异全文 + 新侧总行数(代码审查页签展开某文件时按需取)
  git_file_diff: (a) => napiCall('getGitFileDiff', a.dir, a.path, a.staged),
  // 任务规划(会话大纲;详请规划文档经提示词注入,不由接口获取)
  list_plan_files: (a) => napiCall('listPlanFiles', a.session_id),
  read_plan_file: (a) => napiCall('readPlanFile', { sessionId: a.session_id, taskId: a.task_id }),
  // skills（napi-rs 方法名：Rust snake_case → camelCase）
  list_skills: (a) => napiCall('listSkills', a.enabledOnly ?? undefined, a.category ?? undefined),
  search_skills: (a) => napiCall('searchSkills', a.query, a.limit ?? undefined),
  get_skill: (a) => napiCall('getSkill', a.name),
  create_skill: (a) => napiCall('createSkill', a.req),
  update_skill: (a) => napiCall('updateSkill', a.name, a.updates),
  toggle_skill: (a) => napiCall('toggleSkill', a.name, a.enabled),
  remove_skill: (a) => napiCall('removeSkill', a.name),
  discover_skills: () => napiCall('discoverSkills'),
  import_skills: (a) => napiCall('importSkills', a.paths),
  import_skill_package: (a) => napiCall('importSkillPackage', a.path),
  // agents（智能体卡片：设置页「智能体」增删改查；内置卡片由后端拒绝编辑/删除）
  list_agents: () => napiCall('listAgents'),
  get_agent: (a) => napiCall('getAgent', a.id),
  create_agent: (a) => napiCall('createAgent', a.req),
  update_agent: (a) => napiCall('updateAgent', a.id, a.updates),
  remove_agent: (a) => napiCall('removeAgent', a.id),
  // tools
  list_tools: (a) => napiCall('listTools', a.category ?? undefined),
  search_tools: (a) => napiCall('searchTools', a.query, a.limit ?? undefined),
  // 邮件（napi-rs 方法名：Rust snake_case → camelCase）
  mail_state: () => napiCall('mailState'),
  mail_add_account: (a) => napiCall('mailAddAccount', a),
  mail_update_account: (a) => napiCall('mailUpdateAccount', a.name, a.input),
  mail_remove_account: (a) => napiCall('mailRemoveAccount', a.name),
  mail_activate_account: (a) => napiCall('mailActivateAccount', a.name),
  mail_folders: (a) => napiCall('mailFolders', a.account ?? undefined),
  mail_folder_counts: (a) => napiCall('mailFolderCounts', a.account ?? undefined),
  mail_messages: (a) => napiCall('mailMessages', a),
  mail_previews: (a) => napiCall('mailPreviews', a.folder ?? undefined, a.account ?? undefined, a.uids ?? []),
  mail_message: (a) =>
    napiCall('mailMessage', a.uid, a.folder ?? undefined, a.account ?? undefined, a.refresh ?? undefined),
  mail_set_meta: (a) => napiCall('mailSetMeta', a.uid, a.folder ?? undefined, a.account ?? undefined, a.body),
  mail_move: (a) => napiCall('mailMove', a.uid, a.folder ?? undefined, a.account ?? undefined, a.dest),
  mail_attachment: (a) =>
    napiCall('mailAttachment', a.uid, a.index, a.folder ?? undefined, a.account ?? undefined),
  mail_attachment_to_temp: (a) =>
    napiCall('mailAttachmentToTemp', a.uid, a.index, a.folder ?? undefined, a.account ?? undefined),
  mail_send: (a) => napiCall('mailSend', a),
  // 知识库（napi-rs 方法名：Rust snake_case → camelCase；真实文件系统 CRUD）
  kb_list: () => napiCall('kbList'),
  kb_create: (a) => napiCall('kbCreate', a.req),
  kb_rename: (a) => napiCall('kbRename', a.path, a.new_name),
  kb_delete: (a) => napiCall('kbDelete', a.path),
  kb_update_meta: (a) => napiCall('kbUpdateMeta', a.meta),
  kb_read: (a) => napiCall('kbRead', a.path),
  kb_read_buffer: (a) => napiCall('kbReadBuffer', a.path),
  kb_write: (a) => napiCall('kbWrite', a.path, a.content),
  kb_import: (a) => napiCall('kbImport', a.parent, a.paths),
  // scheduled tasks（napi DTO 输入同样为 camelCase：cronExpr / workspacePath，与前端请求体一致）
  list_scheduled_tasks: () => napiCall('listScheduledTasks'),
  generate_scheduler_workspace: () => napiCall('generateSchedulerWorkspace'),
  add_scheduled_task: (a) => napiCall('addScheduledTask', a),
  update_scheduled_task: (a) => napiCall('updateScheduledTask', a),
  delete_scheduled_task: (a) => napiCall('deleteScheduledTask', a.id),
  toggle_scheduled_task: (a) => napiCall('toggleScheduledTask', a.id, a.enabled),
  run_scheduled_task_now: (a) => napiCall('runScheduledTaskNow', a.id),
  // 页签状态上报(tabs 工具的镜像;渲染层在页签变化时推送,snake_case JSON 字符串)
  update_tabs_state: (a) => napiCall('updateTabsState', a.session_id, a.state_json),
  // 长期记忆（napi-rs 方法名：Rust snake_case → camelCase）
  // 只读查询：概念列表直接读盘（索引未建也能列出），检索索引优先、缺失时子串兜底
  memory_root: () => napiCall('memoryRoot'),
  memory_status: () => napiCall('memoryStatus'),
  memory_list_concepts: () => napiCall('memoryListConcepts'),
  memory_show_concept: (a) => napiCall('memoryShowConcept', a.concept_id),
  memory_search: (a) => napiCall('memorySearch', a.query, a.limit ?? undefined),
  memory_validate: () => napiCall('memoryValidate'),
  memory_read_log: (a) => napiCall('memoryReadLog', a.limit ?? undefined),
  memory_list_raw_sources: () => napiCall('memoryListRawSources'),
  memory_read_raw_source: (a) => napiCall('memoryReadRawSource', a.stem),
  // 编译触发/取消/重建索引（编译为异步：进度与结果走 napi:event 的 type:"memory"）
  memory_compile: (a) => napiCall('memoryCompile', a.force ?? undefined, a.dry_run ?? undefined),
  memory_cancel: () => napiCall('memoryCancel'),
  memory_reindex: () => napiCall('memoryReindex')
}

const NODE_COMMANDS: Record<string, (a: Args) => Promise<unknown>> = {
  get_settings: () => getSettings(),
  security_config_file: async () => ({ path: securityFile() }),
  open_security_config_file: () => openSecurityConfigFile(),
  read_security_config_file: () => readSecurityConfigFile(),
  write_security_config_file: (a) => writeSecurityConfigFile(String(a.content ?? '')),
  save_settings: (a) => saveSettings(a),
  // 约定文件（AGENTS.md）：全局那份是用户目录下的真实文件，供设置页「全局约定」读写
  // （会话空间那份是工作区里的普通文件，由后端直接读取，不在此维护）
  read_global_agents: () => readGlobalAgents(),
  write_global_agents: (a) => writeGlobalAgents(String(a.content ?? '')),
  list_mcp_servers: () => mcpServers(),
  read_mcp_config_file: () => readMcpConfigFile(),
  write_mcp_config_file: (a) => writeMcpConfigFile(a),
  toggle_mcp_server: (a) => toggleMcpServer(a),
  connect_mcp_server: (a) => connectMcpServer(a),
  disconnect_mcp_server: (a) => disconnectMcpServer(a),
  set_mcp_server_auth: (a) => setMcpServerAuth(a),
  begin_mcp_oauth: (a) => beginMcpOAuth(a),
  mcp_oauth_status: (a) => mcpOAuthStatus(a),
  // MCP 工具列表来自后端（core 的 mcp 模块；未连接的服务器返回空）
  list_mcp_server_tools: async (a) => napiCall('listMcpServerTools', String(a.server ?? '')),
  // 插件系统（plugin.json 清单 + userData/plugins 目录 + plugins.json 注册表）
  plugin_scan: () => scanPlugins(),
  plugin_install: (a) => installPlugin(String(a.dir ?? '')),
  plugin_remove: (a) => removePlugin(String(a.id ?? '')),
  plugin_set_enabled: (a) => setPluginEnabled(String(a.id ?? ''), Boolean(a.enabled)),
  plugin_save_config: (a) =>
    savePluginConfig(String(a.id ?? ''), (a.config ?? {}) as Record<string, unknown>),
  plugin_dir: (a) => Promise.resolve(pluginDirFor(String(a.id ?? ''))),
  // 内置浏览器数据(设置页「自动化控制」):Chrome Cookie 由 Rust 解密,再由本进程写入会话
  import_chrome_cookies: () => importChromeCookies(),
  clear_browser_cache: () => clearBrowserCache().then(() => ({ ok: true })),
  clear_browser_data: () => clearBrowserData()
}

/** 注册 backend:invoke（渲染层 services/backend 的唯一调用入口）。 */
export function registerBackendIpc(): void {
  ipcMain.handle('backend:invoke', async (_e, command: string, args?: Args): Promise<unknown> => {
    const a: Args = args ?? {}
    if (NODE_COMMANDS[command]) return NODE_COMMANDS[command](a)
    if (NAPI_COMMANDS[command]) return NAPI_COMMANDS[command](a)
    throw new Error(`unknown backend command: ${command}`)
  })
}