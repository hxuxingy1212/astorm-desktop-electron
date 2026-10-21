# astorm 云端无头服务（feature-cloud / Phase A）

> 桌面版之外的第二宿主：把 astorm-core 直接暴露为 HTTP + SSE API，跑在 Docker 容器里。
> Phase A 目标：**纯无头 API/CLI**；Phase B（另开阶段）：单用户容器 + Web UI + 全能力。

## 架构

```
astorm CLI（薄 HTTP 客户端）──┐
                              ├─ HTTP + SSE（Bearer Token）→ astorm-server（axum）
浏览器 Web UI（Phase B）──────┘                                    │ 直接调 facade
桌面版 Electron（不变）── IPC → astorm-napi（.node）───────────────┤
                                                                   ▼
                                    astorm-core（共享内核：App::init / session / skills / …）
                                    react-loop · llm-provider · astorm-storage · astorm-sandbox
```

要点：
- **两个宿主，一套内核**：`astorm-server` 与 `astorm-napi` 是同一 core 的两个宿主，
  服务端不经 napi 直接调 `App::init` + 各 facade，业务逻辑零分叉；桌面版完全不动。
- **Linux 沙箱**：`astorm-sandbox` 新增 `Platform::Linux`（Passthrough 直通 + 静态预检，
  强制力诚实上报 NotSupported，见 `enforcement_report` 的 Linux 清单）；容器边界即隔离边界。
- **事件流**：core 的广播总线无重放；server 启动即常驻订阅，给每条事件分配单调 seq，
  按 `session_id` 存环形缓冲（默认 200 条）。SSE 先回放 `?after=` 之后的缓冲再续播直播，
  `id:` 即 seq（支持断线续传），解决「POST 建会话后再开 SSE 必漏事件」的竞态。

## 启动（本机）

```bash
cd backend
cargo build --release -p astorm-server -p astorm-cli

ASTORM_DATA_DIR=/tmp/astorm-data \
ASTORM_TOKEN=my-token \
./target/release/astorm-server --port 8787
# 工作空间/知识库/记忆缺省落在数据根下（/tmp/astorm-data/{workspace,knowledge,memory}）；
# 要换地方用 --workspace <用户目录根>（注意：这是工作空间/知识库的**上级**目录，
# core 取其下 workspace/ 作会话工作区——传 /tmp/ws 会得到 /tmp/ws/workspace/<ts>）。
# LLM 供应商不预置：浏览器登录后在「设置 → 模型」配置（写 <data>/config/providers.json）。
# 自动化/CI 可用 ASTORM_LLM_BASE_URL / ASTORM_LLM_API_KEY / ASTORM_LLM_MODEL 预置。
```

配置优先级：`--config app.json`（与桌面版传给 napi `start()` 的 JSON **同构**）为底，
env 覆盖其上：`ASTORM_DATA_DIR`（可选 `ASTORM_LLM_*` 三件套：配置无供应商时合成
default 供应商）；工作空间根缺省 = 数据根，`--workspace` 可覆盖。
`--token` 缺省时随机生成并打印；`--no-auth` 仅限本机调试。

## CLI

```bash
export ASTORM_SERVER=http://127.0.0.1:8787 ASTORM_TOKEN=my-token
astorm status                                   # 服务端状态
astorm newtask "帮我写个脚本" --detach           # 创建任务（后台跑）
astorm newtask "..."                            # 创建并跟随输出直到完成
astorm tasks                                    # 任务列表
astorm show <id> [--follow]                     # 查看历史 / 观战
astorm stop <id>                                # 取消
astorm pending                                  # 待审批列表（含命令与原因）
astorm approve <id> [--scope session]           # 同意（once/session）
astorm deny <id>                                # 拒绝
```

所有命令支持 `--json`（原始 JSON 输出）。

