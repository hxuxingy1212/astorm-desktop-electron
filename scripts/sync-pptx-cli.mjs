// json2pptx-cli 构建 + 同步：ai-ppt 仓库(环境变量 AI_PPT_ROOT 可覆盖,默认 ~/Desktop/workspace/ai-ppt)
// cargo build --release --bin json2pptx-cli → frontend/resources/bin/{平台产物}
// 供内置插件 ppt-inspector 解包 .pptx；electron-builder 以 extraResources 打进安装包。
import { spawnSync } from 'node:child_process'
import { copyFileSync, chmodSync, existsSync, mkdirSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const cliName = process.platform === 'win32' ? 'json2pptx-cli.exe' : 'json2pptx-cli'

// 解包 CLI 源码仓库(含 json2pptx lib + cli)。默认本机开发路径,可用 AI_PPT_ROOT 覆盖
const aiPptRoot = (process.env.AI_PPT_ROOT || join(homedir(), 'Desktop', 'workspace', 'ai-ppt')).trim()

console.log('[sync-pptx-cli] cargo build --release --bin json2pptx-cli (', aiPptRoot, ') ...')
if (!existsSync(join(aiPptRoot, 'Cargo.toml'))) {
  console.error(
    `[sync-pptx-cli] 未找到 ai-ppt 仓库: ${aiPptRoot}\n` +
      '  内置 PPT 解包依赖 json2pptx-cli,请设置环境变量 AI_PPT_ROOT 指向 json2pptx 源码仓库(含 cli crate)后重试'
  )
  process.exit(1)
}
const r = spawnSync('cargo', ['build', '--release', '--bin', 'json2pptx-cli'], {
  cwd: aiPptRoot,
  stdio: 'inherit'
})
if (r.status !== 0) {
  console.error('[sync-pptx-cli] cargo build failed, exit', r.status)
  process.exit(r.status ?? 1)
}

const src = join(aiPptRoot, 'target', 'release', cliName)
if (!existsSync(src)) {
  console.error('[sync-pptx-cli] build 产物缺失:', src)
  process.exit(1)
}
const destDir = join(root, 'frontend', 'resources', 'bin')
mkdirSync(destDir, { recursive: true })
const dest = join(destDir, cliName)
copyFileSync(src, dest)
if (process.platform !== 'win32') chmodSync(dest, 0o755)
console.log('[sync-pptx-cli] ok:', dest)
