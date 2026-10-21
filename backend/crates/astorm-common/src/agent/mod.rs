//! 智能体接口契约：门面 trait + 请求定义（纯 serde，不依赖 napi）。
//!
//! 智能体卡片（[`AgentRecord`]）由业务层（`astorm-core` 的 `AgentRegistry`）持久化管理，
//! napi 绑定层经 `astorm-core` 的 `App` 门面调用，不直接依赖本 trait 的实现类型。

pub mod req;

pub use req::{AgentUpdate, CreateAgentRequest};

use crate::types::agent::AgentRecord;

/// 智能体管理门面：智能体卡片的增删改查。
///
/// 内置卡片（见 [`crate::types::agent::BUILTIN_MAIN_AGENT_ID`]）不可编辑 / 删除。
pub trait Agent {
    /// 列出全部智能体（内置卡片置前）。
    fn list_agents(&self) -> Result<Vec<AgentRecord>, String>;
    /// 读取单个智能体（不存在返回 None）。
    fn get_agent(&self, agent_id: String) -> Result<Option<AgentRecord>, String>;
    /// 新建智能体（名称必填且唯一；返回落盘后的卡片）。
    fn create_agent(&self, request: CreateAgentRequest) -> Result<AgentRecord, String>;
    /// 更新智能体字段（缺省字段保持原值；内置卡片拒绝）。
    fn update_agent(&self, agent_id: String, updates: AgentUpdate) -> Result<AgentRecord, String>;
    /// 删除智能体（内置卡片拒绝）。
    fn remove_agent(&self, agent_id: String) -> Result<(), String>;
}
