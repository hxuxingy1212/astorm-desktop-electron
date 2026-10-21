/**
 * 浏览器页签截图（UI 侧按钮，区别于 AI 代理操控的截图链路）。
 *
 * 采集方式：`webContents.capturePage(rect)` —— Electron 自带合成器快照。
 * 为什么不用 CDP：`webContents.debugger` 是**独占**资源，已被 `browser-agent/bridge.ts`
 * 占用（AI 代理操控那条链路），无法另开一条 CDP 会话；而 capturePage 不依赖桥的生命周期。
 *
 * 与 AI 截图（`bridge.ts:294-335` 的「宿主页会话 + iframe 矩形 clip」）口径一致：
 * 都只截 iframe **当前可见区域**（越界部分拿不到，两边限制相同）。
 *
 * 落盘：`<工作区>/.astorm/browser-capture/<tabId>-<yyyyMMdd-HHmmss>[-n].png`，
 * 命名与保留策略对齐后端 `tool/browser.rs:500-557`（保留最近 SNAPSHOT_KEEP 张）。
 */
import { app, ipcMain, type WebContents } from 'electron'
import { join } from 'path'
import { mkdir, readdir, stat, unlink, writeFile } from 'fs/promises'

/** 采集矩形：渲染层视口坐标下的 CSS px（与 preload/index.d.ts 的 CaptureRect 保持一致） */
interface CaptureRect {
  x: number
  y: number
  width: number
  height: number
}

/** 截图请求附加信息（与 preload/index.d.ts 的 BrowserCaptureOptions 保持一致） */
interface CaptureOptions {
  /** 网页页签 id，仅用于文件名（主进程会做字母数字截断） */
  tabId?: string
  /** 绝对工作区目录；缺省回落到系统图片目录 */
  workspaceDir?: string
}

export interface BrowserCaptureResult {
  ok: boolean
  /** 成功时的绝对路径 */
  path?: string
  /** 失败原因（错误码或异常信息） */
  message?: string
}

/** 单帧结果（录像用；不落盘，直接把 JPEG 字节交给渲染层） */
export interface BrowserFrameResult {
  ok: boolean
  /** JPEG 字节（渲染层 decode 后画到 canvas 供 MediaRecorder 编码） */
  data?: Uint8Array
  width?: number
  height?: number
  message?: string
}

/** 每目录保留的截图/录像数量（对齐后端 SNAPSHOT_KEEP） */
const SNAPSHOT_KEEP = 50

/** 产物扩展名：截图与录像共用同一个目录与保留策略 */
const CAPTURE_EXT = ['.png', '.webm']

/** 单帧 JPEG 质量：录制是逐帧传输，用有损压缩把每帧压到几十 KB */
const FRAME_JPEG_QUALITY = 80

function isRect(v: unknown): v is CaptureRect {
  if (!v || typeof v !== 'object') return false
  const r = v as Record<string, unknown>
  return (['x', 'y', 'width', 'height'] as const).every(
    (k) => typeof r[k] === 'number' && Number.isFinite(r[k] as number)
  )
}

/**
 * 渲染层 CSS px → capturePage 坐标。
 *
 * 页面缩放（`zoom:set` 改 webContents zoomFactor）会改变 CSS px 与设备无关像素的换算，
 * 需按 zoomFactor 补偿；zoomFactor 为 1（默认）时原样透传。
 */
function toCaptureRect(rect: CaptureRect, wc: WebContents): Electron.Rectangle {
  const zoom = wc.getZoomFactor() || 1
  return {
    x: Math.round(rect.x * zoom),
    y: Math.round(rect.y * zoom),
    width: Math.round(rect.width * zoom),
    height: Math.round(rect.height * zoom)
  }
}

/** 本地时间戳 `yyyyMMdd-HHmmss`（与后端 snapshot_path 同格式） */
function stamp(d = new Date()): string {
  const p = (n: number): string => String(n).padStart(2, '0')
  return (
    `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}` +
    `-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`
  )
}

/** tabId → 文件名安全片段：仅字母数字，截 16 位（与后端 browser.rs 同规则） */
function safeTabId(tabId?: string): string {
  const s = (tabId ?? '').replace(/[^a-zA-Z0-9]/g, '').slice(0, 16)
  return s || 'tab'
}

/** 目标目录：工作区下 `.astorm/browser-capture/`；无工作区时回落到系统图片目录 */
function captureDir(workspaceDir?: string): string {
  const ws = (workspaceDir ?? '').trim()
  if (ws) return join(ws, '.astorm', 'browser-capture')
  return join(app.getPath('pictures'), 'astorm-captures')
}

