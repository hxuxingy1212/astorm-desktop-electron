# astorm 单用户云容器部署指南

把 astorm 部署为**单用户云容器**：一个容器 = 一个用户的专属实例，内含
无头服务端（HTTP + SSE API）、完整 Web UI（浏览器直接访问）、CLI 客户端与
Chromium 渲染能力。桌面版不受影响，两者共享同一套 Rust 内核。

> 前置阅读：[cloud-server.md](./cloud-server.md)（架构与能力边界）；
> CLI 使用见 [cli-manual.md](./cli-manual.md)。

---

## 1. 快速开始（Docker Compose）

```bash
git clone <本仓库> && cd astorm-desktop-eletron/backend

# 1) 准备配置（compose 读取同目录 .env；只需访问令牌）
cat > .env <<'EOF'
ASTORM_TOKEN=换成你的访问令牌
EOF

# 2) 构建并启动（在仓库根构建，Dockerfile 在 backend/）
docker compose up -d --build

# 3) 验证
curl http://127.0.0.1:8787/health        # {"ok":true}
```

**4) 首次使用：配置 LLM**（容器不预置 LLM 配置）——打开 `http://<host>:8787/`，
用令牌登录 → 左侧「设置 → 模型」→「添加供应商」（名称 / Base URL / API Key）
→ 在该供应商下「新增模型」→ 保存（右侧「检测」可先验证文本+图片识别能力）。保存即热更，
无需重启；之后新建会话即可对话。

启动后三种使用方式：

- **浏览器**：打开 `http://<host>:8787/`，输入访问令牌登录——完整前端
  （与桌面同一套 Vue 工程）；
- **CLI**：`ASTORM_SERVER=http://<host>:8787 ASTORM_TOKEN=… astorm status`；
- **API**：`/api/v1/*`（见 §6）。

---

## 2. 参数配置

配置分三层，优先级从高到低：**命令行旗标 → 环境变量 → 配置文件 JSON**。

### 2.1 必填参数（compose `.env`）

| 变量 | 说明 |
|---|---|
| `ASTORM_TOKEN` | 访问令牌（Bearer；Web 登录、CLI、API 共用）。**泄露等同 shell 授权** |

**LLM 供应商默认在 Web 设置页配置**（容器不预置 LLM 配置）：登录后「设置 → 模型」
添加供应商与模型，保存写入 `/data/config/providers.json` 并热更——列表第一项即默认
供应商，其第一个模型为默认模型（新建会话未指定模型时用它）。apiKey 支持 `${ENV}`
占位（见 §2.4）。

自动化/CI 需要**预置**时才用下面三件套（三件套齐全才生效；仅在配置文件无供应商时
合成名为 `default` 的供应商，启动时物化为 `providers.json` 种子）：

| 变量 | 说明 |
|---|---|
| `ASTORM_LLM_BASE_URL` | LLM API 地址（OpenAI 兼容 `/v1` 结尾；也支持 Anthropic/Responses 协议，见 §2.4） |
| `ASTORM_LLM_API_KEY` | LLM 密钥（支持 `${ENV_VAR}` 占位写法，见 §2.4） |
| `ASTORM_LLM_MODEL` | 默认模型名（同时作为模型引用 id） |

### 2.2 服务端旗标（`docker-compose.yml` 的 command 或 `docker run` 参数）

| 旗标 | 默认 | 说明 |
|---|---|---|
| `--host` | `0.0.0.0` | 监听地址 |
| `--port` | `8787` | 监听端口 |
| `--token` | 读 `ASTORM_TOKEN` | Bearer 令牌（缺省随机生成并打印；`--no-auth` 关闭鉴权，仅限本机调试） |
| `--data-dir` | 读 `ASTORM_DATA_DIR` | 数据根目录（镜像内默认 `/data`，挂卷） |
| `--workspace` | 数据根（容器内 `/data`） | **用户目录根**（工作空间/知识库/记忆的上级；core 取其下 `workspace/` 作会话工作区）。会话仍可按请求覆盖 |
| `--config <json>` | 无 | 应用配置文件（与桌面版 `start(config)` 的 JSON **同构**；§2.4） |
| `--event-buffer` | `200` | 每会话事件环形缓冲条数（SSE/WS 断线回放深度） |
| `--web-dir` | 读 `ASTORM_WEB_DIR` | Web UI 静态资源目录（镜像内已内置 `/app/web`） |

