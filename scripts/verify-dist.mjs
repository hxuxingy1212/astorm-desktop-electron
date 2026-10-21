#!/usr/bin/env node
/**
 * 分发前置校验：安装包必须带齐「随包分发」的原生物料。
 *
 * 背景：sync-office / sync-agent-browser 在源缺失时是**软降级**（占位 viewer、跳过 CLI），
 * 这适合 dev，但不适合分发——否则会产出一个用户侧缺少 office/浏览器能力、还可能需要
 * 联网安装的安装包。打包脚本（pnpm run dist:*）在 electron-builder 之前跑本脚本，
 * 缺任何一项就带修复指引硬失败。
 *
 * 校验项：
 * - office：5 个 CLI（统一入口 + 四格式）可执行、技能树总纲 + 四格式分册、viewer 非占位 stub；
 * - agent-browser：当前平台目录下的可执行文件；
 * - 后端 napi 绑定；
 * - 前端构建产物 out/renderer/index.html。
 *
 * 注意：CLI 是原生二进制，只能校验**当前平台/架构**的产物；跨平台安装包须在对应平台构建。
 */
import { existsSync, readFileSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { loadArtifactConfig, platformKey, readVersionFile } from './lib/artifacts.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const frontend = join(root, 'frontend')
const isWin = process.platform === 'win32'
const exe = (name) => (isWin ? `${name}.exe` : name)
const cfg = loadArtifactConfig(root)

const errors = []
const warnings = []
const rel = (p) => p.slice(root.length + 1)

// 原生二进制按宿主平台构建：在 mac 上跑 dist:win 会把 mac 的 .node/office/agent-browser
// 塞进 Windows 安装包。dist:* 显式传 --target，跨平台打包直接拒绝。
const targetIdx = process.argv.indexOf('--target')
const target = targetIdx >= 0 ? process.argv[targetIdx + 1] : undefined
if (target && process.platform !== target) {
  errors.push(
    `当前宿主是 ${process.platform}，不能打包 ${target}：napi/office/agent-browser 都是宿主平台二进制，请在 ${target} 机器上构建`
  )
}

function requireFile(path, label) {
  if (!existsSync(path)) {
    errors.push(`缺少${label}: ${rel(path)}`)
    return false
  }
  if (statSync(path).size === 0) {
    errors.push(`${label}为空文件: ${rel(path)}`)
    return false
  }
  return true
}

// ---- office 套件（5 CLI + 技能树 + viewer）----
const officeBin = join(frontend, 'resources', 'office', 'bin')
const officeBins = ['astorm-office', 'json2docx', 'json2xlsx', 'json2pptx', 'json2pdf']
for (const name of officeBins) {
  const path = join(officeBin, exe(name))
  if (requireFile(path, ` office CLI ${name}`) && !isWin && !(statSync(path).mode & 0o111)) {
    errors.push(`office CLI 不可执行: ${rel(path)}（chmod +x 后重跑 sync:office）`)
  }
}

const officeSkill = join(frontend, 'resources', 'office', 'skill')
requireFile(join(officeSkill, 'SKILL.md'), ' office 技能总纲')
for (const format of ['word', 'excel', 'ppt', 'pdf']) {
  requireFile(join(officeSkill, format, 'SKILL.md'), ` office ${format} 分册`)
}

const viewer = join(frontend, 'resources', 'office', 'viewer', 'office-viewer.js')
if (requireFile(viewer, ' office 预览组件')) {
  const js = readFileSync(viewer, 'utf8')
  // sync-office 在 kit 缺失时写的是抛错占位组件（文件名相同，必须按内容识别）
  if (js.includes('office 预览组件未同步') || js.includes('OfficeViewerStub') || js.length < 4096) {
    errors.push(' office 预览组件仍是占位 stub（astorm-office kit 未同步）')
  }
}

// 版本只告警不拦截：允许本地 ASTORM_OFFICE_KIT / 源码构建覆盖发布版；
// 但打包时最好与 scripts/external-artifacts.json 对齐，便于复现。
const officeVersionPath = join(frontend, 'resources', 'office', 'VERSION')
if (existsSync(officeVersionPath)) {
  const actual = readFileSync(officeVersionPath, 'utf8').trim()
  if (actual !== cfg.officeKit.version) {
    warnings.push(
      `office-kit 版本 ${actual} 与清单里的 ${cfg.officeKit.version} 不一致（本地覆盖或未升级）`
    )
  }
}

// ---- agent-browser（browser 工具的随包 CLI）----
const agentBrowserDir = join(frontend, 'resources', 'agent-browser', platformKey())
const agentBrowser = join(agentBrowserDir, exe('agent-browser'))
if (requireFile(agentBrowser, ' agent-browser CLI')) {
  const v = readVersionFile(join(agentBrowserDir, 'VERSION'))
  if (!v.version || v.version === 'unknown') {
    warnings.push('agent-browser 是历史本地构建（VERSION 无版本号），与发布版行为可能不一致')
  } else if (v.version !== cfg.agentBrowser.version) {
    warnings.push(
      `agent-browser 版本 ${v.version} 与清单里的 ${cfg.agentBrowser.version} 不一致（本地覆盖或未升级）`
    )
  }
}

// ---- 后端 napi 绑定 ----
requireFile(join(frontend, 'resources', 'astorm-backend.node'), '后端 napi 绑定')

// ---- 前端构建产物 ----
requireFile(join(frontend, 'out', 'renderer', 'index.html'), '前端构建产物（out/renderer/index.html）')

if (errors.length) {
  console.error(`[verify-dist] 发现 ${errors.length} 项分发物料缺失，已阻止打包：`)
  for (const e of errors) console.error(`  -${e}`)
  for (const w of warnings) console.error(`  ! ${w}`)
  console.error(
    '\n修复顺序：\n' +
      '  pnpm run build:backend\n' +
      '  pnpm run sync:office        # 默认从 GitHub Release 下载当前平台 kit；也可用 ASTORM_OFFICE_KIT 指向本地导出包\n' +
      '  pnpm run sync:agent-browser # 默认下载上游预编译二进制；也可用 AGENT_BROWSER_SRC/ROOT 指向本地构建\n' +
      '  pnpm -C frontend build\n' +
      '再重跑 pnpm run dist:mac / dist:win / dist:linux'
  )
  process.exit(1)
}

for (const w of warnings) console.warn(`[verify-dist] 警告：${w}`)

console.log(
  `[verify-dist] ok：office 5 CLI + 技能树 + viewer、agent-browser(${platformKey()})、` +
    'napi 绑定与前端产物均已就位'
)
