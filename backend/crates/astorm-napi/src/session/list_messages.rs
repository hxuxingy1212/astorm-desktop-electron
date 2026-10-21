//! list_messages：读取会话历史消息（jsonl,按 seq 排序）。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

use crate::session::dto::MessageRecord;

/// 读取会话历史消息（供前端打开会话时回灌;按 seq 升序）。
#[napi]
pub fn list_messages(session_id: String) -> napi::Result<Vec<MessageRecord>> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .list_messages(&session_id)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}