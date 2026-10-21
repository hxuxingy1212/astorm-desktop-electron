import { contextBridge, ipcRenderer, webUtils } from 'electron'

/** 附件绝对路径 → astorm-attachment:// URL(各段编码后按路径分隔符拼回)。
 *  仅 staging 目录与会话工作区 .astorm/attachment/ 下的路径可读(主进程协议侧校验)。 */
function attachmentUrl(absPath: string): string {
  // UNC（\\server\share\...）按分隔符切分会丢掉前导 \\；用显式标记保住。
  const unc = /^[\\/]{2}/.test(absPath)
  const segs = absPath
    .split(/[\\/]+/)
    .filter(Boolean)
    .map((s) => encodeURIComponent(s))
  return `astorm-attachment://file/${unc ? '__UNC__/' : ''}${segs.join('/')}`
}

/** 拾取选项:页面内提示条文案 + 悬停卡片字段标签(主进程无 i18n,由渲染层传入) */
interface ElementPickerOptions {
  hint?: string
  labels?: Record<string, string>
}

/** 截图矩形:渲染层视口坐标下的 CSS px(与 src/main/browser-capture.ts 保持一致) */
interface CaptureRect {
  x: number
  y: number
  width: number
  height: number
}

/** 跨平台系统权限项标识(与 src/main/permissions.ts 保持一致) */
type PermissionKind = 'microphone' | 'camera' | 'screen' | 'accessibility' | 'fullDisk'
type PermissionState =
  | 'granted'
  | 'denied'
  | 'not-determined'
  | 'restricted'
  | 'unknown'
  | 'unsupported'

interface PermissionItem {
  kind: PermissionKind
  state: PermissionState
  /** 是否支持主动弹窗请求(仅 macOS 麦克风/摄像头) */
  canRequest: boolean
  /** 是否能跳转到系统设置对应页 */
  canOpenSettings: boolean
}

/** 浏览器页签信息(主进程 CDP 桥用;targetId = 页签 id,iframe name 同名) */
interface BrowserAgentTabInfo {
  id: string
  url: string
  title?: string
  active: boolean
}

/** 主进程下发的浏览器页签命令(createTarget / closeTarget / activateTarget / 弹窗转页签) */
interface BrowserAgentCommand {
  requestId: string
  action: 'create' | 'close' | 'activate'
  url?: string
  tabId?: string
}

/** 截图附加信息(与 src/main/browser-capture.ts 保持一致) */
interface BrowserCaptureOptions {
  /** 网页页签 id,仅用于文件名 */
  tabId?: string
  /** 绝对工作区目录;缺省写到系统图片目录 */
  workspaceDir?: string
}

/** 截图结果:成功给绝对路径,失败给原因 */
interface BrowserCaptureResult {
  ok: boolean
  path?: string
  message?: string
}

/** 单帧结果(录像用):JPEG 字节 + 像素尺寸 */
interface BrowserFrameResult {
  ok: boolean
  data?: Uint8Array
  width?: number
  height?: number
  message?: string
}

/** 录像落盘入参:webm 字节 + 归属信息 */
interface BrowserRecordSavePayload {
  data: Uint8Array
  tabId?: string
  workspaceDir?: string
}

/** 悬停卡片的一行(key = inspect.* 字段名;值与页面内卡片同源) */
interface PickCardRow {
  key: string
  value: string
  swatch?: string
  sep?: boolean
  path?: boolean
}

/** office/pdf 预览结果(结构同 src/main/officePreview.ts 的 OfficePreviewResult) */
interface OfficePreviewResult {
  ok: boolean
  kind?: 'docx' | 'xlsx' | 'pptx' | 'pdf'
  /** 单文件产物(分片内联 + 媒体 data URI),直接作 OfficeViewer 的 data prop */
  data?: unknown
  message?: string
}

