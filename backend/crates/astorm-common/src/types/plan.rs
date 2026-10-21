//! 任务规划记录：`task_list` 工具的落盘模型（sqlite `plans` 表 `data` 列 / Storage trait 通用）。
//!
//! 归会话所有（`session_id` 作归属键，记录内不冗余携带）。

use serde::{Deserialize, Serialize};

/// 规划步骤（节点）：线性任务列表中的一个步骤。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// 步骤唯一 id。
    pub task_id: String,
    /// 步骤名称。
    pub task_name: String,
    /// 步骤描述。
    pub task_description: String,
    /// 状态：pending / in_progress / completed / failed。
    pub status: String,
    /// 创建时间（unix 秒）。
    pub created_at: i64,
    /// 更新时间（unix 秒）。
    pub updated_at: i64,
}

/// 任务规划：标题 + 线性步骤列表。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRecord {
    /// 规划唯一 id（工具暴露给模型复用）。
    pub task_id: String,
    /// 任务标题。
    pub title: String,
    /// 任务描述。
    pub description: String,
    /// 步骤列表（顺序即执行顺序）。
    pub nodes: Vec<PlanStep>,
    /// 创建时间（unix 秒）。
    pub created_at: i64,
    /// 更新时间（unix 秒）。
    pub updated_at: i64,
}