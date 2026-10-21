//! get_agent：读取单个智能体。

use astorm_common::Agent;
use napi_derive::napi;

use crate::agent::dto::AgentCard;

/// 读取单个智能体（不存在返回 null）。
#[napi]
pub fn get_agent(agent_id: String) -> napi::Result<Option<AgentCard>> {
    crate::app()?
        .agents()
        .get_agent(agent_id)
        .map(|card| card.map(Into::into))
        .map_err(|e| napi::Error::from_reason(e))
}