## API（/api/v1，信封 `{code,message,data}`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | /health | 探活（免鉴权） |
| GET | /status | 版本 / 平台 / 路径 / LLM 配置 |
| POST | /sessions | 创建会话（`userMessage` 必填；`model` 空则用服务端默认；`autoRun` 缺省 true） |
| GET/DELETE | /sessions, /sessions/{id} | 列表 / 详情 / 删除 |
| POST | /sessions/{id}/messages | 追加消息并驱动循环（202） |
| POST | /sessions/{id}/start · /cancel · /fork · /rewind · /summarize | 生命周期 |
| POST | /sessions/{id}/interrupt | 打断并优先执行 |
| POST | /sessions/{id}/interrupts/resolve | 提交审批/表单决议（`InterruptResolution`） |
| POST | /sessions/{id}/interrupts/{toolCallId}/reopen | 重开被忽略的表单 |
| PATCH | /sessions/{id}/settings | 会话级设置（缺省字段保持） |
| GET | /sessions/{id}/messages · /messages/archived | 历史 / 归档分页 |
| GET | /sessions/{id}/system-prompt | 系统提示词 |
| GET | /sessions/{id}/events · /events | SSE 事件流（单会话 / 全局；`?after=<seq>`） |
| POST | /llm/ask | 单次补全（SSE） |
| GET | /deliverables · /sessions/{id}/deliverables | 交付物 |

请求/响应直接复用 core 的 serde 类型（`CreateSessionRequest` / `SessionSettings` /
`SessionRecord` / `MessageRecord` / `InterruptResolution`，camelCase），无镜像 DTO。

## Web UI（复用桌面前端工程，同源托管）

镜像内带完整 Web UI：`docker compose up -d` 后浏览器直接打开
`http://<host>:8787/` 即完整前端（与桌面同一套 Vue 工程）。

- **复用方式**：`electron-vite build` 产出 `out/renderer` 静态 SPA（hash 路由），
  server 以 `--web-dir` 同源托管（SPA fallback）；`index.html` 引入的
  `web-bridge.js`（`frontend/src/renderer/public/`）在浏览器环境实现
  `window.api` 桥——桌面 preload 已暴露时脚本自退，双端零分叉。
- **桥接面**：65 通道 preload 契约 → REST（chat/git/providers/fs/office）+
  `/api/v1/bridge`（backend.invoke 的 ~90 命令直调 core facade）+ WS 事件
  （`/api/v1/ws?token=&after=`，与桌面 `napi:event` 载荷一致，断线重连按
  seq 回放补缺口）。会话请求体在 shim 做 napi→core 字段映射
  （message→userMessage、mode→sessionMode、permission→permissionMode）。
- **登录**：首次打开弹令牌遮罩（ASTORM_TOKEN），存 localStorage；
  401 自动重弹。
- **web 降级矩阵**（与桌面差异）：原生对话框→浏览器上传（附件进服务端
  staging，`toUrl` 映射为 `/api/v1/attachment/file/…`）；元素拾取/页签截图/
  录制/iframe DevTools→报错（无 CDP 桥；iframe 页签本身可用）；窗口控制/
  权限/power→no-op；插件运行时、Chrome cookie 导入→明确降级（不做/无本机
  浏览器）；终端页签在 web 下全功能（PTY 走 `/api/v1/bridge` 的 pty_* 命令；
  切换页签不断连、关闭页签才断开）。**MCP 已可用**：连接器在 core（Rust），
  见下节。
- **无窗口装饰布局**：web-bridge 注入 `window.api.__web=true`，渲染层据此走
  「浏览器部署」分支（`utils/platform.ts` 的 `isWebShell`/`hasTrafficLights`/
  `showWindowControls`）——没有红黄绿也没有自绘窗口按钮：侧栏品牌行显示
  logo+名称、折叠按钮常驻顶栏左缘、顶栏不预留窗口按钮留白、不画最小化/
  最大化/关闭。桌面端（preload 无 `__web`）行为不变。
- **桌面独有设置页隐藏**：设置注册表 `webHidden` 标记（当前只有插件页——
  安装走本地目录对话框，容器里不可用）在 web 下从菜单隐藏，直达
  `/settings/plugins` 回落系统设置。
- **配置**：providers.json / settings.json 落 `<data>/config/`（UI 设置页
  直接编辑并热更 core）——**LLM 供应商默认就在 Web 设置页配置**，容器不预置；
  可选 env 预置（`ASTORM_LLM_*`，自动化/CI 用）在启动时物化为该文件种子
  （UI 可读可改；空列表保存不会清掉 env 配置——App.vue 的 models 深度
  watcher 启动即 persist 一次，服务端做了防御）。

## Electron 壳子（同一容器，不用浏览器也行）

单用户云容器除了浏览器访问，也能用**桌面壳子**接：Electron 启动时先走**引导**，
云端就是把窗口直接指向容器端点——同一套 Web UI、同一个令牌，只是换成原生窗口（有菜单栏、
标题栏、Dock 图标），不必开浏览器标签页。

