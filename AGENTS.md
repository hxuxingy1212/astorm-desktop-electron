# AGENTS.md

Monorepo: **Electron + Vue 3 frontend** (`frontend/`) + **Rust backend** (`backend/`, cargo workspace). JSON-config contract between the two lives in `docs/` (see `docs/README.md`).

## Commands (run from repo root)

- `pnpm run dev` — if `frontend/resources/astorm-backend.node` is missing, auto-compiles backend + syncs dll, then `electron-vite dev` (hot reload).
- `pnpm run build` — `scripts/sync-backend.mjs` (cargo build --release `-p astorm-napi` + copy dll) → `scripts/sync-office.mjs` (astorm-office 套件) → `electron-vite build`.
- `pnpm run build:backend` — only compile backend + sync dll (`backend/target/release/astorm_napi.dll` → `frontend/resources/astorm-backend.node`).
- `pnpm run sync:office` — sync the astorm-office kit (five CLIs: unified `astorm-office` + four format CLIs, builtin office skill, preview viewer). Priority: local `../astorm-office` repo (or `ASTORM_OFFICE_ROOT` / `ASTORM_OFFICE_KIT`) → GitHub Release download for the current platform (version pinned in `scripts/external-artifacts.json`) → placeholder viewer. Missing artifacts degrade gracefully instead of failing the build. Details: `docs/office-integration.md`, `docs/external-artifacts.md`.
- `pnpm run sync:agent-browser` — fetch the agent-browser CLI: local `AGENT_BROWSER_SRC` / `AGENT_BROWSER_ROOT` → GitHub Release prebuilt binary for the current platform (version + sha256 pinned in `scripts/external-artifacts.json`) → NOT-SYNCED placeholder.
- `pnpm run verify:dist` — distribution preflight: hard-fails unless the office kit (5 CLIs + skill tree + non-stub viewer), `agent-browser/<os>-<arch>`, the napi binding, and `frontend/out/renderer/index.html` are all present; `--target` rejects cross-platform packaging. `pnpm run dist:mac` / `dist:win` / `dist:linux` run full build → this check → electron-builder.
- `pnpm run build:frontend` — `pnpm -C frontend build`.
- `pnpm run build:preview` — GitHub Pages 预览站：介绍页 + 真实渲染层 iframe + 浏览器 mock 桥（`frontend/src/preview`），产物 `frontend/preview-dist/`（gitignored）；`pnpm run dev:preview` 本地调试。部署走 `.github/workflows/preview-pages.yml`，细节见 `docs/preview-site.md`。
- `pnpm run typecheck` — frontend only; runs both `tsc` (node) and `vue-tsc` (web). Two-step, not a single tsc.
- `pnpm run test:e2e:onboarding` — 启动引导 e2e：真跑一遍 Electron（欢迎 → 选择连接方式 → 偏好，本机与云端两分支 + 负例）。隔离实例（`ASTORM_DATA_DIR` 临时目录 + `ASTORM_CDP_PORT`）驱动 CDP 真实点击，云端分支用本地桩服务端；**跑的是构建产物**，改了渲染层要先 `pnpm -C frontend build`。退出码 = 失败断言数，脚本在 `frontend/scripts/e2e/`。
- `pnpm run test:e2e:fileref` — 正文文件引用 e2e：真跑一轮对话（桩 LLM 按 SSE 回带 `` `auth.py:47` `` 的正文）→ 断言引用 chip → 点击开文件页签并定位到行。隔离实例 + 临时**用户目录**（见 `seedLocalApp`：不设 `backend-settings.json` 的 workspace 会写进真实 `~/astorm`），**跑的是构建产物**。脚本 `frontend/scripts/e2e/fileref.mjs`。
- `pnpm run test:backend` — 后端 e2e（默认全 `#[ignore]`，需要 `backend/.env` 中的真实 LLM 凭据）。脚本使用 `--manifest-path backend/Cargo.toml`，可直接在仓库根目录运行。