### 2.3 环境变量汇总

| 变量 | 默认 | 说明 |
|---|---|---|
| `ASTORM_DATA_DIR` | 平台约定（镜像内 `/data`） | 数据根：sqlite / 会话 jsonl / 索引 / 日志；**工作空间根缺省也取它** |
| `ASTORM_TOKEN` | — | 访问令牌 |
| `ASTORM_LLM_*` | — | 可选：预置供应商（自动化/CI）；默认在 Web 设置页配置，见 §2.1 |
| `ASTORM_WEB_DIR` | `/app/web` | Web UI 静态目录（`web-bridge.js` 由专用路由 no-cache 下发） |
| `ASTORM_CHROME_BIN` | `/usr/bin/chromium` | 渲染用 Chromium（webfetch/websearch 依赖） |
| `ASTORM_WEB_RENDER_BIN` / `_SCRIPT` | `/usr/local/bin/astorm-render` / `builtin` | 页面渲染助手（说 WebRender 的 stdin/stdout 协议） |
| `ASTORM_OFFICE_BIN_DIR` | `/opt/astorm/office-bin/bin` | office 五件套目录（可选层，见 §4） |
| `ASTORM_BROWSER_ENDPOINT` | — | 浏览器工具 CDP 端点覆盖（Phase B） |
| `TZ` | 宿主时区 | 建议设 `Asia/Shanghai`，会话命名/日志按本地时间 |

### 2.4 配置文件（`--config` 或 Web 设置页）

数据卷内 `config/` 下三个 JSON 文件，均可由 Web 设置页直接编辑（保存即热更，无需重启）：

| 文件 | 内容 |
|---|---|
| `providers.json` | LLM 供应商列表（`{providers:[{name,baseUrl,apiKey,apiFormat,models:[{name,order}]}]}`）；apiKey 支持 `${ENV}` 占位 |
| `settings.json` | 设置聚合：`locale / summary / compact / goal / webSearch / workspace / security / memory / instructions`（与桌面设置页同构） |
| `mcp_servers.json` | MCP 服务器声明（`{"mcpServers":{"名字":{"command":"npx","args":[...],"env":{...}}}}`，或 `url` 走 HTTP/Streamable HTTP）；**连接器在 core（Rust）**，保存即重载并连接启用项，`enabled` 缺省为 true。远程服务器可带 `headers`（手动 Bearer / API Key）与 `timeout`（毫秒，握手与单次工具调用共用） |

远程 MCP 返回 HTTP 401 时，core 会把服务器标记为「需要授权」，设置页卡片出现「授权」入口：
- **OAuth**：core 读取 `WWW-Authenticate` 的 resource metadata → 动态注册客户端 → PKCE(S256) →
  前端用系统浏览器打开授权页 → 本机回环 `127.0.0.1:<随机端口>/callback` 接收 code 并换 token；
  token 落 `config/mcp_oauth.json`（0600），过期自动用 refresh_token 刷新。该流程要求浏览器与
  后端在同一台机器，**云容器只支持手动填写 headers**（`begin_mcp_oauth` 在云端明确降级）。
- **手动**：授权页可粘贴 Bearer Token 或填写任意请求头，保存后写入该服务器的 `headers` 并重连。

MCP 服务器开箱可跑：镜像内置 node/npm（npx 系）与 python3/pip + uv（uvx 系），
包缓存落 `<数据根>/.cache/`（容器重建不重下）。连接后的工具直接进入会话工具集
（无需逐个授权；工具名与内置重名时自动加 `{服务器}__` 前缀）。

