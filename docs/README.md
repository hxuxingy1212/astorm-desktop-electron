# 文档索引

统一管理前后端文档。

## 前后端契约

- [backend-api.md](./backend-api.md) — 后端 API 契约(napi/mock DTO、工作区/会话接口、映射到渲染层文件)
- [backend-contract.md](./backend-contract.md) — Rust napi 后端的构建/加载契约、配置字段、命名映射
- [plugin-system.md](./plugin-system.md) — 插件系统(清单/sdk/消息协议/安装/设置/安全;含内置 Demo 说明)

## 后端内部

- [api/react-loop.md](./api/react-loop.md) — ReactLoop 引擎 API 参考(上下文、hook、工具、事件协议)
- [cloud-server.md](./cloud-server.md) — 云端无头服务(astorm-server/astorm-cli:Docker 部署、
  SSE 事件流、Phase A 边界与 Phase B 规划)
- [deployment.md](./deployment.md) — **单用户云容器部署指南**(快速开始/参数配置三层/
  数据持久化/网络安全/排查)
- [cli-manual.md](./cli-manual.md) — **astorm CLI 用户手册**(命令详解/审批流/脚本自动化/
  疑难排查)
- [web-vs-desktop.md](./web-vs-desktop.md) — **Web 端(单用户云容器)与桌面端功能对照**
  (同一内核两种宿主、一致项/降级项/粗糙点/补齐候选)

## 功能设计(设计稿)

- [browser-agent.md](./browser-agent.md) — 内置浏览器 AI 代理操控(agent-browser + iframe 作用域 CDP 代理):
  实现形式选型、工具封装与动作映射、分级授权与人工干预、实例生命周期、实测清单
- [memory.md](./memory.md) — 长期记忆(OKF v0.2 编译管道):三层架构与不变量、Bundle 布局、分类体系、
  阈值与去噪、触发/增量、命令与 CLI、agent 集成(`memory` 工具 + 每轮注入)与测试方式

## 前端

- [ui-style.md](./ui-style.md) — **UI 风格规范(渲染层唯一视觉权威)**:设计语言与七条硬规则、令牌层
  (字体/色板/形状/间距/动效/**图标受控目录**/**共享组件与复用规则**)、组件级规范(外壳/对话/浮层/表单/数据展示/页面)、
  会话正文 Markdown 专章、覆盖矩阵、待补参考清单、迁移债务与自查清单。**改 UI 前必读**(AGENTS.md 已挂强制引用)
- [ui-dedup-plan.md](./ui-dedup-plan.md) — **前端复用修整方案(去重)**:2026-10-15 全量审计的重复实现清单
  (UI 层 + 逻辑层,含证据 file:line)、八项最值得做的、P0–P3 分批实施、明确不做清单、验收与防回流机制
  （原 `check:reuse` 棘轮脚本已随回滚移除；规则见 AGENTS.md 与本文档）
- [preview-site.md](./preview-site.md) — **GitHub Pages 交互预览**:复用渲染层 + mock 桥的构建方式、
  本地命令、mock 数据位置与 Pages 部署步骤

## 构建与分发

- [external-artifacts.md](./external-artifacts.md) — **随包外部产物**:office-kit 的按平台发布流程、
  agent-browser 预编译二进制、版本钉住、下载/校验/离线镜像与故障排查
- [office-integration.md](./office-integration.md) — office 套件集成(五 CLI/技能树/viewer 的消费链路)
- [platform-compat-todo.md](./platform-compat-todo.md) — 跨平台打包待办

## 开发流程速查

见根 [README](../README.md)。