- **引导**（`frontend/src/renderer/src/onboarding/`：`OnboardingApp.vue` + `StepWelcome` /
  `StepChoice` / `StepModel` / `StepPrefs`）：① 欢迎 → ② 选择连接方式（本机 / 云容器）→
  ③ **配置模型（仅本机分支）** → ④ 偏好（**可跳过**）。步骤表由所选分支决定（`flow` computed）：
  本机 4 步、云端 3 步，步骤点数量跟着变。启动时读 `connection.json` 的 `onboarded` 标记：
  **未完成引导**才落在第 ① 步；**已完成**则按 `mode` 直接进应用（旧配置按 mode 自动补写标记）。
  只有菜单/设置点「连接方式…」才会重新打开引导页；已配置过直接落在第 ② 步，左上角「返回」
  回当前形态。欢迎页不再提供「已有配置，直接进入」。
  - 云容器卡**就地展开**端点 + 令牌表单（令牌即容器启动时的 `ASTORM_TOKEN`），带「测试连接」
    （`GET <端点>/api/v1/status`，Bearer），成功显示服务端模型与数据目录；端点归一化：
    缺协议补 `http://`、去尾斜杠、粘成 `…/api/v1` 也认。**「继续」要求先连通**（没测过就现测，
    连不上不放行）；本机分支无需额外配置，选好直接「继续」。
  - **配置模型（本机独有）**：本机后端要有一个模型才能干活，而全新安装是空的。五个预设
    （DeepSeek / OpenAI / 智谱 GLM / Moonshot / Anthropic / 自定义）点一下填好地址、协议与默认
    模型；「保存并继续」会自动走 `providers:test` 发一次**图文能力探测**（30s 超时）：同一张
    739 图 + 文字指令，模型必须同时读图并遵循格式才通过，成功文案「文本与图片识别正常」；
    不支持视图的模型提示「不支持文本+图片识别」并拦截在当前步（可「跳过」）。
    「保存并继续」= 先落盘再探测，**探测不通过不放行**；一个字没填则直接过。
    数据面与设置页完全同一套（`useModelsStore` → `providers:sync` → 主进程 safeStorage 加密写
    `config/providers.json` + 热更本机后端），保存语义是**同名就地替换**，重复跑引导不会冲掉
    用户已有的其它供应商。云端分支不出现这一步：容器的模型在容器自己的「设置 → 模型」里配，
    壳子不碰远端配置。
  - 第 ④ 步的明暗 / 语言 / 保持电脑运行**即时生效并落盘**（引导阶段 App.vue 没挂载，
    由 OnboardingApp 自己 watch），所以跳不跳都不亏。
  - 视觉：**无卡片/无面板**——内容（含偏好行的发丝线分隔）直接落在动态背景上，只有控件本身
    有底（选项卡、输入框、分段控件），像系统开箱设置而不是弹窗表单；主题色光斑缓慢漂移
    （`prefers-reduced-motion` 下静止）+ 内容区一圈无边界柔光压住文字；步骤淡入上浮过渡
    （260ms 同 macOS 缓动）；欢迎步是居中 hero + 居中主按钮；主按钮按动作按钮规范压回
    **`--accent-text` 实心**（全局把 `.arco-btn-primary` 压成中性灰）；选中卡淡底用
    `color-mix(in srgb, var(--accent-text) 12%, transparent)` 而非 `--primary-1`
    （后者是浅色档，深色主题下会变一块白）；
    中英文文案在 `i18n/locales` 的 `onboarding.*`，**探测结果文案由渲染层按 code 本地化**
    （主进程没有 i18n 实例，只回 code + 结构化字段，
    免得英文界面里蹦中文）。注：整屏是拖动区（无边框窗口），步骤点必须 `-webkit-app-region:
    no-drag`，否则真实鼠标点不到；留白走 `--ob-stage-pad` / `--ob-step-pad` 两个变量，窗口矮
    （`max-height: 720px`）时自动收紧，根容器纵向可滚（背景改 `position: fixed`，否则
    `inset: -12vmax` 的光斑会撑出一片可滚的空白）——最小窗口 780×600 下模型步不再被裁掉。
