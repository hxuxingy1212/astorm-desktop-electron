// agent-browser 同步:把浏览器自动化 CLI 拷进 frontend/resources/agent-browser/<os>-<arch>/
// 供 browser 工具(内置浏览器「AI 代理操控」)spawn;electron-builder 以 extraResources 打进安装包。
//
// 来源顺序:
//   1. ASTORM_BROWSER_SRC 指向已有可执行文件（直接拷，不编译）；
//   2. AGENT_BROWSER_ROOT 指向源码仓库（显式设置时 cargo build --release；默认 ~/workspace/agent-browser
//      仅作为开发期便利，存在才用）；
//   3. 目标已是 scripts/external-artifacts.json 里的版本 → 跳过；
//   4. 从 GitHub Release 下载当前平台的预编译二进制（v0.37.1，带 sha256 校验）；
//   5. 目标已存在但版本未知（历史本地构建）→ 保留并告警；
//   6. 全部缺失 → 写 NOT-SYNCED 占位，browser 工具运行时降级。
//
// 下载相关环境变量:
//   AGENT_BROWSER_URL       直接指定二进制 URL（优先于 GitHub Release）
//   AGENT_BROWSER_SHA256    配合上面的 URL 使用的 sha256
//   ASTORM_GITHUB_MIRROR    GitHub 镜像前缀，例如 https://ghproxy.net/
//   ASTORM_ARTIFACT_CACHE   产物缓存目录（默认 ~/.cache/astorm-desktop）
//   AGENT_BROWSER_FORCE_DOWNLOAD=1  即使已有历史本地构建也重新下载发布版
import { spawnSync } from 'node:child_process'
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync
} from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  artifactCacheDir,
  downloadFile,
  githubReleaseUrl,
  loadArtifactConfig,
  platformKey,
  readVersionFile,
  sha256File
} from './lib/artifacts.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const exeName = process.platform === 'win32' ? 'agent-browser.exe' : 'agent-browser'
const platform = platformKey()
const destDir = join(root, 'frontend', 'resources', 'agent-browser', platform)
const dest = join(destDir, exeName)
const cfg = loadArtifactConfig(root)
const pinnedVersion = cfg.agentBrowser.version
const assetInfo = cfg.agentBrowser.assets?.[platform]

function fail(msg) {
  console.error(`[sync-agent-browser] ${msg}`)
  process.exit(1)
}

/** 写 VERSION（key=value），供 verify-dist 与后续同步判断版本。 */
function writeVersionFile({ source, note, version, sha256 }) {
  writeFileSync(
    join(destDir, 'VERSION'),
    [
      `source=${source}`,
      `note=${note}`,
      `version=${version || 'unknown'}`,
      `sha256=${sha256 || 'unknown'}`,
      `size=${statSync(dest).size}`,
      `syncedAt=${new Date().toISOString()}`,
      ''
    ].join('\n')
  )
}

function copyIn(src, { source, note, version }) {
  mkdirSync(destDir, { recursive: true })
  try {
    copyFileSync(src, dest)
  } catch (e) {
    if (e?.code === 'EBUSY' || e?.code === 'EPERM') {
      fail(`目标文件被占用，请先关闭正在运行的 Astorm / agent-browser 后重试: ${dest}`)
    }
    throw e
  }
  // 真实同步成功,清掉缺源降级时写的占位文件
  const placeholder = join(root, 'frontend', 'resources', 'agent-browser', 'NOT-SYNCED')
  if (existsSync(placeholder)) rmSync(placeholder, { force: true })
  if (process.platform !== 'win32') chmodSync(dest, 0o755)
  writeVersionFile({ source, note, version, sha256: sha256File(dest) })
  console.log('[sync-agent-browser] ok:', dest)
}

function sourceVersion(repo) {
  try {
    const toml = readFileSync(join(repo, 'cli', 'Cargo.toml'), 'utf8')
    return /^version\s*=\s*"([^"]+)"/m.exec(toml)?.[1]
  } catch {
    return undefined
  }
}

/** 从源码仓库编译（开发期覆盖；失败不阻塞下载路径）。 */
function buildFromSource(repo, { required }) {
  const manifest = join(repo, 'cli', 'Cargo.toml')
  if (!existsSync(manifest)) return false
  console.log('[sync-agent-browser] cargo build --release (', repo, ') ...')
  const r = spawnSync('cargo', ['build', '--release'], { cwd: join(repo, 'cli'), stdio: 'inherit' })
  if (r.status !== 0) {
    const msg = `cargo build 失败(exit ${r.status})`
    if (required) fail(msg)
    console.warn(`[sync-agent-browser] ${msg}，改用发布版二进制`)
    return false
  }
  const src = join(repo, 'cli', 'target', 'release', exeName)
  if (!existsSync(src)) {
    const msg = `编译产物缺失: ${src}`
    if (required) fail(msg)
    console.warn(`[sync-agent-browser] ${msg}，改用发布版二进制`)
    return false
  }
  copyIn(src, { source: 'local-build', note: repo, version: sourceVersion(repo) })
  return true
}