## Backend (Rust)

- Cargo workspace, **edition 2024**. Crates: `astorm-common`, `astorm-core` (logic), `astorm-napi` (the dll/bindings), `astorm-storage`, `llm-provider`, `react-loop`, plus `tests`.
- The backend is a **napi-rs dll that Electron requires as a `.node` file**. The dll is a gitignored build artifact; it must be synced by `scripts/sync-backend.mjs` before the frontend can use it. Renaming `.dll` → `.node` is required by `createRequire`.
- `frontend/src/main/napi.ts` loads the binding; if missing it silently falls back to mock (`napiCall` throws with `BACKEND_UNAVAILABLE` prefix). If a feature appears to do nothing, the dll likely isn't synced.
- napi-rs exports Rust snake_case function names as **camelCase**, and many request/DTO fields are snake_case. Keep this mapping in mind when adding bindings.
- Backend e2e tests (in `astorm-tests`) are all `#[ignore]`d and require real LLM credentials from `backend/.env` (`LLM_BASE_URL` / `LLM_MODEL` / `LLM_API_KEY`). That file is gitignored — never commit real keys. Missing key → test panics with a hint. Run with `--ignored --nocapture`.

## Frontend (Electron + Vue 3)

- package name is `astorm-agent-electron` (not `astorm-desktop`).
- electron-vite; alias `@` → `src/renderer/src`. Renderer uses Arco Design, UnoCSS.
- `.npmrc` pins Electron + electron-builder binaries to npmmirror (China) mirrors — unexpected network slowdowns for Electron downloads may be this.
- 正文里的**引用**有两类，判定与渲染都只有一处实现：**文件引用**（`` `auth.py:47` `` → 可点击 chip，点了开文件页签并定位到行）在 `utils/fileRefs.ts`；**网页引用角标**（`[1](url)` → 编号角标，点了在应用内浏览器打开来源，不弹外链确认框）在 `components/markdown/LinkRenderer.vue`（`WebRefBadge`）。markdown 节点渲染器共用 `components/markdown/renderers.ts`（新增 markdown 调用方一律引用它，别各写一份）。文件引用的相对路径按注入的基准目录解析：对话正文 = 会话工作区（ChatPanel provide），文件预览 = 该文件所在目录（FilePreviewPanel provide）。**模型侧书写约定**在后端系统提示（`astorm-core/src/agent/prompt.rs`「行为准则」最后两条：文件写反引号包裹的 `路径:行号`、网页写 `[n](完整URL)` 角标）——改渲染器判定规则时同步核对该两句（两个单测守住，e2e 断言这两句随请求下发）。

## UI 规范（渲染层，强制）

**任何渲染层 UI 改动（新增页面/组件、调样式、动主题变量）开工前先读 [`docs/ui-style.md`](docs/ui-style.md)**——那是视觉的唯一权威来源，第 4 章给出每个现有组件/页面对应的章节与「参考图锚定 / 推导待确认」状态。

