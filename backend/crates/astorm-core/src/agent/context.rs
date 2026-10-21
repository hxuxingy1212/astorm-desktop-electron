//! 循环上下文构建：由会话记录 + 用户请求组装完备初始化的 [`LoopContext`]。
//!
//! 会话状态续接（轮次 / 累计用量）、metadata 平铺（camelCase 前端契约）、
//! 请求配置（模型 / 思考强度）均在此完成。

use std::sync::Arc;

use astorm_common::session::req::CreateSessionRequest;
use llm_provider::{ChatMessage, ChatRequest};
use react_loop::LoopContext;
use serde_json::json;

use crate::agent::session::Session;

/// 由会话 + 本条请求构建完整初始化的循环上下文（历史 / 模型 / 思考强度 +
/// 会话状态续接 + 全量元数据）。
///
/// `round` / `total_usage` / `context_tokens` 从会话记录续接（循环内由 after_llm 回写）；
/// `metadata` 平铺 camelCase 会话信息（与前端 serde 契约一致）。工具事件由引擎经
/// `emit_event` hook 直接转发，无需额外事件通道。
pub(crate) fn build_loop_context(
    session: &Arc<Session>,
    request: &CreateSessionRequest,
    history: Vec<ChatMessage>,
) -> Option<LoopContext> {
    let record = session.record.lock().ok()?.clone();
    let cancel = session.cancel.read().ok()?.clone();

    let mut ctx = LoopContext::new(record.id.to_string(), cancel);
    // 注入循环可用工具：**初始 LLM 可用集** = 可见工具集剔除「按需加载」集
    // （schedule_task / mail / browser 初始不下发，模型经 tool_load 加载后由 loop_start
    //  每轮重建生效，见 agent::dynamic_tools）+ 已连接的 MCP 服务器工具。空列表 = 无工具。
    // 应用未初始化（测试等）时保持空工具集。
    if let Some(app) = crate::app::App::try_global() {
        ctx.tools = app.loop_tool_map(&record.tool_list, &record.loaded_tool_list);
    }
    // 会话级状态续接：轮次 / 累计用量 / 上一轮真实上下文 token
    // （round 为累计值，供会话信息展示；本轮上限由引擎的 turn_round 单独计时）
    ctx.round = record.current_round.max(0) as u32;
    ctx.total_usage = record.session_usage.clone();
    ctx.last_prompt_tokens = record.context_tokens;
    // 全量可序列化会话信息进 metadata（camelCase 键）
    ctx.set_meta("title", json!(record.title));
    ctx.set_meta("pid", json!(record.pid));
    ctx.set_meta("workspace", json!(record.workspace));
    ctx.set_meta("sessionMode", json!(record.session_mode));
    ctx.set_meta("permissionMode", json!(record.permission_mode));
    ctx.set_meta("skillList", json!(record.skill_list));
    ctx.set_meta("toolList", json!(record.tool_list));
    ctx.set_meta("mcpList", json!(record.mcp_list));
    ctx.set_meta("summaryTimes", json!(record.summary_times));
    ctx.set_meta("questionTimes", json!(record.question_times));
    ctx.set_meta("currentRound", json!(record.current_round));

    // 请求配置：历史 + 模型；思考强度请求优先，缺省回退会话记录。
    // 多供应商路由：请求 `provider` → 客户端（None 回落默认供应商，见 LlmRegistry）；
    // 模型引用 id → API 标识（name），未匹配原样透传。
    let client = session.llm.client_for(request.provider.as_deref());
    let api_model = session.llm.resolve_model_name(request.provider.as_deref(), &request.model);
    let mut chat_request = ChatRequest::new(history).with_model(&api_model);
    if let Some(effort) = request
        .reasoning_effort
        .or(Some(record.reasoning_effort))
        .and_then(|e| e.as_str())
    {
        chat_request = chat_request.with_reasoning_effort(effort);
    }
    ctx.chat_request = chat_request;
    // 本循环 LLM 客户端（引擎按 ctx.client 路由，None 回落引擎默认客户端）
    ctx.client = client;

    Some(ctx)
}