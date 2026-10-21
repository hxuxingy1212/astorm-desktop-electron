// 后端编译 + dll 同步:{debug|release}/{平台产物} → frontend/resources/astorm-backend.node
//
// 默认 release（thin LTO + 单 codegen 单元，产物小、性能好，用于打包）。
// `--dev` / `ASTORM_BACKEND_DEV=1` 时改用 **debug** 档：链接快得多、可增量，适合
// `pnpm dev` 的「源码变了就重编」场景（release 的 thin-LTO 整体链接会静默卡几分钟）。
import { spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const backend = join(root, 'backend')

const dev = process.argv.includes('--dev') || process.env.ASTORM_BACKEND_DEV === '1'
const profileDir = dev ? 'debug' : 'release'
const buildArgs = dev
  ? ['build', '-p', 'astorm-napi']
  : ['build', '--release', '-p', 'astorm-napi']

// 平台相关产物名:Windows=.dll macOS=.dylib Linux=.so
const isWin = process.platform === 'win32'
const isMac = process.platform === 'darwin'
const libFile = isWin ? 'astorm_napi.dll' : isMac ? 'libastorm_napi.dylib' : 'libastorm_napi.so'
const dll = join(backend, 'target', profileDir, libFile)
const dest = join(root, 'frontend', 'resources', 'astorm-backend.node')

console.log(`[sync-backend] cargo ${buildArgs.join(' ')} (backend/, ${profileDir}) ...`)
const r = spawnSync('cargo', buildArgs, { cwd: backend, stdio: 'inherit' })
if (r.status !== 0) {
  console.error('[sync-backend] cargo build failed, exit', r.status)
  process.exit(r.status ?? 1)
}
if (!existsSync(dll)) {
  console.error('[sync-backend] backend lib not found:', dll)
  process.exit(1)
}
mkdirSync(dirname(dest), { recursive: true })
try {
  copyFileSync(dll, dest)
} catch (e) {
  if (e && (e.code === 'EBUSY' || e.code === 'EPERM')) {
    console.error('[sync-backend] 目标文件被占用，请先关闭正在运行的 Astorm 后重试:', dest)
    process.exit(1)
  }
  throw e
}

// 符号剥离(体积优化):macOS/Linux 用 strip -x 去掉本地符号(保留导出符号,不影响 napi 注册)。
// Windows 无对应工具时跳过。best-effort:失败仅告警,不阻断同步。
if (!isWin) {
  const strip = spawnSync('strip', ['-x', dest], { stdio: 'pipe' })
  if (strip.status === 0) {
    console.log('[sync-backend] stripped local symbols')
  } else {
    console.warn('[sync-backend] strip skipped:', String(strip.stderr || strip.error || '').trim())
  }
}

// macOS:兜底处理 libswift_Concurrency 的 @rpath back-deployment 引用。
// backend/.cargo/config.toml 已设 MACOSX_DEPLOYMENT_TARGET=12.0(产物直接记录系统绝对
// 路径,此处 -change 变为无操作);对旧产物或绕过 cargo config 的构建仍生效。
if (isMac) {
  const fix = spawnSync(
    'install_name_tool',
    ['-change', '@rpath/libswift_Concurrency.dylib', '/usr/lib/swift/libswift_Concurrency.dylib', dest],
    { stdio: 'pipe' }
  )
  if (fix.status === 0) {
    console.log('[sync-backend] patched libswift_Concurrency load path (macOS)')
  } else {
    console.log('[sync-backend] swift concurrency already resolved (no @rpath reference)')
  }
  // strip / install_name_tool 都会破坏原签名 → 重新 ad-hoc 签名(Apple Silicon 必需,否则无法加载)
  const sign = spawnSync('codesign', ['-f', '-s', '-', dest], { stdio: 'pipe' })
  if (sign.status !== 0) {
    console.error('[sync-backend] codesign failed:', String(sign.stderr))
    process.exit(sign.status ?? 1)
  }
  console.log('[sync-backend] ad-hoc codesigned (macOS)')
}

console.log('[sync-backend] ok:', dest)
