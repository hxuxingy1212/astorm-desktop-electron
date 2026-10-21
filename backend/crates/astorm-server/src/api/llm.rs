//! `POST /api/v1/llm/ask`：单次 LLM 补全（SSE 流式）。
//!
//! 复用 core 的 `spawn_llm_chat`：以服务端生成的 `call_id` 把 `type:"llm"` 事件
//! 发布到全局总线，本端点订阅中枢全局流、按 `call_id` 过滤，遇到 `end` 帧收尾。
//! 事件契约见 `astorm-core/src/app/ask_llm.rs`（chunk / reasoning / end / error）。

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::Event;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::ReceiverStream;

use astorm_core::app::AskLlmRequest;
use llm_provider::ChatMessage;

use crate::error::ApiError;
use crate::events::SseStream;
use crate::state::ServerState;

/// 单条对话消息。
#[derive(Debug, Deserialize)]
pub struct ChatMessageBody {
    /// `system` / `user` / `assistant`
    pub role: String,
    pub content: String,
}

/// ask 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskBody {
    /// 模型引用 id（空 = 默认模型）
    #[serde(default)]
    pub model: String,
    /// 供应商名（缺省 = 默认供应商）
    #[serde(default)]
    pub provider: Option<String>,
    pub messages: Vec<ChatMessageBody>,
    /// 思考强度（low / medium / high / max）
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// `POST /api/v1/llm/ask`：SSE 流式返回（`event` = kind，`data` = 总线事件）。
pub async fn ask(
    State(state): State<ServerState>,
    Json(body): Json<AskBody>,
) -> Result<SseStream, ApiError> {
    if body.messages.is_empty() {
        return Err(ApiError::bad_request("messages 不能为空"));
    }
    let messages: Vec<ChatMessage> = body
        .messages
        .into_iter()
        .map(|m| ChatMessage {
            role: m.role,
            content: Some(m.content),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            metadata: None,
        })
        .collect();
    let call_id = uuid::Uuid::new_v4().simple().to_string();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    // 先订阅全局流再 spawn：上游秒败时 error/end 事件可能先于订阅发布，
    // 后订阅会漏掉 end → SSE 永不终止（审查项）
    let mut live = state.hub.subscribe_global();
    state
        .app
        .spawn_llm_chat(
            AskLlmRequest {
                model: body.model,
                provider: body.provider,
                messages,
                reasoning_effort: body.reasoning_effort,
            },
            call_id.clone(),
        )
        .map_err(ApiError::from)?;

    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default()
                .event("start")
                .data(json!({ "callId": call_id }).to_string())))
            .await;
        loop {
            match live.recv().await {
                Ok((_, ev)) => {
                    if ev.get("call_id").and_then(|v| v.as_str()) != Some(call_id.as_str()) {
                        continue;
                    }
                    let kind = ev.get("kind").and_then(|v| v.as_str()).unwrap_or("event");
                    let done = matches!(kind, "end");
                    if tx.send(Ok(Event::default().event(kind).data(ev.to_string()))).await.is_err() {
                        return; // 客户端断开
                    }
                    if done {
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
    Ok(axum::response::Sse::new(ReceiverStream::new(rx))
        .keep_alive(axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping")))
}
