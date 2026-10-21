# Astorm Agent 前端

Astorm Agent 的 Electron 桌面壳与 Vue 3 渲染层。完整的项目介绍、技术架构、构建打包与数据目录说明见根 [README](../README.md)。

## 技术栈

| 用途 | 技术 |
| --- | --- |
| 桌面壳 | Electron + electron-vite |
| UI | Vue 3 + Arco Design Vue |
| 样式 | UnoCSS + CSS 变量（主题令牌） |
| 状态 | Pinia |
| 国际化 | vue-i18n（zh-CN / en-US） |
| 编辑器 / 预览 | CodeMirror、vue-stream-markdown、docx/xlsx/pptx/pdf 预览组件 |

## 开发

```bash
# 在仓库根目录安装依赖（会联动安装本目录依赖）
pnpm install

# 推荐：从仓库根启动，自动编译并同步 Rust 后端 .node
pnpm run dev

# 只在本目录开发（要求 ../../frontend/resources/astorm-backend.node 已存在）
pnpm dev
pnpm typecheck
pnpm build
```

`pnpm build` 只构建前端到 `out/`；正式打包请从仓库根执行 `pnpm run dist:mac` / `dist:win` / `dist:linux`，它会先构建后端并校验分发物料。

## 目录

```text
src/
├── main/        Electron 主进程：窗口、IPC、Provider 配置、插件协议、浏览器桥、napi 生命周期
├── preload/     contextBridge → window.api（本机 / 云端两套）
└── renderer/src/
    ├── views/          会话、知识库、邮件、定时任务、任务板等页面
    ├── components/     对话、页签、设置、Markdown、侧栏等组件
    ├── panels/         文件预览、浏览器、终端、记忆、代码审查等面板
    ├── stores/         Pinia 状态
    ├── onboarding/     首次启动引导
    ├── i18n/           中英文案
    ├── styles/         主题令牌与全局样式
    └── preview/        GitHub Pages 预览站（独立构建入口）
```

## 相关文档

- [UI 风格规范](../docs/ui-style.md)（改 UI 前必读）
- [前后端契约](../docs/backend-contract.md)
- [预览站说明](../docs/preview-site.md)
- [文档索引](../docs/README.md)
