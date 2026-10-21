//! archived_messages_before：归档分页读取（UI「加载更早」逐批回看被摘要压缩的前文）。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

use crate::session::dto::MessageRecord;

/// 最新摘要备份段中 `seq < before_seq` 的消息，取末尾 max_user_turns 个用户轮次；
/// 返回空数组 = 已到归档起点。
#[napi]
pub fn archived_messages_before(
    session_id: String,
    before_seq: i64,
    max_user_turns: u32,
) -> napi::Result<Vec<MessageRecord>> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .archived_messages_before(&session_id, before_seq, max_user_turns)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
