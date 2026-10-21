import { app, ipcMain, BrowserWindow, dialog, nativeTheme, shell, webContents } from 'electron'
import { execFile } from 'child_process'
import { existsSync, readFileSync } from 'fs'
import { mkdtemp, readFile, rm, writeFile } from 'fs/promises'
import { tmpdir } from 'os'
import { join } from 'path'
import { registerElementPickerIpc } from './element-picker'
import { registerOfficePreviewIpc } from './officePreview'
import { systemDir } from './systemPaths'
import {
  stageClipboardImage,
  stageImageBytes,
  stagePaths,
  type StageResult
} from './attachmentFiles'

const clampZoom = (factor: number): number => Math.min(2, Math.max(0.5, Number(factor) || 1))

/** 图片预览二进制上限(8MB):超出返回空,前端回退外部打开 */
const IMAGE_MAX_BYTES = 8 * 1024 * 1024
/** office 等二进制预览读取上限(64MB) */
const PREVIEW_MAX_BYTES = 64 * 1024 * 1024

/* ---------- git(git -C <dir> ...) ---------- */

/** 分支菜单一次拉齐的状态(与 preload `window.api.git.branchState` 对应)。 */
export interface GitBranchState {
  /** 当前分支(分离 HEAD 时 git 返回 "HEAD" 字面量) */
  current: string
  /** 本地分支(refs/heads) */
  local: string[]
  /** 远程分支(refs/remotes,如 origin/master;含 origin/HEAD) */
  remotes: string[]
  /** 未提交的文件数(含未跟踪) */
  uncommitted: number
}

/** 提交行(与 preload `window.api.git.log` 对应)。 */
export interface GitCommit {
  /** 短 hash */
  hash: string
  author: string
  /** ISO 时间(用于格式化显示) */
  dateISO: string
  /** git 的相对时间(如 "2 hours ago",用于 tooltip) */
  dateRel: string
  subject: string
  /** ref 装饰(`%D` 按 ", " 拆开;如 ["HEAD -> master", "origin/master"]);`local`/`remotes` 用于区分同名分支 */ 
  decorations: string[]
}

/** git log 记录数上限(默认 200) */
const GIT_LOG_DEFAULT = 200
const GIT_LOG_MAX = 1000

/**
 * 跑一次 git 子进程并取 stdout(`git -C <dir> <args...>`)。
 *
 * 非 0 退出时 ok=false 且 error 为 stderr **末行**——调用方自己决定是回退空值(读类命令)
 * 还是把原因透给用户(写类命令，如 checkout)。
 */
function runGit(
  dir: string,
  args: string[],
  timeout = 8000
): Promise<{ ok: boolean; stdout: string; error: string }> {
  return new Promise((resolve) => {
    execFile(
      'git',
      ['-c', 'core.quotePath=false', '-C', dir, ...args],
      // 默认 maxBuffer 只有 1MB,大仓库的 status / log 会超;这里放宽到 8MB
      { timeout, maxBuffer: 8 * 1024 * 1024, windowsHide: true },
      (err, stdout, stderr) => {
        if (err) {
          const last = String(stderr || '')
            .trim()
            .split(/\r?\n/)
            .filter(Boolean)
            .pop()
          resolve({ ok: false, stdout: '', error: last || err.message })
          return
        }
        resolve({ ok: true, stdout: String(stdout), error: '' })
      }
    )
  })
}

/** 按行拆分 git 输出(去空行) */
function gitLines(out: string): string[] {
  return out
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean)
}

/**
 * 取 ref 全名并去掉分类前缀：`refs/heads/x` → `x`、`refs/remotes/origin/HEAD` → `origin/HEAD`。
 *
 * **不能用 `%(refname:short)`**：对符号 ref `refs/remotes/origin/HEAD` 它返回 `origin`
 * （把 `/HEAD` 丢了），而 `git log` 的 `%D` 给的是 `origin/HEAD`。两边对不上，
 * 徽章就会把 `origin/HEAD` 判成「未知 ref」而丢掉。
 */
function refShortNames(out: string, prefix: string): string[] {
  return gitLines(out)
    .map((n) => (n.startsWith(prefix) ? n.slice(prefix.length) : n))
    .filter(Boolean)
}