也可以在启动时传与桌面同构的完整 JSON：`--config /data/config/app.json`。
热更语义：`providers.json` **空列表不清空** env 注入的供应商（防误清）；安全配置
`security.enabled=false` 会关闭 bash 防护（容器内不建议）。

---

## 3. 数据与持久化

```
/data（Docker 卷 astorm-data）
├── config/          # providers.json / settings.json / mcp_servers.json
├── data/
│   ├── sqlite.db          # 会话/智能体/任务/交付物元数据
│   ├── session/<id>/      # message.jsonl 消息历史 + 摘要备份
│   ├── index/ skill_index/ mail/   # 全文索引 / 邮件缓存 / 技能索引
│   └── attachments/staging/        # Web 上传附件（24h TTL 自动清扫）
├── log/             # astorm.log（按天滚动）+ audit/<会话id>.jsonl（安全审计）
├── workspace/       # agent 工作空间（按会话时间戳建子目录：workspace/<ts>/）
├── knowledge/       # 知识库（真实文件树 + .manifest.json）
└── memory/          # 长期记忆 Bundle（OKF：raw/ 来源 + wiki/ 概念）
```

用户侧目录（workspace / knowledge / memory / schedule）都以**用户目录根**为上级，
该根缺省 = 数据根（容器内 `/data`），可用 `--workspace` 覆盖；因此卷内不会出现
`workspace/workspace` 这类双层嵌套。

- **备份**：`docker run --rm -v backend_astorm-data:/data -v $PWD:/bak alpine tar czf /bak/astorm-data.tgz /data`
- **恢复**：反向解包到卷后 `docker compose restart`
- **升级**：`git pull && docker compose up -d --build`（数据卷不动）
- **重启语义**：会话/设置/历史全保留；SSE/WS 实时回放缓冲是内存态，重启后为空（历史走 `show`/消息接口）

---

## 4. office 工具 + 内置 skill（默认内置）

office 五件套（统一入口 astorm-office + json2docx/xlsx/pptx/pdf）与 office skill **默认内置**，构建时自动从
兄弟仓库 `../astorm-office` 编译 Linux 版（rustls，无 OpenSSL 依赖）：

```bash
# 构建前同步 office 构建源（剪枝后 ~8M，拷入 backend/.office/）
pnpm run sync:office-build        # 等价：node backend/scripts/sync-office-build.mjs
docker compose up -d --build
```

- 源仓库缺失时自动降级：镜像不含 office，工具按契约报「未找到 CLI」，其余能力不受影响
- 内置内容：五个 CLI（`/opt/astorm/office/bin`，含 `astorm-office` 统一入口）+ office skill
  （`/opt/astorm/office/skill`，启动时注册为 builtin 技能，`list_skills` 可见 `astorm-office`）
- 对应环境变量：`ASTORM_OFFICE_BIN_DIR` / `ASTORM_OFFICE_SKILL_DIR`（镜像已设，
  自定义部署时可覆盖）；`office` 段随启动与设置保存热更注入配置

---

## 4b. 浏览器工具（agent-browser CLI，默认内置）

容器里的 `browser` 工具**与桌面同一条路**：core 把动作编译成 argv → spawn `agent-browser`
CLI → CLI 用 `--cdp` 附着到服务端自有 headless Chromium（该浏览器同时供面板画面流）。
差分只有两处：端点文件由**服务端**写（桌面是主进程写）、CLI 由镜像内置而非桌面安装包。

```bash
# 构建前同步 agent-browser 源码（剪枝后 ~7M，拷入 backend/.agent-browser/）
pnpm run sync:agent-browser-src   # 等价：node backend/scripts/sync-agent-browser-src.mjs
docker compose up -d --build
```

- 源码 = `vercel-labs/agent-browser`（Apache-2.0），版本记在
  `backend/.agent-browser/manifest.json`，**须与桌面 `frontend/resources/agent-browser/<os>-<arch>`
  同一版本**（动作词汇/ref 语义一致）；环境变量 `AGENT_BROWSER_ROOT` 可指向别处的 clone
