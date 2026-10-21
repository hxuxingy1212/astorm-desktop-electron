//! reopen_interrupt：重开一条被忽略（或被取代 / 被中断）的提问表单。
//!
//! 用户曾点「忽略」的 `chat_with_user` 提问，其表单定义仍保留在那条 tool 消息的
//! `metadata.interrupt.request.form` 里；本接口据此换新 `request_id` 重新登记为待决中断，
//! 并发出 `interrupt` 事件 → 前端把**原表单**（空白）弹回输入框位置，用户提交后
//! 回答仍写回同一条 tool 结果并续跑（见 `astorm_core::agent::reopen`）。

use napi_derive::napi;
use uuid::Uuid;

/// 重开提问：按工具调用 id 重新登记待决中断（仅限会话空闲且无其他待决中断时）。
#[napi]
pub fn reopen_interrupt(session_id: String, tool_call_id: String) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .reopen_form_interrupt(&session_id, &tool_call_id)
        .map_err(napi::Error::from_reason)
}