/** 目录内保留最近 SNAPSHOT_KEEP 个产物(截图 + 录像)，其余按 mtime 从旧到新删除 */
async function pruneCaptures(dir: string): Promise<void> {
  try {
    const names = (await readdir(dir)).filter((n) =>
      CAPTURE_EXT.some((e) => n.toLowerCase().endsWith(e))
    )
    if (names.length <= SNAPSHOT_KEEP) return
    const stats = await Promise.all(
      names.map(async (n) => {
        const p = join(dir, n)
        try {
          return { p, mtime: (await stat(p)).mtimeMs }
        } catch {
          return { p, mtime: 0 }
        }
      })
    )
    stats.sort((a, b) => b.mtime - a.mtime)
    for (const { p } of stats.slice(SNAPSHOT_KEEP)) {
      try {
        await unlink(p)
      } catch {
        /* 已消失则忽略 */
      }
    }
  } catch {
    /* 目录不可读时忽略(不影响本次截图) */
  }
}

/** 在目录内挑一个未占用的文件名：`<tabId>-<时间戳>[-n].<ext>` */
async function freeName(dir: string, tabId: string | undefined, ext: string): Promise<string> {
  const base = `${safeTabId(tabId)}-${stamp()}`
  let file = join(dir, `${base}.${ext}`)
  for (let n = 2; n < 100; n++) {
    try {
      await stat(file)
      file = join(dir, `${base}-${n}.${ext}`)
    } catch {
      break // 不存在即可用
    }
  }
  return file
}

/** IPC 传来的二进制可能是 Uint8Array / ArrayBuffer / Buffer，统一成 Buffer */
function toBuffer(v: unknown): Buffer | null {
  if (v instanceof Uint8Array) return Buffer.from(v.buffer, v.byteOffset, v.byteLength)
  if (v instanceof ArrayBuffer) return Buffer.from(v)
  return null
}

export function registerBrowserCaptureIpc(): void {
  // 截图：capturePage → PNG 落盘
  ipcMain.handle(
    'browser:capture',
    async (event, rect: unknown, opts: unknown): Promise<BrowserCaptureResult> => {
      if (!isRect(rect)) return { ok: false, message: 'BAD_RECT' }
      if (rect.width <= 0 || rect.height <= 0) return { ok: false, message: 'EMPTY_AREA' }

      const options = (opts ?? {}) as CaptureOptions
      const wc = event.sender

      try {
        const image = await wc.capturePage(toCaptureRect(rect, wc))
        if (image.isEmpty()) return { ok: false, message: 'CAPTURE_EMPTY' }

        const dir = captureDir(options.workspaceDir)
        await mkdir(dir, { recursive: true })
        const file = await freeName(dir, options.tabId, 'png')

        await writeFile(file, image.toPNG())
        void pruneCaptures(dir)
        return { ok: true, path: file }
      } catch (err) {
        return { ok: false, message: err instanceof Error ? err.message : String(err) }
      }
    }
  )

  // 录制取帧：capturePage → JPEG 字节(不落盘)。由渲染层按帧率驱动，
  // 逐帧 decode 到 canvas，再由 MediaRecorder 编码成 webm。
  ipcMain.handle(
    'browser:frame',
    async (event, rect: unknown): Promise<BrowserFrameResult> => {
      if (!isRect(rect)) return { ok: false, message: 'BAD_RECT' }
      if (rect.width <= 0 || rect.height <= 0) return { ok: false, message: 'EMPTY_AREA' }
      const wc = event.sender
      try {
        const image = await wc.capturePage(toCaptureRect(rect, wc))
        if (image.isEmpty()) return { ok: false, message: 'CAPTURE_EMPTY' }
        const { width, height } = image.getSize()
        return { ok: true, data: image.toJPEG(FRAME_JPEG_QUALITY), width, height }
      } catch (err) {
        return { ok: false, message: err instanceof Error ? err.message : String(err) }
      }
    }
  )

  // 录像落盘：渲染层把 MediaRecorder 产出的 webm 字节交回来写文件
  ipcMain.handle(
    'browser:record-save',
    async (event, payload: unknown): Promise<BrowserCaptureResult> => {
      const p = (payload ?? {}) as { data?: unknown; tabId?: string; workspaceDir?: string }
      const buf = toBuffer(p.data)
      if (!buf || buf.byteLength === 0) return { ok: false, message: 'EMPTY_DATA' }
      try {
        const dir = captureDir(p.workspaceDir)
        await mkdir(dir, { recursive: true })
        const file = await freeName(dir, p.tabId, 'webm')
        await writeFile(file, buf)
        void pruneCaptures(dir)
        return { ok: true, path: file }
      } catch (err) {
        return { ok: false, message: err instanceof Error ? err.message : String(err) }
      }
    }
  )
}
