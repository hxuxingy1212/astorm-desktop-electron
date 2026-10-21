# Astorm Agent

Astorm Agent 是一个**本地优先**的跨平台桌面 AI 智能体客户端。它把大模型放进一个可操作的桌面工作台：模型可以读写文件、执行 Shell、操作 Git、检索网页、驱动内置浏览器、生成和编辑 Office 文档、管理邮件与定时任务，并在每一步受到权限审批与安全沙箱约束。

- **桌面形态**：Electron + Vue 3 渲染层，Rust 内核经 napi-rs 编译为 `.node` 原生模块嵌入主进程，无需额外启动网络服务。
- **云端形态**：同一套 Rust 内核可编译为无头服务（HTTP + SSE + WebSocket），配套 Web UI、CLI 与 Docker 单用户容器。
- **公开仓库**：<https://github.com/hxuxingy1212/astorm-desktop-electron>
- **文档索引**：[docs/README.md](docs/README.md)

> 本仓库是公开代码快照，仅保留单次提交，不包含开发历史。

## 功能

### 对话与 Agent 循环

- 多轮 ReAct 循环：模型自主规划、调用工具、读取结果并继续推理，直到完成任务。
- 流式输出、工具调用卡片、思考过程、审批卡、中断表单、计划/待办、目标循环、交付物汇总。
- 会话管理：新建、列表、回读、自动标题、摘要压缩、rewind、fork、删除；支持项目型会话空间与工作区文件树。
- 多模型供应商：OpenAI 兼容 Chat Completions、Responses、Anthropic Messages；文本与图片输入；重试、限流、熔断与结构化输出。

### 工具能力

| 能力 | 说明 |
| --- | --- |
| 文件 | 读、写、编辑、追加、列目录、正则/glob 搜索；工作区内直接操作，工作区外走审批 |
| Shell | 在工作区执行命令，支持超时、取消、输出截断与持久 HOME |
| Git | 状态、差异、分支等仓库操作与变更审查 |
| 搜索 | websearch / webfetch / GitHub Trending / arXiv / 历史会话检索 |
| 浏览器 | 内置浏览器页签 + `agent-browser` CLI/CDP，AI 可导航、点击、输入、截图、取帧 |
| Office | docx / xlsx / pptx / pdf 的生成、编辑、转换与预览（astorm-office 五 CLI） |
| 邮件 | 多账号 IMAP 收信、SMTP 发信、本地缓存与附件 |
| 记忆 | OKF 长期记忆编译管道：会话 JSONL → 知识库 Bundle，支持概念检索 |
| 定时任务 | Cron 调度、运行记录与工作目录 |
| 技能 / 子会话 | 技能加载、搜索、注册；子会话与多智能体协作 |
| 页签 | AI 可打开、切换、关闭前端工作区页签（文件、浏览器、终端、预览等） |

### 安全与可控

- **分层沙箱**：策略层（文件/网络/危险命令）→ 平台强制层（macOS Seatbelt、Windows AppContainer + Job Object + WFP）→ 会话策略层（Plan / Execute / Auto）→ 配置热更。Linux 桌面端以容器/用户权限为边界。
- **默认拒绝**：黑名单优先、白名单其次、其余默认拒绝；危险命令与资源超限直接拦截，不提供审批。
- **人工把关**：工作区外读写、联网、写操作等可挂起审批；同意后一次性放行，拒绝后短路。
- **全程留痕**：拦截、批准、执行都有审计记录，前端可查看与导出。
- **本地环境探测**：自动探测工具链并注入沙箱白名单，可在设置里按工具开关。

### 扩展与多端

- **MCP**：stdio 子进程与 HTTP/Streamable HTTP；远程 401 支持 OAuth（桌面）或手动 headers（云端）。
- **插件系统**：`plugin.json` 清单、iframe sandbox、`astorm-plugin://` 独立 origin、零依赖 SDK、权限门控与会话级 KV。
- **云端**：`astorm-server`（HTTP + SSE + WS）、`astorm` CLI、浏览器 Web UI、Docker 单用户容器；桌面壳也可直接连接云端端点。
- **界面**：中英双语、亮/暗/跟随系统、可折叠侧栏、可拖拽页签面板、全局缩放、引导页（本机/云端）。
- **预览站**：复用真实渲染层 + mock 桥的 GitHub Pages 交互预览。

