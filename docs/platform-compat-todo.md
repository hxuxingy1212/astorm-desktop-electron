# 平台兼容性 — 待办与无法在本环境验证的事项

> 背景：2026-10 的跨平台兼容性全量排查与修复（提交 `1cea7c9` → `c7a1a5b`）在
> Windows 11 + Node 24 + pnpm 10.33 环境完成，`cargo test --workspace` 与前端
> typecheck / build / atomic-write 全绿。本文记录**无法在当前 Windows 开发机
> 上验证**的事项与**有意未修**的低优先级项，供后续跟进。

## 一、需要目标平台真机验证

| 项 | 平台 | 现状 | 验证方式 |
| --- | --- | --- | --- |
| macOS ad-hoc 签名 + entitlements 打包 | macOS | 配置已加（`identity: "-"`、`hardenedRuntime`、`entitlements.mac.plist`），未实际出包 | `pnpm -C frontend run build:mac`，验证安装启动、麦克风/摄像头权限弹窗、Gatekeeper |
| deb/rpm 打包 | Linux | 元数据已补（author email / homepage），未实际出包 | `pnpm -C frontend run build:linux`（需 Linux 构建机，rpm 目标另需 rpmbuild） |
| electronLanguages 裁剪（Win/Linux pak / macOS lproj） | 三平台 | 构建期行为按 app-builder-lib 源码推断，未逐平台出包核对 | 各平台打包后检查 `locales/` 与 `.lproj` 残留是否只剩中英 |
| page-render 助手打包分支（argv 识别 → `createRequire` 直载脚本） | 安装包 | 逻辑已在 bundle 核对，未做安装包烟测 | 安装版触发 websearch/webfetch，确认 Rust 侧收到 `ready` |
| NSIS 安装包（electronLanguages 修复、artifactName 带 arch） | Windows | 未实际出包安装 | `pnpm -C frontend run build:win` 后安装，验证中文系统菜单/右键 |
| macOS GUI 启动 PATH 注入（登录 shell 探测） | macOS | 代码就绪；Finder 启动场景无法在 Windows 验证 | mac 真机从 Finder 启动，设置页确认 git/node 可见 |
| Seatbelt cwd 限定工作区（不再下发宿主 cwd） | macOS | 已改为 session workspace，需 mac 真机跑沙箱用例 | `cargo test -p astorm-sandbox --test sandbox_macos_tests` + 手动 bash 矩阵 |
| 容器非 root（USER astorm）+ 中文字体 + office skill 全量 | Linux 容器 | Dockerfile 已改，未跑 gap e2e | Linux/Docker 环境 `bash scripts/e2e/run_gap_e2e.sh` |
| Linux 全量测试（fsx/ACL/沙箱分支的 Unix 行为） | Linux | `cargo test --workspace` 未在 Linux 跑过 | Linux 机全量测试 |
| gap e2e 的 ws 回退路径（Node <22 → frontend ws 依赖） | Node <22 | 本机 Node 24 走内置 WebSocket；回退分支只做了路径/依赖静态核对 | Node 20 环境跑 `run_gap_e2e.sh` 的 ⑤b/①c 段 |

## 二、有意未修（低优先级 / 影响面小）

| 项 | 审计编号 | 说明 |
| --- | --- | --- |
| Windows 长路径在全部 `std::fs` 写点的覆盖 | P2-5 | 已在 CreateProcess cwd/程序路径与原生 ACL 边界处理；工具层 `std::fs` 依赖 Rust std 的 verbatim 处理，极端深路径未实测 |
| WOW64 ARM64 子进程 PEB 偏移 | P2-6 | x86/x64 已按 `IsWow64Process2` 动态选择；ARM64 布局未实现，回退宿主偏移（宿主为 ARM64 时不受影响） |
| 次要运行时产物的原子化（summary/compact/office 预览缓存/审计追加） | P2-17 部分 | 配置类写点已全部原子化；运行时产物截断影响小 |
| `astorm-cli` Windows 控制台 UTF-8（chcp 65001） | P3 | 输出中文可能乱码，功能不受影响 |
| macOS 沙箱降级时 `protected=true` 上报口径 | P2-9 | 降级原因已在 Note 事件说明，字段口径待统一 |
| 非管理员下防火墙能力字段仍报 `EvictionOnly` 语义 | P2-8 部分 | Note 已明确降级说明；结构化能力字段口径待统一 |

## 三、验证清单（拿到对应环境后按序执行）

1. **macOS 真机**：`cargo test --workspace` → `build:mac` 出包 → 安装/启动/权限弹窗 → Finder 启动验证 PATH → bash 矩阵手测。
2. **Linux 真机**：`cargo test --workspace` → `build:linux` 出包 → AppImage/deb 安装。
3. **Docker**：`docker build` → `bash scripts/e2e/run_gap_e2e.sh`（覆盖非 root、中文字体、office skill 全量、WS 探针、弹窗导航）。
4. **Windows 安装包**：`build:win` 出包 → 安装 → 中文系统菜单、websearch 渲染助手、`pnpm -C frontend build:win` 的 prebuild sync 链路。
