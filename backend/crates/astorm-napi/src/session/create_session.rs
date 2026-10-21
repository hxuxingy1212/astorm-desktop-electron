//! create_session：创建会话接口（委托业务门面）。

use astorm_common::session::Session;
use napi_derive::napi;

use crate::session::dto::CreateSessionRequest;

/// 创建会话：生成 session_id 并触发首轮 ReAct 循环，返回 session_id。
#[napi]
pub fn create_session(request: CreateSessionRequest) -> napi::Result<String> {
    crate::app()?
        .session()
        .create_session(request.into())
        .map(|id| id.to_string())
        .map_err(|e| napi::Error::from_reason(e))
}
