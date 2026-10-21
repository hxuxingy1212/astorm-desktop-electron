//! list_agents：列出全部智能体（内置置前）。

use astorm_common::Agent;
use napi_derive::napi;

use crate::agent::dto::AgentCard;

/// 列出全部智能体。
#[napi]
pub fn list_agents() -> napi::Result<Vec<AgentCard>> {
    crate::app()?
        .agents()
        .list_agents()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
