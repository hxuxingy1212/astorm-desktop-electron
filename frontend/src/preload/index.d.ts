export interface WindowApi {
  /**
   * 连接方式（本机 / 云端）：壳子启动时「去哪找后端」。
   * `info` 在 preload 加载期同步取值（`welcome` = 本次加载的是欢迎页）。
   */
  shellConnection: {
    info: {
      mode: 'local' | 'cloud' | 'unset'
      endpoint: string
      hasToken: boolean
      welcome: boolean
      /** 是否完成过引导（启动直接进应用的全局标记） */
      onboarded: boolean
    }
    /** 探测端点。失败回 code（渲染层本地化）；成功回模型 / 数据目录等结构化字段 */
    test: (
      endpoint: string,
      token: string
    ) => Promise<{
      ok: boolean
      code?: 'endpointInvalid' | 'unauthorized' | 'notAstorm' | 'timeout' | 'unreachable' | 'http'
      message?: string
      status?: number
      detail?: string
      model?: string
      dataDir?: string
    }>
    save: (patch: {
      mode: 'local' | 'cloud'
      endpoint?: string
      token?: string
      onboarded?: boolean
    }) => Promise<{
      mode: 'local' | 'cloud' | 'unset'
      endpoint: string
      hasToken: boolean
      onboarded: boolean
    }>
    openWelcome: () => Promise<void>
    /** 已配置过时从引导页返回当前连接形态（不重新落盘） */
    closeWelcome: () => Promise<void>
  }
  shell: {
    /** 系统浏览器打开 http(s) 链接（MCP OAuth 授权页）；返回 '' 成功，否则错误信息 */
    openExternal: (url: string) => Promise<string>
  }
  /** 应用系统目录（config/ data/ log/ plugins/ 等的父目录） */
  system: {
    appDir: () => Promise<string>
  }
  window: {
    minimize: () => void
    toggleMaximize: () => void
    close: () => void
    isMaximized: () => Promise<boolean>
    onMaximizedChange: (callback: (maximized: boolean) => void) => () => void
    isFullscreen: () => Promise<boolean>
    onFullscreenChange: (callback: (fullscreen: boolean) => void) => () => void
  }
  zoom: {
    setFactor: (factor: number) => void
  }
  appearance: {
    /** 原生侧外观跟随应用主题(macOS 红黄绿失焦色、原生菜单与弹窗;见 main/ipc.ts) */
    setThemeMode: (mode: 'light' | 'dark' | 'system') => void
  }
  office: {
    /** office/pdf 高保真预览:CLI unpack --inline 单文件产物(带缓存;失败返回 ok:false + message) */
    preview: (filePath: string) => Promise<OfficePreviewResult>
  }
  browser: {
    /** 打开浏览器页签 iframe 的调试控制台;返回 true = 已打开 iframe 自身控制台 */
    openDevtools: (url: string) => Promise<boolean>
    /** 选择元素:主进程向 iframe 注入拾取脚本,用户点击后返回元素信息 */
    pickElement: (url: string, options: ElementPickerOptions) => Promise<ElementPickResult>
    /** 取消进行中的拾取(页面内 Esc 同样可取消) */
    cancelPick: (url: string) => Promise<boolean>
    /** 页签截图:rect = iframe 可视区域的视口坐标 CSS px;主进程 capturePage 后落盘,返回绝对路径 */
    capture: (rect: CaptureRect, opts: BrowserCaptureOptions) => Promise<BrowserCaptureResult>
    /** 录制取一帧:同一矩形,返回 JPEG 字节(不落盘);由渲染层按帧率驱动并编码 */
    frame: (rect: CaptureRect) => Promise<BrowserFrameResult>
    /** 录像落盘:把 MediaRecorder 产出的 webm 字节交回主进程写文件 */
    recordSave: (payload: BrowserRecordSavePayload) => Promise<BrowserCaptureResult>
  }
  /** AI 浏览器代理操控(一会话一桥,多目标 Target 域) */
  browserAgent: {
    /** 上报某会话的全部浏览器页签(含非激活);页签为空 → 停桥 */
    setTabs: (payload: {
      sessionId: string
      tabs: BrowserAgentTabInfo[]
      activeId: string
    }) => Promise<BrowserAgentStatus>
    /** 订阅主进程页签命令(create/close/activate);返回取消订阅函数 */
    onCommand: (callback: (cmd: BrowserAgentCommand) => void) => () => void
    /** 回执命令执行结果 */
    commandResult: (payload: {
      requestId: string
      ok: boolean
      tabId?: string
      message?: string
    }) => Promise<boolean>
    status: () => Promise<BrowserAgentStatus>
    stop: () => Promise<BrowserAgentStatus>
  }
  dialog: {
    openDirectory: () => Promise<string>
    /** 多选文件或文件夹(知识库导入等);取消返回空数组 */
    openFiles: () => Promise<string[]>
    /** 单选 JSON 文件并读回文本(MCP 配置导入);取消返回 null */
    openJson: () => Promise<{ path: string; content: string } | null>
    /** 另存为(导出):返回保存路径,取消返回空串 */
    saveFile: (defaultName: string) => Promise<string>
    /** 单选 .pptx(内置插件 ppt-inspector 导入);defaultPath 为初始目录,取消返回空串 */
    openPptx: (defaultPath?: string) => Promise<string>
    /** 多选附件(输入框「+」):不限类型,图片也在其中(按内容识别);取消返回空数组 */
    openAttachments: () => Promise<string[]>
  }
  /** 用户消息附件:图片 staging 落盘副本;其他文件不落盘、原样引用原始路径 */
  attachment: {
    /** 本地路径 → 图片 staging / 其他文件原样引用;返回成功项描述与逐项失败原因 */
    stagePaths: (paths: string[]) => Promise<StageResult>
    /** 二进制(拖拽的无路径 File / 已读出的字节)→ staging */
    stageBytes: (bytes: Uint8Array, name: string) => Promise<StageResult>
    /** 粘贴截图:主进程读取系统剪贴板图片 → staging */
    stageClipboard: () => Promise<StageResult>
    /** 拖拽文件取真实路径(Electron 43 已移除 File.path,经 webUtils 桥接);取不到返回 '' */
    pathForFile: (file: File) => string
    /** 附件绝对路径 → astorm-attachment:// URL(缩略图 / 大图直读) */
    toUrl: (path: string) => string
  }
  /** 内置 PPT 解包(ppt-inspector 插件):.pptx → 产物 deck JSON */
  pptx: {
    unpack: (
      file: string
    ) => Promise<{ ok: boolean; file?: string; deck?: unknown; message?: string }>
  }
  fs: {
    /** 用系统文件管理器打开目录;返回 '' 成功,否则错误信息 */
    openFolder: (dir: string) => Promise<string>
    /** 读文件为 base64(图片预览;8MB 上限,失败/超限返回空) */
    readFileBase64: (path: string) => Promise<string>
    /** 读文件为 ArrayBuffer(office 等二进制预览;64MB 上限,失败/超限返回 null) */
    readArrayBuffer: (path: string) => Promise<ArrayBuffer | null>
    /** 写文本文件(markdown 编辑保存) */
    writeTextFile: (path: string, content: string) => Promise<{ ok: boolean; message?: string }>
    /** 浏览器部署专用:单层列目录(仅目录,范围=数据根;缺省起点=工作空间根) */
    listDirs?: (path?: string) => Promise<{
      path: string
      parent: string | null
      root: string
      dirs: { name: string; path: string }[]
    }>
    /** 浏览器部署专用:在选择器里新建文件夹(数据根内,单层名) */
    mkdir?: (parent: string, name: string) => Promise<{ path: string }>
  }
  git: {
    currentBranch: (dir: string) => Promise<string>
    /** 目录是否位于 git 工作树内(空仓库也算;非仓库 false) */
    isRepo: (dir: string) => Promise<boolean>
    listBranches: (dir: string) => Promise<string[]>
    /** 分支菜单状态:当前分支 + 本地/远程分支 + 未提交文件数(非仓库全空) */
    branchState: (dir: string) => Promise<GitBranchState>
    /** 当前分支的提交历史(拓扑序) */
    log: (dir: string, branch: string, limit?: number) => Promise<{ commits: GitCommit[]; error?: string }>
    /** 切换分支(失败时 error 为 git 原始 stderr) */
    checkout: (dir: string, branch: string) => Promise<{ ok: boolean; error?: string }>
    /** 创建并检出分支 */
    createBranch: (dir: string, name: string) => Promise<{ ok: boolean; error?: string }>
  }
  providers: {
    list: () => Promise<{ providers: ProviderDto[]; encryptionAvailable: boolean }>
    sync: (providers: ProviderDto[]) => Promise<{ providers: ProviderDto[] }>
    test: (name: string, model: string) => Promise<{ ok: boolean; message: string }>
  }
  /** 单次 LLM 补全(插件 SDK llm.chat 通路);返回退订函数,事件经回调流式到达 */
  llm: {
    ask: (
      request: unknown,
      onChunk: (payload: { callId: string; chunk: { type?: 'chunk' | 'reasoning' | 'end' | 'error'; delta?: string; message?: string } }) => void
    ) => () => void
  }
  /** 聊天(仅在加载到 Rust napi 后端时存在;缺席则前端回退 Mock) */
  chat?: {
    createSession: (request: unknown) => Promise<string>
    sendMessage: (sessionId: string, request: unknown) => Promise<void>
    /** 启动「仅创建」的会话:以已有历史(末尾那条指令)驱动一轮,不追加新的用户消息 */
    startSession: (sessionId: string) => Promise<void>
    cancel: (sessionId: string) => Promise<void>
    /** 撤回历史:保留前 keepUserMessages 条用户消息,删除其后全部(内存+磁盘),并中断当前循环 */
    rewindToUserMessage: (sessionId: string, keepUserMessages: number) => Promise<void>
    /** 基于已有会话 fork 出新会话(共用工作区),返回新会话 id；beforeFinal 见后端契约 */
    forkSession: (sessionId: string, beforeFinal?: number) => Promise<string>
    /** 手动触发一次会话摘要(/compact,绕过阈值);返回是否完成摘要 */
    summarizeSession: (sessionId: string) => Promise<boolean>
    /** 更新会话级设置(模式/审批/思考强度/模型/供应商):不必发消息,下一个工具调用即生效 */
    updateSessionSettings: (sessionId: string, request: unknown) => Promise<void>
    /** 设置/修改/删除会话目标(/goal):goal 为 null/空白 = 删除,不必发消息 */
    updateSessionGoal: (
      sessionId: string,
      goal: string | null,
      goalLang?: string
    ) => Promise<void>
    /** 立即发送:进入后端打断消息队列并取消当前循环优先执行 */
    interruptMessage: (sessionId: string, request: unknown) => Promise<void>
    /** 中断决议:审批同意/拒绝、表单回答/忽略(决议即 tool 结果,不追加用户消息;失效请求 reject) */
    submitInterrupt: (sessionId: string, request: unknown) => Promise<void>
    /** 重开提问:被忽略/被取代的提问用原表单定义重新登记待决中断(表单卡重新弹出;运行中/已有待决时 reject) */
    reopenInterrupt: (sessionId: string, toolCallId: string) => Promise<void>
    listSessions: () => Promise<unknown[]>
    listMessages: (sessionId: string) => Promise<unknown[]>
    /** 归档分页(「加载更早」):空数组 = 已到归档起点 */
    archivedBefore: (
      sessionId: string,
      beforeSeq: number,
      maxUserTurns?: number
    ) => Promise<unknown[]>
    /** 会话系统提示词(按当前配置实时拼装,不落盘;供「会话轨迹」展示) */
    getSystemPrompt: (sessionId: string) => Promise<string>
    /** 删除会话:后端清理 sqlite 元数据 + jsonl 会话目录 */
    deleteSession: (sessionId: string) => Promise<void>
    onEvent: (callback: (chunk: unknown) => void) => () => void
  }
  /** 设置页后端命令(技能/工具/定时任务 → napi,设置/MCP → 主进程);无 .node 时前端回退 Mock */
  backend: {
    invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>
    /** 订阅后端全局事件流(napi:event;聊天气泡/终端输出等按 type 分发) */
    onEvent: (callback: (chunk: unknown) => void) => () => void
  }
  /** 跨平台系统权限(主进程 systemPreferences;非 macOS 多为 unsupported,引导到系统设置) */
  permission: {
    status: () => Promise<PermissionItem[]>
    /** 主动请求授权(仅 macOS 麦克风/摄像头),返回刷新后的状态 */
    request: (kind: PermissionKind) => Promise<PermissionItem[]>
    /** 打开系统设置对应隐私页;返回是否成功 */
    openSettings: (kind: PermissionKind) => Promise<boolean>
  }
  /** 电源管理(桌面端全局):阻止系统因空闲休眠 */
  power: {
    /** 设置「保持电脑运行」;开启后阻止系统空闲休眠(手动睡眠/合盖仍生效),返回是否处于抑制中 */
    setKeepAwake: (enabled: boolean) => Promise<boolean>
  }
  versions: {
    electron: string
    chrome: string
    node: string
  }
  /** 当前运行平台('darwin'/'win32'/'linux'…):macOS 用系统红黄绿,其余平台渲染层自绘窗口按钮 */
  platform: string
  /** 是否支持 `astorm-icon://` 本地图标缓存协议（Electron preload 注入；浏览器/预览无此能力） */
  iconProtocol?: boolean
  /** 浏览器部署标记(web-bridge 桥接注入):无 Electron 窗口装饰,桌面独有设置页隐藏;桌面端为 undefined */
  __web?: boolean
}

