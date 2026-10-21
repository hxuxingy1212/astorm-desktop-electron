# GitHub Pages 交互预览

> 源码与命令细节见 [`frontend/src/preview/README.md`](../frontend/src/preview/README.md)。

预览站由 `frontend/src/preview` 下的两个 Vite 页面组成，直接复用
`frontend/src/renderer` 的真实组件、样式与图标：

| 路径 | 内容 |
| --- | --- |
| `/` | 极简首页（GitHub 地址 + 下载 + 单屏完整应用窗口） |
| `/docs.html` | 平台能力文档（三种形态、对话/工具/工作区/知识/自动化/模型/安全/部署/平台支持） |
| `/app/` | 独立应用预览，默认 `#/chat/preview-auth` |

首页只保留 GitHub 地址、下载入口与预览窗口；能力介绍集中在 `docs.html`，由顶部「文档」进入。
介绍页用 iframe 挂载应用；`app/preview-bridge.js` 在浏览器里注入同形状的
`window.api`，提供会话、历史、流式回复与文件内容。首屏的会话/历史是占位种子；
场景数据改用本地运行导出的真实会话替换（字段与 `chat.listSessions` /
`chat.listMessages` 一致）。所有数据只在内存中，刷新恢复种子，不连接任何后端。

## 命令

```bash
pnpm run build:preview   # 产物 frontend/preview-dist/（gitignored）
pnpm run dev:preview     # Vite dev server
```

`scripts` 落在 `frontend/package.json`（`build:preview` / `dev:preview`），根目录只是透传。

## 部署

[`.github/workflows/preview-pages.yml`](../.github/workflows/preview-pages.yml) 在推送
`master` / `main` 时构建并发布 `frontend/preview-dist`。工作流会先跑 `sync:office`：
office kit 是 gitignored 构建产物，干净检出时该脚本写入一个占位 viewer，
保证 Vite 能解析 `@astorm/office-viewer`（预览页不涉及 Office 文件）。

1. 仓库 Settings → Pages → Source 选择 **GitHub Actions**；
2. 推送或手动触发 workflow；
3. 地址为 `https://<user>.github.io/<repo>/`。

Vite `base` 为相对路径，fork / 子路径部署无需改配置。页面内 GitHub 链接在 Pages
环境下按当前域名与仓库路径自动推导。
