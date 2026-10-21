//! summarize_session：手动触发一次会话摘要（/compact）。

use napi_derive::napi;
use uuid::Uuid;

/// 手动触发一次会话摘要（`/compact`，绕过阈值）。返回是否完成了摘要。
///
/// 会话正在生成时返回错误（请稍后再试）。摘要会把会话历史重写为
/// 「系统提示词 + 摘要消息 + 保留尾部」，旧活动日志留作备份段，并递增 `summaryTimes`。
#[napi]
pub async fn summarize_session(session_id: String) -> napi::Result<bool> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .summarize_session(&session_id)
        .await
        .map_err(|e| napi::Error::from_reason(e))
}