- **本机/云端两形态**（`frontend/src/main/connection.ts` + `main/index.ts`）：
  配置落 `<userData>/connection.json`（`mode` + `endpoint` + 令牌 + `onboarded` 引导完成标记，
  令牌经 safeStorage 加密，不可用时降级明文并标注 storage——与 providers.json 同套做法）；
  **窗口形态是创建期选项**
  （本机 = 无边框 + 桌面 preload；云端 = 原生边框 + `preload/cloud.js`），故切形态 = 按当前
  尺寸重建窗口（`openWindow`）。欢迎页与本机同属「本机形态」，**不换窗口**只重载。
  - 本机形态：一切照旧（进程内 napi 后端、无边框窗口、自绘窗口按钮）；
  - 云端形态：**不启本机后端、不装 demo 插件、不推后端配置**，窗口 `loadURL(<端点>/)`，
    `will-navigate` 只放行同源（外链交系统浏览器）。从云端切回本机时补推一次后端配置
    （`bootstrapLocalBackend`）。
- **云端 preload 刻意不暴露 `window.api`**（`frontend/src/preload/cloud.ts`）：它只把令牌写进
  远端 origin 的 `localStorage['astorm:web:token']`（免登录遮罩）。挂 `window.api` 会让远端
  Web UI 的 `web-bridge.js` 误判成桌面形态去调本机 IPC——那条链路就错了。
- **切换入口**：macOS 应用菜单「连接方式…」（⌘⇧M；云端窗口不隐藏菜单栏，Win/Linux 靠它当
  可见入口）+ 设置页「连接方式」（`webHidden`，只在桌面壳可见——浏览器部署没有「本机」可选）。
- **原生边框的理由**：云端跑的是**浏览器形态**的 Web UI，它按浏览器语义隐藏自绘窗口按钮、
  也不为 macOS 红黄绿预留位置；交回系统标题栏三者都不冲突（本机形态继续无边框）。
- 实测（2026-10-13，dev + 生产构建各一遍）：首次启动出欢迎页；`127.0.0.1:8787`（不带协议）
  也能连上并显示「已连接 · 模型 deepseek-flash · 数据 /data/data」；连云端后窗口直接是容器
  的 Web UI、无令牌遮罩（令牌已注入）、窗口内容高比窗口矮 32px（原生标题栏）；菜单
  「连接方式…」→ 欢迎页 → 使用本机 → 本地会话列表回来（39 任务，与本机数据一致）；
  生产构建（`pnpm -C frontend start`，file:// 加载）同样跑通。
  注：本机模式启动到云端启动、再切回本机，全过程窗口不重启应用（只重建窗口）。

### 引导 e2e（`pnpm run test:e2e:onboarding`）

`frontend/scripts/e2e/onboarding.mjs`（驱动库 `lib.mjs`）在**隔离实例**上真跑一遍 Electron：
`ASTORM_DATA_DIR` 指向临时目录（数据目录与单实例锁都与用户正在用的那份无关，绝不碰真实数据），
`ASTORM_CDP_PORT` 起 CDP 用真实点击/输入驱动（不做内部状态注入），云端分支用**本地桩服务端**
  （`/api/v1/status` 令牌校验 + 一次图文能力探测 + 带标记的首页），模型探测也打在同一个桩上，

不依赖真实容器与真实模型。

- 四组用例：① 首次启动走完本机四步（预设回填、模型图文能力探测、供应商落盘与密钥加密、
  「上一步」回退带出已存配置、明暗/语言即时生效、落盘断言）
  ② 重启直接进应用 + 切换连接方式打开引导页并「返回」 ③ 云端：测连接（裸 `host:port` 验归一化）→ 跳过偏好
  （**并断言云端不出现模型步**、步骤点 3 个）→ 加载远端 UI + 令牌注入 + 远端页不暴露 `window.api`
  ④ 负例：令牌错误 → 报错且「继续」不放行。
- 退出码 = 失败断言数；失败保留沙箱（含 `fail.png` 现场截图与 app 日志尾部）。
- 两个已踩的坑（脚本里处理了）：`.bin/electron` 只是启动壳，**杀它不杀 app**（残留实例会占着
  CDP 端口，下一轮就静默连到旧窗口上——必须按数据目录扫进程组杀）；**被遮挡的窗口 Chromium
  会节流定时器**，Vue 的 `out-in` 过渡迟迟不结束，页面停在「空白卡片」上（启动参数带
  `--disable-background-timer-throttling` / `--disable-backgrounding-occluded-windows` /
  `--disable-renderer-backgrounding`）。
