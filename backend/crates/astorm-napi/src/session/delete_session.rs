//! delete_session：删除会话。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

/// 删除会话：取消其循环并移除注册表（同时清理 `tabs` 工具的页签镜像）。
#[napi]
pub fn delete_session(session_id: String) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    astorm_core::tool::tabs::forget_state(&session_id.to_string());
    crate::app()?
        .session()
        .delete_session(&session_id)
        .map_err(|e| napi::Error::from_reason(e))
}