/** 元素拾取结果(结构同 src/main/element-picker.ts 的 ElementPickResult / 渲染层 PickedElementData) */
interface ElementPickResult {
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

const api = {
  /**
   * 连接方式（本机 / 云端）：壳子启动时的「去哪找后端」。
   * `info` 在 preload 加载期**同步**取值（页面脚本可直接分支：welcome = 本次加载的是欢迎页）；
   * 其余动作都是异步 IPC，由主进程落盘并导航窗口。
   */
  shellConnection: {
    info: ipcRenderer.sendSync('shellConnection:get-sync') as {
      mode: 'local' | 'cloud' | 'unset'
      endpoint: string
      hasToken: boolean
      welcome: boolean
      onboarded: boolean
    },
    /** 探测端点（GET /api/v1/status，Bearer 令牌） */
    test: (endpoint: string, token: string): Promise<unknown> =>
      ipcRenderer.invoke('shellConnection:test', endpoint, token),
    /** 落盘并切换（主进程随后导航窗口：本机 → 本地 UI；云端 → 远端 Web UI） */
    save: (patch: {
      mode: 'local' | 'cloud'
      endpoint?: string
      token?: string
      onboarded?: boolean
    }): Promise<unknown> => ipcRenderer.invoke('shellConnection:save', patch),
    /** 打开欢迎页（重新选择连接方式） */
    openWelcome: (): Promise<void> => ipcRenderer.invoke('shellConnection:open-welcome'),
    /** 已配置过时从引导页返回当前连接形态 */
    closeWelcome: (): Promise<void> => ipcRenderer.invoke('shellConnection:exit-welcome')
  },
  shell: {
    /** 系统浏览器打开 http(s) 链接（MCP OAuth 授权页）；返回 '' 成功，否则错误信息 */
    openExternal: (url: string): Promise<string> => ipcRenderer.invoke('shell:open-external', url)
  },
  system: {
    /** 应用系统目录（config/ data/ log/ plugins/ 等的父目录） */
    appDir: (): Promise<string> => ipcRenderer.invoke('system:app-dir')
  },
  window: {
    minimize: (): void => ipcRenderer.send('window:minimize'),
    toggleMaximize: (): void => ipcRenderer.send('window:toggle-maximize'),
    close: (): void => ipcRenderer.send('window:close'),
    isMaximized: (): Promise<boolean> => ipcRenderer.invoke('window:is-maximized'),
    onMaximizedChange: (callback: (maximized: boolean) => void): (() => void) => {
      const listener = (_event: Electron.IpcRendererEvent, maximized: boolean): void => {
        callback(maximized)
      }
      ipcRenderer.on('window:maximized-changed', listener)
      return () => {
        ipcRenderer.removeListener('window:maximized-changed', listener)
      }
    },
    isFullscreen: (): Promise<boolean> => ipcRenderer.invoke('window:is-fullscreen'),
    onFullscreenChange: (callback: (fullscreen: boolean) => void): (() => void) => {
      const listener = (_event: Electron.IpcRendererEvent, fullscreen: boolean): void => {
        callback(fullscreen)
      }
      ipcRenderer.on('window:fullscreen-changed', listener)
      return () => {
        ipcRenderer.removeListener('window:fullscreen-changed', listener)
      }
    }
  },
  zoom: {
    setFactor: (factor: number): void => ipcRenderer.send('zoom:set', factor)
  },
  appearance: {
    /** 原生侧外观跟随应用主题(macOS 红黄绿失焦色、原生菜单与弹窗;见 main/ipc.ts) */
    setThemeMode: (mode: 'light' | 'dark' | 'system'): void =>
      ipcRenderer.send('appearance:set-theme-mode', mode)
  },
  office: {
    // office/pdf 高保真预览:CLI unpack --inline 产出单文件自包含 JSON(带缓存,见 main/officePreview.ts);
    // 失败返回 ok:false + message(前端回退旧渲染器或外部打开)
    preview: (filePath: string): Promise<OfficePreviewResult> =>
      ipcRenderer.invoke('office:preview-open', filePath)
  },
  browser: {
    // 打开浏览器页签 iframe 的调试控制台(跨域 iframe 为独立 webContents,按 URL 匹配)
    openDevtools: (url: string): Promise<boolean> => ipcRenderer.invoke('browser:open-devtools', url),
    // 选择元素:主进程向 iframe 注入拾取脚本,等待用户点击后返回元素信息(取消返回 canceled)
    // options.hint = 页面内底部提示条文案;options.labels = 悬停卡片字段标签(随界面语言)
    pickElement: (url: string, options: ElementPickerOptions): Promise<ElementPickResult> =>
      ipcRenderer.invoke('browser:pick-element', url, options),
    // 取消进行中的拾取(页面内 Esc 同样可取消)
    cancelPick: (url: string): Promise<boolean> => ipcRenderer.invoke('browser:cancel-pick', url),
    // 页签截图:rect = iframe 可视区域的视口坐标 CSS px;主进程 capturePage 后落盘,返回绝对路径
    capture: (rect: CaptureRect, opts: BrowserCaptureOptions): Promise<BrowserCaptureResult> =>
      ipcRenderer.invoke('browser:capture', rect, opts),
    // 录制取一帧:同一矩形,返回 JPEG 字节(不落盘);由渲染层按帧率驱动并编码
    frame: (rect: CaptureRect): Promise<BrowserFrameResult> =>
      ipcRenderer.invoke('browser:frame', rect),
    // 录像落盘:把 MediaRecorder 产出的 webm 字节交回主进程写文件
    recordSave: (payload: BrowserRecordSavePayload): Promise<BrowserCaptureResult> =>
      ipcRenderer.invoke('browser:record-save', payload)
  },
  // AI 浏览器代理操控:渲染层上报**本会话全部浏览器页签**(含非激活),主进程按会话起停多目标 CDP 桥
  browserAgent: {
    /**
     * 上报某会话的全部浏览器页签(多页签;targetId = 页签 id)。
     * 页签列表为空 → 停桥;会话切换 → 旧桥销毁、按新会话重建。
     */
    setTabs: (payload: { sessionId: string; tabs: BrowserAgentTabInfo[]; activeId: string }) =>
      ipcRenderer.invoke('browser-agent:set-tabs', payload),
    /** 订阅主进程下发的页签命令(createTarget/closeTarget/activateTarget 或弹窗转页签) */
    onCommand: (cb: (cmd: BrowserAgentCommand) => void): (() => void) => {
      const handler = (_e: unknown, cmd: BrowserAgentCommand): void => cb(cmd)
      ipcRenderer.on('browser-agent:command', handler as never)
      return () => ipcRenderer.removeListener('browser-agent:command', handler as never)
    },
    /** 回执主进程命令的执行结果(新增页签 id / 是否成功) */
    commandResult: (payload: {
      requestId: string
      ok: boolean
      tabId?: string
      message?: string
    }) => ipcRenderer.invoke('browser-agent:command-result', payload),
    status: () => ipcRenderer.invoke('browser-agent:status'),
    stop: () => ipcRenderer.invoke('browser-agent:stop')
  },
  dialog: {
    openDirectory: (): Promise<string> => ipcRenderer.invoke('dialog:open-directory'),
    // 多选文件或文件夹(知识库导入等);取消返回空数组
    openFiles: (): Promise<string[]> => ipcRenderer.invoke('dialog:open-files'),
    // 单选 JSON 文件并读回文本(MCP 配置导入);取消返回 null
    openJson: (): Promise<{ path: string; content: string } | null> =>
      ipcRenderer.invoke('dialog:open-json'),
    // 另存为(导出):返回保存路径,取消返回空串
    saveFile: (defaultName: string): Promise<string> => ipcRenderer.invoke('dialog:save-file', defaultName),
    // 单选 .pptx(内置插件 ppt-inspector 导入);defaultPath 为初始目录,取消返回空串
    openPptx: (defaultPath?: string): Promise<string> => ipcRenderer.invoke('dialog:open-pptx', defaultPath),
    // 多选附件(输入框「+」):不限类型,图片也在其中(按内容识别);取消返回空数组
    openAttachments: (): Promise<string[]> => ipcRenderer.invoke('dialog:open-attachments')
  },
  // 用户消息附件:图片 staging 落盘副本;其他文件**不落盘**、原样引用用户原始路径
  attachment: {
    stagePaths: (paths: string[]) => ipcRenderer.invoke('attachment:stage-paths', paths),
    stageBytes: (bytes: Uint8Array, name: string) =>
      ipcRenderer.invoke('attachment:stage-bytes', bytes, name),
    stageClipboard: () => ipcRenderer.invoke('attachment:stage-clipboard'),
    // Electron 43 起 File.path 已移除:拖拽文件需经 webUtils 取真实路径(取不到返回 '')
    pathForFile: (file: File): string => {
      try {
        return webUtils.getPathForFile(file) || ''
      } catch {
        return ''
      }
    },
    toUrl: (path: string): string => attachmentUrl(path)
  },
  fs: {
    // 用系统文件管理器打开目录;返回 '' 成功,否则错误信息
    openFolder: (dir: string): Promise<string> => ipcRenderer.invoke('shell:open-path', dir),
    // 读文件为 base64(图片预览;8MB 上限,失败/超限返回空)
    readFileBase64: (path: string): Promise<string> => ipcRenderer.invoke('file:read-buffer', path),
    // 读文件为 ArrayBuffer(office 等二进制预览;64MB 上限,失败/超限返回 null)
    readArrayBuffer: (path: string): Promise<ArrayBuffer | null> =>
      ipcRenderer.invoke('file:read-arraybuffer', path),
    // 写文本文件(markdown 编辑保存)
    writeTextFile: (path: string, content: string): Promise<{ ok: boolean; message?: string }> =>
      ipcRenderer.invoke('file:write', path, content)
  },
  pptx: {
    // 内置 PPT 解包(.pptx → 产物 deck JSON;供 ppt-inspector 插件渲染)
    unpack: (file: string): Promise<{ ok: boolean; file?: string; deck?: unknown; message?: string }> =>
      ipcRenderer.invoke('pptx:unpack', file)
  },
  git: {
    // 所选目录的真实 git 当前分支(非仓库/失败返回空串)
    currentBranch: (dir: string): Promise<string> => ipcRenderer.invoke('git:current-branch', dir),
    // 目录是否位于 git 工作树内(空仓库也算;非仓库 false)
    isRepo: (dir: string): Promise<boolean> => ipcRenderer.invoke('git:is-repo', dir),
    // 所选目录 git 仓库的全部本地分支(仅选择不切换;非仓库返回空数组)
    listBranches: (dir: string): Promise<string[]> => ipcRenderer.invoke('git:list-branches', dir),
    // 分支菜单状态:当前分支 + 本地/远程分支 + 未提交文件数(非仓库全空)
    branchState: (
      dir: string
    ): Promise<{ current: string; local: string[]; remotes: string[]; uncommitted: number }> =>
      ipcRenderer.invoke('git:branch-state', dir),
    // 当前分支的提交历史(已按拓扑序;decorations 为 %D 拆开的 ref 名,如 "HEAD -> master")
    log: (
      dir: string,
      branch: string,
      limit?: number
    ): Promise<{
      commits: {
        hash: string
        author: string
        dateISO: string
        dateRel: string
        subject: string
        decorations: string[]
      }[]
      error?: string
    }> => ipcRenderer.invoke('git:log', dir, branch, limit),
    // 切换分支(会改动工作区;失败时 error 为 git 原始 stderr,如「本地改动会被覆盖」)
    checkout: (dir: string, branch: string): Promise<{ ok: boolean; error?: string }> =>
      ipcRenderer.invoke('git:checkout', dir, branch),
    // 创建并检出分支(checkout -b)
    createBranch: (dir: string, name: string): Promise<{ ok: boolean; error?: string }> =>
      ipcRenderer.invoke('git:create-branch', dir, name)
  },
  providers: {
    // 供应商配置(主进程 safeStorage 加密,见 src/main/providers.ts)
    list: (): Promise<unknown> => ipcRenderer.invoke('providers:list'),
    sync: (providers: unknown): Promise<unknown> => ipcRenderer.invoke('providers:sync', { providers }),
    test: (name: string, model: string): Promise<{ ok: boolean; message: string }> =>
      ipcRenderer.invoke('providers:test', { name, model })
  },
  llm: {
    // 单次 LLM 补全(插件 SDK llm.chat):事件经后端全局总线 napi:event(type:"llm")回传,
    // 此处按 call_id 过滤并归一为 chunk/end/error 形状
    ask: (
      request: unknown,
      onChunk: (chunk: { callId: string; chunk: unknown }) => void
    ): (() => void) => {
      const callId = `llm-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
      const listener = (_event: Electron.IpcRendererEvent, payload: unknown): void => {
        const ev = payload as {
          type?: string
          call_id?: string
          kind?: 'chunk' | 'reasoning' | 'end' | 'error'
          delta?: string
          message?: string
          usage?: unknown
          finishReason?: string
        } | null
        if (!ev || ev.type !== 'llm' || ev.call_id !== callId) return
        const chunk = {
          type: ev.kind ?? 'chunk',
          delta: ev.delta,
          message: ev.message,
          usage: ev.usage,
          finishReason: ev.finishReason
        }
        onChunk({ callId, chunk })
        // 流结束(end)后自动退订,避免按 call_id 过滤的残留监听
        if (ev.kind === 'end' || ev.kind === 'error') ipcRenderer.removeListener('napi:event', listener)
      }
      ipcRenderer.on('napi:event', listener)
      void ipcRenderer
        .invoke('llm:ask', { callId, request })
        .then((res: { ok?: boolean; error?: string }) => {
          if (res?.ok) return
          ipcRenderer.removeListener('napi:event', listener)
          onChunk({ callId, chunk: { type: 'error', message: res?.error ?? 'LLM 补全请求失败' } })
          onChunk({ callId, chunk: { type: 'end', usage: null, finishReason: 'error' } })
        })
        .catch((err: Error) => {
          ipcRenderer.removeListener('napi:event', listener)
          onChunk({ callId, chunk: { type: 'error', message: err?.message ?? 'LLM 补全请求失败' } })
          onChunk({ callId, chunk: { type: 'end', usage: null, finishReason: 'error' } })
        })
      return () => {
        ipcRenderer.removeListener('napi:event', listener)
      }
    }
  },
  chat: {
    // 聊天:转发到 Rust napi 后端(send/流式事件经 napi:event 推送)
    createSession: (request: unknown): Promise<string> => ipcRenderer.invoke('chat:create-session', request),
    sendMessage: (sessionId: string, request: unknown): Promise<void> =>
      ipcRenderer.invoke('chat:send-message', { session_id: sessionId, request }),
    // 启动:把「仅创建」的会话跑起来(以已有历史驱动一轮,不追加新的用户消息)
    startSession: (sessionId: string): Promise<void> =>
      ipcRenderer.invoke('chat:start-session', sessionId),
    // 立即发送:进入后端打断消息队列,取消当前循环优先执行
    interruptMessage: (sessionId: string, request: unknown): Promise<void> =>
      ipcRenderer.invoke('chat:interrupt-message', { session_id: sessionId, request }),
    // 中断决议:审批同意/拒绝、表单回答/忽略(决议即 tool 结果,不追加用户消息)
    submitInterrupt: (sessionId: string, request: unknown): Promise<void> =>
      ipcRenderer.invoke('chat:submit-interrupt', { session_id: sessionId, request }),
    // 重开提问:被忽略/被取代的提问用原表单定义重新登记待决中断 → 表单卡重新弹出(空白表单)
    reopenInterrupt: (sessionId: string, toolCallId: string): Promise<void> =>
      ipcRenderer.invoke('chat:reopen-interrupt', {
        session_id: sessionId,
        tool_call_id: toolCallId
      }),
    cancel: (sessionId: string): Promise<void> => ipcRenderer.invoke('chat:cancel', sessionId),
    // 撤回历史:保留前 keepUserMessages 条用户消息,删除其后全部(内存+磁盘),并中断当前循环
    rewindToUserMessage: (sessionId: string, keepUserMessages: number): Promise<void> =>
      ipcRenderer.invoke('chat:rewind', { session_id: sessionId, keepUserMessages }),
    // 基于已有会话 fork 出新会话(共用工作区),返回新会话 id
    forkSession: (sessionId: string, beforeFinal?: number): Promise<string> =>
      ipcRenderer.invoke('chat:fork-session', { session_id: sessionId, beforeFinal }),
    // 手动触发一次会话摘要(/compact);返回是否完成摘要
    summarizeSession: (sessionId: string): Promise<boolean> =>
      ipcRenderer.invoke('chat:summarize', sessionId),
    // 会话级设置(模式/审批/思考强度/模型/供应商):不必发消息即可改写,后端下一个工具调用即生效
    updateSessionSettings: (sessionId: string, request: unknown): Promise<void> =>
      ipcRenderer.invoke('chat:update-session-settings', { session_id: sessionId, request }),
    // 会话目标(/goal):goal 为 null/空白 = 删除;不必发消息,后端写活记录并广播 session_info
    updateSessionGoal: (
      sessionId: string,
      goal: string | null,
      goalLang?: string
    ): Promise<void> =>
      ipcRenderer.invoke('chat:update-session-goal', {
        session_id: sessionId,
        goal,
        goal_lang: goalLang
      }),
    listSessions: (): Promise<unknown[]> => ipcRenderer.invoke('chat:list-sessions'),
    listMessages: (sessionId: string): Promise<unknown[]> =>
      ipcRenderer.invoke('chat:list-messages', sessionId),
    // 归档分页(「加载更早」):最新摘要备份段中 seq < beforeSeq 的消息,末尾 maxUserTurns 个用户轮次;
    // 返回空数组 = 已到归档起点
    archivedBefore: (
      sessionId: string,
      beforeSeq: number,
      maxUserTurns = 5
    ): Promise<unknown[]> =>
      ipcRenderer.invoke('chat:archived-before', sessionId, beforeSeq, maxUserTurns),
    // 会话系统提示词(按当前配置实时拼装,不落盘;供「会话轨迹」展示)
    getSystemPrompt: (sessionId: string): Promise<string> =>
      ipcRenderer.invoke('chat:system-prompt', sessionId),
    // 删除会话:后端同时清理 sqlite 元数据与 jsonl 会话目录,重启不回填
    deleteSession: (sessionId: string): Promise<void> =>
      ipcRenderer.invoke('chat:delete-session', sessionId),
    onEvent: (callback: (chunk: unknown) => void): (() => void) => {
      const listener = (_event: Electron.IpcRendererEvent, chunk: unknown): void => callback(chunk)
      ipcRenderer.on('napi:event', listener)
      return () => {
        ipcRenderer.removeListener('napi:event', listener)
      }
    }
  },
  backend: {
    // 设置页命令:技能 / 工具 / 定时任务 → Rust napi;设置 / MCP → 主进程本地实现
    invoke: (command: string, args?: Record<string, unknown>): Promise<unknown> =>
      ipcRenderer.invoke('backend:invoke', command, args),
    // 订阅后端全局事件流(napi:event;终端输出等按 type 分发)
    onEvent: (callback: (chunk: unknown) => void): (() => void) => {
      const listener = (_event: Electron.IpcRendererEvent, chunk: unknown): void => callback(chunk)
      ipcRenderer.on('napi:event', listener)
      return () => {
        ipcRenderer.removeListener('napi:event', listener)
      }
    }
  },
  permission: {
    // 查询各系统权限的当前状态(macOS 真实状态;其他平台 unsupported)
    status: (): Promise<PermissionItem[]> => ipcRenderer.invoke('permission:status'),
    // 主动请求授权(仅 macOS 麦克风/摄像头有效),返回刷新后的状态
    request: (kind: PermissionKind): Promise<PermissionItem[]> =>
      ipcRenderer.invoke('permission:request', kind),
    // 打开系统设置的对应隐私页;返回是否成功
    openSettings: (kind: PermissionKind): Promise<boolean> =>
      ipcRenderer.invoke('permission:open-settings', kind)
  },
  power: {
    // 设置「保持电脑运行」:开启后阻止系统空闲休眠(手动睡眠/合盖仍生效),返回是否处于抑制中
    setKeepAwake: (enabled: boolean): Promise<boolean> =>
      ipcRenderer.invoke('power:set-keep-awake', enabled)
  },
  versions: {
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    node: process.versions.node
  },
  // 当前运行平台(渲染层据此切换窗口控制布局:darwin 用系统红黄绿,其余自绘)
  platform: process.platform,
  // 桌面端支持 astorm-icon:// 图标缓存协议；浏览器部署 / 预览桥没有此协议（回落直连）
  iconProtocol: true
}

contextBridge.exposeInMainWorld('api', api)