/** 系统权限项标识(与 src/main/permissions.ts 保持一致) */
export type PermissionKind = 'microphone' | 'camera' | 'screen' | 'accessibility' | 'fullDisk'

/** 权限状态:macOS 由系统查询;其他平台无统一接口 → unsupported */
export type PermissionState =
  | 'granted'
  | 'denied'
  | 'not-determined'
  | 'restricted'
  | 'unknown'
  | 'unsupported'

export interface PermissionItem {
  kind: PermissionKind
  state: PermissionState
  /** 是否支持主动弹窗请求(仅 macOS 麦克风/摄像头) */
  canRequest: boolean
  /** 是否能跳转到系统设置对应页 */
  canOpenSettings: boolean
}

/** 已 staging 的图片附件(见 src/main/attachmentFiles.ts) */
export interface StagedAttachment {
  /** staging 绝对路径(随消息下发给后端) */
  path: string
  /** 原始文件名(展示用) */
  name: string
  size: number
  mime: string
}

/** staging 结果:成功项 + 逐项失败原因(前端即时提示) */
export interface StageResult {
  ok: StagedAttachment[]
  failed: { name: string; reason: string }[]
}

/** 供应商配置 DTO:与主进程 providers.ts / 契约文档 §2 保持一致 */
export interface ProviderModelDto {
  name: string
  order?: number
}/** 拾取选项:页面内提示条文案 + 悬停卡片字段标签(主进程无 i18n,由渲染层传入) */
export interface ElementPickerOptions {
  hint?: string
  labels?: Record<string, string>
}

