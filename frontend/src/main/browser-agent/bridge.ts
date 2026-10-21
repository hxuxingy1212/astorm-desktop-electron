import { randomUUID } from 'crypto'
import { WebSocketServer, type WebSocket } from 'ws'
import type { WebContents } from 'electron'

/**
 * 内置浏览器「AI 代理操控」桥接层(会话级,多目标)。
 *
 * 一个 astorm 会话 → 一条桥 → 管理该会话的**多个浏览器页签**(渲染层每个页签一个 iframe)。
 * 对 agent-browser 暴露一个"多页签浏览器":`Target.getTargets` 列出全部页签,
 * `Target.createTarget/closeTarget/activateTarget` 转成渲染层的开/关/切页签,
 * `Target.attachToTarget` 把某个页签的 OOPIF CDP 会话包装成一个合成 sessionId。
 *
 * 关键关联方式:渲染层给每个 iframe 设 `name = 页签 id`,桥在各 OOPIF 会话内
 * `Runtime.evaluate('window.name')` 反查到页签 id(URL 匹配作 fallback)。
 *
 * 为什么不是纯 1:1 转发(结论来自 docs/browser-agent.md §16 实测):
 *   - iframe 是 OOPIF:窗口 `webContents.debugger` + `Target.setAutoAttach({flatten:true})`
 *     能拿到它自己的 sessionId;会话内 Runtime/DOM/Input/Page.navigate/Accessibility 可用;
 *   - `Page.captureScreenshot` / `Page.getNavigationHistory` / `Page.bringToFront` **只在顶层 target**
 *     可执行 → 截图必须转译为"顶层页面会话 + clip";其余顶层专用命令给可读错误;
 *   - `attachedToTarget` 异步到达且 `targetInfo.url` 不可靠 → 等事件 + 重试 + 会话内反查。
 *
 * 安全边界(硬规则):
 *   - 只监听 127.0.0.1 + 一次性 token;不开调试端口、不设 remote-allow-origins;
 *   - 拒绝浏览器级命令(`Browser.close` 等于关应用)、持久注入、改应用面板尺寸;
 *   - 只转发**已绑定浏览器页签**会话的事件,宿主页/浏览器级事件一律丢弃。
 */

/** 一个浏览器页签(渲染层上报;id 同时作为 CDP targetId 与 iframe name) */
export interface BridgeTabInfo {
  id: string
  url: string
  title?: string
  active: boolean
}

/** 主进程请求渲染层开/关/切页签的结果 */
export interface BridgeCommandResult {
  ok: boolean
  tabId?: string
  message?: string
}

/** 主进程 → 渲染层的页签命令(由 manager 注入;渲染层回执) */
export type BridgeCommand = (
  action: 'create' | 'close' | 'activate',
  payload: { url?: string; tabId?: string }
) => Promise<BridgeCommandResult>

/** 桥接对外暴露的连接信息(agent-browser 侧用 wsUrl;token 已含在 URL 里) */
export interface BridgeEndpoint {
  /** 形如 ws://127.0.0.1:53421/astorm/<token> */
  wsUrl: string
  token: string
  /** 当前激活页签的 URL(审批 origin / 展示用) */
  frameUrl: string
  /** 当前激活页签的 frameId(= CDP targetId) */
  frameId: string
  /** 当前激活页签的渲染层 id */
  tabId?: string
  /** 端点所属 astorm 会话(工具据此校验会话一致) */
  sessionId?: string
}

export interface FrameBridgeOptions {
  /** 宿主窗口的 webContents(各浏览器页签 iframe 在其中,通过 debugger 会话访问) */
  wc: WebContents
  /** 初始页签列表 */
  tabs: BridgeTabInfo[]
  /** 端点所属 astorm 会话 id */
  sessionId: string
  /** iframe 元素选择器(默认 '.bw-frame';激活实例带 .bw-pane--active) */
  frameSelector?: string
  /** 渲染层页签命令(开/关/切);缺省时 createTarget/closeTarget/activateTarget 不可用 */
  command?: BridgeCommand
  /** 定位 iframe 会话的等待上限(默认 4000ms) */
  resolveTimeoutMs?: number
  log?: (msg: string) => void
}