## 技术架构

```text
┌────────────────────────────── Electron 桌面端 ──────────────────────────────┐
│ Renderer：Vue 3 + Pinia + Arco Design + UnoCSS + vue-i18n                   │
│   Chat / 工作区 / 页签面板 / 设置 / 引导 / 知识库 / 邮件 / 定时任务 …          │
│        │  window.api（contextBridge，本机 preload / 云端 preload）            │
│ Preload：受控 IPC 通道                                                       │
│        │                                                                     │
│ Main：窗口与菜单、文件协议、插件托管、浏览器桥、Provider 配置、自动更新物料       │
│        │  napi-rs（astorm-backend.node，进程内）                              │
│ Rust Core：ReAct 循环、工具、策略/沙箱、存储、记忆、MCP、LLM Provider           │
└──────────────────────────────────────────────────────────────────────────────┘
                    │ 同一套 astorm-core（两种宿主）
┌──────────────────────────── 云端形态（可选）────────────────────────────┐
│ astorm-server（axum HTTP + SSE + WebSocket） + Web UI + astorm CLI       │
│ Docker：单用户容器，数据卷 /data，浏览器访问 http://<host>:8787            │
└──────────────────────────────────────────────────────────────────────────┘
```

### 前端（`frontend/`）

| 层 | 职责 |
| --- | --- |
| `src/main/` | Electron 主进程：窗口、IPC、Provider 配置、插件协议、文件/知识库/附件/图标协议、浏览器 CDP 桥、后端 napi 生命周期 |
| `src/preload/` | `contextBridge` 暴露 `window.api`；本机与云端两套 preload |
| `src/renderer/` | Vue 3 应用：会话、工作区、页签面板、设置、引导、知识库、邮件、定时任务、审计、用量统计等 |
| `src/preview/` | GitHub Pages 预览站：介绍页 + 真实渲染层 iframe + 浏览器 mock 桥 |

渲染层不直接访问 Node/Electron API，所有能力都经 preload 白名单通道进入主进程。

### Rust 后端（`backend/`）

Cargo workspace，edition 2024。核心 crate：

| Crate | 职责 |
| --- | --- |
| `astorm-core` | 应用框架与业务实现：ReAct 循环接入、工具集、会话、配置、策略、MCP、记忆、技能、邮件、浏览器 |
| `react-loop` | 与业务无关的 ReAct 多轮推理/工具执行引擎 |
| `llm-provider` | OpenAI 兼容流式 HTTP 客户端：限流、熔断、重试、结构化输出 |
| `astorm-sandbox` | 跨平台 Shell 安全沙箱：文件/网络/危险命令防护、运行时监视 |
| `astorm-storage` | 会话持久化接口：JSONL、SQLite、全文索引 |
| `astorm-common` | 前后端共享的数据模型、会话 API 与存储契约 |
| `astorm-napi` | napi-rs 绑定层：应用生命周期与全局状态，编译为 `.node` |
| `astorm-server` | 无头服务宿主：HTTP + SSE/WS API；另含 `astorm-render` 渲染助手 |
| `astorm-cli` | 无头服务客户端：`newtask` / `tasks` / `show` / `stop` / `pending` / `approve` |
| `astorm-mail` | 多账户邮件：AES-256-GCM 账号库 + IMAP + SMTP + SQLite 缓存 |
| `astorm-memory` | OKF 长期记忆编译管道 |

桌面端与云端共用同一个 `astorm-core`，业务逻辑不分叉；差异只在宿主能力（原生对话框、本机 Chromium/CDP、文件路径等）。

### 随包外部产物

Office 套件与浏览器自动化 CLI 不直接放在源码树里，而是由脚本按平台获取：

- `scripts/sync-office.mjs`：本地 `../astorm-office` / `ASTORM_OFFICE_KIT` → GitHub Release 下载 → 占位 viewer。
- `scripts/sync-agent-browser.mjs`：本地源码/构建 → GitHub Release 预编译二进制 → NOT-SYNCED 占位。
- 版本与 sha256 统一钉在 `scripts/external-artifacts.json`。

