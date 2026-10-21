//! 定时任务（schedule）接口：列表 / 增删改 / 启停 / 立即执行。

mod add_scheduled_task;
mod delete_scheduled_task;
mod dto;
mod generate_scheduler_workspace;
mod list_scheduled_tasks;
mod run_scheduled_task_now;
mod toggle_scheduled_task;
mod update_scheduled_task;

pub use add_scheduled_task::add_scheduled_task;
pub use delete_scheduled_task::delete_scheduled_task;
pub use dto::{GeneratedWorkspace, ScheduledTask, ScheduledTaskRequest};
pub use generate_scheduler_workspace::generate_scheduler_workspace;
pub use list_scheduled_tasks::list_scheduled_tasks;
pub use run_scheduled_task_now::run_scheduled_task_now;
pub use toggle_scheduled_task::toggle_scheduled_task;
pub use update_scheduled_task::update_scheduled_task;

/// 定时任务**定义**变更事件（新增 / 删除 / 修改 / 启停 / 立即执行后的会话回写）。
///
/// 为什么需要它：任务定义不在任何会话里，落库时没有会话级事件可依附 —— 前端此前只能在
/// 进入「定时任务」页 / 看板时重拉一次，别的窗口（或本窗口另一页）改完任务名，看板上的
/// 归属徽标要等下次进入才更新。广播一条全局事件（`session_id` 为空）即可让各视图即时重拉。
pub const SCHEDULED_TASKS_CHANGED_EVENT: &str = "scheduled_tasks_changed";

/// 广播「定时任务定义变了」（全局事件，`session_id` 为空；前端 `services/schedulesSync.ts` 消费）。
pub(crate) fn emit_scheduled_tasks_changed() {
    let Ok(app) = crate::app() else {
        // 后端未初始化（纯单测/极端时序）时静默跳过：这只是刷新提示，不该让写操作失败
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    app.emit(serde_json::json!({
        "type": SCHEDULED_TASKS_CHANGED_EVENT,
        "session_id": serde_json::Value::Null,
        "timestamp": ts,
    }));
}