//! `task_list` 工具：任务计划管理（大纲 + 详细规划文档）。
//!
//! - `create`：创建分步骤计划（uuid 计划 id / 步骤 id，步骤状态默认 pending）；
//!   可选携带 `detail`（markdown 详细规划），随大纲落盘到 `会话空间/.astorm/plan/{task_id}.md`
//! - `update_detail`：写 / 更新该规划的详细规划 markdown 文档（详细内容不走工具回读,经提示词注入）
//! - `update_nodes`：按步骤编号（1 基）批量更新状态（pending / in_progress / completed / failed）
//! - `delete`：删除计划（连带删除详请文档）
//!
//! 大纲规划归会话所有，经 `Storage`（`App::global().storage`，sqlite `plans` 表）持久化，
//! 重启不丢；详细规划文档落在会话工作区 `.astorm/plan/`，随会话工作区留存。

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use astorm_common::types::{PlanRecord, PlanStep};

use crate::agent::session::unix_now;
use crate::tool::{storage, ToolContext};

/// 业务层专属事件类型：任务计划（大纲 / 详情文档）发生变更。
///
/// 前端据此**即时**刷新任务规划胶囊，不必等「消息变化 + 防抖」的兜底刷新
/// （与 `agent::title::TITLE_EVENT` 同约定：业务侧定义构造器，不进入 `react_loop::event`）。
pub const PLAN_UPDATED_EVENT: &str = "plan_updated";

/// 当前 unix 毫秒时间戳（事件时间，与前端 `Date.now()` 对齐）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 计划变更事件：`{ type, session_id, timestamp, action, task_id? }`。
///
/// `action` = create / update_detail / update_nodes / delete；`task_id` 缺省表示无具体计划
/// （当前各动作都带 id）。前端只需 `type` + `session_id` 即可触发刷新。
fn plan_updated_event(session_id: &str, action: &str, task_id: Option<&str>) -> serde_json::Value {
    let mut event = json!({
        "type": PLAN_UPDATED_EVENT,
        "session_id": session_id,
        "timestamp": now_ms(),
        "action": action,
    });
    if let Some(id) = task_id {
        event["task_id"] = json!(id);
    }
    event
}

/// 广播计划变更事件（App / 会话不可达时静默跳过：事件只是**即时性**优化，
/// 前端的消息变化兜底刷新仍然存在）。
fn emit_plan_updated(session_id: &str, action: &str, task_id: Option<&str>) {
    let Some(app) = crate::app::App::try_global() else {
        return;
    };
    app.session()
        .emit_event_of(session_id, plan_updated_event(session_id, action, task_id));
}

/// 合法步骤状态。
const PLAN_STATUSES: [&str; 4] = ["pending", "in_progress", "completed", "failed"];

/// `task_list` 工具。
pub struct TaskListTool;

/// 步骤参数（create 时 LLM 只需提供名称与描述）。
#[derive(Debug, Deserialize)]
struct PlanStepArg {
    task_name: String,
    #[serde(default)]
    task_description: Option<String>,
}

/// 工具参数。
#[derive(Debug, Deserialize)]
struct TaskListArgs {
    /// 操作类型：create / update_nodes / delete。
    action: String,
    /// 任务标题（create 时必填）。
    #[serde(default)]
    title: Option<String>,
    /// 任务描述（create 时可选）。
    #[serde(default)]
    description: Option<String>,
    /// 步骤列表（create 时必填）。
    #[serde(default)]
    steps: Option<Vec<PlanStepArg>>,
    /// 计划 id（update_nodes / delete 时必填）。
    #[serde(default)]
    task_id: Option<String>,
    /// 要更新状态的步骤编号（1 基，update_nodes 时必填）。
    #[serde(default)]
    step_numbers: Option<Vec<i32>>,
    /// 目标状态（update_nodes 时必填）。
    #[serde(default)]
    status: Option<String>,
    /// 详细规划内容（markdown;create 时可选落盘,update_detail 时必填）。
    #[serde(default)]
    detail: Option<String>,
}

