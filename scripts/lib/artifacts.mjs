// 外部产物（office-kit / agent-browser）的下载、校验与解压公共逻辑。
//
// 设计约束：
// - 构建脚本在 Node 20+ 直接运行，不引入 npm 依赖（根 package.json 保持零依赖）；
// - 下载失败一律由调用方软降级，不在这里退出进程；
// - 支持本地镜像/代理：ASTORM_GITHUB_MIRROR 会拼在完整 GitHub URL 前面，
//   例如 https://ghproxy.net/ + https://github.com/...。
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync
} from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, resolve, sep } from 'node:path'

/** 读取 scripts/external-artifacts.json（版本与校验信息的单一来源）。 */
export function loadArtifactConfig(root) {
  return JSON.parse(readFileSync(join(root, 'scripts', 'external-artifacts.json'), 'utf8'))
}

/** 当前平台键：darwin-arm64 / darwin-x64 / linux-x64 / win32-x64 … */
export function platformKey() {
  return `${process.platform}-${process.arch}`
}

/** 产物缓存目录。默认 ~/.cache/astorm-desktop，可用 ASTORM_ARTIFACT_CACHE 覆盖。 */
export function artifactCacheDir(...parts) {
  const base =
    (process.env.ASTORM_ARTIFACT_CACHE || '').trim() ||
    join(process.env.XDG_CACHE_HOME || join(homedir(), '.cache'), 'astorm-desktop')
  return join(base, ...parts)
}

export function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

/** 拼接 GitHub Release 资源 URL（可选镜像前缀）。 */
export function githubReleaseUrl(repo, tag, asset) {
  const url = `https://github.com/${repo}/releases/download/${tag}/${asset}`
  const mirror = (process.env.ASTORM_GITHUB_MIRROR || '').trim()
  if (!mirror) return url
  return `${mirror.replace(/\/+$/, '')}/${url}`
}

/** 下载到文件；先写 .part 再原子替换，失败不会留下半个文件。 */
export async function downloadFile(url, dest, { timeoutMs = 120000 } = {}) {
  mkdirSync(dirname(dest), { recursive: true })
  const tmp = `${dest}.part`
  rmSync(tmp, { force: true })
  let res
  try {
    res = await fetch(url, {
      redirect: 'follow',
      headers: { 'User-Agent': 'astorm-desktop-build' },
      signal: AbortSignal.timeout(timeoutMs)
    })
  } catch (e) {
    throw new Error(`下载失败（${url}）：${e?.message || e}`)
  }
  if (!res.ok) throw new Error(`下载失败 HTTP ${res.status} ${res.statusText}：${url}`)
  const buf = Buffer.from(await res.arrayBuffer())
  if (buf.length === 0) throw new Error(`下载结果为空：${url}`)
  writeFileSync(tmp, buf)
  try {
    rmSync(dest, { force: true })
    renameSync(tmp, dest)
  } catch (e) {
    rmSync(tmp, { force: true })
    if (e?.code === 'EBUSY' || e?.code === 'EPERM') {
      throw new Error(`目标文件被占用，请先关闭正在运行的程序后重试：${dest}`)
    }
    throw e
  }
  return dest
}

/** 下载文本（用于 Release 里的 SHA256SUMS）。 */
export async function fetchText(url, { timeoutMs = 30000 } = {}) {
  const res = await fetch(url, {
    redirect: 'follow',
    headers: { 'User-Agent': 'astorm-desktop-build' },
    signal: AbortSignal.timeout(timeoutMs)
  })
  if (!res.ok) throw new Error(`下载失败 HTTP ${res.status} ${res.statusText}：${url}`)
  return await res.text()
}

/** 解压 .tar.gz。Windows 10+ 自带 bsdtar，macOS/Linux 自带 tar。 */
export function extractTarGz(archive, destDir) {
  rmSync(destDir, { recursive: true, force: true })
  mkdirSync(destDir, { recursive: true })
  const r = spawnSync('tar', ['-xzf', archive, '-C', destDir], { encoding: 'utf8' })
  if (r.error || r.status !== 0) {
    const detail = String(r.stderr || r.error?.message || '').trim()
    throw new Error(
      `解压失败（需要系统 tar）：${archive}${detail ? `\n${detail}` : ''}`
    )
  }
}

/** 在解压目录里找 office-kit-<version>（也容忍压缩包顶层目录改名）。 */
export function findKitDir(dir, version) {
  const expected = join(dir, `office-kit-${version}`)
  if (existsSync(join(expected, 'bin')) && existsSync(join(expected, 'VERSION'))) return expected
  if (!existsSync(dir)) return undefined
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory() && existsSync(join(p, 'bin')) && existsSync(join(p, 'VERSION'))) {
      return p
    }
  }
  return undefined
}

/** 从 Release 的 SHA256SUMS 文本里取指定文件名的摘要。 */
export function checksumForAsset(text, assetName) {
  for (const line of text.split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64})\s+\*?(.+)$/.exec(line.trim())
    if (!m) continue
    const name = m[2].trim().split(/[\\/]/).pop()
    if (name === assetName) return m[1].toLowerCase()
  }
  return undefined
}

/**
 * 按目录内的 SHA256SUMS 校验全部文件（相对 kit 根）。
 * 返回错误列表；空数组表示通过。
 */
export function verifyChecksums(dir, sumsPath = join(dir, 'SHA256SUMS')) {
  if (!existsSync(sumsPath)) return [`缺少校验清单：${sumsPath}`]
  const root = resolve(dir)
  const errors = []
  for (const line of readFileSync(sumsPath, 'utf8').split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64})\s+\*?(.+)$/.exec(line.trim())
    if (!m) continue
    const expected = m[1].toLowerCase()
    const rel = m[2].trim()
    const path = resolve(root, rel)
    // 防止清单里的 ../ 把校验带到目录外
    if (path !== root && !path.startsWith(root + sep)) {
      errors.push(`非法路径：${rel}`)
      continue
    }
    if (!existsSync(path)) {
      errors.push(`缺少文件：${rel}`)
      continue
    }
    if (sha256File(path) !== expected) errors.push(`校验失败：${rel}`)
  }
  return errors
}

/** 读取 key=value 形式的 VERSION 文件。 */
export function readVersionFile(path) {
  if (!existsSync(path)) return {}
  const out = {}
  for (const line of readFileSync(path, 'utf8').split(/\r?\n/)) {
    const i = line.indexOf('=')
    if (i > 0) out[line.slice(0, i).trim()] = line.slice(i + 1).trim()
  }
  return out
}
