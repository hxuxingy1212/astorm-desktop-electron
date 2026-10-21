// astorm-office 套件同步：从 office-kit 导出包拷贝 {bin, skill, viewer} → frontend/resources/office/
// 供 Rust `office` 工具(spawn 五 CLI：统一入口 + 四格式)、内置 office skill(SKILL.md 树)与 renderer 预览组件(vite alias)使用。
// electron-builder 以 extraResources 打进安装包(asar 内不可 spawn/不可跨包读盘)。
//
// kit 来源优先级：
//   1. ASTORM_OFFICE_KIT(现成导出包目录)；
//   2. ASTORM_OFFICE_ROOT / 兄弟目录 ../astorm-office / ~/workspace/astorm-office 的 dist-kit
//      （没有现成 kit 时现场 node scripts/export.mjs）；
//   3. GitHub Release 下载当前平台的 office-kit（版本与仓库见 scripts/external-artifacts.json）；
//   4. 全部缺失 → 写抛错占位 viewer，构建继续，运行时 office 功能降级。
//
// 下载相关环境变量：
//   ASTORM_OFFICE_KIT_URL        直接指定归档 URL（优先于 GitHub Release）
//   ASTORM_OFFICE_KIT_SHA256     归档的 sha256（配合上面的 URL 使用）
//   ASTORM_GITHUB_MIRROR         GitHub 镜像前缀，例如 https://ghproxy.net/
//   ASTORM_ARTIFACT_CACHE        产物缓存目录（默认 ~/.cache/astorm-desktop）
//   ASTORM_OFFICE_KIT_DISABLE_DOWNLOAD=1  只用本地来源，不联网
import { spawnSync } from 'node:child_process'
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync
} from 'node:fs'
import { homedir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  artifactCacheDir,
  checksumForAsset,
  downloadFile,
  extractTarGz,
  fetchText,
  findKitDir,
  githubReleaseUrl,
  loadArtifactConfig,
  platformKey,
  sha256File,
  verifyChecksums
} from './lib/artifacts.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const dest = join(root, 'frontend', 'resources', 'office')
const cfg = loadArtifactConfig(root)

/** 抛错占位组件：kit 未同步时保证 renderer 构建通过，运行时明确报错而非构建失败 */
function writeViewerStub() {
  mkdirSync(join(dest, 'viewer'), { recursive: true })
  writeFileSync(
    join(dest, 'viewer', 'office-viewer.js'),
    `// 占位：astorm-office kit 未同步（scripts/sync-office.mjs）。运行时功能自动降级。
export const OfficeViewer = {
  name: 'OfficeViewerStub',
  render() {
    throw new Error('office 预览组件未同步：请运行 pnpm run sync:office（需 astorm-office 仓库或 GitHub Release）')
  }
}
export async function loadProduct() {
  throw new Error('office 预览组件未同步：请运行 pnpm run sync:office')
}
`
  )
  writeFileSync(join(dest, 'viewer', 'office-viewer.css'), '')
  writeFileSync(join(dest, 'viewer', 'index.d.ts'), 'export declare const OfficeViewer: any\nexport declare function loadProduct(...args: unknown[]): Promise<unknown>\n')
  writeViewerPackageJson('0.0.0')
}

/** viewer 目录补 package.json：vite alias 按目录解析 + TS 经 types 字段取 index.d.ts */
function writeViewerPackageJson(version) {
  writeFileSync(
    join(dest, 'viewer', 'package.json'),
    JSON.stringify(
      {
        name: '@astorm/office-viewer',
        version,
        type: 'module',
        main: './office-viewer.js',
        types: './index.d.ts',
        exports: {
          '.': './office-viewer.js',
          './style.css': './office-viewer.css'
        }
      },
      null,
      2
    )
  )
}

function findOfficeRoot() {
  const env = (process.env.ASTORM_OFFICE_ROOT || '').trim()
  const candidates = env
    ? [env]
    : [join(dirname(root), 'astorm-office'), join(homedir(), 'workspace', 'astorm-office')]
  return candidates.find((p) => existsSync(join(p, 'Cargo.toml')))
}

