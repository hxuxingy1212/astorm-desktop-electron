# astorm-office 集成

外部仓库 `astorm-office`（GitHub `hxuxingy1212/astorm-office`；本地开发常见兄弟目录
`../astorm-office`）为 LLM 提供 Word / Excel / PPT / PDF 的生成·解析·精准编辑与互转能力。
本仓库以「内置 skill + 随包 CLI + 预览组件」三种形态消费它，全部产物来自对方的
**office-kit 导出包**（`dist-kit/office-kit-<version>/`，含 `{bin, skill, viewer, samples,
VERSION, SHA256SUMS, _metadata.json}`，由 office 仓库 `node scripts/export.mjs` 产出）。

## 分发链路（sync:office）

`scripts/sync-office.mjs`（`pnpm run sync:office`，并挂在 `pnpm run build` 链上）：

1. kit 来源优先级：`ASTORM_OFFICE_KIT`（现成导出包）> `ASTORM_OFFICE_ROOT`（默认 `../astorm-office`
   / `~/workspace/astorm-office`）下的 `dist-kit/` 最新版本目录 > 现场运行 office 仓库
   `node scripts/export.mjs`（release 产物已存在时 `--skip-build` 复用）> **GitHub Release 下载**
   当前平台的 `office-kit-<version>-<platform>.tar.gz`（版本固定在 `scripts/external-artifacts.json`，
   校验 Release `SHA256SUMS` 与 kit 内部 `SHA256SUMS`）。
2. 拷贝 `{bin, skill, viewer, VERSION, …}` → `frontend/resources/office/`（gitignored 构建产物；
   electron-builder extraResources `office/`），`SHA256SUMS` best-effort 校验。
3. **降级不阻塞**：本地来源与 GitHub Release 都不可用时写抛错占位 viewer，构建照常通过；运行时
   office 工具报「未同步」、预览回退旧渲染器（docx/xlsx/pptx JS 库）或外部打开（pdf）。
4. viewer 目录取真实版本（`_metadata.json`），`VERSION` / `_metadata.json` 可用于判断 kit 新旧。
5. **分发即离线可用**：五个 CLI、技能树与 viewer 全部作为静态文件随安装包分发（`extraResources`
   `office/`），运行时只读本地文件——不执行 `cargo` / `npm install`、不联网下载。CLI 是原生二进制，
   mac / Windows / Linux 安装包须在对应平台各自构建（不能在一个平台交叉打包另一平台的 office 套件）；
   打包后可从 `<Resources>/office/bin/astorm-office formats --json` 验证五个 CLI 均已就位。
6. **打包前置校验**：`pnpm run dist:mac|dist:win|dist:linux` 先跑 `scripts/verify-dist.mjs`
   （office 5 CLI / 技能树 / 非占位 viewer / agent-browser / napi / 前端产物，缺一即硬失败并给修复指引；
   `--target` 还会阻止在 mac 上打 Windows/Linux 包），再交给 electron-builder；只查当前平台产物。
   单独校验用 `pnpm run verify:dist`。

office-kit 的发布方式（按平台构建 + GitHub Release + 版本钉住）见
[`external-artifacts.md`](./external-artifacts.md)。

## 三种形态

### ① LLM 执行层：`office` 工具（Rust 内置）

`backend/crates/astorm-core/src/tool/office.rs`，入 `ToolRegistry::builtin()`（内置主智能体动态
全量可见；General 子智能体卡片已加 `office`）：

- 参数 `{ bin: 'docx'|'xlsx'|'pptx'|'pdf'|'office', args: string[], description?: string }` →
  spawn `<binDir>/json2<bin>`（`bin='office'` 时 spawn 统一入口 `<binDir>/astorm-office`，可跨格式
  `batch`、按扩展名自动分发），**在会话工作区执行**，args 原样透传（CLI 命令契约见 office 仓库
  `docs/cli-conventions.md`）；`description` 是**可选的本次调用目的**（与 `browser` 同款，
  只做展示——对话过程行的说明文字、写操作的审批卡与审计条目里带上，让用户知道为什么改这份文档）；