- 实测：2026-10-13 连续四轮 32/32 全绿；加「配置模型」步后 2026-10-14 复跑 **41/41 全绿**，
  2026-10-20 加引导完成标记 / 移除「直接进入」后 **42/42 全绿**
  （图文能力探测打桩、供应商落盘与加密、云端不出现模型步都在断言里）。同时按「明/暗 × 中/英」
  逐屏截图核验过（欢迎/选择/配置模型/偏好四步、本机与云端两分支、错误态）。

## 代码审查与业务流程实测记录（2026-10-12）

**审查**：全量 diff 审查（5 P0 / 9 P1 / 若干 P2）全部修复——附件路径遍历与
分隔符重建、审批决议字段转换、设置聚合契约（snake→camel 映射）、MCP/KB/
交付物契约对齐桌面、WS 回放去重、seq 重启水位、fs 越界与上传限额、
providers 聚合契约（{providers:[...]}）等；回归 e2e 18/18。

**MCP 实测（2026-10-12，真实 LLM + 容器）**：设置 → 工具 → MCP 写配置即自动连接
（`npx -y @modelcontextprotocol/server-everything` 13 个工具、`uvx mcp-server-fetch` 1 个），
`list_mcp_servers` 两项 `connected:true`、设置页卡片显示「已连接」并展开工具清单；真实模型
按提示调用 `get-sum` 工具（工具结果 "The sum of 3 and 4 is 7." 原样回报）→ 连接器 + 工具下发
+ 调用回路全通。首次连接 npx/uvx 要下载包（实测 18s，握手超时已放宽到 45s；包缓存落
`/data/.cache/`，容器重建不重下）。

**Web 页面业务流程实测**（真实 LLM，浏览器逐项）：
- 对话流式渲染 + 多工具真实执行（file_operation/bash 写容器卷）✓
- 表单流程（awaiting_form 卡渲染 → 回答提交 → 模型续跑）✓
- 审批流（审批卡渲染 → 同意执行 → 沙箱扩白名单重跑 → done）✓
- 会话模式切换（PATCH settings：plan→execute 实证）✓
- 设置页四板块（模型供应商显示/安全/全局约定保存读回/记忆）✓
- 交付物/定时任务/邮箱降级页加载 ✓；持久化（重启后 providers/AGENTS.md/
  设置聚合/会话全保留）✓；真实网络访问 ✓

**已知边界（记录不阻塞）**：~~三项遗留~~ 已全部修复并实测（c52fdc1）——
① 草稿双条目合并（createSession 后合并事件登记条目，终验 0 重复；
   也是连续表单卡选项不渲染的根因）；② 终端页签 web 创建 + PTY 交互
   （含 pty_write 串行链修复：web fetch 并发乱序会交错键入）；③ 连续表单回归通过。
   另修：stage* 返回对齐桌面 StageResult {ok,failed}、rewind body snake_case、
   web-bridge.js no-cache 路由（缓存旧桥混跑实测踩坑）、kb_root/memory_root 补齐。
   剩余：非多模态模型在「检测」阶段即被拦截（不支持文本+图片识别）；图片附件仍受具体供应商能力限制。

**LLM 配置改由 Web 设置页负责（2026-10-12，ea18c2b/8ff823f）**：容器不再注入
LLM env（compose 只留 `ASTORM_TOKEN`），首次使用在「设置 → 模型」添加供应商+模型。
实测无 env 全新容器全流程：页面加供应商 → 「检测」文本+图片识别通过 → 对话真实回复；重建
容器后配置仍在（卷内 providers.json）且对话可用。**同轮修掉三处缺口**：
① 启动不加载已保存配置——`AppState.config` 启动即固定、`update_config` 只换内部
注册表不回写它，只在保存时热更的话容器重启后核心退回 env 默认（实测 `/status`
providers=[] 且对话空回复）→ 启动即 `sync_config_to_core`（文件存在才做）；
② `providers/test` 漏了 UI DTO→AppConfig 的 `expand_providers` 转换（models 缺
`id`）→ 反序列化失败一律误报「供应商 X 不存在」，「检测」按钮从未成功过；
③ 默认模型解析（会话 model 为空时）改读 `effective_providers`（providers.json
第一供应商第一模型优先），CLI `newtask` 不带 `--model` 也能用。

