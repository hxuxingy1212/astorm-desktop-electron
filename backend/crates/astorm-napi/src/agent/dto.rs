//! 智能体（agent）DTO 镜像。

use astorm_common::agent::req::{
    AgentUpdate as CoreAgentUpdate, CreateAgentRequest as CoreCreateAgentRequest,
};
use astorm_common::types::agent::{
    AgentExecMode as CoreAgentExecMode, AgentPermissionMode as CoreAgentPermissionMode,
    AgentRecord as CoreAgentRecord,
};
use napi_derive::napi;
use serde::Serialize;

/// 智能体执行模式：只读 / 读写 / 继承。
#[napi(string_enum = "snake_case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecMode {
    /// 只读：会话恒为 plan 模式（工具层硬拒写）
    ReadOnly,
    /// 读写：会话为 execute 模式
    ReadWrite,
    /// 自动：会话为 auto 模式（自动判断 + 可审批项自动同意）
    Auto,
    /// 继承：不干预（沿用请求 / 父会话 / 缺省）
    Inherit,
}

impl From<AgentExecMode> for CoreAgentExecMode {
    fn from(v: AgentExecMode) -> Self {
        match v {
            AgentExecMode::ReadOnly => CoreAgentExecMode::ReadOnly,
            AgentExecMode::ReadWrite => CoreAgentExecMode::ReadWrite,
            AgentExecMode::Auto => CoreAgentExecMode::Auto,
            AgentExecMode::Inherit => CoreAgentExecMode::Inherit,
        }
    }
}

impl From<CoreAgentExecMode> for AgentExecMode {
    fn from(v: CoreAgentExecMode) -> Self {
        match v {
            CoreAgentExecMode::ReadOnly => AgentExecMode::ReadOnly,
            CoreAgentExecMode::ReadWrite => AgentExecMode::ReadWrite,
            CoreAgentExecMode::Auto => AgentExecMode::Auto,
            CoreAgentExecMode::Inherit => AgentExecMode::Inherit,
        }
    }
}

/// 智能体权限模式：手动 / 自动 / 继承。
#[napi(string_enum = "snake_case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissionMode {
    /// 手动授权（弹审批卡）
    Manual,
    /// 自动授权（可审批项自动同意）
    Auto,
    /// 继承：不干预
    Inherit,
}

impl From<AgentPermissionMode> for CoreAgentPermissionMode {
    fn from(v: AgentPermissionMode) -> Self {
        match v {
            AgentPermissionMode::Manual => CoreAgentPermissionMode::Manual,
            AgentPermissionMode::Auto => CoreAgentPermissionMode::Auto,
            AgentPermissionMode::Inherit => CoreAgentPermissionMode::Inherit,
        }
    }
}

impl From<CoreAgentPermissionMode> for AgentPermissionMode {
    fn from(v: CoreAgentPermissionMode) -> Self {
        match v {
            CoreAgentPermissionMode::Manual => AgentPermissionMode::Manual,
            CoreAgentPermissionMode::Auto => AgentPermissionMode::Auto,
            CoreAgentPermissionMode::Inherit => AgentPermissionMode::Inherit,
        }
    }
}

/// 智能体卡片（列表 / 详情 / 创建 / 更新结果）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// 智能体 id（自定义 = uuid；内置 = builtin-main）
    pub agent_id: String,
    /// 名称
    pub name: String,
    /// 描述（供模型判断该委派给谁）
    pub description: String,
    /// 额外的用户指引（非空则拼进系统提示词之后）
    pub guidance: String,
    /// 工具集（空 = 无工具可用）
    pub tool_ids: Vec<String>,
    /// 技能集（空 = 环境提示词不列技能；不影响 skills 工具加载）
    pub skill_ids: Vec<String>,
    /// 执行模式：只读 / 读写 / 继承
    pub mode: AgentExecMode,
    /// 权限模式：手动 / 自动 / 继承
    pub permission: AgentPermissionMode,
    /// 是否内置（内置不可编辑 / 删除）
    pub builtin: bool,
    /// 创建时间（unix 秒）
    pub created_at: i64,
    /// 更新时间（unix 秒）
    pub updated_at: i64,
}

impl From<CoreAgentRecord> for AgentCard {
    fn from(v: CoreAgentRecord) -> Self {
        AgentCard {
            agent_id: v.agent_id,
            name: v.name,
            description: v.description,
            guidance: v.guidance,
            tool_ids: v.tool_ids,
            skill_ids: v.skill_ids,
            mode: v.mode.into(),
            permission: v.permission.into(),
            builtin: v.builtin,
            created_at: v.created_at,
            updated_at: v.updated_at,
        }
    }
}

/// 新建智能体请求。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    /// 名称（必填，不可与已有智能体重名）
    pub name: String,
    /// 描述（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 额外的用户指引（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    /// 工具集（可选；空 = 无工具）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_ids: Option<Vec<String>>,
    /// 技能集（可选；空 = 不列技能）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_ids: Option<Vec<String>>,
    /// 执行模式（可选；缺省 inherit = 不干预）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentExecMode>,
    /// 权限模式（可选；缺省 inherit = 不干预）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<AgentPermissionMode>,
}

impl From<CreateAgentRequest> for CoreCreateAgentRequest {
    fn from(v: CreateAgentRequest) -> Self {
        CoreCreateAgentRequest {
            name: v.name,
            description: v.description.unwrap_or_default(),
            guidance: v.guidance.unwrap_or_default(),
            tool_ids: v.tool_ids.unwrap_or_default(),
            skill_ids: v.skill_ids.unwrap_or_default(),
            mode: v.mode.map(Into::into).unwrap_or_default(),
            permission: v.permission.map(Into::into).unwrap_or_default(),
        }
    }
}

/// 更新智能体（缺省字段保持原值；给出空数组 = 清空该集合）。
#[napi(object)]
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdate {
    /// 名称（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 描述（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 额外的用户指引（可选；空串 = 清空指引）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    /// 工具集（可选；给出即整体替换）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_ids: Option<Vec<String>>,
    /// 技能集（可选；给出即整体替换）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_ids: Option<Vec<String>>,
    /// 执行模式（可选；给出即替换，`inherit` = 恢复不干预）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentExecMode>,
    /// 权限模式（可选；给出即替换，`inherit` = 恢复不干预）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<AgentPermissionMode>,
}

impl From<AgentUpdate> for CoreAgentUpdate {
    fn from(v: AgentUpdate) -> Self {
        CoreAgentUpdate {
            name: v.name,
            description: v.description,
            guidance: v.guidance,
            tool_ids: v.tool_ids,
            skill_ids: v.skill_ids,
            mode: v.mode.map(Into::into),
            permission: v.permission.map(Into::into),
        }
    }
}
