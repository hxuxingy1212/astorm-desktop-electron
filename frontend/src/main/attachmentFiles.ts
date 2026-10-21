/**
 * 用户消息附件：图片 staging 落盘 + `astorm-attachment://` 协议。
 *
 * 数据流：渲染层三入口（「+」文件选择 / 粘贴截图 / 拖拽）统一交主进程处理，按**文件内容**分两类：
 * - **图片**（magic bytes 仅 JPEG/PNG/GIF/WebP、单张 ≤20MB）：复制到
 *   `<userData>/attachment-staging/<uuid>.<ext>`，返回 staging 路径；后端再复制进会话工作区
 *   `.astorm/attachment/` 并作为图片上送。
 * - **其他文件**：**不落盘、不复制**，原样返回用户原始路径（后端只把这路径写进用户消息告诉
 *   模型，由 agent 按类型自行读取（PDF/Office 用 astorm-office 技能、图片用 read_images、
 *   文本用 file_operation））。故这里只 stat 取体积，不读内容。
 * 两类都返回 `{ path, name, size, mime }[]`（见 {@link StagedAttachment}）。
 *
 * 缩略图/大图经 `astorm-attachment://` 协议直接 `<img>` 读取（避开 `file:read-buffer`
 * 的 8MB base64 上限）。协议**只允许**两类路径：staging 目录内、以及工作区附件目录
 * `**\/.astorm/attachment/**`（越界 403）——非图片不需要取字节（前端只显示文件图标），
 * 因此用户原始路径不在白名单内也不影响。
 */
import { app, clipboard, protocol } from 'electron'
import { promises as fs } from 'fs'
import { randomUUID } from 'crypto'
import { basename, extname, join, normalize, sep } from 'path'

/** 单张图片字节上限（与后端一致：20MB）。 */
const MAX_IMAGE_BYTES = 20 * 1024 * 1024
/** 单次调用图片数量上限（与后端单条消息上限一致）。 */
const MAX_IMAGES = 8
/** 非图片附件单文件体积上限（仅提示用：既不读内容也不复制，放宽到 100MB）。 */
const MAX_FILE_BYTES = 100 * 1024 * 1024
/** 非图片附件数量上限（与后端 `MAX_EXTERNAL_FILES` 一致）。 */
const MAX_FILES = 20
/** 判定文件类型只需读文件头这么多字节（4 种图片格式的魔数都在这之内）。 */
const HEAD_SNIFF_BYTES = 64
/** staging 副本保留时长（超过则启动时清理；未发送的排队消息在 24h 内不受影响）。 */
const STAGING_TTL_MS = 24 * 60 * 60 * 1000

/**
 * 附件描述（渲染层展示 + 随消息下发）。
 *
 * `path` 的含义分两类：图片 = staging 副本路径（后端据此复制进工作区）；
 * 非图片 = **用户原始路径**（后端只记录，不复制）。
 */
export interface StagedAttachment {
  /** 图片:staging 绝对路径；非图片:用户原始绝对路径。 */
  path: string
  /** 原始文件名（展示用）。 */
  name: string
  /** 字节数。 */
  size: number
  /** MIME（图片按 magic bytes；非图片按扩展名粗判，兜底 `application/octet-stream`）。 */
  mime: string
}

/** staging 结果：成功项 + 逐项失败原因（供前端即时提示）。 */
export interface StageResult {
  ok: StagedAttachment[]
  failed: { name: string; reason: string }[]
}

/** 按 magic bytes 判定图片格式（与后端一致；不看扩展名）。 */
function detectFormat(buf: Buffer): { mime: string; ext: string } | null {
  if (buf.length >= 3 && buf[0] === 0xff && buf[1] === 0xd8 && buf[2] === 0xff) {
    return { mime: 'image/jpeg', ext: 'jpg' }
  }
  if (
    buf.length >= 8 &&
    buf[0] === 0x89 && buf[1] === 0x50 && buf[2] === 0x4e && buf[3] === 0x47 &&
    buf[4] === 0x0d && buf[5] === 0x0a && buf[6] === 0x1a && buf[7] === 0x0a
  ) {
    return { mime: 'image/png', ext: 'png' }
  }
  if (buf.length >= 6 && (buf.subarray(0, 6).toString('latin1') === 'GIF87a' || buf.subarray(0, 6).toString('latin1') === 'GIF89a')) {
    return { mime: 'image/gif', ext: 'gif' }
  }
  if (buf.length >= 12 && buf.subarray(0, 4).toString('latin1') === 'RIFF' && buf.subarray(8, 12).toString('latin1') === 'WEBP') {
    return { mime: 'image/webp', ext: 'webp' }
  }
  return null
}

function humanSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`
  return `${bytes} B`
}

/** staging 目录（应用数据目录下；启动时创建）。 */
export function stagingDir(): string {
  return join(app.getPath('userData'), 'attachment-staging')
}

/** 启动时清理过期 staging 副本（未发送的排队消息在 TTL 内不受影响）。 */
export async function cleanupStaging(): Promise<void> {
  const dir = stagingDir()
  try {
    const entries = await fs.readdir(dir, { withFileTypes: true })
    const now = Date.now()
    await Promise.all(
      entries
        .filter((e) => e.isFile())
        .map(async (e) => {
          const path = join(dir, e.name)
          try {
            const stat = await fs.stat(path)
            if (now - stat.mtimeMs > STAGING_TTL_MS) await fs.unlink(path)
          } catch {
            /* 单个文件失败不影响其他 */
          }
        })
    )
  } catch {
    /* 目录不存在（首次启动）→ 无需清理 */
  }
}

/** 校验并落盘一张图片到 staging；返回描述（失败给出可读原因）。 */
async function stageBuffer(
  buf: Buffer,
  name: string
): Promise<StagedAttachment | { reason: string }> {
  if (buf.length === 0) return { reason: '文件为空' }
  if (buf.length > MAX_IMAGE_BYTES) {
    return { reason: `图片过大（${humanSize(buf.length)}，上限 ${humanSize(MAX_IMAGE_BYTES)}）` }
  }
  const format = detectFormat(buf)
  if (!format) return { reason: '不支持的图片格式（仅支持 JPEG/PNG/GIF/WebP）' }
  const dir = stagingDir()
  await fs.mkdir(dir, { recursive: true })
  const path = join(dir, `${randomUUID()}.${format.ext}`)
  await fs.writeFile(path, buf)
  return { path, name, size: buf.length, mime: format.mime }
}

/** 从本地路径批量处理（「+」文件选择 / 拖拽得到的真实路径）：图片 staging，其他文件原样引用。 */
export async function stagePaths(paths: string[]): Promise<StageResult> {
  const result: StageResult = { ok: [], failed: [] }
  let images = 0
  let files = 0
  for (const raw of paths) {
    const name = basename(raw) || raw
    try {
      // 只读文件头判类型：图片才值得全量读入并落盘，其他文件仅 stat（不读内容、不复制）
      const head = await readHead(raw, HEAD_SNIFF_BYTES)
      if (detectFormat(head)) {
        if (images >= MAX_IMAGES) {
          result.failed.push({ name, reason: `一次最多添加 ${MAX_IMAGES} 张图片` })
          continue
        }
        images += 1
        const buf = await fs.readFile(raw)
        const staged = await stageBuffer(buf, name)
        if ('reason' in staged) result.failed.push({ name, reason: staged.reason })
        else result.ok.push(staged)
        continue
      }
      if (files >= MAX_FILES) {
        result.failed.push({ name, reason: `一次最多添加 ${MAX_FILES} 个文件` })
        continue
      }
      const stat = await fs.stat(raw)
      if (stat.isDirectory()) {
        result.failed.push({ name, reason: '这是目录，不是文件' })
        continue
      }
      if (stat.size > MAX_FILE_BYTES) {
        result.failed.push({
          name,
          reason: `文件过大（${humanSize(stat.size)}，上限 ${humanSize(MAX_FILE_BYTES)}）`
        })
        continue
      }
      files += 1
      result.ok.push({ path: raw, name, size: stat.size, mime: extMime(name) })
    } catch (e) {
      result.failed.push({ name, reason: `读取失败：${e instanceof Error ? e.message : String(e)}` })
    }
  }
  return result
}

/** 读文件头前 `n` 字节（文件更短则返回已有的部分）。 */
async function readHead(path: string, n: number): Promise<Buffer> {
  const fh = await fs.open(path, 'r')
  try {
    const buf = Buffer.alloc(n)
    const { bytesRead } = await fh.read(buf, 0, n, 0)
    return buf.subarray(0, bytesRead)
  } finally {
    await fh.close()
  }
}

/** 非图片附件 MIME：按扩展名粗判（内容不嗅探），未知一律通用二进制。 */
const EXT_MIME: Record<string, string> = {
  pdf: 'application/pdf',
  doc: 'application/msword',
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  xls: 'application/vnd.ms-excel',
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  ppt: 'application/vnd.ms-powerpoint',
  pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  txt: 'text/plain',
  md: 'text/markdown',
  csv: 'text/csv',
  json: 'application/json',
  zip: 'application/zip',
  svg: 'image/svg+xml'
}

function extMime(name: string): string {
  const ext = extname(name).replace(/^\./, '').toLowerCase()
  return EXT_MIME[ext] ?? 'application/octet-stream'
}

/** 从字节 staging（拖拽的无路径 File / 渲染层已读出的二进制）。 */
export async function stageImageBytes(bytes: Uint8Array, name: string): Promise<StageResult> {
  try {
    const staged = await stageBuffer(Buffer.from(bytes), name || 'image.png')
    return 'reason' in staged
      ? { ok: [], failed: [{ name, reason: staged.reason }] }
      : { ok: [staged], failed: [] }
  } catch (e) {
    return { ok: [], failed: [{ name, reason: `写入失败：${e instanceof Error ? e.message : String(e)}` }] }
  }
}

/** 粘贴截图：主进程直接读系统剪贴板图片并落盘（无路径来源的统一入口）。 */
export async function stageClipboardImage(): Promise<StageResult> {
  const image = clipboard.readImage()
  if (image.isEmpty()) return { ok: [], failed: [{ name: '剪贴板', reason: '剪贴板中没有图片' }] }
  const buf = image.toPNG()
  const name = `粘贴图片_${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.png`
  return stageImageBytes(buf, name)
}

/** staging 目录内 / 工作区附件目录内的路径才允许经协议读取。 */
export function isAllowedAttachmentPath(path: string): boolean {
  const p = normalize(path).replace(/\\/g, '/')
  const staging = stagingDir().replace(/\\/g, '/')
  if (p === staging || p.startsWith(staging + '/')) return true
  // 会话工作区附件目录（后端复制落点）
  return p.includes('/.astorm/attachment/')
}

const MIME_BY_EXT: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp'
}

/** 注册 `astorm-attachment://` 协议（scheme 权限在 app ready 前注册）。 */
export function registerAttachmentProtocol(): void {
  protocol.handle('astorm-attachment', async (request) => {
    const url = new URL(request.url)
    // 形如 astorm-attachment://file/<seg1>/<seg2>/x.png（各段 encodeURIComponent 后的绝对路径）
    const segs = url.pathname
      .split('/')
      .filter(Boolean)
      .map((s) => {
        try {
          return decodeURIComponent(s)
        } catch {
          return s
        }
      })
    if (!segs.length) return new Response('Not Found', { status: 404 })
    // preload 对 UNC 路径插入了 __UNC__ 标记（切分时会丢掉前导 \\）
    let unc = false
    if (segs[0] === '__UNC__') {
      unc = true
      segs.shift()
    }
    let target = segs.join(sep)
    if (unc) {
      target = sep + sep + target
    } else if (process.platform !== 'win32' && !target.startsWith(sep)) {
      // 非 Windows：绝对路径以分隔符开头（分段拼回时丢掉了前导分隔符）
      target = sep + target
    }
    target = normalize(target)
    if (!isAllowedAttachmentPath(target)) return new Response('Forbidden', { status: 403 })
    try {
      const buf = await fs.readFile(target)
      const ext = (target.split('.').pop() ?? '').toLowerCase()
      return new Response(buf, {
        headers: {
          'content-type': MIME_BY_EXT[ext] ?? 'application/octet-stream',
          'cache-control': 'no-cache'
        }
      })
    } catch {
      return new Response('Not Found', { status: 404 })
    }
  })
}
