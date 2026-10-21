//! send_message：追加用户消息并驱动循环（会话正忙时排队）。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

use crate::session::dto::CreateSessionRequest;

/// 发送消息：完整请求入队（含前端对话框设置），触发 ReAct 循环。
#[napi]
pub fn send_message(session_id: String, request: CreateSessionRequest) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .send_message(&session_id, request.into())
        .map_err(|e| napi::Error::from_reason(e))
}