/** 目录参数是否可用 */
function hasDir(dir: unknown): dir is string {
  return typeof dir === 'string' && !!dir.trim()
}

/**
 * 分支名是否可用。
 *
 * **不能用 `--` 分隔符**规避:命令会被解析成 `git checkout -- <name>`,那是「恢复工作区路径」,
 * 不是切分支。故改为在这里拒掉以 `-` 开头的名字(会被 git 当成选项)。
 */
function validRefName(name: unknown): name is string {
  return typeof name === 'string' && !!name.trim() && !name.trim().startsWith('-')
}

/**
 * 窗口控制与缩放 IPC
 * 渲染进程通过 window.api(见 preload)调用
 */
export function registerIpcHandlers(): void {
  ipcMain.on('window:minimize', (event) => {
    BrowserWindow.fromWebContents(event.sender)?.minimize()
  })

  ipcMain.on('window:toggle-maximize', (event) => {
    const win = BrowserWindow.fromWebContents(event.sender)
    if (!win) return
    if (win.isMaximized()) win.unmaximize()
    else win.maximize()
  })

  ipcMain.on('window:close', (event) => {
    BrowserWindow.fromWebContents(event.sender)?.close()
  })

  ipcMain.handle('window:is-maximized', (event) => {
    return BrowserWindow.fromWebContents(event.sender)?.isMaximized() ?? false
  })

  ipcMain.handle('window:is-fullscreen', (event) => {
    return BrowserWindow.fromWebContents(event.sender)?.isFullScreen() ?? false
  })

  ipcMain.on('zoom:set', (event, factor: number) => {
    event.sender.setZoomFactor(clampZoom(factor))
  })

  // 原生侧外观跟随应用主题（渲染层 applyTheme 每次应用/切换都推一次）。
  //
  // 为什么必须同步：macOS 的红黄绿是系统按**窗口外观**绘制的 —— 窗口外观若是浅色，
  // 失焦时系统把三个点画成「深色填充」；是深色则画成浅灰。而窗口外观默认跟着**系统**
  // 走（用户系统是浅色、应用选了深色主题时，失焦的三个点就是近黑 (3,3,3)，在深色标题栏
  // 上等于消失）。同步后深色主题下失焦点是浅灰 (≈73,73,73)，与参考实现一致；顺带让原生
  // 菜单 / 弹窗的外观也与应用主题一致（此前深色主题 + 浅色系统会出现浅色弹窗）。
  ipcMain.on('appearance:set-theme-mode', (_event, mode: unknown) => {
    if (mode === 'light' || mode === 'dark' || mode === 'system') nativeTheme.themeSource = mode
  })

  // 打开 iframe 的调试控制台:跨域 iframe(OOPIF)是独立 webContents,
  // 按 URL 匹配找到对应 guest 并打开独立 DevTools;找不到(同进程/未加载)退回宿主页 DevTools。
  // 返回 true = 已打开 iframe 自身控制台
  ipcMain.handle('browser:open-devtools', (event, url: unknown): boolean => {
    const wanted = typeof url === 'string' ? url.replace(/\/$/, '') : ''
    const target = wanted
      ? webContents
          .getAllWebContents()
          .find((c) => c !== event.sender && c.getURL().replace(/\/$/, '') === wanted)
      : undefined
    if (target) {
      if (!target.isDevToolsOpened()) target.openDevTools({ mode: 'detach' })
      return true
    }
    event.sender.openDevTools({ mode: 'detach' })
    return false
  })

  // 内置浏览器「选择元素」:注入拾取脚本 + 取消(见 element-picker.ts)
  registerElementPickerIpc()

  // office/pdf 高保真预览:CLI unpack --inline 单文件产物(见 officePreview.ts)
  registerOfficePreviewIpc()

  // 目录选择:打开系统对话框,返回单个目录路径(取消返回空字符串)
  ipcMain.handle('dialog:open-directory', async (event) => {
    const win = BrowserWindow.fromWebContents(event.sender)
    if (!win) return ''
    const result = await dialog.showOpenDialog(win, {
      title: '选择目录',
      properties: ['openDirectory']
    })
    return result.canceled ? '' : result.filePaths[0] ?? ''
  })

  // 多选导入:文件或文件夹(macOS 原生面板支持两者混选;Windows/Linux 上
  // openFile 与 openDirectory 不能同时生效——按 Electron 文档会退化为目录选择器，
  // 因此这两个平台只做多选文件;选目录请走 dialog:open-directory)
  ipcMain.handle('dialog:open-files', async (event) => {
    const win = BrowserWindow.fromWebContents(event.sender)
    if (!win) return []
    const properties: Array<'openFile' | 'openDirectory' | 'multiSelections'> =
      process.platform === 'darwin'
        ? ['openFile', 'openDirectory', 'multiSelections']
        : ['openFile', 'multiSelections']
    const result = await dialog.showOpenDialog(win, {
      title: '导入文件',
      properties
    })
    return result.canceled ? [] : result.filePaths
  })

  // 单选 JSON 文件并读回文本(MCP 配置导入);取消返回 null
  ipcMain.handle(
    'dialog:open-json',
    async (event): Promise<{ path: string; content: string } | null> => {
      const win = BrowserWindow.fromWebContents(event.sender)
      if (!win) return null
      const result = await dialog.showOpenDialog(win, {
        title: '导入 JSON 配置',
        properties: ['openFile'],
        filters: [{ name: 'JSON', extensions: ['json'] }]
      })
      const path = result.filePaths[0]
      if (result.canceled || !path) return null
      // 去 BOM：Windows 下常见，serde_json / JSON.parse 都会拒绝
      const content = (await readFile(path, 'utf8')).replace(/^\uFEFF/, '')
      return { path, content }
    }
  )

  // 另存为:系统保存对话框,返回文件路径(取消返回空串)
  ipcMain.handle('dialog:save-file', async (event, defaultName: unknown): Promise<string> => {
    const win = BrowserWindow.fromWebContents(event.sender)
    if (!win) return ''
    const name =
      typeof defaultName === 'string' && defaultName.trim() ? defaultName.trim() : 'audit.jsonl'
    const result = await dialog.showSaveDialog(win, {
      title: '导出审计日志',
      defaultPath: name,
      filters: [
        { name: 'JSONL', extensions: ['jsonl'] },
        { name: 'JSON', extensions: ['json'] }
      ]
    })
    return result.canceled || !result.filePath ? '' : result.filePath
  })

  // 应用系统目录（config/ data/ log/ plugins/ 等的父目录）：设置页只读展示 + 打开目录
  ipcMain.handle('system:app-dir', (): string => systemDir())

  // 用系统文件管理器打开目录(Explorer/Finder);返回 '' 成功,否则错误信息
  ipcMain.handle('shell:open-path', async (_event, dir: unknown): Promise<string> => {
    if (typeof dir !== 'string' || !dir.trim()) return '路径为空'
    try {
      const err = await shell.openPath(dir.trim())
      return err || ''
    } catch (e) {
      return e instanceof Error ? e.message : String(e)
    }
  })

  // 系统浏览器打开 http(s) 链接(MCP OAuth 授权页);返回 '' 成功,否则错误信息
  ipcMain.handle('shell:open-external', async (_event, url: unknown): Promise<string> => {
    if (typeof url !== 'string' || !/^https?:\/\//i.test(url)) return '链接无效'
    try {
      await shell.openExternal(url)
      return ''
    } catch (e) {
      return e instanceof Error ? e.message : String(e)
    }
  })

  // 单选 .pptx 文件(内置插件 ppt-inspector 导入用;defaultPath 可选,取消返回空串)
  ipcMain.handle('dialog:open-pptx', async (event, defaultPath: unknown): Promise<string> => {
    const win = BrowserWindow.fromWebContents(event.sender)
    if (!win) return ''
    const result = await dialog.showOpenDialog(win, {
      title: '选择 PowerPoint 文件',
      defaultPath: typeof defaultPath === 'string' && defaultPath.trim() ? defaultPath.trim() : undefined,
      properties: ['openFile'],
      filters: [{ name: 'PowerPoint', extensions: ['pptx'] }]
    })
    return result.canceled ? '' : result.filePaths[0] ?? ''
  })

  // 多选附件(输入框「+」):**不限类型**(图片也在其中,由 staging 按内容识别),返回绝对路径数组
  ipcMain.handle('dialog:open-attachments', async (event): Promise<string[]> => {
    const win = BrowserWindow.fromWebContents(event.sender)
    if (!win) return []
    const result = await dialog.showOpenDialog(win, {
      title: '选择附件',
      properties: ['openFile', 'multiSelections']
    })
    return result.canceled ? [] : result.filePaths
  })

  // 附件处理:本地路径 → 图片落 staging 副本 / 其他文件原样引用(入口即校验,返回描述 + 逐项失败原因)
  ipcMain.handle('attachment:stage-paths', async (_event, paths: unknown): Promise<StageResult> => {
    if (!Array.isArray(paths)) {
      return { ok: [], failed: [{ name: '附件', reason: '参数非法' }] }
    }
    const list = paths.filter((p): p is string => typeof p === 'string' && !!p.trim())
    return stagePaths(list)
  })

  // 附件 staging:二进制(拖拽的无路径 File / 渲染层已读出的字节)
  ipcMain.handle(
    'attachment:stage-bytes',
    async (_event, bytes: unknown, name: unknown): Promise<StageResult> => {
      const display = typeof name === 'string' && name ? name : 'image.png'
      if (bytes instanceof Uint8Array) return stageImageBytes(bytes, display)
      if (bytes instanceof ArrayBuffer) return stageImageBytes(new Uint8Array(bytes), display)
      return { ok: [], failed: [{ name: display, reason: '参数非法' }] }
    }
  )

  // 附件 staging:粘贴截图(主进程直接读系统剪贴板图片,无路径来源的统一入口)
  ipcMain.handle('attachment:stage-clipboard', async (): Promise<StageResult> => stageClipboardImage())

  // 写文本文件(markdown 编辑保存):返回 { ok, message? }
  ipcMain.handle('file:write', async (_event, path: unknown, content: unknown): Promise<{ ok: boolean; message?: string }> => {
    if (typeof path !== 'string' || !path.trim()) return { ok: false, message: '路径为空' }
    if (typeof content !== 'string') return { ok: false, message: '内容非法' }
    try {
      await writeFile(path.trim(), content, 'utf8')
      return { ok: true }
    } catch (err) {
      return { ok: false, message: err instanceof Error ? err.message : String(err) }
    }
  })

  // office 等二进制预览:读文件为 Buffer(64MB 上限;失败/超限返回 null)
  ipcMain.handle('file:read-arraybuffer', async (_event, path: unknown): Promise<Buffer | null> => {
    if (typeof path !== 'string' || !path.trim()) return null
    try {
      const buf = await readFile(path.trim())
      if (buf.length > PREVIEW_MAX_BYTES) return null
      return buf
    } catch {
      return null
    }
  })

  // 图片预览:读文件为 base64(8MB 上限;失败/超限返回空,前端回退外部打开)
  ipcMain.handle('file:read-buffer', async (_event, path: unknown): Promise<string> => {
    if (typeof path !== 'string' || !path.trim()) return ''
    try {
      const buf = await readFile(path.trim())
      if (buf.length > IMAGE_MAX_BYTES) return ''
      return buf.toString('base64')
    } catch {
      return ''
    }
  })

  // 读取所选目录的真实 git 当前分支(git -C <dir> rev-parse --abbrev-ref HEAD);
  // 非 Git 仓库 / git 不可用 / 出错时返回空字符串(由前端决定回退)
  ipcMain.handle('git:current-branch', async (_event, dir: string) => {
    if (!hasDir(dir)) return ''
    const r = await runGit(dir, ['rev-parse', '--abbrev-ref', 'HEAD'], 5000)
    return r.ok ? r.stdout.trim() : ''
  })

  // 目录是否位于 git 工作树内(`rev-parse --is-inside-work-tree`)。
  // 与 branch-state 不同：**空仓库 / 无提交(unborn HEAD)也算仓库**(返回 true)，
  // 供代码审查页签在调用 `git diff` 前探测，避免非仓库时 stderr 刷出大段 usage。
  ipcMain.handle('git:is-repo', async (_event, dir: string): Promise<boolean> => {
    if (!hasDir(dir)) return false
    const r = await runGit(dir, ['rev-parse', '--is-inside-work-tree'], 5000)
    return r.ok && r.stdout.trim() === 'true'
  })

  // 列出所选目录 git 仓库的全部分支(git -C <dir> for-each-ref)。
  // 仅用于下拉选择,不执行任何 checkout 切换;非仓库返回空数组。
  ipcMain.handle('git:list-branches', async (_event, dir: string): Promise<string[]> => {
    if (!hasDir(dir)) return []
    const r = await runGit(dir, ['for-each-ref', '--format=%(refname:short)', 'refs/heads/'], 5000)
    return r.ok ? gitLines(r.stdout) : []
  })

  // 分支菜单的状态(一次拉齐:当前分支 / 本地 / 远程 / 未提交文件数)。
  // 非仓库、git 不可用、或仓库尚无提交(unborn HEAD,rev-parse 会失败)→ 全空,不抛错。
  ipcMain.handle('git:branch-state', async (_event, dir: string): Promise<GitBranchState> => {
    const empty: GitBranchState = { current: '', local: [], remotes: [], uncommitted: 0 }
    if (!hasDir(dir)) return empty
    const head = await runGit(dir, ['rev-parse', '--abbrev-ref', 'HEAD'], 5000)
    if (!head.ok) return empty
    const [local, remotes, status] = await Promise.all([
      runGit(dir, ['for-each-ref', '--format=%(refname)', 'refs/heads/'], 5000),
      runGit(dir, ['for-each-ref', '--format=%(refname)', 'refs/remotes/'], 5000),
      // porcelain 每行一个文件条目(重命名也是一行),行数即「N 个文件」
      runGit(dir, ['status', '--porcelain=v1', '--untracked-files=all'], 8000)
    ])
    return {
      current: head.stdout.trim(),
      local: local.ok ? refShortNames(local.stdout, 'refs/heads/') : [],
      remotes: remotes.ok ? refShortNames(remotes.stdout, 'refs/remotes/') : [],
      uncommitted: status.ok ? gitLines(status.stdout).length : 0
    }
  })

  // 提交历史(Git 图谱)。**只取当前分支**——该分支历史恒为线性,图列无需泳道算法。
  // 字段用 US(0x1f) 分隔、记录用 RS(0x1e) 分隔:subject 里可能有任何字符,
  // 拿 \t / | 当分隔符都会被真实提交信息打乱。
  ipcMain.handle(
    'git:log',
    async (
      _event,
      dir: string,
      branch: string,
      limit?: number
    ): Promise<{ commits: GitCommit[]; error?: string }> => {
      if (!hasDir(dir) || !validRefName(branch)) return { commits: [] }
      const n = Math.min(GIT_LOG_MAX, Math.max(1, Number(limit) || GIT_LOG_DEFAULT))
      const fmt = '%h%x1f%an%x1f%ad%x1f%ar%x1f%s%x1f%D%x1e'
      const r = await runGit(
        dir,
        ['log', branch, '--topo-order', '-n', String(n), '--date=iso-strict', `--format=${fmt}`],
        10000
      )
      if (!r.ok) return { commits: [], error: r.error }
      const commits: GitCommit[] = []
      for (const raw of r.stdout.split('\x1e')) {
        // 每条记录以 RS 结尾、git 再补一个换行 → 下一条记录开头会带 \n
        const rec = raw.replace(/^\s+/, '')
        if (!rec) continue
        const [hash, author, dateISO, dateRel, subject, decorate] = rec.split('\x1f')
        commits.push({
          hash: (hash ?? '').trim(),
          author: (author ?? '').trim(),
          dateISO: (dateISO ?? '').trim(),
          dateRel: (dateRel ?? '').trim(),
          subject: subject ?? '',
          decorations: (decorate ?? '')
            .split(',')
            .map((d) => d.trim())
            .filter(Boolean)
        })
      }
      return { commits }
    }
  )

  // 切换分支(**会改动用户仓库的工作区**,由用户在下拉里显式点击触发)。
  // 失败时把 git 的原始原因透回渲染层(如「本地改动会被覆盖」),由 UI 原样提示。
  ipcMain.handle(
    'git:checkout',
    async (_event, dir: string, branch: string): Promise<{ ok: boolean; error?: string }> => {
      if (!hasDir(dir) || !validRefName(branch)) return { ok: false }
      const r = await runGit(dir, ['checkout', branch.trim()])
      return r.ok ? { ok: true } : { ok: false, error: r.error }
    }
  )

  // 创建并检出分支(`checkout -b`;用 checkout 而非 switch,兼容更老的 git)
  ipcMain.handle(
    'git:create-branch',
    async (_event, dir: string, name: string): Promise<{ ok: boolean; error?: string }> => {
      if (!hasDir(dir) || !validRefName(name)) return { ok: false }
      const r = await runGit(dir, ['checkout', '-b', name.trim()])
      return r.ok ? { ok: true } : { ok: false, error: r.error }
    }
  )

  // 内置 PPT 解包:spawn 随包分发的 json2pptx-cli 把 .pptx 解到临时目录,读回
  // presentation.json(字符串路径的 slide 逐个内联)组装 deck JSON 返回。
  // 插件页(ppt-inspector)拿 deck 直接渲染;媒体文件不返回(图片元素渲染占位)。
  ipcMain.handle(
    'pptx:unpack',
    async (_event, filePath: unknown): Promise<{ ok: boolean; file?: string; deck?: unknown; message?: string }> => {
      if (typeof filePath !== 'string' || !filePath.trim()) return { ok: false, message: '路径为空' }
      const cli = pptxCliPath()
      if (!existsSync(cli)) {
        return {
          ok: false,
          message: `未找到 PPT 解包程序: ${cli}\n请运行 pnpm run build 或 pnpm run build:ppt（需 AI_PPT_ROOT 指向 json2pptx 源码仓库）后重试`
        }
      }
      const workDir = await mkdtemp(join(tmpdir(), 'ppt-unpack-'))
      try {
        await runCli(cli, ['unpack', filePath.trim(), '-o', workDir])
        const pres = JSON.parse(await readFile(join(workDir, 'presentation.json'), 'utf8'))
        const slides = Array.isArray(pres.slides)
          ? pres.slides.map((s: unknown) => {
              if (typeof s === 'string') {
                // presentation.json 里 slide 可能是相对路径引用,逐个读取内联
                const p = join(workDir, s)
                return existsSync(p) ? JSON.parse(readFileSync(p, 'utf8')) : null
              }
              return s
            })
          : pres.slides
        return {
          ok: true,
          file: filePath.trim(),
          deck: {
            name: pres.name,
            width: pres.width,
            height: pres.height,
            meta: pres.meta,
            theme: pres.theme,
            slides: slides.filter((s: unknown) => s !== null)
          }
        }
      } catch (e) {
        return { ok: false, message: e instanceof Error ? e.message : String(e) }
      } finally {
        rm(workDir, { recursive: true, force: true }).catch(() => undefined)
      }
    }
  )
}

/** 随包分发的解包 CLI 路径:优先 astorm-office kit(office/bin/json2pptx,sync-office.mjs 同步),
 * 回退旧独立仓库产物 bin/json2pptx-cli(sync-pptx-cli.mjs,已由 json2pptx 取代)。
 * 打包 = resources,dev = frontend/resources。 */
function pptxCliPath(): string {
  const base = app.isPackaged ? process.resourcesPath : join(app.getAppPath(), 'resources')
  const office = join(base, 'office', 'bin', process.platform === 'win32' ? 'json2pptx.exe' : 'json2pptx')
  if (existsSync(office)) return office
  return join(base, 'bin', process.platform === 'win32' ? 'json2pptx-cli.exe' : 'json2pptx-cli')
}

/** 执行 CLI(超时 90s;失败抛含 stderr 的错误) */
function runCli(cli: string, args: string[]): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(cli, args, { timeout: 90_000, windowsHide: true, maxBuffer: 16 * 1024 * 1024 }, (err, _stdout, stderr) => {
      if (err) {
        const detail = String(stderr || '').trim() || (err instanceof Error ? err.message : String(err))
        reject(new Error(`PPT 解包失败: ${detail}`))
        return
      }
      resolve()
    })
  })
}