**工作空间根 = 数据根（2026-10-12，0b86312）**：卷里出现 `workspace/workspace`
双层的根因——core 的 `AppConfig.workspace` 是**用户目录根**（下辖 workspace/
knowledge/ memory/ schedule/），会话工作区 = `<用户目录根>/workspace/<时间戳>`；
原 compose 设 `ASTORM_WORKSPACE=/data/workspace` 就多出一层。已删该 env（含
config.rs 兜底），用户目录根缺省 = 数据根 → 容器内会话 `/data/workspace/<ts>`、
知识库 `/data/knowledge`、记忆 `/data/memory`；`--workspace` 仍可显式覆盖（传的是
用户目录根这一级）。改动前的旧会话目录仍在 `/data/workspace/workspace/`（会话记录
指向旧路径，不自动迁移）。

**终端与无窗口装饰（2026-10-12，8532d6a/9c08a70）**：容器内终端改信息式提示符
（`bash --norc` + `PS1=[\u@\h \w]\$`，实测 `[root@<host> /path]#` 对齐 SSH 观感；
桌面 $SHELL 场景不注入）；页签语义实测——终端内 `cd /tmp` 后切页签/切会话再切回
仍在 `/tmp`（PTY 服务端持久），点页签 X 关闭后重开回到工作区（新 shell）。布局按
「无窗口装饰」修正：修复前 Mac 浏览器被 shim 报成 darwin，侧栏品牌行只剩带 76px
缩进的折叠按钮、顶栏折叠按钮带 72px 偏移（截图确认）；修复后品牌行 logo+名称、
折叠按钮常驻顶栏左缘、无窗口按钮；插件设置页在 web 下隐藏且直达 URL 回落。

**终端串行链「中毒」根因（2026-10-12，6984f3b）**：容器/后端重启后，页面里首个
`pty_resize`/`pty_write` 会 500（PTY 随重启消失）。shim 的 `ptyChain`（为修并发乱序
而加的全局串行链）用 `ptyChain.then(run)` 串联——**某环 reject 后整条链永久 rejected**，
此后每次键入都短路回放旧错误、请求根本不发出，症状是"终端敲什么都没反应，刷新页面
才恢复"（曾误判为 PTY/沙箱问题，实际后端一次都没收到请求——用页面内 fetch 包装日志
定位）。改 `.then(run, run)` 让失败后链照样推进；配合面板在 `终端会话未打开` 时写一次
「关闭并重开页签可重建 shell」提示。实测：重启容器（页面不刷新）→ 键入得到断开提示 →
关页签重开 → 输入恢复正常，无需刷新页面。

## E2E（本地 Docker 部署测试，可重复）

两套脚本，都是「起 mock LLM + 起容器 + 逐项断言」，退出码 = 失败数：

```bash
cd backend
bash scripts/e2e/run_e2e.sh      # ① 主链路：部署 / ReAct 循环 / SSE / 审批 / 持久化
bash scripts/e2e/run_gap_e2e.sh  # ② 补齐功能：目录选择器 / 审计导出 / 无文件管理器降级 / MCP
# 两者都可 E2E_KEEP=1 保留容器供检查
```

**② run_gap_e2e.sh（72 断言，2026-10-13 连跑 72/72）** 覆盖本轮补齐的六项：
- 目录选择器：`/fs/list-dirs` 缺省起点=会话工作区根、只列目录、隐藏目录跳过、
  `../..` 词法规范化后越界仍拒；`/fs/mkdir` 新建落卷 + 非法名（`.x`/`a/b`）与越界拒；
- 审计导出：审批流（network 中断 → approve）产生的审计条目经 `read_audit_log`
  返回（含 `approval=agree`、`category=network`，可拼 ndjson 下载）；
- 无文件管理器降级：web shim 的 `__web` 标记 / `fs.openFolder` unsupported /
  服务端无「打开文件夹」命令；
- MCP：写配置即自动连接 → 工具进目录（含重名 `mock__bash`）→ **mock LLM 驱动模型
  调用 MCP 工具**（`echo`）→ 结果经 stdio 子进程回流（`echo: MCP_ECHO_OK`）→
  断开后工具消失 → 重连回执；
