//! 业务 hook trait：循环生命周期感知点。
//!
//! 由业务方实现。循环状态与数据经 [`LoopContext`] / [`LlmResult`] 传递，
//! 引擎只依赖本 trait 与注入的 LLM 客户端，不触碰任何业务对象。

use async_trait::async_trait;
use llm_provider::ChatMessage;
use serde_json::Value;

use crate::engine::{LlmResult, LoopContext};

#[async_trait]
pub trait ReactLoop: Send + Sync {
    // ── 生命周期 hook（按执行阶段排序） ──────────────────────────────

    /// loop 循环执行前（整个循环前调用一次：注入 system prompt、初始化元数据）。
    async fn before_loop(&self, ctx: &mut LoopContext) -> Result<(), String>;

    /// loop 内每轮启动阶段（每轮 LLM 推理前调用，业务在此做剪支 / 摘要等准备；
    /// 每条用户消息的每一轮都会触发，长任务中途也能压缩上下文）。
    async fn loop_start(&self, ctx: &mut LoopContext);

    /// 每次 LLM 调用后（每轮 LLM 推理完成后调用）。
    async fn after_llm(&self, ctx: &mut LoopContext, result: &LlmResult);

    /// loop 内每轮收尾阶段（每轮工具执行完成后、进入下一轮前调用；
    /// 因无工具调用 / 中断 / 取消而结束的轮次不会触发）。
    fn loop_end(&self, ctx: &mut LoopContext);

    /// loop 循环执行后（整个循环后调用一次：持久化用量、清理）。
    async fn after_loop(&self, ctx: &mut LoopContext) -> Result<(), String>;

    // ── 公共方法（循环任意阶段可调用） ───────────────────────────────

    /// 持久化追加一条消息（业务落库；引擎产生 assistant / tool 消息时调用，
    /// 消息元数据随消息保留，但不会上送 LLM）。
    fn append_message(&self, ctx: &LoopContext, message: ChatMessage);

    /// 推送事件（业务：协议事件路由到前端）。
    fn emit_event(&self, ctx: &LoopContext, event: Value);
}
