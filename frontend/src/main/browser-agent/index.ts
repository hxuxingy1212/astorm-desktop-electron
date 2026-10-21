import { app, BrowserWindow, ipcMain, type WebContents } from 'electron'
import { randomUUID } from 'crypto'
import { join } from 'path'
import { rmSync } from 'fs'
import { writeFileAtomicSync } from '../atomicWrite'
import { startFrameBridge, type BridgeCommandResult, type BridgeEndpoint, type BridgeTabInfo, type FrameBridge } from './bridge'

/**
 * 「AI 浏览器代理操控」主进程侧管理器（会话级，多页签）。
 *
 * 生命周期（与用户确认的设计一致）：
 *   - **一个 astorm 会话一个实例**：覆盖该会话**全部浏览器页签**（渲染层每个页签一个 iframe）；
 *   - 页签增删/切换只 `updateTabs`，不重建桥（保住 WS 与 agent-browser daemon 连接）；
 *   - 会话切换 / 页签清空 → 桥整体销毁；
 *   - 实例空闲回收交给 agent-browser 自己的 idle 超时（工具侧设 20 分钟）。
 *
 * 端点落盘到 userData/browser-agent.json(0600)：Rust 工具与 provider 插件没有 Rust→JS
 * 回调通道，只能靠文件/环境变量拿端点。端点带 `sessionId`，Rust 工具据此校验"这条端点
 * 属于我这条会话"，避免工具跨会话误连。
 */

/** 浏览器页签 iframe 的选择器：只命中**激活**页签的 iframe（多页签时不能不带作用域） */
const FRAME_SELECTOR = '.bw-pane--active .bw-frame'

export interface BrowserAgentStatus {
  running: boolean
  endpoint?: BridgeEndpoint
  /** 启动失败原因(如用户正开着 DevTools) */
  reason?: string
  /** endpoint 文件绝对路径(工具/插件读取用) */
  endpointFile: string
}

interface Current {
  sessionId: string
  bridge: FrameBridge
}

let current: Current | null = null
/** 串行化：渲染侧页签变化很密集，必须一次一次地起停/更新 */
let queue: Promise<unknown> = Promise.resolve()
/** main → renderer 命令的待回执表 */
const pending = new Map<string, (r: BridgeCommandResult) => void>()

function endpointFile(): string {
  return join(app.getPath('userData'), 'browser-agent.json')
}

function writeEndpointFile(endpoint: BridgeEndpoint): void {
  try {
    // 带上 pid 与时间戳：工具/插件可据此判断端点是"活的"还是上次遗留的
    const payload = { ...endpoint, pid: process.pid, updatedAt: new Date().toISOString() }
    writeFileAtomicSync(endpointFile(), JSON.stringify(payload, null, 2) + '\n', 'utf8', 0o600)
  } catch (err) {
    console.warn('[browser-agent] 写端点文件失败:', (err as Error).message)
  }
}

function removeEndpointFile(): void {
  try {
    rmSync(endpointFile(), { force: true })
  } catch {
    /* 忽略 */
  }
}

/**
 * 停止当前实例。**即使没有实例在跑也要删端点文件**（不变式：running ⇔ 端点文件存在），
 * 否则会留下指向死端口的 browser-agent.json。
 */
async function teardown(): Promise<void> {
  const cur = current
  current = null
  removeEndpointFile()
  if (cur) {
    try {
      await cur.bridge.stop()
    } catch (err) {
      console.warn('[browser-agent] 停止失败:', (err as Error).message)
    }
  }
}

/** debugger.attach 失败(典型:该 webContents 已开着 DevTools)—— 给出可读原因 */
function attachReason(err: unknown): string {
  const msg = String((err as Error)?.message ?? err)
  if (/another debugger/i.test(msg)) return 'DevTools 已打开,与 AI 操控互斥;请先关闭 DevTools'
  return msg
}

/** 主进程 → 渲染层请求开/关/切页签（渲染层执行后经 `browser-agent:command-result` 回执） */
function sendCommand(
  sender: WebContents,
  action: 'create' | 'close' | 'activate',
  payload: { url?: string; tabId?: string }
): Promise<BridgeCommandResult> {
  const requestId = randomUUID()
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      pending.delete(requestId)
      resolve({ ok: false, message: '渲染层命令超时（页面可能未响应）' })
    }, 5000)
    pending.set(requestId, (r) => {
      clearTimeout(timer)
      pending.delete(requestId)
      resolve(r)
    })
    try {
      sender.send('browser-agent:command', { requestId, action, ...payload })
    } catch (err) {
      clearTimeout(timer)
      pending.delete(requestId)
      resolve({ ok: false, message: String((err as Error)?.message ?? err) })
    }
  })
}

