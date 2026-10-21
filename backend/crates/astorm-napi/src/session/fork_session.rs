//! fork_session：基于已有会话创建一个共用工作区的新会话。

use napi_derive::napi;
use uuid::Uuid;

/// 基于 `session_id` fork 出新会话，返回新会话 id。
///
/// 新会话共用源会话工作区；继承源会话「最后一个无工具调用的助手消息」之前的消息
/// （含 system / 最近一次摘要记录作上下文前缀）。`before_final` 指定在无工具助手消息中
/// fork 在其之前的下标（0-based，供逐条 fork）；缺省 = 最后一条。
#[napi]
pub fn fork_session(session_id: String, before_final: Option<u32>) -> napi::Result<String> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .fork_session(&session_id, before_final)
        .map(|id| id.to_string())
        .map_err(|e| napi::Error::from_reason(e))
}