/// 写入规划详情 markdown:`{工作区}/.astorm/plan/{task_id}.md`,返回文档路径。
fn write_plan_doc(session_id: &str, task_id: &str, detail: &str) -> Result<String, String> {
    use std::fs;
    let path = crate::tool::plan_doc_path(session_id, task_id)?;
    fs::create_dir_all(path.parent().expect("plan doc 必有父目录"))
        .map_err(|e| format!("创建规划详情目录失败: {e}"))?;
    fs::write(&path, detail).map_err(|e| format!("写入规划详情失败: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 删除规划详情 markdown(不存在则忽略)。
fn remove_plan_doc(session_id: &str, task_id: &str) {
    use std::fs;
    let Ok(path) = crate::tool::plan_doc_path(session_id, task_id) else {
        return;
    };
    let _ = fs::remove_file(path);
}

#[async_trait]
impl crate::tool::Tool for TaskListTool {
    fn name(&self) -> &str {
        "task_list"
    }

    fn description(&self) -> &str {
        "任务计划管理：create 创建分步骤大纲（可选顺带写入详细规划 markdown）→ update_detail 写/更新详细规划文档 → update_nodes 更新步骤状态 → delete 删除计划（含详请文档）。当需要修改已有规划时，先 delete 删除再 create 重新创建。规划大纲持久化，详请文档存于会话空间。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update_detail", "update_nodes", "delete"],
                    "description": "操作类型：create 创建计划 / update_detail 写详细规划文档 / update_nodes 更新步骤状态 / delete 删除"
                },
                "title": { "type": "string", "description": "任务标题（create 时需要）" },
                "description": { "type": "string", "description": "一句话说明本次调用的目的/任务描述（展示给用户，简短）" },
                "steps": {
                    "type": "array",
                    "description": "步骤列表（create 时需要），每步只需提供名称和描述",
                    "items": {
                        "type": "object",
                        "properties": {
                            "task_name": { "type": "string", "description": "步骤名称" },
                            "task_description": { "type": "string", "description": "步骤描述" }
                        },
                        "required": ["task_name"]
                    }
                },
                "task_id": { "type": "string", "description": "任务 ID（update_detail / update_nodes / delete 时需要）" },
                "detail": { "type": "string", "description": "详细规划内容（markdown，请包含背景/目标/分步方案/交付物等完整规划；create 可选、update_detail 必填）" },
                "step_numbers": {
                    "type": "array",
                    "description": "要更新状态的步骤编号，1 表示第一个步骤（update_nodes 时需要）",
                    "items": { "type": "integer" }
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "failed"],
                    "description": "目标状态（update_nodes 时需要）"
                }
            },
            "required": ["action", "description"]
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<String, String> {
        let args: TaskListArgs = serde_json::from_value(params).map_err(|e| format!("参数错误: {e}"))?;
        if args.action.is_empty() {
            return Err("action 必填（create / update_detail / update_nodes / delete）".into());
        }

        let session_id = ctx.session_id.clone();
        let storage = storage().ok_or("App 存储未初始化，无法持久化规划")?;

        let (msg, task_id) = match args.action.as_str() {
            "create" => {
                let title = args.title.as_deref().ok_or("title 必填")?;
                let description = args.description.as_deref().unwrap_or("");
                let steps = args.steps.ok_or("steps 必填")?;
                if steps.is_empty() {
                    return Err("steps 不能为空".into());
                }

                let now = unix_now();
                let nodes: Vec<PlanStep> = steps
                    .into_iter()
                    .map(|s| PlanStep {
                        task_id: Uuid::new_v4().to_string(),
                        task_name: s.task_name,
                        task_description: s.task_description.unwrap_or_default(),
                        status: "pending".into(),
                        created_at: now,
                        updated_at: now,
                    })
                    .collect();
                let plan = PlanRecord {
                    task_id: Uuid::new_v4().to_string(),
                    title: title.to_string(),
                    description: description.to_string(),
                    nodes,
                    created_at: now,
                    updated_at: now,
                };
                storage
                    .upsert_plan(&session_id, &plan)
                    .map_err(|e| format!("保存规划失败: {e}"))?;
                // 可选:随大纲一并写入详细规划 markdown(存会话空间 .astorm/plan/{task_id}.md)
                let mut msg = format!("任务id：{}，已创建规划（{} 个步骤）", plan.task_id, plan.nodes.len());
                if let Some(detail) = args.detail.as_deref().filter(|d| !d.trim().is_empty()) {
                    let path = write_plan_doc(&session_id, &plan.task_id, detail)?;
                    msg.push_str(&format!("；详细规划已保存：{path}"));
                }
                (msg, Some(plan.task_id))
            }

            "update_detail" => {
                let task_id = args.task_id.as_deref().ok_or("task_id 必填")?;
                let detail = args.detail.as_deref().ok_or("detail 必填（markdown 详细规划）")?;
                if detail.trim().is_empty() {
                    return Err("detail 不能为空".into());
                }
                // 仅对已存在的规划写详请文档(配置大纲为准)
                storage
                    .get_plan(&session_id, task_id)
                    .map_err(|e| format!("读取规划失败: {e}"))?
                    .ok_or_else(|| format!("未找到 task_id 为 {task_id} 的规划"))?;
                let path = write_plan_doc(&session_id, task_id, detail)?;
                (format!("已保存详细规划至：{path}"), Some(task_id.to_string()))
            }

            "update_nodes" => {
                let task_id = args.task_id.as_deref().ok_or("task_id 必填")?;
                let step_numbers = args.step_numbers.ok_or("step_numbers 必填")?;
                let status = args.status.as_deref().ok_or("status 必填")?;
                if !PLAN_STATUSES.contains(&status) {
                    return Err(format!("status 必须是 {} 之一", PLAN_STATUSES.join(" / ")));
                }
                if step_numbers.is_empty() {
                    return Err("step_numbers 不能为空".into());
                }

                let mut plan = storage
                    .get_plan(&session_id, task_id)
                    .map_err(|e| format!("读取规划失败: {e}"))?
                    .ok_or_else(|| format!("未找到 task_id 为 {task_id} 的规划"))?;

                let now = unix_now();
                for step_num in &step_numbers {
                    let idx = (*step_num - 1) as usize;
                    let node = plan
                        .nodes
                        .get_mut(idx)
                        .ok_or_else(|| format!("步骤 {step_num} 不存在"))?;
                    node.status = status.to_string();
                    node.updated_at = now;
                }
                plan.updated_at = now;
                storage
                    .upsert_plan(&session_id, &plan)
                    .map_err(|e| format!("保存规划失败: {e}"))?;
                (
                    format!("已更新 {} 个步骤状态为 {}", step_numbers.len(), status),
                    Some(task_id.to_string()),
                )
            }

            "delete" => {
                let task_id = args.task_id.as_deref().ok_or("task_id 必填")?;
                storage
                    .delete_plan(&session_id, task_id)
                    .map_err(|e| format!("删除规划失败: {e}"))?;
                remove_plan_doc(&session_id, task_id);
                (format!("已删除规划：{task_id}"), Some(task_id.to_string()))
            }

            _ => {
                return Err(format!(
                    "不支持的操作: {}（可用: create / update_detail / update_nodes / delete）",
                    args.action
                ))
            }
        };
        // 大纲 / 详情文档已变更 → 广播事件，前端据此即时刷新任务规划胶囊
        emit_plan_updated(&session_id, &args.action, task_id.as_deref());
        Ok(msg)
    }
}