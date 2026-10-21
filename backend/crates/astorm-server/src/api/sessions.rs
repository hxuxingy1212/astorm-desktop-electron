//! 会话端点：创建 / 收发 / 审批 / fork / rewind / 摘要 / 事件流 / 交付物。
//!
//! 全部是对 core 门面的薄封装：请求/响应直接复用 core 的 serde 类型
//! （`CreateSessionRequest` / `SessionSettings` / `SessionRecord` / `MessageRecord` /
//! `InterruptResolution`，均为 camelCase），本层不做 DTO 镜像。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use astorm_common::session::interrupt::InterruptResolution;
use astorm_common::session::{CreateSessionRequest, Session as _, SessionSettings};

use crate::error::{ok, ApiError};
use crate::events::sse_stream;
use crate::local;
use crate::state::ServerState;

/// 解析路径里的会话 id。
fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|e| ApiError::bad_request(format!("session_id 非法: {e}")))
}

/// `POST /api/v1/sessions`：创建会话（`message` 非空 + `autoRun` 缺省 = 立即起首轮循环）。
///
/// 便利语义：`model` 为空串时填服务端默认模型引用 id（CLI/脚本最小请求体即可用）。
pub async fn create(
    State(state): State<ServerState>,
    Json(mut req): Json<CreateSessionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.model.trim().is_empty() {
        // 默认模型：providers.json（Web 设置页写的真源）第一供应商第一模型优先，
        // 否则回退启动配置的 default_model/defaultProvider（env 预置）
        req.model = local::effective_default_model(&state.local, state.app.config())
            .ok_or_else(|| ApiError::bad_request("model 不能为空（服务端也未配置默认模型）"))?;
    }
    let id = state
        .app
        .session()
        .create_session(req)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "id": id.to_string() })))
}

/// `GET /api/v1/sessions`：全部会话记录。
pub async fn list(State(state): State<ServerState>) -> Result<Json<serde_json::Value>, ApiError> {
    let records = state.app.session().list_sessions().map_err(ApiError::from)?;
    Ok(ok(serde_json::to_value(records).unwrap_or_default()))
}

/// `GET /api/v1/sessions/{id}`：单条会话记录。
pub async fn get_one(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let record = state
        .app
        .session()
        .list_sessions()
        .map_err(ApiError::from)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::not_found(format!("会话 {id} 不存在")))?;
    Ok(ok(serde_json::to_value(record).unwrap_or_default()))
}

/// `DELETE /api/v1/sessions/{id}`：删除会话（同时清理事件缓冲）。
pub async fn delete(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state.app.session().delete_session(&id).map_err(ApiError::from)?;
    state.hub.forget_session(&id.to_string());
    Ok(ok(json!({ "deleted": true })))
}

/// `POST /api/v1/sessions/{id}/messages`：追加用户消息并驱动循环（正忙时排队）。
pub async fn send_message(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .send_message(&id, req)
        .map_err(ApiError::from)?;
    Ok((StatusCode::ACCEPTED, ok(json!({ "queued": true }))))
}

/// `GET /api/v1/sessions/{id}/messages`：会话历史（jsonl，按 seq 升序）。
pub async fn messages(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let records = state
        .app
        .session()
        .list_messages(&id)
        .map_err(ApiError::from)?;
    Ok(ok(serde_json::to_value(records).unwrap_or_default()))
}

/// 归档分页查询参数。
#[derive(Debug, Deserialize)]
pub struct ArchivedQuery {
    /// 分界 seq（读该值之前的归档段）
    pub before_seq: i64,
    /// 最多回读的用户轮次（缺省 20）
    #[serde(default = "default_max_user_turns")]
    pub max_user_turns: u32,
}
fn default_max_user_turns() -> u32 {
    20
}

/// `GET /api/v1/sessions/{id}/messages/archived`：「加载更早」的归档分页。
pub async fn archived(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivedQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let records = state
        .app
        .session()
        .archived_messages_before(&id, q.before_seq, q.max_user_turns)
        .map_err(ApiError::from)?;
    Ok(ok(serde_json::to_value(records).unwrap_or_default()))
}

/// `POST /api/v1/sessions/{id}/start`：以已有历史启动一轮（「仅创建」会话的启动按钮）。
pub async fn start(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state.app.session().start_session(&id).map_err(ApiError::from)?;
    Ok(ok(json!({ "started": true })))
}

/// `POST /api/v1/sessions/{id}/cancel`：取消当前循环。
pub async fn cancel(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .cancel_session(&id)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "cancelled": true })))
}

/// `POST /api/v1/sessions/{id}/interrupt`：打断并优先执行新请求。
pub async fn interrupt(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .interrupt_message(&id, req)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "queued": true })))
}