/** 截图矩形:渲染层视口坐标下的 CSS px(见 src/main/browser-capture.ts) */
export interface CaptureRect {
  x: number
  y: number
  width: number
  height: number
}

/** 截图附加信息(见 src/main/browser-capture.ts) */
export interface BrowserCaptureOptions {
  /** 网页页签 id,仅用于文件名(主进程做字母数字截断) */
  tabId?: string
  /** 绝对工作区目录;产物写入其下 `.astorm/browser-capture/`;缺省写到系统图片目录 */
  workspaceDir?: string
}

/** 截图结果:成功给绝对路径,失败给原因('BAD_RECT' | 'EMPTY_AREA' | 'CAPTURE_EMPTY' 为约定码) */
export interface BrowserCaptureResult {
  ok: boolean
  path?: string
  message?: string
}

/** 单帧结果(录像用):JPEG 字节 + 像素尺寸(见 src/main/browser-capture.ts) */
export interface BrowserFrameResult {
  ok: boolean
  data?: Uint8Array
  width?: number
  height?: number
  message?: string
}

/** 录像落盘入参:webm 字节 + 归属信息 */
export interface BrowserRecordSavePayload {
  data: Uint8Array
  tabId?: string
  workspaceDir?: string
}

/** 悬停卡片的一行(key = inspect.* 字段名;值与页面内卡片同源) */
export interface PickCardRow {
  key: string
  value: string
  swatch?: string
  sep?: boolean
  path?: boolean
}