/** 找 dist-kit 下最新版本的导出包（按目录名倒序） */
function findLocalKit(officeRoot) {
  const base = join(officeRoot, 'dist-kit')
  if (!existsSync(base)) return undefined
  const kits = readdirSync(base)
    .filter((n) => /^office-kit-/.test(n))
    // 版本号按数字段排序（否则 0.9.0 会排在 0.10.0 之后）
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))
    .reverse()
  return kits
    .map((n) => join(base, n))
    .find((p) => existsSync(join(p, 'bin')) && existsSync(join(p, 'viewer', 'office-viewer.js')))
}

/** 本地来源：显式 kit > 兄弟仓库 dist-kit > 现场 export */
function resolveLocalKit() {
  const explicitKit = (process.env.ASTORM_OFFICE_KIT || '').trim()
  if (explicitKit) {
    if (existsSync(join(explicitKit, 'bin'))) return explicitKit
    console.warn('[sync-office] ASTORM_OFFICE_KIT 指向的目录缺少 bin/:', explicitKit)
    return undefined
  }

  const officeRoot = findOfficeRoot()
  if (!officeRoot) return undefined

  let kit = findLocalKit(officeRoot)
  if (kit) return kit

  // 现场导出：release 产物已存在则 --skip-build（复用既有构建，避免每次全量 cargo build）
  const releaseDir = join(officeRoot, 'target', 'release')
  const exe = process.platform === 'win32' ? 'json2docx.exe' : 'json2docx'
  const skipBuild = existsSync(join(releaseDir, exe)) ? '--skip-build' : null
  console.log(
    `[sync-office] 未找到现成导出包，运行 office 侧 export.mjs${skipBuild ? '（复用已有构建）' : '（含 release 构建，首次较久）'} ...`
  )
  const r = spawnSync(process.execPath, ['scripts/export.mjs', ...(skipBuild ? [skipBuild] : [])], {
    cwd: officeRoot,
    stdio: 'inherit'
  })
  if (r.status !== 0) {
    console.warn('[sync-office] office 侧导出失败，尝试从 GitHub Release 下载')
    return undefined
  }
  return findLocalKit(officeRoot)
}

/** 从 GitHub Release 下载当前平台的 office-kit（带 sha256 与内部文件校验） */
async function resolveRemoteKit() {
  if (process.env.ASTORM_OFFICE_KIT_DISABLE_DOWNLOAD === '1') {
    console.warn('[sync-office] ASTORM_OFFICE_KIT_DISABLE_DOWNLOAD=1，跳过 GitHub Release 下载')
    return undefined
  }

  const platform = platformKey()
  const version = cfg.officeKit.version
  const tag = cfg.officeKit.tag || `v${version}`
  const assetName = (cfg.officeKit.assetPattern || 'office-kit-{version}-{platform}.tar.gz')
    .replaceAll('{version}', version)
    .replaceAll('{platform}', platform)
  const directUrl = (process.env.ASTORM_OFFICE_KIT_URL || '').trim()
  if (directUrl && !/^https?:\/\//i.test(directUrl)) {
    console.warn('[sync-office] ASTORM_OFFICE_KIT_URL 必须是 http(s) URL，已忽略:', directUrl)
    return undefined
  }
  const url = directUrl || githubReleaseUrl(cfg.officeKit.repo, tag, assetName)
  const cache = artifactCacheDir('office-kit', version)
  const extractDir = join(cache, `extract-${platform}`)
  const kitDir = join(extractDir, `office-kit-${version}`)
  const archiveName = directUrl ? basename(new URL(directUrl).pathname) || assetName : assetName
  const archive = join(cache, archiveName)

  // 缓存命中：内部校验通过就不再联网（离线可重复构建）
  if (existsSync(kitDir)) {
    const cachedErrors = verifyChecksums(kitDir)
    if (cachedErrors.length === 0) {
      console.log('[sync-office] 复用已下载的 kit:', kitDir)
      return kitDir
    }
    console.warn('[sync-office] 缓存 kit 校验失败，重新下载:', cachedErrors.slice(0, 3).join('; '))
    rmSync(kitDir, { recursive: true, force: true })
  }

  try {
    if (!existsSync(archive)) {
      console.log(`[sync-office] 下载 office-kit ${version} (${platform}) ...`)
      console.log('[sync-office]', url)
      await downloadFile(url, archive, { timeoutMs: 300000 })
    }

    // 归档校验：优先显式 sha256，其次 Release 的 SHA256SUMS；都没有则只校验解压后的文件
    const explicitSha = (process.env.ASTORM_OFFICE_KIT_SHA256 || '').trim().toLowerCase()
    if (explicitSha) {
      if (sha256File(archive) !== explicitSha) {
        throw new Error(`office-kit 归档 sha256 不匹配：${archive}`)
      }
    } else if (!directUrl) {
      try {
        const sumsUrl = githubReleaseUrl(
          cfg.officeKit.repo,
          tag,
          cfg.officeKit.checksumAsset || 'SHA256SUMS'
        )
        const sums = await fetchText(sumsUrl)
        const expected = checksumForAsset(sums, archiveName)
        if (!expected) {
          console.warn('[sync-office] Release SHA256SUMS 里没有当前平台归档，跳过归档级校验')
        } else if (sha256File(archive) !== expected) {
          throw new Error(`office-kit 归档 sha256 不匹配：${archive}`)
        }
      } catch (e) {
        console.warn('[sync-office] 获取 Release SHA256SUMS 失败，跳过归档级校验:', e.message)
      }
    }

    extractTarGz(archive, extractDir)
    const kit = findKitDir(extractDir, version)
    if (!kit) throw new Error(`解压后未找到 office-kit 目录：${extractDir}`)
    const errors = verifyChecksums(kit)
    if (errors.length) {
      throw new Error(`office-kit 内部校验失败：${errors.slice(0, 5).join('; ')}`)
    }
    console.log('[sync-office] 已下载并校验 office-kit:', kit)
    return kit
  } catch (e) {
    console.warn('[sync-office] 从远端获取 office-kit 失败:', e.message)
    // 归档可能损坏：下次重试时强制重新下载
    rmSync(archive, { force: true })
    rmSync(extractDir, { recursive: true, force: true })
    return undefined
  }
}

