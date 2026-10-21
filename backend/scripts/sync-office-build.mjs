#!/usr/bin/env node
/**
 * sync-office-build.mjs — 为 Docker 镜像内置 office 五件套做准备。
 *
 * 把兄弟仓库 ../astorm-office 的**构建必需源码**（剪枝：排除 target/测试素材/文档）
 * 拷贝到 backend/.office/，供 backend/Dockerfile 在 Linux 侧编译
 * astorm-office（统一入口）/ json2docx / json2xlsx / json2pptx / json2pdf 并内置 office skill。
 *
 * - 源仓库存在 → 同步（可重复执行，先清后拷）
 * - 源仓库缺失 → .office 只留 manifest.json（synced:false），
 *   Dockerfile 检测后跳过 office 编译（工具按契约报未找到，不阻塞构建）
 *
 * 用法：node scripts/sync-office-build.mjs（仓库根执行；docker build 前运行）
 * 环境变量：ASTORM_OFFICE_ROOT 覆盖源仓库路径（缺省 ../astorm-office）
 */

import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const srcRoot = resolve(
  repoRoot,
  process.env.ASTORM_OFFICE_ROOT || '../astorm-office'
)
const dest = join(repoRoot, 'backend', '.office')

/** 顶层要拷贝的条目（构建五件套 CLI + skill 所需；office-web/web-server 与 CLI 无关） */
const COPY_ENTRIES = [
  'Cargo.toml',
  'Cargo.lock',
  'office-core',
  'office-cli',
  'ai-word',
  'ai-excel',
  'ai-ppt',
  'ai-pdf',
  'skill',
]
/** 目录级剪枝：构建用不到的大块头（测试素材/文档/示例/产物） */
const PRUNE_DIRS = new Set([
  'target',
  'tests',
  'testdata',
  'fixtures',
  'samples',
  'docs',
  'dist-kit',
  'node_modules',
  '.git',
])
/** 文件级剪枝：媒体/大素材（cargo build 不需要） */
const PRUNE_EXT = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.webp', '.mp4', '.webm',
  '.docx', '.xlsx', '.pptx', '.pdf', '.zip', '.ttf', '.otf',
])

/** 递归拷贝（顶层条目按 COPY_ENTRIES；目录内按剪枝规则） */
function copyTree(src, dest, isTop = false) {
  mkdirSync(dest, { recursive: true })
  for (const name of readdirSync(src)) {
    if (isTop && !COPY_ENTRIES.includes(name)) continue
    const s = join(src, name)
    const d = join(dest, name)
    const st = statSync(s)
    if (st.isDirectory()) {
      if (!isTop && PRUNE_DIRS.has(name)) continue
      copyTree(s, d, false)
    } else {
      const dot = name.lastIndexOf('.')
      const ext = dot >= 0 ? name.slice(dot).toLowerCase() : ''
      if (!isTop && PRUNE_EXT.has(ext)) continue
      writeFileSync(d, readFileSync(s))
    }
  }
}

// ---------- 主流程 ----------
const hasSrc = existsSync(join(srcRoot, 'Cargo.toml'))
// 先清空目标（保留目录本身），避免上次同步的残留
rmSync(dest, { recursive: true, force: true })
mkdirSync(dest, { recursive: true })

if (!hasSrc) {
  writeFileSync(
    join(dest, 'manifest.json'),
    JSON.stringify({ synced: false, reason: `源仓库不存在：${srcRoot}` }, null, 2)
  )
  console.log(
    `[sync-office-build] office 源仓库缺失（${srcRoot}），镜像将不含 office 工具（不影响其余能力）`
  )
  process.exit(0)
}

copyTree(srcRoot, dest, true)
writeFileSync(
  join(dest, 'manifest.json'),
  JSON.stringify({ synced: true, from: srcRoot }, null, 2)
)
console.log(`[sync-office-build] office 源已同步 → ${dest}（来自 ${srcRoot}）`)
