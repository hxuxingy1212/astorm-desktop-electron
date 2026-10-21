//! update_agent：更新智能体字段。

use astorm_common::Agent;
use napi_derive::napi;

use crate::agent::dto::{AgentCard, AgentUpdate};

/// 更新智能体（缺省字段保持原值；内置卡片拒绝）。
#[napi]
pub fn update_agent(agent_id: String, updates: AgentUpdate) -> napi::Result<AgentCard> {
    crate::app()?
        .agents()
        .update_agent(agent_id, updates.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
