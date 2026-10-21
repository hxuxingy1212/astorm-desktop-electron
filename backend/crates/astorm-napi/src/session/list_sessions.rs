//! list_sessions：列出全部会话记录。

use astorm_common::session::Session;
use napi_derive::napi;

use crate::session::dto::SessionRecord;

/// 列出全部会话记录（供前端列表渲染）。
#[napi]
pub fn list_sessions() -> napi::Result<Vec<SessionRecord>> {
    crate::app()?
        .session()
        .list_sessions()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
