// 全链路构建:编译后端并同步 dll → 同步 astorm-office 套件(CLI/skill/预览组件) → 同步 agent-browser(浏览器自动化) → electron-vite 构建前端
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const frontend = join(root, 'frontend')

function run(cmd, args, cwd) {
  const r = spawnSync(cmd, args, { cwd, stdio: 'inherit' })
  if (r.status !== 0) process.exit(r.status ?? 1)
}

run(process.execPath, ['scripts/sync-backend.mjs'], root)
// office 套件缺失时脚本内降级(占位 viewer + 运行时回退),不阻塞构建
run(process.execPath, ['scripts/sync-office.mjs'], root)
// 浏览器自动化 CLI(browser 工具用;已有产物时脚本内幂等跳过)
run(process.execPath, ['scripts/sync-agent-browser.mjs'], root)
// pnpm 为 .cmd shim,Windows 下需 shell 解析；路径含空格时要显式加引号
const frontendArg = process.platform === 'win32' ? `"${frontend}"` : frontend
const pnpm = spawnSync('pnpm', ['-C', frontendArg, 'build'], {
  cwd: root,
  stdio: 'inherit',
  shell: process.platform === 'win32',
  windowsHide: true
})
if (pnpm.status !== 0) process.exit(pnpm.status ?? 1)