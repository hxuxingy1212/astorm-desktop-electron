//! 定时任务请求体（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

use crate::session::types::{PermissionMode, SessionMode};

/// 定时任务请求体（新增 / 更新共用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskRequest {
    /// 任务 id（更新时必填；新增时可选，缺省生成）
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
    /// 执行指令
    #[serde(default)]
    pub instruction: String,
    /// 会话目录（触发时创建的会话 workspace）
    #[serde(default)]
    pub workspace_path: String,

    /// 会话执行模式（规划 / 执行 / 自动）；缺省 = 沿用任务已存值（增量语义）
    #[serde(default)]
    pub session_mode: Option<SessionMode>,
    /// 会话审批模式（手动 / 自动）
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// 执行智能体 id
    #[serde(default)]
    pub agent_type: Option<String>,
    // 模型 / 供应商 / 思考强度不在此列：定时会话一律用设置里的默认模型与默认思考强度
}
