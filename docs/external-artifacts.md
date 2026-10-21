# 外部随包产物（office-kit / agent-browser）

安装包里有两份**不在本仓库**的原生资源：

| 产物 | 内容 | 来源 | 代码里的版本 |
| --- | --- | --- | --- |
| office-kit | 5 个 CLI（`astorm-office` + `json2docx/xlsx/pptx/pdf`）、技能树、`@astorm/office-viewer` | `hxuxingy1212/astorm-office` 的 GitHub Release | `scripts/external-artifacts.json` 的 `officeKit.version` |
| agent-browser | 浏览器自动化 CLI | `vercel-labs/agent-browser` 的 GitHub Release（官方预编译二进制） | `scripts/external-artifacts.json` 的 `agentBrowser.version` |

`scripts/sync-office.mjs` 与 `scripts/sync-agent-browser.mjs` 的默认行为是：**本地有就用本地，没有就按当前平台从 GitHub Release 下载并校验**。下载失败仍然软降级（写占位/跳过），不会阻塞 `pnpm run build`；但 `pnpm run dist:*` 的前置校验会拒绝缺料的安装包。

## 1. 发布 office-kit

office 仓库的 `.github/workflows/release-kit.yml` 会在 push `v*` tag 时按平台构建并挂 Release：

```text
office-kit-<version>-linux-x64.tar.gz
office-kit-<version>-linux-arm64.tar.gz
office-kit-<version>-darwin-arm64.tar.gz
office-kit-<version>-darwin-x64.tar.gz
office-kit-<version>-win32-x64.tar.gz
SHA256SUMS
```

发布步骤：

1. 确认 `astorm-office/Cargo.toml` 的 `[workspace.package] version` 与要发的版本一致；
2. `git tag v0.2.0 && git push origin v0.2.0`（tag 必须等于 `v` + workspace version，工作流会校验）；
3. 等工作流把归档和 `SHA256SUMS` 挂到 Release；
4. 在本仓库把 `scripts/external-artifacts.json` 的 `officeKit.version` / `tag` 改成新版本并提交。

归档结构就是 office 仓库 `node scripts/export.mjs` 的 `dist-kit/office-kit-<version>/`：`bin/ skill/ viewer/ samples/ VERSION SHA256SUMS _metadata.json`。桌面端只拷贝 `bin/ skill/ viewer/ VERSION SHA256SUMS _metadata.json`。

> Windows 的 kit 里 CLI 文件名带 `.exe`；`export.mjs` 已按平台生成，桌面端 `office` 工具在 Windows 也按 `.exe` 解析。

## 2. agent-browser

上游每个 release 都提供预编译二进制：

```text
agent-browser-darwin-arm64
agent-browser-darwin-x64
agent-browser-linux-arm64
agent-browser-linux-x64
agent-browser-win32-x64.exe
```

`scripts/external-artifacts.json` 里锁定了版本（当前 `0.37.1`）和每个平台的 sha256。升级时：

1. 确认新版本与 `frontend/src/main/browser-agent/` 的桥接契约兼容（`--json` 信封、snapshot 结构、动作词汇）；
2. 从上游 Release 取新二进制，把 `name` 与 `sha256` 写进清单；
3. 删除 `frontend/resources/agent-browser/<platform>/` 后重跑 `pnpm run sync:agent-browser`。

本地开发仍可用源码覆盖：

```bash
# 直接给一个已有可执行文件
AGENT_BROWSER_SRC=/path/to/agent-browser pnpm run sync:agent-browser

# 或指向源码仓库（会 cargo build --release）
AGENT_BROWSER_ROOT=/path/to/agent-browser pnpm run sync:agent-browser
```

已有历史本地构建（`VERSION` 里没有 `version=`）默认保留不覆盖；要强制换成发布版：

```bash
AGENT_BROWSER_FORCE_DOWNLOAD=1 pnpm run sync:agent-browser
```

## 3. 离线 / 国内网络

所有下载都支持镜像前缀与环境变量覆盖：

| 变量 | 作用 |
| --- | --- |
| `ASTORM_GITHUB_MIRROR` | 拼在完整 GitHub URL 前面，例如 `https://ghproxy.net/` |
| `ASTORM_OFFICE_KIT` | 直接指向本地 office-kit 目录（跳过下载） |
| `ASTORM_OFFICE_ROOT` | 指向本地 astorm-office 仓库（有 `dist-kit` 就复用，否则现场 export） |
| `ASTORM_OFFICE_KIT_URL` / `ASTORM_OFFICE_KIT_SHA256` | 直接指定归档 URL 与校验值 |
| `ASTORM_OFFICE_KIT_DISABLE_DOWNLOAD=1` | 禁用 office 联网下载 |
| `AGENT_BROWSER_SRC` / `AGENT_BROWSER_ROOT` | 本地二进制 / 源码仓库 |
| `AGENT_BROWSER_URL` / `AGENT_BROWSER_SHA256` | 直接指定二进制 URL 与校验值 |
| `ASTORM_ARTIFACT_CACHE` | 产物缓存目录（默认 `~/.cache/astorm-desktop`） |

下载的文件会缓存在 `ASTORM_ARTIFACT_CACHE`；office kit 解压后内部 `SHA256SUMS` 校验通过就直接复用，**重复构建不再联网**。

## 4. 校验

```bash
pnpm run sync:office
pnpm run sync:agent-browser
pnpm run verify:dist        # 或 pnpm run dist:mac / dist:win / dist:linux
```

`verify-dist` 会检查：

- office 5 CLI、技能树、非占位 viewer；
- agent-browser 当前平台二进制；
- 后端 napi 绑定与 `frontend/out/renderer/index.html`；
- `dist:*` 传入 `--target` 时要求宿主平台与目标一致（原生二进制不能跨平台复用）；
- 版本与 `scripts/external-artifacts.json` 不一致时只告警（允许本地覆盖），不拦截。

## 5. 发布桌面安装包

在对应平台的机器上执行：

```bash
pnpm install
pnpm run dist:mac     # macOS
pnpm run dist:win     # Windows
pnpm run dist:linux   # Linux
```

`dist:*` 会依次执行：`build.mjs`（后端 → office → agent-browser → 前端）→ `verify-dist` → `electron-builder`。不要用 `pnpm -C frontend run build:mac/win/linux` 作为发布入口——那是低层命令，不跑 `verify-dist`，可能产出缺 office/browser 能力的安装包。