/**
 * 设置某会话的浏览器页签集合（渲染层上报）。
 * 页签为空 / 无会话 → 停桥；同会话 → 只更新目标表（保住 WS 与 daemon）。
 */
export async function setSessionTabs(
  sender: WebContents,
  info: { sessionId?: string; tabs?: BridgeTabInfo[]; activeId?: string }
): Promise<BrowserAgentStatus> {
  const run = async (): Promise<BrowserAgentStatus> => {
    const sessionId = String(info.sessionId ?? '')
    const tabs = Array.isArray(info.tabs) ? info.tabs.filter((t) => t && t.id) : []
    if (!sessionId || tabs.length === 0) {
      await teardown()
      return { running: false, endpointFile: endpointFile() }
    }
    // 同会话：只更新页签表，不重建桥
    if (current && current.sessionId === sessionId) {
      current.bridge.updateTabs(tabs, sessionId)
      writeEndpointFile(current.bridge.endpoint)
      return { running: true, endpoint: current.bridge.endpoint, endpointFile: endpointFile() }
    }
    await teardown()
    try {
      const bridge = await startFrameBridge({
        wc: sender,
        tabs,
        sessionId,
        frameSelector: FRAME_SELECTOR,
        command: (action, payload) => sendCommand(sender, action, payload),
        log: (m) => console.log('[browser-agent]', m)
      })
      current = { sessionId, bridge }
      writeEndpointFile(bridge.endpoint)
      return { running: true, endpoint: bridge.endpoint, endpointFile: endpointFile() }
    } catch (err) {
      removeEndpointFile()
      const reason = attachReason(err)
      console.warn('[browser-agent] 启动失败:', reason)
      return { running: false, reason, endpointFile: endpointFile() }
    }
  }
  queue = queue.then(run, run)
  return queue as Promise<BrowserAgentStatus>
}

/**
 * 页面弹窗（`window.open` / `target=_blank`）转内置新页签：
 * 当前会话已有桥 → 请求渲染层新建页签（自动前台）；返回是否已转内置。
 */
export async function openPopupAsTab(url: string): Promise<boolean> {
  const cur = current
  if (!cur) return false
  const id = await cur.bridge.createTab(url)
  return !!id
}

/** 当前是否有可用的浏览器实例（弹窗判定：有实例才转内置，否则维持系统浏览器行为） */
export function hasBrowserAgentBridge(): boolean {
  return current !== null
}

export async function stopBrowserAgent(): Promise<void> {
  queue = queue.then(teardown, teardown)
  await queue
}

export function browserAgentStatus(): BrowserAgentStatus {
  if (!current) return { running: false, endpointFile: endpointFile() }
  return { running: true, endpoint: current.bridge.endpoint, endpointFile: endpointFile() }
}

export function registerBrowserAgentIpc(): void {
  ipcMain.handle('browser-agent:set-tabs', async (event, info: unknown) => {
    const win = BrowserWindow.fromWebContents(event.sender)
    // 只接受来自窗口渲染进程的上报(iframe 在窗口渲染页里)
    const sender = win?.webContents ?? event.sender
    return setSessionTabs(sender, (info ?? {}) as { sessionId?: string; tabs?: BridgeTabInfo[]; activeId?: string })
  })
  // 渲染层回执：开/关/切页签命令的异步结果
  ipcMain.handle('browser-agent:command-result', (_event, result: unknown) => {
    const r = (result ?? {}) as { requestId?: string; ok?: boolean; tabId?: string; message?: string }
    if (r.requestId) pending.get(r.requestId)?.(r as BridgeCommandResult)
    return true
  })
  ipcMain.handle('browser-agent:status', () => browserAgentStatus())
  ipcMain.handle('browser-agent:stop', async () => {
    await stopBrowserAgent()
    return browserAgentStatus()
  })
  // 退出前清掉端点文件(避免留下一个死端点让工具连)
  app.on('before-quit', () => {
    void stopBrowserAgent()
  })
}
