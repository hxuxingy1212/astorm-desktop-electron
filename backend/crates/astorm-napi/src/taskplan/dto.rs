//! 任务规划 DTO 镜像。

use astorm_common::taskplan::{
    PlanFile as CorePlanFile, PlanStep as CorePlanStep, PlanSummary as CorePlanSummary,
    ReadPlanFileRequest as CoreReadPlanFileRequest,
};
use napi_derive::napi;
use serde::Serialize;

/// 规划步骤（线性大纲节点）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// 步骤 id
    pub task_id: String,
    /// 步骤名称
    pub task_name: String,
    /// 步骤描述
    pub task_description: String,
    /// 背景说明（可选）
    pub background: Option<Vec<String>>,
    /// 约束（可选）
    pub constraints: Option<Vec<String>>,
    /// 交付物（可选）
    pub deliverables: Option<Vec<String>>,
    /// 状态：pending / in_progress / completed / failed
    pub status: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 外部引用（如 "S1"）
    pub node_ref: Option<String>,
    /// 负责 agent id（可选）
    pub agent_id: Option<String>,
    /// 负责 agent 名（可选）
    pub agent_name: Option<String>,
}

impl From<CorePlanStep> for PlanStep {
    fn from(v: CorePlanStep) -> Self {
        PlanStep {
            task_id: v.task_id,
            task_name: v.task_name,
            task_description: v.task_description,
            background: v.background,
            constraints: v.constraints,
            deliverables: v.deliverables,
            status: v.status,
            created_at: v.created_at,
            updated_at: v.updated_at,
            node_ref: v.node_ref,
            agent_id: v.agent_id,
            agent_name: v.agent_name,
        }
    }
}

/// 规划摘要（列表项）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    /// 规划 id
    pub task_id: String,
    /// 标题
    pub title: String,
    /// 描述
    pub description: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 总节点数
    pub total_nodes: u32,
    /// 已完成节点数
    pub completed_nodes: u32,
}

impl From<CorePlanSummary> for PlanSummary {
    fn from(v: CorePlanSummary) -> Self {
        PlanSummary {
            task_id: v.task_id,
            title: v.title,
            description: v.description,
            created_at: v.created_at,
            updated_at: v.updated_at,
            total_nodes: v.total_nodes as u32,
            completed_nodes: v.completed_nodes as u32,
        }
    }
}

/// 规划完整文件（大纲：线性步骤；详细内容为会话空间下的 markdown 文档,经提示词注入）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFile {
    /// 规划 id
    pub task_id: String,
    /// 标题
    pub title: String,
    /// 描述
    pub description: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 步骤列表（顺序即执行顺序）
    pub nodes: Vec<PlanStep>,
}

impl From<CorePlanFile> for PlanFile {
    fn from(v: CorePlanFile) -> Self {
        PlanFile {
            task_id: v.task_id,
            title: v.title,
            description: v.description,
            created_at: v.created_at,
            updated_at: v.updated_at,
            nodes: v.nodes.into_iter().map(Into::into).collect(),
        }
    }
}

/// 读取规划文件请求。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadPlanFileRequest {
    /// 所属会话 id
    pub session_id: String,
    /// 规划 id
    pub task_id: String,
}

impl From<ReadPlanFileRequest> for CoreReadPlanFileRequest {
    fn from(r: ReadPlanFileRequest) -> Self {
        CoreReadPlanFileRequest {
            session_id: r.session_id,
            task_id: r.task_id,
        }
    }
}