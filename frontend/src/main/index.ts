import {
  app,
  shell,
  BrowserWindow,
  protocol,
  Menu,
  nativeTheme,
  type MenuItemConstructorOptions
} from 'electron'
import { join } from 'path'
import { release } from 'node:os'
import { registerIpcHandlers } from './ipc'
import { registerProvidersIpc } from './providers'
import { registerBrowserAgentIpc, openPopupAsTab, hasBrowserAgentBridge } from './browser-agent'
import { allowSubframeEmbedding } from './framing'
import { registerBrowserCaptureIpc } from './browser-capture'
import { registerPermissionIpc } from './permissions'
import { registerPowerIpc } from './power'
import { registerChatIpc } from './chat'
import { registerBackendIpc } from './backend'
import { registerLlmIpc } from './llm'
import { ensureDemoPlugin, registerPluginProtocol } from './plugins'
import { bootstrapConfig } from './settings-sync'
import { cloudUrl, connectionInfo, registerConnectionIpc } from './connection'
import { isNapiLoaded, loadNapiBackend, shutdownNapiBackend } from './napi'
import { electronDir } from './systemPaths'
import { registerKbProtocol } from './kbFiles'
import { cleanupStaging, registerAttachmentProtocol } from './attachmentFiles'
import { registerIconProtocol } from './iconCache'
import appIcon from '../../resources/icon.png?asset'
import { createRequire } from 'node:module'
import { existsSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

// 打包后 Rust 以 `<exe> <resources>/page-render.cjs --concurrency ...` 拉起
// 页面渲染助手。打包后的 Electron 会忽略脚本参数、重新加载 app.asar 主入口，
// 因此这里在启动最初识别该参数：当前进程直接运行 page-render.cjs，并跳过
// 正常应用初始化（否则会二次启动应用并撞上单实例锁）。
const pageRenderHelper = (() => {
  const candidate = process.argv[1]
  return !!candidate && /[\\/]page-render\.cjs$/i.test(candidate) && existsSync(candidate)
    ? candidate
    : ''
})()

if (pageRenderHelper) {
  createRequire(__filename)(pageRenderHelper)
} else {
  /** macOS/Linux GUI 启动不加载登录 shell 的 PATH（Homebrew/nvm/pyenv 等不可见）。 */
  function seedLoginShellPath(): void {
    if (process.platform === 'win32') return
    const home = process.env.HOME || ''
    const extras = [
      '/opt/homebrew/bin',
      '/usr/local/bin',
      ...(home ? [`${home}/.local/bin`, `${home}/.cargo/bin`] : [])
    ].filter((p) => existsSync(p))
    try {
      const shell = process.env.SHELL || '/bin/zsh'
      const out = execFileSync(shell, ['-ilc', 'printf %s "$PATH"'], {
        encoding: 'utf8',
        timeout: 2500,
        stdio: ['ignore', 'pipe', 'ignore']
      })
        .trim()
        // shell profile 可能先输出提示/欢迎语；PATH 在最后一行
        .split(/\r?\n/)
        .pop() || ''
      if (out) extras.push(...out.split(':'))
    } catch {
      /* 登录 shell 探测失败：保留进程默认 PATH */
    }
    const merged = [...new Set([...extras, ...(process.env.PATH || '').split(':')])]
      .filter(Boolean)
      .join(':')
    if (merged) process.env.PATH = merged
  }
  seedLoginShellPath()


// 禁用 overlay 滚动条:Windows 11 上 Chromium 默认启用,
// overlay 模式下滚动条浮于内容之上且 CSS(webkit 与标准属性)完全无法定制,
// 会显示系统默认的粗滚动条并与列表项重叠。
// 同时禁用单/复数两个 feature 名(Chromium 版本间命名有差异)
app.commandLine.appendSwitch('disable-features', 'OverlayScrollbar,OverlayScrollbars')

// 调试用 CDP(仅当显式设置 ASTORM_CDP_PORT 时启用,便于脚本经 DevTools 协议驱动真实 UI)
if (process.env.ASTORM_CDP_PORT) {
  app.commandLine.appendSwitch('remote-debugging-port', process.env.ASTORM_CDP_PORT)
  app.commandLine.appendSwitch('remote-allow-origins', '*')
  console.log('[main] CDP remote-debugging enabled on port', process.env.ASTORM_CDP_PORT)
}

// 自定义协议权限（须在 app ready 前注册）：标准 URL 语义 + 独立 origin
protocol.registerSchemesAsPrivileged([
  { scheme: 'astorm-plugin', privileges: { standard: true, secure: true, supportFetchAPI: true } },
  // 知识库文件(markdown 相对资源经此读取;知识库根随用户目录)
  { scheme: 'astorm-kb', privileges: { standard: true, secure: true, supportFetchAPI: true } },
  // 用户消息附件取字节(仅**图片**需要:staging 副本 + 会话工作区 .astorm/attachment;仅这两处可读)
  { scheme: 'astorm-attachment', privileges: { standard: true, secure: true, supportFetchAPI: true } },
  // 站点 / 邮件图标(主进程按域名缓存 7 天,详见 iconCache.ts)
  { scheme: 'astorm-icon', privileges: { standard: true, secure: true, supportFetchAPI: true } }
])

// 尽早加载 napi 并重定向 Chromium userData 到系统目录(须在 ready / 单实例锁之前):
// 系统目录 = <appData>/astorm/{config,data,log,plugins,skills,electron},用户目录由设置配置
loadNapiBackend()
app.setPath('userData', electronDir())

// 单实例锁:重复启动时聚焦已有窗口
const gotTheLock = app.requestSingleInstanceLock()

if (!gotTheLock) {
  app.quit()
} else {
  /** 窗口形态：本机（无边框 + 桌面 preload）/ 云端（原生边框 + cloud preload） */
  type WindowShape = 'local' | 'cloud'

  let mainWindow: BrowserWindow | null = null
  /** 切换连接方式时正在销毁旧窗/创建新窗：期间 window-all-closed 不能触发退出。 */
  let recreatingWindow = false
  /** 当前加载目标是不是欢迎页（preload 同步读取，渲染层据此只挂欢迎页不启后端） */
  let welcomeLoad = false
  /** 当前窗口形态（本机 = 无边框 + 桌面 preload；云端 = 原生边框 + cloud preload），
   *  形态是创建期选项，切形态只能重建窗口 */
  let windowShape: WindowShape | null = null
  /** 启动时的连接方式：云端启动则整段跳过本机后端/插件初始化（壳子只用远端服务端） */
  const startIsCloud = ((): boolean => {
    const c = connectionInfo()
    return c.onboarded && c.mode === 'cloud' && !!c.endpoint
  })()

  /** 供应商 + 会话上下文配置推给本机 Rust 后端（本机形态启动、以及从云端切回本机时补推） */
  const bootstrapLocalBackend = (): void => {
    void bootstrapConfig()
      .then(() => console.log('[main] backend config bootstrap done'))
      .catch((err) => {
        console.error('[backend-config] bootstrap failed:', err)
      })
  }

  /**
   * 加载本机 UI（欢迎页 / 正常桌面形态共用同一份本地 bundle，靠 `welcomeLoad` 区分）。
   * dev 走 vite server，prod 走打包后的 index.html。
   */
  const loadLocal = (welcome: boolean): void => {
    if (!mainWindow) return
    welcomeLoad = welcome
    if (!app.isPackaged && process.env['ELECTRON_RENDERER_URL']) {
      mainWindow.loadURL(process.env['ELECTRON_RENDERER_URL'])
    } else {
      mainWindow.loadFile(join(__dirname, '../renderer/index.html'))
    }
  }

  /** 加载云端 Web UI（服务端托管的前端；令牌由 cloud preload 预先写进 localStorage） */
  const loadCloud = (): void => {
    if (!mainWindow) return
    welcomeLoad = false
    const url = cloudUrl()
    if (!url) {
      loadLocal(true)
      return
    }
    void mainWindow.loadURL(url)
  }

  /** 按**当前**配置算该开哪种窗口（activate 重建 / 启动共用同一口径）
   *  未完成引导 → 欢迎页；完成过 → 按连接方式直接进应用（仅菜单「连接方式…」能再打开引导） */
  const currentTargetMode = (): 'local' | 'cloud' | 'welcome' => {
    const c = connectionInfo()
    if (!c.onboarded) return 'welcome'
    return c.mode === 'cloud' && c.endpoint ? 'cloud' : 'local'
  }

  /** 切连接方式 / 回欢迎页（IPC 与菜单都走这里）；形态不同则换壳重建窗口 */
  const navigateTo = (target: 'welcome' | 'local' | 'cloud'): void => {
    const wantShape: WindowShape = target === 'cloud' ? 'cloud' : 'local'
    if (windowShape !== wantShape) {
      openWindow(target)
    } else if (target === 'cloud') {
      loadCloud()
    } else {
      loadLocal(target === 'welcome')
    }
    // 从云端切回本机：本机后端配置此前没推过（云启动时跳过），这里补一次
    if (target === 'local') bootstrapLocalBackend()
  }

  const createWindow = (mode: 'local' | 'cloud' | 'welcome'): void => {
    const isMac = process.platform === 'darwin'
    const cloud = mode === 'cloud'
    const acrylic = !cloud && process.platform === 'win32' && Number(release().split('.')[2]) >= 22621
    windowShape = cloud ? 'cloud' : 'local'
    // 原生外观交回系统：本机形态由渲染层按应用主题推（见 ipc.ts 的 appearance:set-theme-mode），
    // 云端形态加载的是远端 Web UI（自带主题、按浏览器语义走）—— 不该被本地会话的主题钉住。
    // 切回本机时渲染层会重新加载并再推一次，故这里置一次即自愈。
    if (cloud) nativeTheme.themeSource = 'system'
    // 默认窗口尺寸（最小 780×600）按参考实现（Codex）对齐：它的窗口实测 2180×1520 物理 px @2x
    // = 1090×760 CSS px。1280×800 明显偏大——宽窗口下设置页内容贴左、聊天列两侧留白过多。
    mainWindow = new BrowserWindow({
      width: 1090,
      height: 760,
      minWidth: 780,
      minHeight: 600,
      show: false,
      // 云端：原生边框 —— 远端跑的是「浏览器形态」的 Web UI，它按浏览器语义隐藏自绘窗口
      // 按钮、也不为 macOS 红黄绿预留位置，故这里交回系统标题栏（三者都不冲突）。
      // 本机：macOS 隐藏标题栏保留原生红黄绿；Windows/Linux 无边框 + 渲染层自绘按钮。
      ...(cloud
        ? {}
        : isMac
          ? { titleBarStyle: 'hiddenInset' as const, trafficLightPosition: { x: 14, y: 15 } }
          : { frame: false }), // Windows/Linux:无边框 + 渲染层自绘右侧窗口按钮
      // 云端要露出菜单栏（连接方式… 是壳子唯一的切换入口；Win/Linux 没有别的可见入口）
      autoHideMenuBar: !cloud,
      // 原生玻璃铺在窗口底层，由不透明的右侧内容遮住，只让侧栏透出。
      ...(!cloud && isMac ? { vibrancy: 'sidebar' as const, visualEffectState: 'active' as const } : {}),
      ...(acrylic ? { backgroundMaterial: 'acrylic' as const } : {}),
      backgroundColor: !cloud && (isMac || acrylic)
        ? '#00000000'
        : nativeTheme.shouldUseDarkColors ? '#181818' : '#ffffff',
      icon: appIcon,
      webPreferences: {
        preload: join(__dirname, cloud ? '../preload/cloud.js' : '../preload/index.js'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: false
      }
    })

    mainWindow.on('ready-to-show', () => {
      mainWindow?.show()
    })

    // 顶层导航:本机形态一律不让应用窗口被导航走(外链交系统浏览器;内置页签内的导航不是顶层,
    // 不受影响);云端形态窗口加载的就是远端站点,同源导航(含 Web UI 的路由)必须放行。
    mainWindow.webContents.on('will-navigate', (event, url) => {
      const devUrl = process.env['ELECTRON_RENDERER_URL']
      const isSelf = cloud
        ? url.startsWith(cloudUrl())
        : devUrl
          ? url.startsWith(devUrl)
          : url.startsWith('file:')
      if (isSelf) return
      event.preventDefault()
      if (/^(https?:|mailto:)/i.test(url)) void shell.openExternal(url)
    })

    // 向渲染进程推送最大化状态变化(用于窗口控制按钮图标切换)
    mainWindow.on('maximize', () => {
      mainWindow?.webContents.send('window:maximized-changed', true)
    })
    mainWindow.on('unmaximize', () => {
      mainWindow?.webContents.send('window:maximized-changed', false)
    })

    // 推送全屏状态变化(用于全屏模式下放大顶部工具栏与侧栏)
    mainWindow.on('enter-full-screen', () => {
      mainWindow?.webContents.send('window:fullscreen-changed', true)
    })
    mainWindow.on('leave-full-screen', () => {
      mainWindow?.webContents.send('window:fullscreen-changed', false)
    })

    // 新窗口 / 新页签(window.open、target="_blank")。
    // 应用自身 UI 从不调用 window.open/target=_blank(只有本地文件用 shell:open-path),
    // 因此窗口打开请求实际只来自内置浏览器页面 → 有浏览器实例时**转成内置新页签**(自动前台),
    // 不再打开系统浏览器;无实例时维持原行为(外链交系统浏览器)。
    mainWindow.webContents.setWindowOpenHandler((details) => {
      const url = details.url
      if (/^https?:/i.test(url) && hasBrowserAgentBridge()) {
        void openPopupAsTab(url).then((ok) => {
          if (!ok) void shell.openExternal(url)
        })
        return { action: 'deny' }
      }
      if (/^(https?:|mailto:)/i.test(url)) void shell.openExternal(url)
      return { action: 'deny' }
    })

    if (cloud) loadCloud()
    else loadLocal(mode === 'welcome')
  }

  /**
   * 打开（或重建）窗口：边框 / 菜单 / preload 都是**创建期选项**，本机形态（无边框 + 桌面
   * preload）与云端形态（原生边框 + cloud preload）无法在同一窗口上切换 —— 故「切连接方式」
   * = 记下当前尺寸与最大化状态 → 销毁旧窗 → 按新形态重建（视觉上等价于原地换壳）。
   */
  const openWindow = (mode: 'local' | 'cloud' | 'welcome'): void => {
    const prev = mainWindow
    const alive = !!prev && !prev.isDestroyed()
    const bounds = alive ? prev!.getBounds() : null
    const wasMaximized = alive ? prev!.isMaximized() : false
    if (alive) {
      // destroy() 会同步触发 window-all-closed（最后一个窗口），先立旗避免 app.quit()
      recreatingWindow = true
      prev!.destroy()
    }
    try {
      createWindow(mode)
    } finally {
      recreatingWindow = false
    }
    if (bounds && mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.setBounds(bounds)
      if (wasMaximized) mainWindow.maximize()
    }
  }

  /**
   * 应用菜单：默认模板 + 「连接方式…」。
   *
   * 云端形态唯一的切换入口（远端 Web UI 是服务端的前端，壳子级操作只能在壳子里给），
   * 所以云端窗口不隐藏菜单栏；本机形态菜单栏照旧隐藏（macOS 的 app 菜单仍在屏幕顶部，
   * 顺带让桌面用户也能从菜单切到云端）。
   */
  const buildAppMenu = (): void => {
    const isMac = process.platform === 'darwin'
    const switchItem: MenuItemConstructorOptions = {
      label: '连接方式…',
      accelerator: isMac ? 'Cmd+Shift+M' : 'Ctrl+Shift+M',
      click: () => navigateTo('welcome')
    }
    const template: MenuItemConstructorOptions[] = [
      ...(isMac
        ? ([
            {
              label: app.name,
              submenu: [
                { role: 'about' },
                { type: 'separator' },
                switchItem,
                { type: 'separator' },
                { role: 'hide' },
                { role: 'hideOthers' },
                { role: 'unhide' },
                { type: 'separator' },
                { role: 'quit' }
              ]
            }
          ] as MenuItemConstructorOptions[])
        : ([
            {
              label: '文件',
              submenu: [switchItem, { type: 'separator' }, { role: 'quit' }]
            }
          ] as MenuItemConstructorOptions[])),
      { role: 'editMenu' },
      { role: 'viewMenu' },
      { role: 'windowMenu' }
    ]
    Menu.setApplicationMenu(Menu.buildFromTemplate(template))
  }

  app.on('second-instance', () => {
    if (mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore()
      mainWindow.focus()
    }
  })

  app.whenReady().then(async () => {
    app.setAppUserModelId('com.astorm.agent')
    // macOS dock 图标:dev 下运行的是 Electron 自带 bundle(默认原子图标),
    // 运行期显式设为品牌图标;打包版由 build/icon.icns 提供,这里保持一致且无副作用
    if (process.platform === 'darwin') app.dock?.setIcon(appIcon)
    console.log('[main] app ready, napi-loaded=' + isNapiLoaded() + ', userData=' + app.getPath('userData'))
    registerIpcHandlers()
    registerProvidersIpc()
    // 内置浏览器页签(跨域 iframe)的 X-Frame-Options / CSP frame-ancestors 放行
    // (github 等站点两都发,不放行则页签永远空白;只作用于 subFrame 响应)
    allowSubframeEmbedding()
    // 内置浏览器「AI 代理操控」:按激活网页页签起停 iframe 作用域的 CDP WS 桥
    registerBrowserAgentIpc()
    // 内置浏览器页签截图(UI 按钮;走 capturePage,与上面的 CDP 桥无关)
    registerBrowserCaptureIpc()
    registerPermissionIpc()
    registerPowerIpc()
    registerChatIpc()
    registerBackendIpc()
    registerLlmIpc()
    registerPluginProtocol()
    registerKbProtocol()
    // 用户消息**图片**附件(缩略图/大图直读;staging 副本过期清理)
    registerAttachmentProtocol()
    // 站点 / 邮件图标：按域名 7 天本地缓存（渲染层经 astorm-icon:// 读取）
    registerIconProtocol()
    void cleanupStaging()
    // 首次运行安装内置 demo 插件(已存在则跳过;先于窗口创建,避免首帧扫描竞态)
    // 云端形态不碰本机后端与插件目录（壳子只用远端服务端），整段跳过
    if (!startIsCloud) {
      try {
        await ensureDemoPlugin()
      } catch (err) {
        console.warn('[plugins] ensureDemoPlugin failed:', (err as Error).message)
      }
    }

    // 连接方式：本机 / 云端 / 首次启动（未选择 → 欢迎页）
    const conn = connectionInfo()
    const startMode = currentTargetMode()
    console.log('[main] connection mode =', startMode, conn.endpoint ? `(${conn.endpoint})` : '')
    registerConnectionIpc(navigateTo, () => welcomeLoad)
    buildAppMenu()
    createWindow(startMode)

    // 供应商 + 会话上下文配置随启动推给 Rust 后端(完整配置,运行中走热更)
    // 云端形态不启本机后端（首次切回本机时由 navigateTo 补推）
    if (startMode === 'local') void bootstrapLocalBackend()

    app.on('activate', () => {
      // macOS:点击 Dock 图标时若无窗口则重建（按**当前**连接方式，不是启动时那个）
      if (BrowserWindow.getAllWindows().length === 0) createWindow(currentTargetMode())
    })
  })

  // 退出前显式停后端：回收 PTY/MCP 子进程并落日志（否则 Windows 上
  // powershell/MCP 服务器可能作为孤儿进程留在后台）。
  app.on('before-quit', () => {
    shutdownNapiBackend()
  })

  app.on('window-all-closed', () => {
    // 连接方式切换会先销毁旧窗再建新窗，期间所有窗口短暂为 0，不能退出。
    if (recreatingWindow) return
    if (process.platform !== 'darwin') app.quit()
  })
}
}