- 浏览器工具（服务端 headless Chromium，驱动 = 镜像内置 agent-browser CLI）：`tool_load
  browser` → navigate → snapshot 出可点引用（`[ref=eN]` / `@eN` 两种驱动都认）→ click +
  read 得到页面 JS 结果（点击真的生效）→ screenshot 落卷且是合法 PNG（magic 校验）；
  CLI 路径证据 = 镜像内 `agent-browser --version` + 端点文件（wsUrl 指向服务端 Chromium、
  sessionId 命中）+ `/root/.agent-browser/astorm-*.sock` daemon 痕迹；
- 面板画面流：`/api/v1/browser/ws` 收帧 + 鼠标点击 + 文本输入（页面 title 变化即证据）
  + state / 帧尺寸都是 1280×900（视口覆盖生效，坐标映射不偏）+ `frameUrl` 跟随导航；
- 开新窗口：页面内 `target=_blank` 与 `window.open` 都变成本页导航（活动页 URL 变到目标页，
  不落面板看不见的隐形页签）；
- git 探测语义：`git_branch_state` / `git_list_branches` / `git_is_repo` 对「目录不存在」
  与「非仓库目录」都返回空值（`current=""`、`local=[]`、`is_repo=false`），真仓库照常
  返回分支名与本地分支列表——对齐桌面主进程 `hasDir` + `head.ok` 的判定（`run_git`
  失败时返回的是 stderr 末行，只读探测必须先看成功标志），渲染层据此在非仓库任务上
  隐藏分支 chip。

与 ① 的隔离：② 用独立容器名/端口/卷（18787 + `astorm-gap-e2e-data`，`docker run`
直起），**不会**碰 ① 的 compose 项目与 `astorm-*` 卷——① 的 `down -v` 会删卷，
故别拿 ① 对着有真实数据的部署跑。

**UI 层同项实测（浏览器，2026-10-12，对着运行中的容器）**：
- 目录选择器：新建对话 → 选择目录 → 模态列出 `/data/workspace` 下的目录（面包屑
  data / workspace、手输路径框预填）→ 输入 `ui-e2e-dir` 新建 → 列表出现 → 选中 →
  「选择此目录」→ 模态关闭、表单显示所选目录；
- 审计导出：设置 → 审计日志 → 选会话（3 条记录）→ 导出 → 捕获到 Blob
  `audit-22343438-….jsonl`（1971 字节、`application/x-ndjson`、3 行、内容首条为
  `category:file_read` 的审计记录）+ 提示「导出成功」；
- 无文件管理器降级：设置 → 全局约定页「打开文件」按钮不存在（web 下隐藏），
  而「选择目录」入口仍在（走服务端模态）。
- office 工具过程行（2026-10-13，真实模型 + 真实 `json2docx`）：让模型 view 一份 docx，
  过程行读数为「Word 品牌图标（`assets/file-icons/word.svg` 的 data URI）+ Word 文档 ·
  `view --json /data/workspace/demo-office.docx /part[1] text` · 说明」；说明两路都验过——
  模型填 `description` 时显示其原文，显式传空串时回落「查看文档内容」（子命令说法）。
  注：`POST /sessions` 的「model 为空即填默认模型」只在该端点上；`POST /sessions/{id}/messages`
  的 `model` 是必填契约字段（web UI / 桌面都随每条消息带上，直连 API 的脚本需自己带）。

脚本自建 OpenAI 兼容 mock LLM（`scripts/e2e/mock_llm.py`，脚本化「工具调用 →
结果回流 → 最终回复」两轮行为，无需真实凭据），然后 compose 全新部署单用户容器，
逐项断言：健康检查 / 401 / CLI 鉴权 → 完整 ReAct 循环（bash 工具调用经 SSE 流式
跟随到 done、工具真实落在容器 /data 卷）→ SSE after 游标续传 → 审批链路
（manual 权限 + 联网命令触发 Network 类中断 → CLI pending 扫描 → approve → 续跑
至 done）→ 容器重启后持久化（sqlite 卷会话/历史仍在；环形回放为空属预期——
内存态语义，历史走 show/message.jsonl）。退出码 = 失败数，两连跑 18/18。

## Docker

```bash
# 构建上下文 = 仓库根（Dockerfile 会取 backend/ 与 frontend/ 两棵树）
docker build -f backend/Dockerfile -t astorm-server:dev .
cd backend && docker compose up -d --build   # backend/.env 只需 ASTORM_TOKEN；LLM 供应商在 Web 设置页配置
# 迭代提示：只改了前端 UI 时，可直接把本地构建产物拷进运行中的容器免重建镜像
#   pnpm -C frontend build && docker cp frontend/out/renderer/. astorm-server:/app/web/
```

