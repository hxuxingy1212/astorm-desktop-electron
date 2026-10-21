//! agent：ReAct 循环 + 会话（业务核心，被 `crate::app` 注入依赖）。
//!
//! 会话由 [`Session`] 直接驱动，模块按职责拆分：
//!
//! - [`session`]：`Session` / `SessionImpl` 结构与门面实现
//! - `run`：消息队列 drain 循环（[`Session::run`]）
//! - `context`：循环上下文构建（`build_loop_context`）
//! - `dynamic_tools`：动态工具加载（`tool_load`；LLM 可用集计算 / 加载态变更纯逻辑）
//! - `prompt`：系统提示词（纯静态 / 前缀缓存友好）+ 会话环境提示词
//! - `history`：历史一致性与消息落盘（tool 配对修复 / `MessageRecord` 转换）
//! - `interrupt`：中断运行时编排（决议解析 / 自动重放被拦工具 / 表单回答写回 / 崩溃恢复）
//! - `reopen`：重开提问（被忽略 / 被取代的 `chat_with_user` 重新登记为待决中断）
//! - `attachment`：用户消息附件（图片复制进会话工作区 / 多模态 user 消息装配 / 冷启动重建）
//! - `hook`：`ReactLoop` hook 编排（薄壳：提示词 / 配对 / 状态持久化 / 摘要）
//! - [`compact`]：会话消息剪支（摘要前对超长工具参数/结果截断归档）
//! - [`summary`]：会话摘要能力（阈值压缩上下文）
//! - [`goal`]：目标循环（`/goal` 外循环：单次判定决定是否继续推进）
//! - [`title`]：会话标题生成（基于首条用户消息提取，创建会话时异步触发）

pub mod session;
pub mod summary;
pub mod title;

mod attachment;
pub(crate) mod dynamic_tools;
mod goal;
mod interrupt;
mod reopen;
mod compact;
mod context;
mod history;
mod hook;
mod prompt;
mod run;