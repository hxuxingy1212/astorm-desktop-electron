//! Web 桥事件 WebSocket：`GET /api/v1/ws?token=&after=`。
//!
//! 浏览器无法给 WebSocket 设 Authorization 头，令牌走查询串；`after` 为
//! 事件中枢 seq 游标（断线重连补齐缺口，与 SSE 同一语义）。
//! 服务端只下行不接收（除协议层 ping/pong 由 axum 自动处理）：
//! 每条消息 `{"t":"event","event":{...原始事件 JSON...}}`，与桌面 `napi:event`
//! 的载荷完全一致（渲染层 onEvent 回调无感）。

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::state::ServerState;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
    #[serde(default)]
    pub after: u64,
}

/// `GET /api/v1/ws`：升级为事件推送 WebSocket。
pub async fn events_ws(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<ServerState>,
) -> Response {
    // 手动鉴权（WS 无法走 axum 中间件层的 Bearer 提取——令牌在查询串）
    if let Some(expected) = &state.token {
        let ok = q.token.as_deref().map(|t| auth::token_eq(t, expected)).unwrap_or(false);
        if !ok {
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    }
    ws.on_upgrade(move |socket| pump(socket, state, q.after))
}

async fn pump(socket: WebSocket, state: ServerState, after: u64) {
    let (mut sender, mut receiver) = socket.split();
    let mut live = state.hub.subscribe_global();
    let backlog = state.hub.replay_global();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        // 回放起点即更新 last：订阅与快照之间存在竞窗（订阅后进直播缓冲的事件
        // 也在快照里），不更新会导致该窗口内事件重发两次（SSE 侧已正确处理）
        let mut last = after;
        // 1) 回放（seq > after）
        for (seq, ev) in backlog {
            if seq > last {
                last = seq;
                let msg = Message::Text(json!({ "t": "event", "seq": seq, "event": ev }).to_string().into());
                if tx.send(msg).await.is_err() {
                    return;
                }
            }
        }
        // 2) 直播（跳过 seq <= last 的重复项；哨兵帧之后服务端只下行）
        loop {
            match live.recv().await {
                Ok((seq, ev)) => {
                    if seq > last {
                        last = seq;
                        let msg = Message::Text(
                            json!({ "t": "event", "seq": seq, "event": ev }).to_string().into(),
                        );
                        if tx.send(msg).await.is_err() {
                            return;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });

    // 转发泵 + 关闭侦测（客户端断开时收掉泵任务）
    let mut shutdown = state.shutdown.clone();
    loop {
        tokio::select! {
            item = rx.recv() => {
                match item {
                    Some(msg) => {
                        if sender.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // 客户端上行忽略（协议层 ping/pong 由 axum 处理）
                    Some(Err(_)) => break,
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BrowserWsQuery {
    pub token: Option<String>,
    pub session_id: String,
}

/// `GET /api/v1/browser/ws?token=&session_id=`：面板画面流（帧下行 + 输入上行）。
///
/// `session_id` 用**根会话 id**（子会话与主会话共享同一个浏览器，同 browser 工具口径）。
pub async fn browser_ws(
    ws: WebSocketUpgrade,
    Query(q): Query<BrowserWsQuery>,
    State(state): State<ServerState>,
) -> Response {
    if let Some(expected) = &state.token {
        let ok = q.token.as_deref().map(|t| auth::token_eq(t, expected)).unwrap_or(false);
        if !ok {
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    }
    if q.session_id.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "session_id required").into_response();
    }
    let hub = state.browser.clone();
    ws.on_upgrade(move |socket| crate::browser::screencast::serve(hub, q.session_id, socket))
}