/** office/pdf 预览结果(见 src/main/officePreview.ts;data 直接作 OfficeViewer 的 data prop) */
export interface OfficePreviewResult {
  ok: boolean
  kind?: 'docx' | 'xlsx' | 'pptx' | 'pdf'
  data?: unknown
  message?: string
}

/** 元素拾取结果(见 src/main/element-picker.ts;渲染层镜像类型为 PickedElementData) */
export interface ElementPickResult {
  ok: boolean
  data?: {
    url: string
    tagName: string
    id: string
    classList: string[]
    xpath: string
    cssSelector: string
    text: string
    component?: {
      name: string
      framework: 'vue2' | 'vue3' | 'react'
      file?: string
      line?: number
      column?: number
    }
    /** 悬停卡片:与页面内高亮卡片同一模型 */
    card: { name: string; size: string; rows: PickCardRow[] }
  }
  /** 用户主动取消(Esc / 再次点击按钮) */
  canceled?: boolean
  /** 'NO_FRAME' | 'EMPTY_URL' 为约定码,其余为注入/页面异常原文 */
  message?: string
}

/** 分支菜单状态(git:branch-state) */
export interface GitBranchState {
  /** 当前分支(分离 HEAD 时为字面量 "HEAD") */
  current: string
  /** 本地分支(refs/heads) */
  local: string[]
  /** 远程分支(refs/remotes,如 origin/master;含 origin/HEAD) */
  remotes: string[]
  /** 未提交的文件数(含未跟踪) */
  uncommitted: number
}

