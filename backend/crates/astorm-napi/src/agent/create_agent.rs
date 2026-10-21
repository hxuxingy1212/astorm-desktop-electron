//! create_agent：新建智能体。

use astorm_common::Agent;
use napi_derive::napi;

use crate::agent::dto::{AgentCard, CreateAgentRequest};

/// 新建智能体（名称必填且唯一）。
#[napi]
pub fn create_agent(request: CreateAgentRequest) -> napi::Result<AgentCard> {
    crate::app()?
        .agents()
        .create_agent(request.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
