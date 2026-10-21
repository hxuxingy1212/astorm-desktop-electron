//! get_system_prompt：读取会话系统提示词（按当前配置实时拼装，不落盘）。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

/// 读取会话系统提示词（静态人设 + AGENTS.md 约定 + 智能体指引）;供「会话轨迹」展示。
#[napi]
pub fn get_system_prompt(session_id: String) -> napi::Result<String> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .system_prompt(&session_id)
        .map_err(|e| napi::Error::from_reason(e))
}
