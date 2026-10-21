//! interrupt_message：立即发送（打断当前循环，进入打断消息队列优先执行）。

use napi_derive::napi;
use uuid::Uuid;

use crate::session::dto::CreateSessionRequest;

/// 立即发送消息：请求进入打断消息队列并取消当前循环，当前循环收尾后优先消费。
#[napi]
pub fn interrupt_message(session_id: String, request: CreateSessionRequest) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .interrupt_message(&session_id, request.into())
        .map_err(|e| napi::Error::from_reason(e))
}