#!/usr/bin/env node
/**
 * sync-agent-browser-src.mjs — 为 Docker 镜像内置 agent-browser CLI（Linux 版）做准备。
 *
 * 把 agent-browser 源码仓库（vercel-labs/agent-browser，Apache-2.0；缺省
 * ~/workspace/agent-browser）的**构建必需源码**（剪枝：排除测试/文档/benchmarks/
 * dashboard/JS 侧与产物）拷到 backend/.agent-browser/，供 backend/Dockerfile 在
 * Linux 侧 `cargo build --release` 出容器用的 `agent-browser` 二进制。
 *
 * 为什么要这份拷贝：**Docker 构建环境连不上 github.com**（实测 75s 超时），
 * 构建期 `git clone` 必失败；源码随构建上下文进去才能离线复现。
 *
 * - 源仓库存在 → 同步（先清后拷，可重复执行）
 * - 源仓库缺失 → .agent-browser 只留 manifest.json（synced:false），
 *   Dockerfile 检测后跳过编译（浏览器工具落回服务端直连 CDP 实现，不阻塞构建）
 *
 * 用法：node scripts/sync-agent-browser-src.mjs（仓库根执行；docker build 前运行）
 * 环境变量：AGENT_BROWSER_ROOT 覆盖源仓库路径（缺省 ~/workspace/agent-browser）
 */

import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const srcRoot = resolve(
  process.env.AGENT_BROWSER_ROOT || join(homedir(), 'workspace', 'agent-browser')
)
const dest = join(repoRoot, 'backend', '.agent-browser')

/** 顶层要拷贝的条目：`cli/` 是那个 Rust crate（含 cdp-protocol/*.json 与 build.rs）；
 *  README/LICENSE 供 cargo 元数据与协议署名（Cargo.toml 里 readme = "../README.md"） */
const COPY_ENTRIES = ['cli', 'README.md', 'LICENSE']

/** 目录级剪枝（构建不参与的大块头） */
const PRUNE_DIRS = new Set(['target', 'node_modules', '.git'])

function pruneInto(src, dst) {
  mkdirSync(dst, { recursive: true })
  for (const entry of readdirSync(src, { withFileTypes: true })) {
    if (PRUNE_DIRS.has(entry.name)) continue
    const from = join(src, entry.name)
    const to = join(dst, entry.name)
    if (entry.isDirectory()) pruneInto(from, to)
    else cpSync(from, to)
  }
}

/** 读源仓库的 CLI 版本（cli/Cargo.toml 的 version） */
function readVersion(root) {
  try {
    const toml = readFileSync(join(root, 'cli', 'Cargo.toml'), 'utf8')
    return /^version\s*=\s*"([^"]+)"/m.exec(toml)?.[1] ?? 'unknown'
  } catch {
    return 'unknown'
  }
}

const version = existsSync(join(srcRoot, 'cli', 'Cargo.toml')) ? readVersion(srcRoot) : null

// 先清后拷（保留目录本身，Dockerfile COPY 需要它存在）
rmSync(dest, { recursive: true, force: true })
mkdirSync(dest, { recursive: true })

if (!version) {
  writeFileSync(
    join(dest, 'manifest.json'),
    `${JSON.stringify({ synced: false, from: srcRoot, version: null }, null, 2)}\n`
  )
  console.warn(
    `[sync-agent-browser-src] 未找到源码仓库（${srcRoot}）：` +
      '镜像将不含 agent-browser CLI，容器里 browser 工具落回服务端直连 CDP 实现。'
  )
  process.exit(0)
}

for (const entry of COPY_ENTRIES) {
  const from = join(srcRoot, entry)
  if (!existsSync(from)) {
    console.warn(`[sync-agent-browser-src] 跳过缺失条目: ${entry}`)
    continue
  }
  const to = join(dest, entry)
  if (statSync(from).isDirectory()) pruneInto(from, to)
  else cpSync(from, to)
}

writeFileSync(
  join(dest, 'manifest.json'),
  `${JSON.stringify({ synced: true, from: srcRoot, version }, null, 2)}\n`
)
console.log(`[sync-agent-browser-src] ok: ${dest}（agent-browser ${version}）`)
