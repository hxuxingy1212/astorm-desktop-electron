// 开发启动:确保后端 dll 存在且不比后端源码旧(否则先编译同步),再 electron-vite dev
import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const frontend = join(root, 'frontend')
const nodeFile = join(frontend, 'resources', 'astorm-backend.node')

/**
 * 后端源码(.rs/.toml)最新修改时间;跳过 target(构建产物)与 .git。
 *
 * 用途:只判断"有没有 dll"会让改了 Rust 却还在跑旧 dll 的情况静默发生
 * (主进程不热重载,renderer 热重载 → 新页面配旧后端,表现为功能"看不到"),故按新旧判断。
 */
function newestSourceMtime(dir) {
  let newest = 0
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      if (entry.name === 'target' || entry.name === '.git') continue
      const path = join(d, entry.name)
      if (entry.isDirectory()) walk(path)
      else if (/\.(rs|toml)$/.test(entry.name)) {
        const mtime = statSync(path).mtimeMs
        if (mtime > newest) newest = mtime
      }
    }
  }
  walk(dir)
  return newest
}

const stale =
  existsSync(nodeFile) &&
  newestSourceMtime(join(root, 'backend')) > statSync(nodeFile).mtimeMs

if (!existsSync(nodeFile) || stale) {
  console.log(
    stale
      ? '[dev] 后端源码比 dll 新,先编译同步(旧 dll 会让新功能静默缺失)...'
      : '[dev] 后端 dll 缺失,先编译同步...'
  )
  // dev 用 debug 档编译后端：链接快、可增量；release 的 thin-LTO 整体链接会静默数分钟
  const r = spawnSync(process.execPath, ['scripts/sync-backend.mjs', '--dev'], {
    cwd: root,
    stdio: 'inherit'
  })
  if (r.status !== 0) process.exit(r.status ?? 1)
  // 目标是 dll 但应用运行中仍持有旧模块:提示需重启(脚本无法代为重启)
  console.log('[dev] 后端 dll 已更新:若应用已在运行,请重启后生效(主进程不热重载)')
}

// Electron 43 起包不再带 postinstall 下载(官方改为首启懒下载),但 electron-vite
// 启动前直接检查 dist/path.txt 并抛 "Error: Electron uninstall",懒下载来不及触发。
// frontend 的 postinstall 已兜底常规安装;这里再兜一层 --ignore-scripts 等跳过脚本的场景。
const electronDir = join(frontend, 'node_modules', 'electron')
if (existsSync(join(electronDir, 'install.js')) && !existsSync(join(electronDir, 'path.txt'))) {
  console.log('[dev] Electron 二进制缺失,先下载...')
  const env = { ...process.env }
  // install.js 手动跑时读不到 .npmrc 的 electron_mirror,需显式注入国内镜像
  const npmrc = join(frontend, '.npmrc')
  const mirror = existsSync(npmrc)
    ? readFileSync(npmrc, 'utf8').match(/^electron_mirror=(\S+)/m)?.[1]
    : undefined
  if (mirror && !env.ELECTRON_MIRROR) env.ELECTRON_MIRROR = mirror
  const r = spawnSync(process.execPath, [join(electronDir, 'install.js')], {
    cwd: frontend,
    stdio: 'inherit',
    env
  })
  if (r.status !== 0) process.exit(r.status ?? 1)
}

// Windows 下 shell:true 时 Node 只做空格拼接；仓库路径含空格必须自行加引号。
const frontendArg = process.platform === 'win32' ? `"${frontend}"` : frontend
const r = spawnSync('pnpm', ['-C', frontendArg, 'dev'], {
  cwd: root,
  stdio: 'inherit',
  shell: process.platform === 'win32',
  windowsHide: true
})
if (r.status !== 0) process.exit(r.status ?? 1)