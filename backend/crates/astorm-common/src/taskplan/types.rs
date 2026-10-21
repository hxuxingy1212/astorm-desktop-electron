//! 任务规划类型（纯 serde，不依赖 napi）。
//!
//! 大纲模型：线性步骤（顺序即执行顺序）。规划详细内容为 markdown 文档，
//! 存于 `会话空间/.astorm/plan/{task_id}.md`，经提示词注入，不通过工具/接口获取。

use serde::{Deserialize, Serialize};

/// 规划步骤（线性大纲节点）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// 步骤 id
    pub task_id: String,
    /// 步骤名称
    pub task_name: String,
    /// 步骤描述
    pub task_description: String,
    /// 背景说明（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Vec<String>>,
    /// 约束（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Vec<String>>,
    /// 交付物（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverables: Option<Vec<String>>,
    /// 状态：pending / in_progress / completed / failed
    pub status: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 外部引用（如 "S1"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ref: Option<String>,
    /// 负责 agent id（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// 负责 agent 名（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
}

/// 规划摘要（列表项）。
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub total_nodes: usize,
    /// 已完成节点数
    pub completed_nodes: usize,
}

/// 规划完整文件（大纲模型：线性步骤列表；详细规划内容为会话空间下的 markdown 文档）。
#[derive(Debug, Clone, Serialize, Deserialize)]
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