/// `POST /api/v1/sessions/{id}/interrupts/resolve`：提交中断决议（审批同意/拒绝、表单回答/忽略）。
///
/// 决议即被中断 tool 消息的结果，入队即校验并取走台账待决项；随后循环续跑。
pub async fn resolve_interrupt(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Json(resolution): Json<InterruptResolution>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .resolve_interrupt(&id, resolution)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "resolved": true })))
}

/// `POST /api/v1/sessions/{id}/interrupts/{tool_call_id}/reopen`：重开被忽略的提问表单。
pub async fn reopen_interrupt(
    State(state): State<ServerState>,
    Path((id, tool_call_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .reopen_form_interrupt(&id, &tool_call_id)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "reopened": true })))
}

/// fork 请求体。
#[derive(Debug, Deserialize)]
pub struct ForkBody {
    /// 在无工具助手消息中 fork 在其之前的下标（0-based；缺省 = 最后一条）
    pub before_final: Option<u32>,
}

/// `POST /api/v1/sessions/{id}/fork`：基于已有会话 fork 新会话（共用工作区）。
pub async fn fork(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    body: Option<Json<ForkBody>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let before_final = body.and_then(|Json(b)| b.before_final);
    let new_id = state
        .app
        .session()
        .fork_session(&id, before_final)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "id": new_id.to_string() })))
}

/// rewind 请求体。
#[derive(Debug, Deserialize)]
pub struct RewindBody {
    /// 保留前 N 条用户消息（0-based 计数；删除第 N 条及其后全部）
    pub keep_user_messages: u32,
}

/// `POST /api/v1/sessions/{id}/rewind`：撤回历史（立即中断当前循环）。
pub async fn rewind(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Json(body): Json<RewindBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .rewind_to_user_message(&id, body.keep_user_messages)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "rewound": true })))
}

/// `POST /api/v1/sessions/{id}/summarize`：手动触发摘要（/compact）。返回是否完成。
pub async fn summarize(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let done = state
        .app
        .session()
        .summarize_session(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "summarized": done })))
}

/// `PATCH /api/v1/sessions/{id}/settings`：会话级设置（缺省字段保持原值）。
pub async fn update_settings(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Json(settings): Json<SessionSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    state
        .app
        .session()
        .update_settings(&id, settings)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "updated": true })))
}

/// `GET /api/v1/sessions/{id}/system-prompt`：会话系统提示词（按需重建，只读）。
pub async fn system_prompt(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let prompt = state
        .app
        .session()
        .system_prompt(&id)
        .map_err(ApiError::from)?;
    Ok(ok(json!({ "prompt": prompt })))
}

/// SSE 查询参数。
#[derive(Debug, Deserialize)]
pub struct SseQuery {
    /// 起始 seq（ exclusive；SSE `Last-Event-ID` 断线续传时把最后收到的 id 传回）
    pub after: Option<u64>,
}

/// `GET /api/v1/sessions/{id}/events`：单会话事件流（SSE；先回放缓冲再续播直播）。
pub async fn sse_session(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Query(q): Query<SseQuery>,
) -> Result<crate::events::SseStream, ApiError> {
    let id = parse_id(&id)?;
    let sid = id.to_string();
    let live = state.hub.subscribe_session(&sid);
    let backlog = state.hub.replay_session(&sid);
    Ok(sse_stream(
        live,
        backlog,
        q.after.unwrap_or(0),
        state.shutdown.clone(),
    ))
}

/// `GET /api/v1/events`：全局事件流（SSE；所有会话 + llm + 系统事件）。
pub async fn sse_global(
    State(state): State<ServerState>,
    Query(q): Query<SseQuery>,
) -> crate::events::SseStream {
    let live = state.hub.subscribe_global();
    let backlog = state.hub.replay_global();
    sse_stream(
        live,
        backlog,
        q.after.unwrap_or(0),
        state.shutdown.clone(),
    )
}

/// `GET /api/v1/sessions/{id}/deliverables`：会话交付物行。
pub async fn session_deliverables(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    parse_id(&id)?;
    let rows = state
        .app
        .list_deliverables(&id)
        .map_err(ApiError::from)?;
    Ok(ok(serde_json::to_value(rows).unwrap_or_default()))
}

/// `GET /api/v1/deliverables?limit=`：跨会话交付物（按更新时间倒序）。
#[derive(Debug, Deserialize)]
pub struct DeliverablesQuery {
    #[serde(default = "default_deliverable_limit")]
    pub limit: i64,
}
fn default_deliverable_limit() -> i64 {
    500
}

pub async fn all_deliverables(
    State(state): State<ServerState>,
    Query(q): Query<DeliverablesQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows = state
        .app
        .list_all_deliverables(q.limit)
        .map_err(ApiError::from)?;
    Ok(ok(serde_json::to_value(rows).unwrap_or_default()))
}
