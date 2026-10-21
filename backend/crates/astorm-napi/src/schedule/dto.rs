//! 定时任务 DTO 镜像。

use astorm_common::schedule::{
    GeneratedWorkspace as CoreGeneratedWorkspace, ScheduledTask as CoreScheduledTask,
    ScheduledTaskRequest as CoreScheduledTaskRequest,
};
use napi_derive::napi;
use serde::Serialize;

use crate::session::{PermissionMode, SessionMode};

/// 定时任务记录。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    /// 任务 id
    pub id: Option<String>,
    /// 任务名
    pub name: String,
    /// 描述
    pub description: String,
    /// cron 表达式
    pub cron_expr: String,
    /// 运行方式："new_session"（每次新建会话）| "continuous_session"（复用同一会话）
    pub mode: String,
    /// 执行指令
    pub instruction: String,
    /// 会话目录（触发时创建的会话 workspace）
    pub workspace_path: String,
    /// 会话执行模式（规划 / 执行 / 自动；缺省 = 全局默认，且智能体卡片可覆盖）
    pub session_mode: Option<SessionMode>,
    /// 会话审批模式（手动 / 自动）
    pub permission_mode: Option<PermissionMode>,
    /// 执行智能体 id（缺省 = 内置默认主智能体）
    pub agent_type: Option<String>,
    // 模型 / 供应商 / 思考强度不是任务属性：定时会话一律用设置里的默认模型与默认思考强度
    /// 绑定的会话 id（最近一次触发；continuous 模式复用）
    pub session_id: String,
    /// 是否启用（0/1）
    pub enabled: i64,
    /// 创建时间（unix 秒）
    pub created_at: i64,
}

impl From<CoreScheduledTask> for ScheduledTask {
    fn from(v: CoreScheduledTask) -> Self {
        ScheduledTask {
            id: v.id,
            name: v.name,
            description: v.description,
            cron_expr: v.cron_expr,
            mode: v.mode,
            instruction: v.instruction,
            workspace_path: v.workspace_path,
            session_mode: v.session_mode.map(Into::into),
            permission_mode: v.permission_mode.map(Into::into),
            agent_type: v.agent_type,
            session_id: v.session_id,
            enabled: v.enabled,
            created_at: v.created_at,
        }
    }
}

/// 定时任务请求体（新增 / 更新共用）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
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
    pub mode: String,
    /// 执行指令
    pub instruction: String,
    /// 会话目录
    pub workspace_path: String,
    /// 会话执行模式（规划 / 执行 / 自动；缺省 = 沿用任务已存值）
    pub session_mode: Option<SessionMode>,
    /// 会话审批模式（手动 / 自动）
    pub permission_mode: Option<PermissionMode>,
    /// 执行智能体 id
    pub agent_type: Option<String>,
    // 模型 / 供应商 / 思考强度不在此列：定时会话一律用设置里的默认模型与默认思考强度
}

impl From<ScheduledTaskRequest> for CoreScheduledTaskRequest {
    fn from(r: ScheduledTaskRequest) -> Self {
        CoreScheduledTaskRequest {
            id: r.id,
            name: r.name,
            description: r.description,
            cron_expr: r.cron_expr,
            mode: r.mode,
            instruction: r.instruction,
            workspace_path: r.workspace_path,
            session_mode: r.session_mode.map(Into::into),
            permission_mode: r.permission_mode.map(Into::into),
            agent_type: r.agent_type,
        }
    }
}

/// 调度专用工作区生成结果。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedWorkspace {
    /// 工作区 id（uuid）
    pub id: String,
    /// 工作区路径
    pub path: String,
}

impl From<CoreGeneratedWorkspace> for GeneratedWorkspace {
    fn from(v: CoreGeneratedWorkspace) -> Self {
        GeneratedWorkspace {
            id: v.id,
            path: v.path,
        }
    }
}