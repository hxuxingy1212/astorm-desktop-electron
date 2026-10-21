//! cancel_session：取消会话当前循环。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

/// 取消会话：cancel 当前循环（会话本身保留）。
#[napi]
pub fn cancel_session(session_id: String) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .cancel_session(&session_id)
        .map_err(|e| napi::Error::from_reason(e))
}
