//! astorm-core：业务门面（应用门面 + ReAct 循环 + 会话 + 工具集 + 技能 / 定时任务）。
//!
//! 数据模型与接口契约已抽离至 [`astorm_common`]（`session` / `types`），本 crate 仅承载运行时；
//! 应用配置（[`app::AppConfig`]）收在 `app` 模块。

pub mod agent;
pub mod agent_registry;
pub mod app;
pub mod browser;
pub mod facade;
pub mod mcp;
pub(crate) mod process;
pub mod scheduler;
pub mod skill;
pub mod tool;
pub mod util;