/** 提交行(git:log);`local`/`remotes` 用于把同名分支区分成「本地/远程」徽章 */
export interface GitCommit {
  /** 短 hash */
  hash: string
  author: string
  /** ISO 时间(用于格式化显示) */
  dateISO: string
  /** git 的相对时间(如 "2 hours ago") */
  dateRel: string
  subject: string
  /** ref 装饰(`%D` 拆开;如 ["HEAD -> master", "origin/master"] 或 ["tag: v1.0"]） */
  decorations: string[]
}

export interface ProviderDto {
  name: string
  baseUrl: string
  apiKey: string
  /** API 协议(chat_completions / responses / anthropic_messages) */
  apiFormat?: 'chat_completions' | 'responses' | 'anthropic_messages'
  models: ProviderModelDto[]
  order?: number
  storage?: string
}

/** AI 浏览器代理操控状态(主进程 browser-agent 管理器) */
export interface BrowserAgentStatus {
  running: boolean
  endpoint?: {
    /** 形如 ws://127.0.0.1:53421/astorm/<token>(token 已含在 URL 里) */
    wsUrl: string
    token: string
    frameUrl: string
    frameId: string
    tabId?: string
    /** 端点所属的 astorm 会话 id(工具据此校验会话一致) */
    sessionId?: string
  }
  /** 启动失败原因(如 DevTools 打开导致 debugger 互斥) */
  reason?: string
  /** 端点文件绝对路径(供给 Rust 工具 / provider 插件读取) */
  endpointFile: string
}

/** 浏览器页签信息(上报主进程;targetId = 页签 id,iframe `name` 同名) */
export interface BrowserAgentTabInfo {
  id: string
  url: string
  title?: string
  active: boolean
}

/** 主进程下发的浏览器页签命令 */
export interface BrowserAgentCommand {
  requestId: string
  action: 'create' | 'close' | 'activate'
  url?: string
  tabId?: string
}

declare global {
  interface Window {
    api: WindowApi
  }
}

export {}