详见 [`docs/external-artifacts.md`](docs/external-artifacts.md) 与 [`docs/office-integration.md`](docs/office-integration.md)。

## 仓库结构

```text
.
├── frontend/                 # Electron + Vue 3 前端
│   ├── src/main/             # 主进程
│   ├── src/preload/          # contextBridge
│   ├── src/renderer/         # Vue 渲染层
│   ├── src/preview/          # GitHub Pages 预览站
│   ├── resources/            # 图标、渲染助手；.node/office/agent-browser 为构建产物
│   └── electron-builder.yml  # 打包配置
├── backend/                  # Rust cargo workspace
│   ├── crates/               # 核心、沙箱、存储、napi、server、cli、mail、memory …
│   ├── tests/                # 后端 e2e（默认 #[ignore]）
│   ├── Dockerfile            # 云端容器镜像
│   └── docker-compose.yml
├── docs/                     # 契约、架构、UI、部署、CLI、云服务等文档
├── plugins/                  # 可分发的插件包 + SDK
├── scripts/                  # 构建/同步/校验编排
├── .github/workflows/        # GitHub Pages 预览部署
└── package.json              # 根命令入口
```

## 环境要求

- **Node.js** `^20.19.0 || >=22.12.0`，**pnpm 10**（`npm i -g pnpm@10`；新版 Node 已移除 corepack，不要依赖 `corepack enable`）。
- **Rust** stable（edition 2024 需要 Rust ≥ 1.85）。
- **平台工具链**：
  - macOS：Xcode Command Line Tools。
  - Windows：MSVC Build Tools（C++ 桌面开发）。
  - Linux：`build-essential`、`pkg-config`、`libssl-dev` 等。
- **可选**：Docker（云端容器）、Chromium（服务端网页渲染）。
- 原生二进制（napi、office、agent-browser）按宿主平台构建，不能跨平台复用。

## 快速开始

```bash
# 1. 安装依赖（根 postinstall 会安装 frontend/ 依赖）
pnpm install

# 2. 启动开发（首次会自动编译 Rust 后端并同步 .node）
pnpm run dev
```

首次启动会进入引导页：

1. 欢迎页 → 选择连接方式（仅本机 / 云端容器）。
2. 仅本机：配置模型供应商（名称、Base URL、API Key、模型），可先做文本 + 图片能力检测。
3. 偏好设置（可跳过）→ 进入应用。

模型也可以稍后在「设置 → 模型」里增删改，保存即热更。云端形态的配置与部署见 [`docs/deployment.md`](docs/deployment.md)。

### 后端 `.node` 与外部产物

- `frontend/resources/astorm-backend.node` 是构建产物，不入库；`pnpm run dev` / `pnpm run build` 会自动编译并同步。
- 直接运行 `pnpm -C frontend dev` 且 `.node` 不存在时，后端不可用（前端会提示 `BACKEND_UNAVAILABLE`）。
- Office / agent-browser 默认从 GitHub Release 下载当前平台产物；离线或内网可用 `ASTORM_OFFICE_KIT`、`AGENT_BROWSER_SRC/ROOT`、`ASTORM_GITHUB_MIRROR` 覆盖。

## 常用命令

所有命令默认在仓库根目录执行。

| 命令 | 作用 |
| --- | --- |
| `pnpm run dev` | 后端 `.node` 缺失或源码更新时先编译，再启动 electron-vite（热更新） |
| `pnpm run build` | 全链路构建：release 后端 → `.node` → office → agent-browser → electron-vite |
| `pnpm run build:backend` | 仅编译 Rust 后端并同步 `.node` |
| `pnpm run build:frontend` | 同步 office 套件后构建前端到 `frontend/out/` |
| `pnpm run typecheck` | 前端类型检查（`tsc` + `vue-tsc`） |
| `pnpm run sync:office` | 同步 office 套件（五 CLI + 技能树 + viewer） |
| `pnpm run sync:agent-browser` | 同步 agent-browser CLI |
| `pnpm run verify:dist` | 分发物料前置校验（不打包） |
| `pnpm run build:preview` | 构建 GitHub Pages 预览站到 `frontend/preview-dist/` |
| `pnpm run dev:preview` | 本地调试预览站 |