- 对话过程行展示（`frontend/src/renderer/src/components/chat/toolDisplay.ts` 的 `office` 分支，
  与其他工具同一套派生）：图标/类型名按 `bin` 取格式——Word/Excel/PPT/PDF 四枚**文件树同款
  品牌图标**（`assets/file-icons/`，`ChatStepList` 的 `iconUrl` 渲染为 `<img>`）配「Word 文档 /
  Excel 表格 / PPT 演示 / PDF 文档」；`bin='office'` 会从 args 的格式别名（`word`/`excel`…）
  或文件扩展名推断格式，同样显示对应品牌图标与关键词；跨格式 `batch` 显示「Office 批处理」，
  其余统一入口回落「Office 文档」；摘要 = CLI 子命令行（`view --json … /part[1] text`，与
  bash 行展示命令同口径）；说明 = 模型填的 `description`，**没填则按子命令回落固定说法**
  （`chat.step.officeCmd.*`，如 view→查看文档内容、render→渲染核对、unpack→解包文档），
  保证过程行永远有一句「这次在干什么」；
- 只读命令（`view/validate/schema/help/templates/dump/query/env/formats/qa/check/palette/design/raw`、
  `edit … get`、`meta get`、`form info`，PDF 的 `extract text|table|attachment`，且无 `-o` /
  `--attachments` / `--media` / `--apply` 等写盘标志）直执行；`batch` / `pages` / `code|content sanitize`
  与其余写命令按分级授权（Plan 硬拒 → 已拒短路 → 审批 / 自动放行，与 `browser` 同一 policy），
  审计记 `file_write`；统一入口（`bin='office'`）会跳过格式别名后判定真实子命令；
- `serve` / `mcp`（常驻服务）硬拒，超时 / 用户取消杀子进程，输出截断 8k 字符；
- 退出码 3（产物校验问题）按成功返回并前置提示（问题清单是数据）；1/2 报错（stderr 尾部）；
- 配置经启动 JSON `office{binDir, skillDir, chromeBin, sofficePath, timeoutSec}` 传入
  （`AppConfig.office`，主进程 `resolveOfficeDir()` 解析；`chromeBin/sofficePath` 转发为
  CLI 环境变量 `ASTORM_CHROME_BIN` / `ASTORM_SOFFICE_PATH`）。

### ② LLM 知识层：内置技能 `astorm-office`

办公室 kit 的 `skill/` 树（总纲 SKILL.md + 四格式分册 + references）。启动时
（`astorm-core/src/app/mod.rs::install_builtin_office_skill`）：

1. **复制**技能树进用户技能目录 `<userData>/skills/astorm-office/`（该目录是 bash/file 的
   **只读白名单**——模型读分册 / references 免审批；总纲用相对路径引用分册，同构布局保证可解析）；
2. 以 `source="builtin"`、**真实路径**注册技能；每次启动按 kit 内容刷新（SKILL.md 内容不同则整树
   重拷），用户启停状态保留，用户同名导入（source=user）不覆盖；
3. 索引时把「桌面端运行方式」附录追加到总纲正文（CLI 已预装于 binDir、优先用 office 工具调用、
   建议 `--json`）。内容单源仍是 office 仓库的 SKILL.md，附录由本端注入。

kit 缺一（bin 或 skill）则整块跳过：技能不注入、工具报未同步，二者同时缺位即功能不存在。

### ③ 前端预览层：`@astorm/office-viewer` 组件

- 同步脚本拷 kit `viewer/`（`office-viewer.js` ESM + css + d.ts）到 `resources/office/viewer/`，
  并生成 `package.json`（exports map）；vite alias（`electron.vite.config.ts`）与 tsconfig paths
  （`tsconfig.web.json`）都指向该目录；**echarts 标记 build external**——它是真可选 peer
  （仅 PPT 图表），运行期动态导入失败 → viewer 渲染占位块，不必安装（省 ~1MB）；