- 源码**随构建上下文进镜像**（构建容器实测连不上 github.com）；该阶段另设 crates 镜像
  （`--build-arg CARGO_MIRROR=sparse+https://rsproxy.cn/index/`，默认可覆盖，境外传空即回官方源）
- 源缺失/编译失败自动降级：镜像不含 CLI（`/opt/astorm/agent-browser/bin/NOT-BUILT` 留痕），
  `browser` 工具落回**服务端直连 CDP** 实现（`astorm-server/src/browser/`），能力不受影响
- 端点文件：`/data/electron/browser-agent.json`（`{wsUrl,token,frameUrl,tabId,sessionId,pid}`），
  由服务端在启动/取页/动作后写入，浏览器回收时撤掉

---

## 5. 网络与安全

| 事项 | 建议 |
|---|---|
| 令牌 | 强随机；Web/CLI/API 共用。泄露等同 shell 授权（agent 可在容器内执行命令） |
| 端口暴露 | 仅本机用：compose 里改 `"127.0.0.1:8787:8787"`；对外暴露务必挂反代加 TLS（容器内是明文 HTTP） |
| 容器边界 | Linux 沙箱为 Passthrough（静态预检生效：危险命令/凭据黑名单命中即拒），内核级隔离依赖容器边界——**不要把 docker socket 挂进容器** |
| 密钥 | `providers.json` 明文落卷（容器卷权限即边界）；优先用 `${ENV}` 占位注入 |
| 单写约束 | 存储单写设计：**一容器一实例一数据卷**；多用户 = 多开容器（各自令牌与卷），不可共享卷跑多副本 |

---

## 6. API 摘要（`/api/v1`，`Authorization: Bearer <令牌>`）

信封 `{code, message, data}`（0=成功）。完整路由见 `docs/cloud-server.md` §API，常用：

```
GET  /health                              探活（免鉴权）
GET  /status                              版本/路径/模型配置
POST /sessions                            创建会话（{model:"",userMessage:"…",autoRun:true}）
POST /sessions/{id}/messages              追加消息（202）
GET  /sessions/{id}/events                SSE 事件流（?after=<seq> 断线续传）
GET  /api/v1/ws?token=&after=             WebSocket 事件流（Web UI 用）
POST /sessions/{id}/interrupts/resolve    审批/表单决议
POST /llm/ask                             单次补全（SSE）
```

---

## 7. 疑难排查

| 现象 | 处理 |
|---|---|
| 构建报 `502 Bad Gateway`（解析基础镜像元数据失败） | 镜像加速源间歇故障（如 daocloud）：**重试即可**；频繁发生则在 Docker Desktop 设置更换/增加 `registry-mirrors`，或可达 Docker Hub 时临时移除该配置。每次 `--build` 都会向镜像源发元数据查询（镜像在本地也如此） |
| 容器起不来：`GLIBC_2.39 not found` | 运行时基础镜像与 builder 不同基线（本镜像已对齐 trixie；自定义改动时注意） |
| `/health` 通但任务无模型 | 供应商未配置：Web 设置页「模型」添加供应商+模型并保存（默认路径，见 §2.1）；若曾把 `providers.json` 写空且靠 env 预置，`down -v` 后重启会重新物化种子 |
| webfetch 报「内置 Chromium 渲染失败」 | 检查 `ASTORM_CHROME_BIN` 与容器内 chromium；渲染助手日志在容器 stdout |
| websearch 只出部分引擎结果 | baidu/google 需渲染防反爬；Chromium 正常时可用，数据中心 IP 仍可能被风控 |
| CLI 报 `会话不存在` | 服务端重启后旧会话首次访问会自动重建重试一次；持续报错属旧数据卷 |
| 想在容器外用 CLI | 单二进制拷走即可（`backend/target/release/astorm`），`ASTORM_SERVER` 指向容器 |