/** 拷贝 kit 到 frontend/resources/office，并补齐 viewer/package.json */
function copyKit(kit) {
  console.log('[sync-office] kit:', kit)
  // 源 kit 校验（samples/ 等不随包拷贝，所以在拷贝前按完整清单校验）
  const sourceErrors = verifyChecksums(kit)
  if (sourceErrors.length) {
    console.warn('[sync-office] 源 kit 校验未通过:', sourceErrors.slice(0, 5).join('; '))
  }
  // 清空重拷，保证旧版本残留（如改名后的 bin/skill 文件）不遗留
  rmSync(dest, { recursive: true, force: true })
  mkdirSync(dest, { recursive: true })
  for (const part of ['bin', 'skill', 'viewer', 'VERSION', 'SHA256SUMS', '_metadata.json']) {
    const from = join(kit, part)
    if (!existsSync(from)) continue
    cpSync(from, join(dest, part), { recursive: true, force: true })
  }

  // viewer 目录 package.json 已在上方拷贝链路后统一补齐
  if (!existsSync(join(dest, 'viewer', 'package.json'))) {
    let version = '0.0.0'
    try {
      version = JSON.parse(readFileSync(join(dest, '_metadata.json'), 'utf8')).version ?? version
    } catch {
      /* VERSION 文件兜底 */
      try {
        version = readFileSync(join(dest, 'VERSION'), 'utf8').trim() || version
      } catch {
        /* 保持默认 */
      }
    }
    writeViewerPackageJson(version)
  }

  console.log('[sync-office] ok:', dest)
}

console.log('[sync-office] 同步 astorm-office 套件 ...')
mkdirSync(dest, { recursive: true })

let kit = resolveLocalKit()
if (!kit) kit = await resolveRemoteKit()

if (!kit) {
  const where = process.env.ASTORM_OFFICE_ROOT
    ? `ASTORM_OFFICE_ROOT=${process.env.ASTORM_OFFICE_ROOT}`
    : '默认查找 ../astorm-office 与 ~/workspace/astorm-office'
  console.warn(
    `[sync-office] 未找到 office-kit（${where}；GitHub Release 也不可用）。` +
      'office 生成/编辑与预览功能不可用；可在 astorm-office 仓库运行 node scripts/export.mjs，' +
      '或用 ASTORM_OFFICE_KIT 指向导出包后重试。'
  )
  rmSync(dest, { recursive: true, force: true })
  writeViewerStub()
  process.exit(0)
}

copyKit(kit)