- 组件 `components/preview/OfficeViewerRenderer.vue`：`defineAsyncComponent` 惰性加载（独立
  chunk），经 IPC `office:preview-open`（`main/officePreview.ts`）调 `json2<kind> unpack
  <file> --inline -o <tmp>` 拿**单文件自包含 JSON**（分片内联 + 媒体 data URI）直接喂
  `<OfficeViewer :data>`——无自定义协议、无 1+N fetch；缓存按 path+mtime+size 哈希落
  `<userData>/office-preview/`，二次打开免重新解包；
- 入口统一走 `FilePreviewPanel.vue` 的 office 分支（而 kindOf 已把 **pdf** 从二进制黑名单移入
  office）：Modal（交付物/附件/邮件）与页签（文件树/工作台）自动全量生效；克服旧限制
  —— PDF 此前只能外部打开；
- 旧版 `.doc/.xls/.ppt/csv` 与解包失败 → 组件内部回退旧渲染器 `OfficePreview.vue`（pdf 失败回退
  外部打开）。主进程 `pptxCliPath()` 优先解析 kit 的 `json2pptx`（旧 `json2pptx-cli` 兜底，
  ppt-inspector 插件不受影响）。

## 悬浮选择 → 输入框（对标浏览器拾取 elementPick）

预览面板右下角「选择路径」开关（默认关，避免阅读误触）；开启后 viewer 悬浮高亮 + PathCard
（组件自带），点击元素 → `@select(path, info)`：

```
OfficeViewerRenderer onSelect → officePick store push({data, token})
  → ChatInput watcher → MentionInput.syncOfficeChips（内联 chip，```office-ref 围栏存 data-fence）
  → canSend 计入 officePick.count
  → sendPayload.officeRefs → sessions.sendMessage(req.officeRefs)
  → 后端 CreateSessionRequest.office_refs → render_refs_note（```office-ref 围栏按序拼回
    喂模型副本，refsNote 元数据落盘不渲染，冷启动重建同 element 围栏）
  → 发送后 clearRefPicks 清空
```

序列化（`utils/officeRef.ts`）：围栏字段 key **不走 i18n**（`file/kind/path/type/name/text` 是
office CLI 契约键），chip 标签 `name → text 摘要 → 紧凑路径`（如 `part1/para2`）。用户在消息文案
里只可见 chip 标签（正文），结构化路径进元数据，模型拿到后直接 `office` 工具按路径 edit。

## 手测清单（dev）

1. `pnpm run sync:office`（前提：office 仓库存在且已 `export.mjs`，或现场导出）→ `pnpm run dev`；
2. 预览：会话交付物 / 文件树打开 `samples/` 下任一 docx / xlsx / pptx / pdf —— 高保真渲染 +
   工具栏「选择路径」；
3. 悬浮：开启开关后 hover 元素出现路径卡，点击 → 输入框出现 chip（操作数编号 + 名称），胶囊正确
   计数；删除 chip 后 store 对账减少；
4. 发送：带上 chip 发消息 → 模型应能按围栏里的文件与路径调 `office` 工具 edit（Plan 模式会被拒，
   Execute/Auto 走审批卡片）；
5. 降级：`ASTORM_OFFICE_ROOT=/tmp/empty pnpm run sync:office` 后 —— office 工具报「未同步」、
   docx 预览回退旧渲染器、pdf 回退外部打开。
6. 过程行展示：让模型 view 一份 docx（`bin=docx`）——过程行应是「Word 图标 + Word 文档 ·
   `view …` 命令行 · 说明」，xlsx / pptx / pdf 各自换成对应品牌图标与类型名；说明先看模型是否
   填了 `description`，再让它「不要传 description」复跑一次，应回落「查看文档内容」这类
   子命令说法（2026-10-13 容器实测两路都过，见 `docs/cloud-server.md` 的 UI 实测段）。