- **令牌唯一**：颜色、圆角、阴影、字号、动效一律走 `styles/themes/*.css` + `theme/tokens.css` 里的 CSS 变量；禁止在组件里新增硬编码色值（`docs/ui-style.md` 第 7 章列了现存债务，触及时顺手收敛）。
- **面分两类**（最易判错）：可交互容器用"抬升面"（浅色＝白底+描边 / 深色＝亮填充），只放内容的"井"（代码块、命令块、面包屑）用"凹陷面"（浅色也是灰填充、无描边）。
- **复用优先，别在页面里重复写组件（`docs/ui-style.md` §1.7）**：写任何 UI 小件前先查 §1.7 的索引表（"我要做 X → 用什么"）；**共享层没有的先建共享件、再在页面里用**，不要"先在页面里写、等第二个页面再抽"。同一个视觉形态在第二个页面出现就是抽出来的信号。改完自查有没有把"本该复用"的东西又写了一遍（计划里的 `check:reuse` 棘轮脚本**已随本轮回滚移除**，规则仍在本段与 `docs/ui-dedup-plan.md`）。规范未落地但已定型的件在 §1.7 里标 ★，去向见 `docs/ui-dedup-plan.md`。
- 图标是受控目录，不许随手选：界面图标一律从手写 UI 图标集里挑（`#/icons` 总览页按使用场景分了 10 类、带"什么时候用"与「常见选错」对照；分类的单一来源是 `utils/uiIconCatalog.ts`）。新增字形四步：放 SVG 进 `assets/ui-icons/` → 在 `uiIconCatalog.ts` 登记分类与提示（漏了会加载期抛错）→ 在 `uiIconGallery.ts` 的 `NAME_TO_GLYPH` 加注册名 → 同步 `docs/ui-style.md` §1.6。`i-lucide-*` 类与 lucide/vscode-icons 是**遗留/已停用**，禁止新增。
- **对话正文 / Markdown 有独立专章（第 4 章）**：`MarkdownBody.vue` 与 `components/markdown/*` 的任何改动先读它——极简渲染（不加容器、不画线、颜色收敛到三类）、代码块三色配色且**与文件预览的完整语法主题是两套，不要合并**、正文代码块无行号（行号只在文件预览）。
- **七条硬规则**（同文档第 0 章）：一个强调色（发送键例外，它是反色单色）；**深色与浅色是两套"面"的语言**（深色＝亮填充无描边，浅色＝白底 + 1px 描边，统一走 `--surface-*` 令牌）；输入/选择类控件带描边、动作类按钮纯填充；浮层才用材质卡（0.5px 发丝 + 毛玻璃 + 圆角 14px）；用户消息有气泡助手没有；新增样式必须在浅色与深色下都成立；动效 120–220ms 且降级 `prefers-reduced-motion`。
- **形态冻结项**：审批卡（`ApprovalBar.vue`）、中断表单（`UserInputForm.vue` / `ChatFormBar.vue` / `MailComposeBar.vue`）、过程展开态（`ChatTurnGroup.vue` / `ChatStepList.vue` / `ToolCallList.vue`）的结构**保持现有 UI，不要重画**，只对齐颜色/字体/字号/圆角/间距。
- Arco 组件已被 `styles/inputs.css`（全应用）/ `styles/settings.css`（设置页）/ `styles/global.css`（全局压平）/ `components/common/mac-modal.css`（弹窗骨架）统一改写——**新控件要么复用这套，要么把覆盖加进对应文件，别在组件 scoped 样式里另起一套**。新弹窗一律带 `modal-class="mac-modal"`。
- 引导页（`onboarding/`）有独立的既定视觉约定（无卡片/无清单），不受卡片规范约束，见 `docs/cloud-server.md`。
- 改完按 `docs/ui-style.md` 第 8 章自查清单过一遍；整体观感变化要出改动前后截图对照（仓内 e2e 只守行为，不守视觉）。

## Runtime data / config (not in repo)

- App runtime data lives in `%APPDATA%\astorm` (sqlite / jsonl / indexes).
- LLM provider config persists in Electron `userData/providers.json`, apiKey encrypted via `safeStorage` (fallback `plaintext`). Contract in `docs/backend-contract.md`; providers IPC in `frontend/src/main/providers.ts`.

## Plugin system

- `plugins/` holds only distributable net packages (one dir per plugin, e.g. `demo-plugin`, `ppt-inspector`) plus the shared SDK `plugins/sdk/astorm-sdk.js`. Spec in `docs/plugin-system.md`.
- Buildable plugin sources used to live in the sibling `plugins_dev/<name>/` (a Vite project) whose `pnpm build` output went into `plugins/<name>/`. That directory was removed on 2026-10-09 along with `ppt-inspector`; if a plugin source tree comes back, keep it **outside** the shipped tree in `plugins_dev/<name>/` and have it build into `plugins/<name>/` — only `plugins/` is packaged.