数据全部落在 `/data` 卷（sqlite / session jsonl / 索引 / 工作空间）。**存储单写**：
一容器一实例，不可共享卷跑多副本。

## Phase A 能力边界

- **页面渲染已容器化**：`astorm-render` 助手（astorm-server 的第二个 bin）与容器内
  Chromium 对接，说 Rust `WebRender` 的同种 stdin/stdout 协议——webfetch（硬依赖渲染）
  与 websearch 的 baidu/google 引擎开箱可用；抽取 JS 与 stealth 反爬脚本从
  `page-render.cjs` / `stealth-preload.cjs` 原样移植。本地调试可设
  `ASTORM_WEB_RENDER_BIN` / `ASTORM_WEB_RENDER_SCRIPT` / `ASTORM_CHROME_BIN` 接任意
  Chromium（容器镜像已内置并默认接好）。
- **browser 工具已容器化（与桌面同一条路 + 回落）**：`astorm-server/src/browser/` 一会话
  （按根会话 id）一 headless Chromium，懒启动、空闲 20 分钟回收；服务端写端点文件
  （`/data/electron/browser-agent.json`，wsUrl = 该浏览器的浏览器级 CDP 地址、pid = 服务端），
  core 因而 spawn **镜像内置的 agent-browser CLI**（构建期编 Linux 版，版本与桌面钉一致）
  附着到它；CLI 缺失/端点属于别的会话时回落 core 直连 CDP（`NativeBrowser`）。两条路
  共用同一套分级授权 / 审批 / 结果整形。**面板画面流**走
  `/api/v1/browser/ws`（`Page.startScreencast` 帧下行 + 鼠标 / 键盘 / 导航上行，单 CDP
  监听广播给多面板），前端 `BrowserPanel` 在 `isWebShell` 下改用 canvas 渲染帧——AI 与
  用户操作同一页面。视口用 `Emulation.setDeviceMetricsOverride` 固定 1280×900（headless
  下 `--window-size` 不保证生效）。`tabs` 工具照常；拾取 / 截图 / 取帧 / 录制 / DevTools
  仍明确报错。
- 插件 / 附件上传 / office 预览缓存等主进程 TS 胶水未移植（Phase B，Web UI 需要时；
  插件入口在 web 下已隐藏）。
- **外链路由**：桌面上 `window.open` / `target=_blank` 由主进程转成内置浏览器新页签，web 壳
  没有主进程 → `web-bridge` 拦下 `window.open`，由当前浏览器面板**就地导航**（面板页签共享
  同一台会话浏览器）；被浏览页面内的 `_blank` / `window.open` 由服务端注入脚本改为本页导航
  （headless 里新开的页签面板看不见，旧行为是"点了像没反应"）。
- **静态资源缓存**：`index.html` 与 assets 一律 `Cache-Control: no-cache`（带 ETag/Last-Modified，
  每次校验 304 极便宜）——否则容器升级后浏览器还跑旧 bundle（实测踩过：面板行为改了但页面是旧逻辑）。
- **MCP 连接器已在 core（Rust）**：`astorm-core/src/mcp` 实现 stdio（子进程常驻，
  换行分隔 JSON-RPC）与 HTTP（JSON-RPC POST，兼容 SSE 响应 + `mcp-session-id`）
  两种传输 + 握手/工具发现/`tools/call`；配置 `<数据根>/config/mcp_servers.json`
  与桌面同格式，启动自动连接 `enabled` 项。连接后的工具**直接进入会话工具集**
  （`AppState::loop_tool_map` 每轮重建，与内置工具重名时暴露 `{服务器}__{工具}`），
  提示词「已连接 MCP 服务」段列出服务器与工具名。镜像内置 node/npm + python3/uv
  供 npx/uvx 系服务器。桌面端同一实现（napi `mcp*` 绑定，主进程只转发命令）。
- Linux 沙箱为 Passthrough（静态预检仍生效：危险命令/黑名单命中即拒）；内核级强制
  （bubblewrap/Landlock）留待后续。
- 真实 LLM 端到端（对话/工具审批全流程）已验证（浏览器内配置供应商后即可对话）；
  本仓 CI/冒烟仍用 env 预置（`ASTORM_LLM_*`，可选路径）。