export interface FrameBridge {
  /** 端点(随激活页签变化就地更新,读它的调用方拿到的是当前值) */
  readonly endpoint: BridgeEndpoint
  /** 渲染层页签列表变化:更新目标表并补发 targetCreated/targetDestroyed */
  updateTabs(tabs: BridgeTabInfo[], sessionId?: string): void
  /** 请求渲染层新建一个页签(弹窗转页签用);返回新页签 id */
  createTab(url: string): Promise<string | null>
  /** 停止:关 WS、摘 debugger、清监听 */
  stop(): Promise<void>
}

/** 只在顶层 target 可执行、无法在 iframe 会话里跑的命令(实测确认) */
const TOP_LEVEL_ONLY = new Set([
  'Page.getNavigationHistory',
  'Page.navigateToHistoryEntry',
  'Page.getAppManifest',
  'Page.getInstallabilityErrors',
  'Page.captureSnapshot',
  'Page.bringToFront'
])

/** 直接拒绝的命令(安全边界):浏览器级、持久注入、改应用面板尺寸 */
const REJECTED = new Set([
  'Browser.close',
  'Browser.crash',
  'Browser.crashGpuProcess',
  'Browser.setDownloadBehavior',
  'Browser.grantPermissions',
  'Page.setDownloadBehavior',
  'Page.addScriptToEvaluateOnNewDocument',
  'Page.setBypassCSP',
  'Emulation.setDeviceMetricsOverride',
  'Emulation.setVisibleSize',
  'Emulation.setPageScaleFactor'
])

/** 允许的浏览器级只读命令(agent-browser 连接时可能用于探测) */
const BROWSER_READONLY = new Set(['Browser.getVersion', 'Browser.getBrowserCommandLine'])

/** 需要"页签可见"才能生效的命令:非激活页签先切前台再发(隐藏帧输入不可靠) */
const NEEDS_VISIBLE = new Set([
  'Input.dispatchMouseEvent',
  'Input.dispatchKeyEvent',
  'Input.insertText',
  'Input.dispatchDragEvent',
  'Page.captureScreenshot'
])

const SYNTH_PREFIX = 'astorm-page:'
const synthSession = (tabId: string): string => `${SYNTH_PREFIX}${tabId}`
const tabOfSynth = (sessionId?: string): string | null =>
  sessionId && sessionId.startsWith(SYNTH_PREFIX) ? sessionId.slice(SYNTH_PREFIX.length) : null

const trimSlash = (u: string): string => u.replace(/\/+$/, '')
const normalize = (u: string): string => {
  try {
    const x = new URL(u)
    return `${x.origin}${trimSlash(x.pathname)}`
  } catch {
    return trimSlash(u)
  }
}

interface FrameCandidate {
  sessionId: string
  frameId: string
  url: string
  name: string
}

/**
 * 启动会话级桥接。
 *
 * 调用方(应用主进程)给「宿主 webContents + 该会话全部浏览器页签 + 会话 id」,
 * 其余(会话定位、WS 服务、多目标 Target 域、命令转译、生命周期)都在这里。
 */
