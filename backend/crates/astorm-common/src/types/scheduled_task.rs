//! 定时任务记录：`schedule_task` 工具的落盘模型（sqlite `scheduled_tasks` 表 `data` 列 / Storage trait 通用）。
//!
//! 全局唯一（不归会话所有）：cron 表达式 + 运行方式 + 触发指令 + **创建会话的参数**，
//! 由 `Scheduler` 加载为定时任务；触发时按这些参数创建 / 复用会话。

use serde::{Deserialize, Serialize};

use crate::session::types::{PermissionMode, SessionMode};

/// 定时任务：cron 触发 + 会话执行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    /// 任务唯一 id（工具暴露给模型 / 定时运行时复用）。
    pub task_id: String,
    /// 任务名称。
    pub name: String,
    /// 任务描述。
    pub description: String,
    /// cron 表达式（tokio-cron-scheduler 语法）。
    pub cron_expr: String,
    /// 运行方式：new_session（每次新建会话）/ continuous_session（复用已有会话）。
    #[serde(default)]
    pub mode: String,
    /// 触发时注入会话的指令（作为用户消息发送）。
    #[serde(default)]
    pub instruction: String,
    /// 会话目录（触发时创建的会话用它作 workspace；空则落在默认调度工作区）。
    #[serde(default)]
    pub workspace_path: String,

    /// 会话执行模式（计划 / 执行 / 自动）；缺省 = 用全局默认。
    /// 注意：所选智能体卡片自带的模式**优先于此值**（`create_session` 现有语义）。
    #[serde(default)]
    pub session_mode: Option<SessionMode>,
    /// 会话审批模式（手动 / 自动）；缺省 = 用全局默认。
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// 执行智能体 id（缺省 = 内置默认主智能体）。
    #[serde(default)]
    pub agent_type: Option<String>,

    // 模型 / 供应商 / 思考强度**不是任务属性**：定时会话一律用设置里配置的默认模型
    // （`llm.defaultModel`，缺省 = 默认供应商的第一个模型）与默认思考强度创建。
    // 旧记录里可能残留 model / provider / reasoningEffort 字段，反序列化时忽略。

    /// 最近一次触发的会话 id（continuous_session 复用目标）。
    #[serde(default)]
    pub session_id: String,
    /// 启用状态（0 / 1）。
    pub enabled: i64,
    /// 创建时间（unix 秒）。
    pub created_at: i64,
    /// 更新时间（unix 秒）。
    pub updated_at: i64,
}
