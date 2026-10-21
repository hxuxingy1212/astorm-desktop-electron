//! 定时任务类型（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

use crate::session::types::{PermissionMode, SessionMode};

/// 定时任务记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    /// 任务 id（SQLite TEXT 主键）
    pub id: Option<String>,
    /// 任务名
    pub name: String,
    /// 描述
    pub description: String,
    /// cron 表达式
    pub cron_expr: String,
    /// 运行方式："new_session" | "continuous_session"
    #[serde(default)]
    pub mode: String,
    /// 执行指令（作为子任务消息）
    #[serde(default)]
    pub instruction: String,
    /// 会话目录（触发时创建的会话 workspace）
    #[serde(default)]
    pub workspace_path: String,
    /// 会话执行模式（规划 / 执行 / 自动）
    #[serde(default)]
    pub session_mode: Option<SessionMode>,
    /// 会话审批模式（手动 / 自动）
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// 执行智能体 id
    #[serde(default)]
    pub agent_type: Option<String>,
    // 模型 / 供应商 / 思考强度不在此列：定时会话一律用设置里的默认模型与默认思考强度
    /// 绑定的会话 id（continuous 模式复用）
    pub session_id: String,
    /// 是否启用（0/1）
    #[serde(default)]
    pub enabled: i64,
    /// 创建时间（unix 秒）
    pub created_at: i64,
}

/// 调度专用工作区生成结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedWorkspace {
    /// 工作区 id（uuid）
    pub id: String,
    /// 工作区路径
    pub path: String,
}

impl From<crate::types::ScheduledTask> for ScheduledTask {
    /// 落盘记录（`task_id` 强类型 id）→ 门面记录（`id` 可选）。
    fn from(v: crate::types::ScheduledTask) -> Self {
        ScheduledTask {
            id: Some(v.task_id),
            name: v.name,
            description: v.description,
            cron_expr: v.cron_expr,
            mode: v.mode,
            instruction: v.instruction,
            workspace_path: v.workspace_path,
            session_mode: v.session_mode,
            permission_mode: v.permission_mode,
            agent_type: v.agent_type,
            session_id: v.session_id,
            enabled: v.enabled,
            created_at: v.created_at,
        }
    }
}