### 测试

| 命令 | 说明 |
| --- | --- |
| `pnpm run typecheck` | 前端类型检查 |
| `pnpm run test:e2e:onboarding` | 引导流程 e2e（本机/云端两分支 + 负例） |
| `pnpm run test:e2e:fileref` | 正文文件引用 chip → 打开文件页签并定位行 |
| `pnpm run test:e2e:summarytitle` | 会话摘要标题 e2e |
| `pnpm run test:e2e:goal` | `/goal` 命令与目标卡片 e2e |
| `pnpm run test:backend` | 后端 e2e（默认全 `#[ignore]`，需要真实 LLM 凭据） |

e2e 跑的是**构建产物**：改了渲染层先执行 `pnpm -C frontend build`。后端 e2e 读取 `backend/.env` 的 `LLM_BASE_URL` / `LLM_MODEL` / `LLM_API_KEY`，该文件已被 `.gitignore` 忽略，请勿提交真实密钥。

## 构建与打包

### 应用构建

```bash
pnpm run build
```

依次执行：

1. `scripts/sync-backend.mjs`：`cargo build --release -p astorm-napi`，把平台动态库复制为 `frontend/resources/astorm-backend.node`。
2. `scripts/sync-office.mjs`：同步 office 五 CLI、技能树与预览组件。
3. `scripts/sync-agent-browser.mjs`：同步当前平台的 agent-browser。
4. `electron-vite build`：构建主进程、preload 与渲染层到 `frontend/out/`。

### 安装包

| 命令 | 平台 | 产物 |
| --- | --- | --- |
| `pnpm run dist:mac` | macOS | `dmg` + `zip` |
| `pnpm run dist:win` | Windows | NSIS 安装包 |
| `pnpm run dist:linux` | Linux | `AppImage` / `deb` / `rpm` |

`dist:*` 的顺序是 **完整构建 → `verify:dist` → electron-builder**，输出目录为 `frontend/release/`。

`verify:dist` 会硬性检查：

- office 五个 CLI 可执行、技能树总纲与四格式分册齐全、viewer 不是占位 stub；
- `frontend/resources/agent-browser/<os>-<arch>/` 存在当前平台可执行文件；
- `frontend/resources/astorm-backend.node` 存在；
- `frontend/out/renderer/index.html` 存在。

原生二进制只能使用宿主平台产物，`verify:dist --target` 会拒绝跨平台打包；Apple Silicon 与 Intel macOS 需要各自在对应架构的机器上打包。`pnpm -C frontend run build:mac/win/linux` 是低层 electron-builder 命令，**不跑** `verify:dist`，不要作为正式发布入口。

### 签名与公证

`frontend/electron-builder.yml` 当前对 macOS 使用 **ad-hoc 签名**（`identity: '-'`）并开启 hardened runtime，适合本地/内部测试。正式公开分发前应替换为 Developer ID 证书并配置 Apple 公证；Windows/Linux 安装包目前未签名。macOS 的麦克风/摄像头/Apple Events 用途说明已在 `extendInfo` 中声明。

### 云端镜像

```bash
cd backend
cat > .env <<'EOF'
ASTORM_TOKEN=换成你的访问令牌
EOF
docker compose up -d --build
```

镜像内置无头服务、Web UI、CLI、Chromium、Node/npm、Python/uv 与 office 套件（构建上下文包含 office 源码时）。LLM 供应商首次登录后在 Web「设置 → 模型」配置。完整参数、数据持久化与网络安全说明见 [`docs/deployment.md`](docs/deployment.md)。

## 数据存储

数据分两层：**系统目录**（应用自身）与**用户目录**（用户内容）。

### 系统目录

| 平台 | 默认路径 |
| --- | --- |
| macOS | `~/Library/Application Support/astorm/` |
| Windows | `%APPDATA%\astorm\` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/astorm/` |

目录布局：

