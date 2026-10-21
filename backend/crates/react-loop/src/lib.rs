//! react-loop：业务无关的 ReAct 循环基础层。
//!
//! 引擎只依赖 [`LoopContext`]（共享循环状态）与 [`ReactLoop`]（业务 hook trait）
//! 驱动循环；数据存储、提示词加载、工具注册、事件路由等业务能力由调用方
//! 实现 trait 注入，本 crate 不触碰任何业务对象。
//!
//! 模块划分：
//! - [`types`]：`LoopEvent` 工具事件通道
//! - [`event`]：前端协议事件 JSON 构造
//! - [`tool`]：`Tool` trait / `ToolContext` / 工具定义助手
//! - [`engine`]：`LoopContext` / `LlmResult` / `ReactEngine`
//! - [`react_loop`]：业务 hook trait
//!
//! # 核心用法
//!
//! 1. 实现 [`Tool`]（业务工具），经 [`ToolMap`] 注册进上下文
//! 2. 实现 [`ReactLoop`]（业务 hook：提示词注入 / 消息持久化 / 事件路由）
//! 3. 装配 [`ReactEngine::new`]（LLM 客户端 + hook），填充 [`LoopContext`]
//!    （初始消息 / 模型名 / 工具映射 / 取消令牌）
//! 4. `engine.run(&mut ctx).await` 驱动多轮推理 + 工具执行循环
//!
//! 完整案例见 `docs/API文档/react-loop.md` 与 `tests/tests/e2e/react_loop.rs`。

mod loop_guard;
pub mod engine;
pub mod event;
pub mod tool;
pub mod types;

pub mod react_loop;

pub use engine::{LoopContext, LlmResult, ReactEngine};
pub use react_loop::ReactLoop;
pub use tool::{Tool, ToolContext, ToolMap};
pub use types::LoopEvent;
