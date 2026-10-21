//! 智能体门面：把 [`crate::agent_registry::AgentRegistry`] 暴露为 [`astorm_common::Agent`]。

use astorm_common::agent::req::{AgentUpdate, CreateAgentRequest};
use astorm_common::agent::Agent;
use astorm_common::types::agent::AgentRecord;

use crate::app::App;

/// 智能体门面（零大小类型，实际状态在 `AppState.agents`）。
pub struct AgentImpl;

impl Agent for AgentImpl {
    fn list_agents(&self) -> Result<Vec<AgentRecord>, String> {
        App::global().agents.list()
    }

    fn get_agent(&self, agent_id: String) -> Result<Option<AgentRecord>, String> {
        App::global().agents.get(&agent_id)
    }

    fn create_agent(&self, request: CreateAgentRequest) -> Result<AgentRecord, String> {
        App::global().agents.create(request)
    }

    fn update_agent(&self, agent_id: String, updates: AgentUpdate) -> Result<AgentRecord, String> {
        App::global().agents.update(&agent_id, updates)
    }

    fn remove_agent(&self, agent_id: String) -> Result<(), String> {
        App::global().agents.remove(&agent_id)
    }
}