```text
<系统目录>/
├── config/       # 应用配置
│   ├── config.json
│   ├── providers.json          # LLM 供应商；apiKey 经 safeStorage 加密（回退 plaintext）
│   ├── backend-settings.json   # 设置聚合（语言、摘要、压缩、workspace 等）
│   ├── security.json           # 文件/网络/危险命令/资源限制
│   ├── mcp_servers.json        # MCP 服务器声明
│   └── mcp_oauth.json          # MCP OAuth token（0600）
├── data/
│   ├── sqlite.db               # 会话、任务、计划、交付物、邮件缓存等
│   ├── session/                # 会话消息 JSONL
│   ├── index/                  # 全文索引
│   ├── skill_index/            # 技能索引
│   └── sandbox/home/           # 沙箱持久 HOME（按工作区隔离）
├── log/                        # 按天滚动日志
├── plugins/                    # 已安装插件 + plugins.json
├── skills/                     # 技能 / 技能包
├── cache/icons/                # 站点与邮件图标（7 天 TTL）
└── electron/                   # Chromium userData（主进程重定向至此）
    ├── connection.json         # 引导状态、连接方式、云端端点
    ├── office-preview/         # Office 预览缓存
    └── attachment-staging/     # 附件暂存（24h TTL）
```

主进程在 `app.ready` 前通过 napi `getDataPaths()` 取系统目录，并执行 `app.setPath('userData', <系统目录>/electron)`。

### 用户目录

默认 `~/astorm/`，可在「设置 → 系统 → 用户数据目录」修改。布局：

```text
<用户目录>/
├── AGENTS.md        # 全局约定（注入每轮对话）
├── workspace/       # 默认会话空间基目录；相对日期/任务工作区归入其下
└── knowledge/       # 知识库真实文件树 + .manifest.json
```

每个会话工作区内还会有 `.astorm/` 运行时目录，例如附件副本、浏览器截图/快照等：

```text
<会话工作区>/.astorm/
├── attachment/          # 用户消息附件副本
├── browser-capture/     # 页签截图
└── browser-snapshot/    # 页面快照
```

### 备份与迁移

- 桌面端：退出应用后，完整复制**系统目录**与**用户目录**即可迁移。
- 云端：备份 Docker 数据卷 `/data`（含 `config/`、`data/`、`workspace/` 等）。
- 备份前请确认没有正在运行的任务；直接复制运行中的 SQLite/JSONL 可能得到不一致快照。

### 隐私与安全

- `.env`、`frontend/resources/astorm-backend.node`、构建产物与本地运行数据均在 `.gitignore` 中，不会进入仓库。
- 桌面端 LLM apiKey 落盘前经 Electron `safeStorage` 加密（不可用时回退明文，并在配置中标记 `plaintext`）。
- 云端形态的 `providers.json` 以卷权限为边界，建议限制数据卷访问并设置强 `ASTORM_TOKEN`。
- 请勿在 Issue / 日志中粘贴真实 API Key、访问令牌或会话内容。

## 配置摘要

| 配置 | 位置 |
| --- | --- |
| LLM 供应商 | 设置 → 模型；`<系统目录>/config/providers.json` |
| 安全策略 | 设置 → 安全；`<系统目录>/config/security.json` |
| MCP 服务器 | 设置 → 工具 → MCP；`<系统目录>/config/mcp_servers.json` |
| 用户数据目录 | 设置 → 系统 → 用户数据目录；`backend-settings.json` 的 `workspace` |
| 全局约定 | `<用户目录>/AGENTS.md` |
| 后端 e2e 凭据 | `backend/.env`（仅本地，不入库） |
| 云端运行参数 | 环境变量 / `--config`；见 [`docs/deployment.md`](docs/deployment.md) |

## 文档

- [文档索引](docs/README.md)
- [前后端契约](docs/backend-contract.md) / [后端 API](docs/backend-api.md)
- [ReAct 循环 API](docs/api/react-loop.md)
- [UI 风格规范](docs/ui-style.md)
- [插件系统](docs/plugin-system.md)
- [浏览器代理](docs/browser-agent.md)
- [长期记忆](docs/memory.md)
- [云端服务](docs/cloud-server.md) / [部署指南](docs/deployment.md) / [CLI 手册](docs/cli-manual.md)
- [Web 与桌面差异](docs/web-vs-desktop.md)
- [外部产物同步](docs/external-artifacts.md) / [Office 集成](docs/office-integration.md)
- [预览站](docs/preview-site.md)

## 许可证

[MIT](LICENSE)
