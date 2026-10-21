//! # astorm-sandbox
//!
//! Rust 跨平台（Windows + macOS + Linux）Shell 执行安全沙箱。
//!
//! 核心能力：
//! - 文件读写防护（黑白名单，优先级：黑名单 > 白名单，都不命中默认拒绝）
//! - 网络进站/出站防护（黑白名单，优先级：黑名单 > 白名单，都不命中默认拒绝）
//! - 危险命令防护（黑名单，命中拒绝，未命中允许）
//! - 防护开关：开启正常检测；关闭直接执行不检测
//! - 内置运行时（Python / Node / Git）随分发打包，用户无需安装
//! - 同步（即时结果）与异步（事件日志流）两种执行方式
//!
//! 架构：可移植核心（策略/预检/执行编排）不接触平台 API，平台差异收敛在
//! [`sandbox`] 抽象层之后（Windows：AppContainer + DACL + 调试器动态命令拦截；
//! macOS：Seatbelt + 进程监视驱逐；Linux：暂无内核强制层，Passthrough 直通 +
//! 静态预检，能力如实上报，隔离依赖容器/宿主边界）。平台强制层会如实上报
//! [`sandbox::SandboxCapabilities`]，绝不静默降级。

pub mod analyzer;
pub mod cli;
pub mod cmdline;
pub mod decision;
pub mod enforcement;
pub mod policy;
pub mod roots;
pub mod runner;
pub mod runtime;
pub mod sandbox;
pub mod toolchain;

/// 库版本号，CLI 与报告共用。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Windows 下子进程不创建控制台窗口（GUI 宿主进程拉起控制台程序时会闪黑窗）。
/// 沙箱自身用 CreateProcessW（见 sandbox/windows），此处供 std Command 路径共用。
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;