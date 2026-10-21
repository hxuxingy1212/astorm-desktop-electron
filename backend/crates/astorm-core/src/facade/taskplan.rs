//! 任务规划门面：列出/读取会话的任务规划大纲（数据源 = sqlite plans 表）。

use astorm_common::taskplan::{PlanFile, PlanStep, PlanSummary, ReadPlanFileRequest, TaskPlan};

/// 任务规划门面。
pub struct TaskPlanImpl;

/// unix 秒 → `YYYY-MM-DD HH:MM:SS` 文本。
fn unix_to_str(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| secs.to_string())
}

impl TaskPlan for TaskPlanImpl {
    /// 列出会话的计划大纲摘要（按创建时间倒序，来自存储层 list_plans）。
    fn list_plan_files(&self, session_id: String) -> Result<Vec<PlanSummary>, String> {
        let storage = crate::tool::storage().ok_or("存储未初始化，无法读取规划")?;
        let plans = storage
            .list_plans(&session_id)
            .map_err(|e| format!("读取规划列表失败: {e}"))?;
        Ok(plans
            .into_iter()
            .map(|p| PlanSummary {
                task_id: p.task_id,
                title: p.title,
                description: p.description,
                created_at: unix_to_str(p.created_at),
                updated_at: unix_to_str(p.updated_at),
                total_nodes: p.nodes.len(),
                completed_nodes: p
                    .nodes
                    .iter()
                    .filter(|n| n.status == "completed")
                    .count(),
            })
            .collect())
    }

    /// 读取单个规划大纲（计划不存在返回错误）。
    fn read_plan_file(&self, request: ReadPlanFileRequest) -> Result<PlanFile, String> {
        let storage = crate::tool::storage().ok_or("存储未初始化，无法读取规划")?;
        let plan = storage
            .get_plan(&request.session_id, &request.task_id)
            .map_err(|e| format!("读取规划失败: {e}"))?
            .ok_or_else(|| format!("未找到任务规划：{}", request.task_id))?;
        Ok(PlanFile {
            task_id: plan.task_id,
            title: plan.title,
            description: plan.description,
            created_at: unix_to_str(plan.created_at),
            updated_at: unix_to_str(plan.updated_at),
            nodes: plan
                .nodes
                .into_iter()
                .map(|n| PlanStep {
                    task_id: n.task_id,
                    task_name: n.task_name,
                    task_description: n.task_description,
                    background: None,
                    constraints: None,
                    deliverables: None,
                    status: n.status,
                    created_at: unix_to_str(n.created_at),
                    updated_at: unix_to_str(n.updated_at),
                    node_ref: None,
                    agent_id: None,
                    agent_name: None,
                })
                .collect(),
        })
    }
}