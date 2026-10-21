// astorm-office 预览:用随包 CLI `unpack --inline` 产出单文件自包含 JSON(分片内联 + 媒体转
// data URI),读回 renderer 直接喂 @astorm/office-viewer 的 data prop——无协议、无 1+N fetch。
// 缓存按 path+mtime+size 哈希落 userData/office-preview,同文件二次打开免重新解包;
// 套件缺失 / 不支持的格式 / 解包失败返回 ok:false(前端回退旧渲染器或外部打开)。
import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { existsSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { app, ipcMain } from 'electron'
import { resolveOfficeDir } from './settings-sync'

/** 新格式 → viewer kind(legacy .doc/.xls/.ppt 不在此列,预览走旧渲染器) */
function kindOf(name: string): 'docx' | 'xlsx' | 'pptx' | 'pdf' | undefined {
  const ext = name.toLowerCase().split('.').pop() ?? ''
  if (ext === 'pdf') return 'pdf'
  if (ext === 'docx' || ext === 'docm') return 'docx'
  if (ext === 'xlsx' || ext === 'xlsm') return 'xlsx'
  if (ext === 'pptx' || ext === 'pptm' || ext === 'ppsx' || ext === 'pps') return 'pptx'
  return undefined
}

export interface OfficePreviewResult {
  ok: boolean
  kind?: 'docx' | 'xlsx' | 'pptx' | 'pdf'
  /** 单文件产物(分片内联 + 媒体 data URI),直接作 OfficeViewer 的 data prop */
  data?: unknown
  message?: string
}

/** 注册 office 预览 IPC(主进程统一注册处调用) */
export function registerOfficePreviewIpc(): void {
  ipcMain.handle('office:preview-open', async (_event, filePath: unknown): Promise<OfficePreviewResult> => {
    if (typeof filePath !== 'string' || !filePath.trim()) return { ok: false, message: '路径为空' }
    const p = filePath.trim()
    const kind = kindOf(p)
    if (!kind) {
      return {
        ok: false,
        message: '该格式不支持 office 高保真预览(支持 docx/docm、xlsx/xlsm、pptx/pptm/ppsx/pps、pdf;旧版 .doc/.xls/.ppt 请先转换为新格式)'
      }
    }
    const office = resolveOfficeDir()
    if (!office) {
      return { ok: false, message: 'office 预览组件未同步:请运行 pnpm run sync:office(需 astorm-office 仓库)后重启' }
    }
    const bin = join(office, 'bin', process.platform === 'win32' ? `json2${kind}.exe` : `json2${kind}`)
    if (!existsSync(bin)) {
      return { ok: false, message: `未找到 office CLI: ${bin}\n请运行 pnpm run sync:office 后重启` }
    }

    // 缓存:path+mtime+size 哈希(内容变化即失效;userData 下按 key 平铺)
    const key = await cacheKey(p)
    const cacheDir = join(app.getPath('userData'), 'office-preview')
    const cacheFile = join(cacheDir, `${key}.json`)
    if (existsSync(cacheFile)) {
      try {
        return { ok: true, kind, data: JSON.parse(await readFile(cacheFile, 'utf8')) }
      } catch {
        // 缓存损坏 → 重新解包覆盖
      }
    }

    const workDir = await mkdtemp(join(tmpdir(), 'office-preview-'))
    try {
      const outPath = join(workDir, 'inline.json')
      await runCli(bin, ['unpack', p, '--inline', '-o', outPath])
      const text = await readFile(outPath, 'utf8')
      const data = JSON.parse(text)
      await mkdir(cacheDir).catch(() => undefined)
      await writeFile(cacheFile, text).catch(() => undefined)
      return { ok: true, kind, data }
    } catch (e) {
      return { ok: false, message: e instanceof Error ? e.message : String(e) }
    } finally {
      rm(workDir, { recursive: true, force: true }).catch(() => undefined)
    }
  })
}

async function cacheKey(p: string): Promise<string> {
  // kit 构建戳(_metadata.json 的 built_at)进 key:office 侧重新导出 kit 后,
  // 同一文件(path+mtime 未变)的旧缓存自动失效,避免「修好了却还看旧解包」
  let kitStamp = ''
  const office = resolveOfficeDir()
  if (office) {
    try {
      kitStamp = String(JSON.parse(readFileSync(join(office, '_metadata.json'), 'utf8')).built_at ?? '')
    } catch {
      /* 元数据缺失退化为不含戳的 key */
    }
  }
  try {
    const st = await stat(p)
    return createHash('sha1')
      .update(`${p}\n${st.mtimeMs}\n${st.size}\n${kitStamp}\n`)
      .digest('hex')
      .slice(0, 20)
  } catch {
    return createHash('sha1').update(`${p}\n${kitStamp}\n`).digest('hex').slice(0, 20)
  }
}

/** 执行 CLI(超时 90s;失败抛含 stderr 尾部的错误) */
function runCli(cli: string, args: string[]): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(
      cli,
      args,
      { timeout: 90_000, windowsHide: true, maxBuffer: 4 * 1024 * 1024 },
      (err, _stdout, stderr) => {
        if (err) {
          const detail = String(stderr || '').trim() || (err instanceof Error ? err.message : String(err))
          reject(new Error(`office 解包失败: ${detail}`))
          return
        }
        resolve()
      }
    )
  })
}