/** 下载 GitHub Release 的预编译二进制。返回是否成功。 */
async function downloadPinned() {
  if (!assetInfo) {
    console.warn(
      `[sync-agent-browser] 没有 ${platform} 的预编译资源（支持: ${Object.keys(
        cfg.agentBrowser.assets || {}
      ).join(', ')}）`
    )
    return false
  }

  const directUrl = (process.env.AGENT_BROWSER_URL || '').trim()
  if (directUrl && !/^https?:\/\//i.test(directUrl)) {
    console.warn('[sync-agent-browser] AGENT_BROWSER_URL 必须是 http(s) URL，已忽略:', directUrl)
    return false
  }
  const tag = cfg.agentBrowser.tag || `v${pinnedVersion}`
  const url = directUrl || githubReleaseUrl(cfg.agentBrowser.repo, tag, assetInfo.name)
  const expected = ((process.env.AGENT_BROWSER_SHA256 || assetInfo.sha256 || '') + '').trim().toLowerCase()
  const cache = artifactCacheDir('agent-browser', pinnedVersion, assetInfo.name)

  try {
    if (existsSync(cache) && expected && sha256File(cache) === expected) {
      console.log('[sync-agent-browser] 复用已下载的二进制:', cache)
    } else {
      console.log(`[sync-agent-browser] 下载 agent-browser ${pinnedVersion} (${platform}) ...`)
      console.log('[sync-agent-browser]', url)
      await downloadFile(url, cache, { timeoutMs: 300000 })
    }
    if (expected) {
      const actual = sha256File(cache)
      if (actual !== expected) throw new Error(`sha256 不匹配: 期望 ${expected}, 实际 ${actual}`)
    } else {
      console.warn('[sync-agent-browser] 未提供 sha256，跳过二进制校验')
    }
    copyIn(cache, { source: 'github-release', note: url, version: pinnedVersion })
    return true
  } catch (e) {
    console.warn('[sync-agent-browser] 从远端获取失败:', e.message)
    rmSync(cache, { force: true })
    return false
  }
}

function writeNotSynced(reason) {
  const placeholderDir = join(root, 'frontend', 'resources', 'agent-browser')
  mkdirSync(placeholderDir, { recursive: true })
  writeFileSync(
    join(placeholderDir, 'NOT-SYNCED'),
    `agent-browser 未同步(${new Date().toISOString()}):${reason}\n` +
      'browser 工具将不可用。可设置 AGENT_BROWSER_SRC(已有二进制) 或 AGENT_BROWSER_ROOT(源码仓库)' +
      '，或检查网络后重跑 pnpm run sync:agent-browser。\n'
  )
  console.warn(
    '[sync-agent-browser] 未获取到 agent-browser，写入 NOT-SYNCED 占位，browser 工具运行时不可用（构建继续）。'
  )
}

// 1) 显式指定的二进制
const explicit = process.env.AGENT_BROWSER_SRC?.trim()
if (explicit) {
  if (!existsSync(explicit)) fail(`AGENT_BROWSER_SRC 指向的文件不存在: ${explicit}`)
  copyIn(explicit, { source: 'AGENT_BROWSER_SRC', note: explicit, version: undefined })
  process.exit(0)
}

// 2) 显式/默认源码仓库编译
const explicitRoot = process.env.AGENT_BROWSER_ROOT?.trim()
const defaultRoot = join(homedir(), 'workspace', 'agent-browser')
const repo = explicitRoot || defaultRoot
if (existsSync(join(repo, 'cli', 'Cargo.toml'))) {
  if (buildFromSource(repo, { required: Boolean(explicitRoot) })) process.exit(0)
  if (explicitRoot) process.exit(1)
}

// 3) 目标已存在：版本一致则跳过；历史本地构建默认保留
const current = readVersionFile(join(destDir, 'VERSION'))
if (existsSync(dest)) {
  if (current.version === pinnedVersion) {
    console.log('[sync-agent-browser] 已存在且版本一致,跳过:', dest)
    process.exit(0)
  }
  if (!current.version || current.version === 'unknown') {
    if (process.env.AGENT_BROWSER_FORCE_DOWNLOAD !== '1') {
      console.warn(
        `[sync-agent-browser] 已存在历史本地构建（${current.source || 'unknown'}），保留不覆盖；` +
          '如需发布版请设置 AGENT_BROWSER_FORCE_DOWNLOAD=1 或删除该目录后重试'
      )
      process.exit(0)
    }
  }
}

// 4) 下载发布版
if (await downloadPinned()) process.exit(0)

// 5) 有旧产物就保留（不把可用构建替换成占位）
if (existsSync(dest)) {
  console.warn('[sync-agent-browser] 下载失败，保留已有二进制:', dest)
  process.exit(0)
}

// 6) 缺源降级
writeNotSynced('GitHub Release / 本地源均不可用')
process.exit(0)
