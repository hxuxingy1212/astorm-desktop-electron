//! remove_agent：删除智能体。

use astorm_common::Agent;
use napi_derive::napi;

/// 删除智能体（内置卡片拒绝；已用它的会话按创建时快照继续运行）。
#[napi]
pub fn remove_agent(agent_id: String) -> napi::Result<()> {
    crate::app()?
        .agents()
        .remove_agent(agent_id)
        .map_err(|e| napi::Error::from_reason(e))
}
