//! rewind_to_user_message：撤回历史到指定用户消息之前。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

/// 撤回会话历史：保留前 `keep_user_messages` 条用户消息，删除第 N 条及其后全部消息
/// （内存与磁盘同步截断），并立即中断当前循环。
///
/// `keep_user_messages` 从 0 开始计数（0 = 撤回第一条用户消息）。
/// 不得跨会话摘要：目标用户消息在最新摘要记录之前时返回错误。
#[napi]
pub fn rewind_to_user_message(session_id: String, keep_user_messages: u32) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .rewind_to_user_message(&session_id, keep_user_messages)
        .map_err(|e| napi::Error::from_reason(e))
}
