//! 智能体请求体：新建 / 更新（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

use crate::types::agent::{AgentExecMode, AgentPermissionMode};

/// 新建智能体的请求体。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    /// 名称（必填，会话内唯一）。
    pub name: String,
    /// 描述（供模型判断该委派给谁）。
    #[serde(default)]
    pub description: String,
    /// 额外的用户指引（非空则拼进系统提示词之后）。
    #[serde(default)]
    pub guidance: String,
    /// 工具集（空 = 无工具可用）。
    #[serde(default)]
    pub tool_ids: Vec<String>,
    /// 技能集（空 = 环境提示词不列任何技能；不影响 `skills` 工具加载）。
    #[serde(default)]
    pub skill_ids: Vec<String>,
    /// 执行模式：只读 / 读写 / 继承（缺省继承 = 不干预）。
    #[serde(default)]
    pub mode: AgentExecMode,
    /// 权限模式：手动 / 自动 / 继承（缺省继承 = 不干预）。
    #[serde(default)]
    pub permission: AgentPermissionMode,
}

/// 更新智能体字段（**缺省 = 保持原值**，不回落默认值；全缺省视为无操作）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdate {
    /// 名称（空串视为未给出）。
    #[serde(default)]
    pub name: Option<String>,
    /// 描述。
    #[serde(default)]
    pub description: Option<String>,
    /// 额外的用户指引（空串视为「清空指引」）。
    #[serde(default)]
    pub guidance: Option<String>,
    /// 工具集（给出即整体替换；空数组 = 清空 = 无工具）。
    #[serde(default)]
    pub tool_ids: Option<Vec<String>>,
    /// 技能集（给出即整体替换；空数组 = 清空）。
    #[serde(default)]
    pub skill_ids: Option<Vec<String>>,
    /// 执行模式（给出即替换；`inherit` = 恢复为不干预）。
    #[serde(default)]
    pub mode: Option<AgentExecMode>,
    /// 权限模式（给出即替换；`inherit` = 恢复为不干预）。
    #[serde(default)]
    pub permission: Option<AgentPermissionMode>,
}

impl AgentUpdate {
    /// 是否一个字段都没给出（调用方可据此跳过落盘）。
    pub fn is_empty(&self) -> bool {
        self.name.as_deref().map(str::trim).unwrap_or("").is_empty()
            && self.description.is_none()
            && self.guidance.is_none()
            && self.tool_ids.is_none()
            && self.skill_ids.is_none()
            && self.mode.is_none()
            && self.permission.is_none()
    }
}
