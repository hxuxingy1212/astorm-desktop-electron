# GitHub Pages 交互预览

预览站直接复用 `frontend/src/renderer` 的真实组件、样式与图标，不是另写一套静态页。
浏览器里没有 Electron preload，所以 `app/preview-bridge.js` 注入了一份同形状的
`window.api`：会话列表、历史消息、流式事件与文件读取都来自内存里的 mock 数据。

## 本地构建 / 预览

```bash
# 从仓库根目录
pnpm run build:preview     # pnpm -C frontend build:preview
pnpm run dev:preview       # Vite dev server（默认 5173，仅本机）
```

产物在 `frontend/preview-dist/`（已 gitignore），可直接用任意静态服务器打开：

```bash
cd frontend/preview-dist && python3 -m http.server 4187
```

## 两个页面

| 路径 | 内容 |
| --- | --- |
| `/` | 极简首页：GitHub 地址、下载入口、单屏完整应用窗口 |
| `/docs.html` | 平台能力文档：三种形态、对话与任务、工具执行、工作区预览、知识与交付、自动化、模型扩展、安全审计、数据部署、平台支持 |
| `/app/` | 独立的应用预览；默认打开 `#/chat/preview-auth` |

首页顶部只保留「文档」入口、GitHub 地址、下载按钮与明暗切换；能力介绍全部收进 `docs.html`。
介绍页通过 iframe 挂载 `/app/`，窗口内以 **macOS 桌面形态**渲染：红黄绿直接落在应用侧栏顶部
（应用自身为系统按钮预留 76px），会话标题与工具栏在同一行，不再额外画一条浏览器式标题栏。
窗口上方的图标按钮负责应用明暗、重置与全屏；`/app/` 单独打开时退回浏览器形态（侧栏显示 logo + 名称）。

## 数据来源

- `frontend/src/preview/app/preview-bridge.js` 里的 `sessions` / `histories` 是**首屏占位种子**。
- 场景数据不再手写 mock：本地启动应用跑完场景后，从 runtime 数据里抽取真实会话替换这两个结构
  （会话 + `message.jsonl`，字段与 `chat.listSessions` / `chat.listMessages` 一致）。
- 其他 mock：`answerFor()` / `streamReply()` 只负责「新发一条消息」的流式演示；
  `FILE_CONTENTS` 供正文文件引用点击后的文件页签。
- 所有数据只在内存中；刷新页面恢复种子。
- 预览窗口上方右侧的「重置」会清掉预览产生的 localStorage 并重载 iframe。

## 交互

- 侧栏切换会话、新建对话
- 「已完成，用时 Xs」展开工具过程（思考 / 命令 / 结果）
- 正文里的文件引用 chip 打开真实文件预览页签
- 在输入框发送消息，观察流式回复与停止按钮
- 介绍页右上角切换明暗；窗口内的太阳/月亮只切换应用主题
- 「全屏」在新标签页打开应用预览

## 部署到 GitHub Pages

仓库自带 [`.github/workflows/preview-pages.yml`](../../../.github/workflows/preview-pages.yml)：

工作流先跑 `pnpm run sync:office`。office kit 是 gitignored 的构建产物，干净检出时该脚本
会写一个占位 viewer，让 Vite 仍能解析 `@astorm/office-viewer`（预览页不涉及 Office 文件）。

1. 推到 `master` 或 `main`（或手动 `workflow_dispatch`）；
2. 仓库 Settings → Pages → Source 选择 **GitHub Actions**；
3. 部署完成后地址为 `https://<user>.github.io/<repo>/`。

Vite 的 `base` 是相对路径（`./`），因此 fork 到任意子路径都能直接工作。
页面里的 GitHub 链接会按当前 Pages 域名与仓库路径自动推导；本地开发时回退到
源码仓库地址。