export async function startFrameBridge(opts: FrameBridgeOptions): Promise<FrameBridge> {
  const { wc } = opts
  const selector = opts.frameSelector ?? '.bw-frame'
  const timeoutMs = opts.resolveTimeoutMs ?? 4000
  const log = opts.log ?? ((): void => {})
  const token = randomUUID().replace(/-/g, '')
  const command = opts.command

  /** 渲染层当前页签(targetId = 页签 id) */
  let tabs: BridgeTabInfo[] = [...opts.tabs]
  let sessionId = opts.sessionId
  let currentTabId: string | null = tabs.find((t) => t.active)?.id ?? tabs[0]?.id ?? null
  /** 页签 → CDP 会话(按需解析) */
  const tabSessions = new Map<string, FrameCandidate>()
  /** CDP 会话 → 页签 */
  const sessionToTab = new Map<string, string>()
  /** iframe 目标(fpId → sessionId),随 attachedToTarget / detachedFromTarget 维护 */
  const iframeSessions = new Map<string, string>()
  let discoverEnabled = false

  // ---------- 事件回传(socket 侧) ----------
  let sockets: WebSocket[] = []

  function pushEvent(method: string, params: unknown, sid?: string): void {
    if (!sockets.length) return
    const text = JSON.stringify({ method, params, sessionId: sid })
    for (const s of sockets) if (s.readyState === 1) s.send(text)
  }

  function relayEvent(method: string, params: unknown, sid?: string): void {
    if (!sockets.length) return
    if (method.startsWith('Target.')) return
    const tabId = sid ? sessionToTab.get(sid) : undefined
    // 只转发已绑定浏览器页签会话的事件;无 sessionId 的宿主页/浏览器级事件一律丢弃
    if (!tabId) return
    pushEvent(method, params, synthSession(tabId))
  }

  // ---------- debugger 会话 ----------
  const dbg = wc.debugger
  let attachedByUs = false
  if (!dbg.isAttached()) {
    dbg.attach('1.3')
    attachedByUs = true
  }
  const send = <T = unknown>(method: string, params?: unknown, sessionId?: string): Promise<T> =>
    dbg.sendCommand(method, params ?? {}, sessionId) as Promise<T>

  const onDebuggerMessage = (
    _e: unknown,
    method: string,
    params: { sessionId?: string; targetInfo?: { targetId: string; type: string }; [k: string]: unknown },
    sid?: string
  ): void => {
    if (method === 'Target.attachedToTarget') {
      const info = (params as { targetInfo?: { targetId: string; type: string } }).targetInfo
      const s = (params as { sessionId?: string }).sessionId
      if (info?.type === 'iframe' && s) iframeSessions.set(info.targetId, s)
    } else if (method === 'Target.detachedFromTarget') {
      const s = (params as { sessionId?: string }).sessionId
      for (const [fid, v] of iframeSessions) if (v === s) iframeSessions.delete(fid)
      for (const [fid, v] of [...tabSessions]) {
        if (v.sessionId === s) {
          tabSessions.delete(fid)
          sessionToTab.delete(s)
        }
      }
    }
    relayEvent(method, params, sid)
  }
  dbg.on('message', onDebuggerMessage as never)

  // ---------- 目标定位 ----------
  const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms))

  /** 反查候选会话的真实 URL 与 window.name(targetInfo.url 实测不可靠) */
  async function collectCandidates(): Promise<FrameCandidate[]> {
    const out: FrameCandidate[] = []
    for (const [frameId, sessionId] of iframeSessions) {
      try {
        const r = await send<{ result: { value?: { href?: string; name?: string } } }>(
          'Runtime.evaluate',
          { expression: '({href: location.href, name: window.name})', returnByValue: true },
          sessionId
        )
        const v = r.result?.value ?? {}
        out.push({ sessionId, frameId, url: v.href ?? '', name: v.name ?? '' })
      } catch {
        /* 会话刚销毁/未就绪:忽略 */
      }
    }
    return out
  }

  /** 让 Chromium 重新派发 autoAttach 并等一小段(事件异步到达) */
  async function kick(): Promise<void> {
    try {
      await send('Target.setDiscoverTargets', { discover: true })
      await send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true })
    } catch {
      /* 宿主页不可用:忽略,后续重试 */
    }
  }

  /** 解析某页签的 CDP 会话(带重试);name 优先,URL fallback */
  async function ensureSession(tabId: string, timeout = timeoutMs): Promise<FrameCandidate | null> {
    const cached = tabSessions.get(tabId)
    if (cached) return cached
    const tab = tabs.find((t) => t.id === tabId)
    const deadline = Date.now() + timeout
    let attempt = 0
    while (Date.now() < deadline) {
      attempt += 1
      await kick()
      await sleep(attempt === 1 ? 500 : 350)
      const cands = await collectCandidates()
      const hit =
        cands.find((c) => c.name === tabId) ??
        (tab?.url ? cands.find((c) => normalize(c.url) === normalize(tab.url)) : undefined)
      if (hit) {
        tabSessions.set(tabId, hit)
        sessionToTab.set(hit.sessionId, tabId)
        log(`[bridge] 页签已绑定 ${tabId} → frameId=${hit.frameId.slice(0, 12)}… url=${hit.url}`)
        return hit
      }
    }
    return null
  }

  /** 当前激活页签(渲染层 active 标记优先,回退 currentTabId) */
  function activeTab(): BridgeTabInfo | undefined {
    return tabs.find((t) => t.active) ?? tabs.find((t) => t.id === currentTabId) ?? tabs[0]
  }

  // 首次解析(激活页签优先;失败不致命,后续按需重试)
  const firstTabId = activeTab()?.id
  if (firstTabId) await ensureSession(firstTabId).catch(() => null)

  /** 端点就地更新(激活页签变化时刷新 frameUrl/tabId/frameId) */
  function refreshEndpoint(): void {
    const a = activeTab()
    endpoint.frameUrl = a?.url ?? ''
    endpoint.tabId = a?.id
    endpoint.frameId = (a && tabSessions.get(a.id)?.frameId) ?? ''
    endpoint.sessionId = sessionId
  }

  // ---------- 截图与几何转译 ----------

  /** 宿主页里**激活**页签 iframe 的矩形(文档坐标,CSS px)与 DPR */
  async function hostRect(): Promise<{ x: number; y: number; width: number; height: number; dpr: number }> {
    const expr = `(() => {
      const f = document.querySelector(${JSON.stringify(`${selector}`)}) || document.querySelector(${JSON.stringify('.bw-pane--active .bw-frame')})
      if (!f) return null
      const r = f.getBoundingClientRect()
      return {
        x: r.x + (window.scrollX || 0),
        y: r.y + (window.scrollY || 0),
        width: r.width,
        height: r.height,
        dpr: window.devicePixelRatio || 1
      }
    })()`
    const r = await send<{ result: { value?: { x: number; y: number; width: number; height: number; dpr: number } } }>(
      'Runtime.evaluate',
      { expression: expr, returnByValue: true }
    )
    const v = r.result?.value
    if (!v) throw new Error(`FRAME_ELEMENT_NOT_FOUND:宿主页里找不到激活的浏览器 iframe`)
    return v
  }

  /** 某页签 iframe 自己的视口与内容尺寸(CSS px) */
  async function frameViewport(
    tabId: string
  ): Promise<{ innerW: number; innerH: number; scrollH: number }> {
    const sess = await ensureSession(tabId)
    if (!sess) return { innerW: 0, innerH: 0, scrollH: 0 }
    const r = await send<{ result: { value?: { innerW: number; innerH: number; scrollH: number } } }>(
      'Runtime.evaluate',
      {
        expression: `(() => {
          const d = document.documentElement
          return { innerW: window.innerWidth, innerH: window.innerHeight, scrollH: d.scrollHeight }
        })()`,
        returnByValue: true
      },
      sess.sessionId
    )
    return r.result?.value ?? { innerW: 0, innerH: 0, scrollH: 0 }
  }

  /**
   * 截图转译:iframe 会话不能截图(仅顶层) → 改发页面会话 + clip(宿主页文档坐标)。
   * 只能截**当前可见**那块:元素/整页越界一律拒绝,而非静默截空白。
   */
  async function screenshot(params: Record<string, unknown>, tabId: string): Promise<unknown> {
    // 截图必须可见:非激活页签先切前台(渲染层 v-show 切换后 iframe 才在可截区域)
    await makeVisible(tabId)
    const rect = await hostRect()
    const view = await frameViewport(tabId)
    const raw = params.clip as { x: number; y: number; width: number; height: number; scale?: number } | undefined
    const full = params.captureBeyondViewport === true
    let clip: { x: number; y: number; width: number; height: number; scale: number }
    if (full) {
      if (view.scrollH > view.innerH + 1) {
        throw new Error(
          `FULL_PAGE_UNSUPPORTED:内置页签只能截可见区域(内容高 ${view.scrollH} > 视口高 ${view.innerH});请用 scroll + screenshot 分段截`
        )
      }
      clip = { x: rect.x, y: rect.y, width: rect.width, height: rect.height, scale: 1 / rect.dpr }
    } else if (raw && raw.width > 0 && raw.height > 0) {
      if (
        raw.x < -1 ||
        raw.y < -1 ||
        raw.x + raw.width > view.innerW + 1 ||
        raw.y + raw.height > view.innerH + 1
      ) {
        throw new Error(
          `CLIP_OUT_OF_VIEW:只能截页签可见区域(帧视口 ${view.innerW}x${view.innerH},请求 ` +
            `${Math.round(raw.x)},${Math.round(raw.y)} ${Math.round(raw.width)}x${Math.round(raw.height)});请先滚动到该区域`
        )
      }
      clip = {
        x: rect.x + raw.x,
        y: rect.y + raw.y,
        width: raw.width,
        height: raw.height,
        scale: raw.scale ?? 1 / rect.dpr
      }
    } else {
      clip = { x: rect.x, y: rect.y, width: rect.width, height: rect.height, scale: 1 / rect.dpr }
    }
    const args: Record<string, unknown> = { format: params.format ?? 'png', clip }
    if (params.quality != null && String(params.format) === 'jpeg') args.quality = params.quality
    if (full) args.captureBeyondViewport = true
    return send('Page.captureScreenshot', args)
  }

  /** 把某页签切到前台(渲染层激活);已是激活页签则跳过 */
  async function makeVisible(tabId: string): Promise<void> {
    if (activeTab()?.id === tabId) return
    if (!command) return
    const r = await command('activate', { tabId })
    if (r.ok) currentTabId = tabId
    // 等渲染层 v-show 生效,让 iframe 进入可截区域
    await sleep(120)
  }

  // ---------- Target 域 shim(多页签) ----------

  function targetInfoOf(tab: BridgeTabInfo): Record<string, unknown> {
    return {
      targetId: tab.id,
      type: 'page',
      title: tab.title ?? '',
      url: tab.url,
      attached: false,
      canAccessOpener: false
    }
  }

  async function targetShim(
    method: string,
    params: Record<string, unknown>
  ): Promise<{ result?: unknown; events?: unknown[] }> {
    switch (method) {
      case 'Target.setDiscoverTargets':
        discoverEnabled = true
        return {
          result: {},
          events: tabs.map((t) => ({ method: 'Target.targetCreated', params: { targetInfo: targetInfoOf(t) } }))
        }
      case 'Target.getTargets':
        return { result: { targetInfos: tabs.map(targetInfoOf) } }
      case 'Target.getTargetInfo': {
        const id = String(params.targetId ?? '')
        const t = tabs.find((x) => x.id === id)
        if (!t) throw new Error(`未找到页签:${id}`)
        return { result: { targetInfo: targetInfoOf(t) } }
      }
      case 'Target.attachToTarget': {
        const id = String(params.targetId ?? '') || activeTab()?.id || ''
        if (!id) throw new Error('NO_TAB:当前没有可附加的浏览器页签')
        const sess = await ensureSession(id)
        if (!sess) throw new Error(`NO_FRAME:页签 ${id} 的 iframe 会话尚未就绪(页面可能仍在加载)`)
        currentTabId = id
        const t = tabs.find((x) => x.id === id)
        const info = t ? targetInfoOf(t) : { targetId: id, type: 'page', title: '', url: sess.url, attached: false, canAccessOpener: false }
        return {
          result: { sessionId: synthSession(id) },
          events: [
            {
              method: 'Target.attachedToTarget',
              params: { sessionId: synthSession(id), targetInfo: { ...info, attached: true }, waitingForDebugger: false }
            }
          ]
        }
      }
      case 'Target.setAutoAttach':
      case 'Target.detachFromTarget':
        return { result: {} }
      case 'Target.activateTarget': {
        const id = String(params.targetId ?? '')
        if (id && command) {
          const r = await command('activate', { tabId: id })
          if (r.ok) currentTabId = id
        }
        return { result: {} }
      }
      case 'Target.createTarget': {
        const url = String(params.url ?? '')
        if (!command) throw new Error('Target.createTarget 不可用:渲染层命令通道未接入')
        const r = await command('create', { url })
        if (!r.ok || !r.tabId) throw new Error(`新建页签失败:${r.message ?? '未知原因'}`)
        // 乐观加入目标表：agent 可能紧接着 getTargets/attach，不能等渲染层去抖上报
        if (!tabs.some((t) => t.id === r.tabId)) {
          tabs = [...tabs, { id: r.tabId, url, title: '', active: false }]
        }
        // 等新 iframe 的 OOPIF 会话就绪(页面可能还在加载;失败也不阻断,后续按需重试)
        await ensureSession(r.tabId, 2500).catch(() => null)
        if (discoverEnabled) {
          const t = tabs.find((x) => x.id === r.tabId)
          if (t) pushEvent('Target.targetCreated', { targetInfo: targetInfoOf(t) })
        }
        return { result: { targetId: r.tabId } }
      }
      case 'Target.closeTarget': {
        const id = String(params.targetId ?? '')
        if (!command) throw new Error('Target.closeTarget 不可用:渲染层命令通道未接入')
        const r = await command('close', { tabId: id })
        if (!r.ok) throw new Error(`关闭页签失败:${r.message ?? '未知原因'}`)
        return { result: { success: true } }
      }
      // 隔离上下文超出「操控本会话页签」的范围 → 明确拒绝
      case 'Target.createBrowserContext':
      case 'Target.disposeBrowserContext':
        throw new Error(`${method} 被桥接层拒绝:AI 只能在内置浏览器的页签里操作`)
      default:
        throw new Error(`${method} 未支持(桥接层只实现多页签视图)`)
    }
  }

  // ---------- 命令分发 ----------

  /** 转发到某页签会话;会话因帧内跨源导航换掉时重解析一次再重试 */
  async function forward(method: string, params: Record<string, unknown>, tabId: string): Promise<unknown> {
    let sess = await ensureSession(tabId)
    if (!sess) throw new Error(`NO_FRAME:页签 ${tabId} 的 iframe 会话不可用`)
    try {
      return await send(method, params, sess.sessionId)
    } catch (err) {
      const msg = String((err as Error)?.message ?? '')
      if (!/session with given id not found|target closed|detached|not found/i.test(msg)) throw err
      tabSessions.delete(tabId)
      sessionToTab.delete(sess.sessionId)
      sess = await ensureSession(tabId)
      if (!sess) throw err
      return send(method, params, sess.sessionId)
    }
  }

  async function dispatch(
    method: string,
    params: Record<string, unknown>,
    synth?: string
  ): Promise<unknown> {
    if (REJECTED.has(method)) throw new Error(`命令被安全策略拒绝:${method}`)
    if (BROWSER_READONLY.has(method)) return send(method, params)
    if (method.startsWith('Target.')) {
      const r = await targetShim(method, params)
      if (r.events) {
        for (const e of r.events) {
          const ev = e as { method: string; params: unknown }
          // Target.* 是浏览器级事件:不携带 sessionId
          pushEvent(ev.method, ev.params)
        }
      }
      return r.result ?? {}
    }
    // 目标页签:命令带合成 sessionId → 该页签;否则当前页签
    const tabId = tabOfSynth(synth) ?? currentTabId ?? activeTab()?.id
    if (!tabId) throw new Error('NO_TAB:当前没有可操作的浏览器页签')
    if (NEEDS_VISIBLE.has(method)) await makeVisible(tabId)
    if (method === 'Page.captureScreenshot') return screenshot(params, tabId)
    // Page.reload 是顶层专用 → 用帧内 location.reload() 等价实现
    if (method === 'Page.reload') {
      await forward('Runtime.evaluate', { expression: 'location.reload()' }, tabId)
      return {}
    }
    if (TOP_LEVEL_ONLY.has(method)) throw new Error(`命令 ${method} 只能在顶层页面执行,当前桥接不支持`)
    return forward(method, params, tabId)
  }

  // ---------- WS 服务(127.0.0.1 + token 门禁) ----------

  const server = new WebSocketServer({ host: '127.0.0.1', port: 0, path: `/astorm/${token}` })
  await new Promise<void>((resolve, reject) => {
    server.once('listening', () => resolve())
    server.once('error', (err) => reject(err))
  })
  const addr = server.address()
  const port = typeof addr === 'object' && addr ? addr.port : 0
  const wsUrl = `ws://127.0.0.1:${port}/astorm/${token}`

  const endpoint: BridgeEndpoint = { wsUrl, token, frameUrl: '', frameId: '', tabId: undefined, sessionId }
  refreshEndpoint()

  server.on('connection', (socket) => {
    sockets = [...sockets, socket]
    log('[bridge] agent 已连接(WS)')
    socket.on('message', (raw) => {
      void (async () => {
        let msg: { id?: number; method?: string; params?: Record<string, unknown>; sessionId?: string }
        try {
          msg = JSON.parse(String(raw))
        } catch {
          return
        }
        if (msg.id == null || !msg.method) return
        try {
          const result = await dispatch(msg.method, msg.params ?? {}, msg.sessionId)
          socket.send(JSON.stringify({ id: msg.id, result: result ?? {} }))
        } catch (err) {
          socket.send(
            JSON.stringify({
              id: msg.id,
              error: { code: -32000, message: err instanceof Error ? err.message : String(err) }
            })
          )
        }
      })()
    })
    socket.on('close', () => {
      sockets = sockets.filter((s) => s !== socket)
      log('[bridge] agent 断开(WS)')
    })
  })

  log(`[bridge] 就绪:${wsUrl}(会话 ${sessionId},${tabs.length} 个页签)`)

  return {
    endpoint,
    updateTabs(next: BridgeTabInfo[], nextSessionId?: string): void {
      const before = new Set(tabs.map((t) => t.id))
      tabs = [...next]
      if (nextSessionId) sessionId = nextSessionId
      // 清理已关闭页签的映射
      for (const id of before) {
        if (!tabs.some((t) => t.id === id)) {
          const s = tabSessions.get(id)
          if (s) sessionToTab.delete(s.sessionId)
          tabSessions.delete(id)
        }
      }
      // 补发目标生命周期事件(agent 的 tab list 据此更新)
      if (discoverEnabled) {
        for (const t of tabs) {
          if (!before.has(t.id)) pushEvent('Target.targetCreated', { targetInfo: targetInfoOf(t) })
        }
        for (const id of before) {
          if (!tabs.some((t) => t.id === id)) pushEvent('Target.targetDestroyed', { targetId: id })
        }
      }
      if (!currentTabId || !tabs.some((t) => t.id === currentTabId)) {
        currentTabId = activeTab()?.id ?? null
      }
      refreshEndpoint()
    },
    async createTab(url: string): Promise<string | null> {
      if (!command) return null
      const r = await command('create', { url })
      if (!r.ok || !r.tabId) return null
      await ensureSession(r.tabId, 2500).catch(() => null)
      return r.tabId
    },
    async stop(): Promise<void> {
      // terminate 而非 close:对端(agent-browser daemon)会长期持有连接,server.close 会一直等
      for (const s of sockets) {
        try {
          s.terminate()
        } catch {
          /* 忽略 */
        }
      }
      sockets = []
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, 300)
        server.close(() => {
          clearTimeout(timer)
          resolve()
        })
      })
      dbg.off('message', onDebuggerMessage as never)
      if (attachedByUs) try { dbg.detach() } catch { /* 忽略 */ }
      log('[bridge] 已停止')
    }
  }
}
