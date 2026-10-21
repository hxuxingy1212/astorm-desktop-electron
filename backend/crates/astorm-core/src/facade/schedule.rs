//! 定时任务门面实现：委托 `AppState.scheduler`（[`crate::scheduler::Scheduler`]）。
//!
//! 调度器方法为 async，napi 调用来自 Node 主线程（libuv，非 tokio worker），
//! 故经应用自建 runtime 句柄 `block_on` 桥接（与 `session` 门面 spawn 模式不同，
//! 此处需要拿到返回值，直接同步等待即可）。

use std::future::Future;

use astorm_common::schedule::{
    GeneratedWorkspace, Schedule, ScheduledTask, ScheduledTaskRequest,
};

use crate::app::App;

/// 定时任务门面（`AppState.schedule`）。
pub struct ScheduleImpl;

impl Schedule for ScheduleImpl {
    fn list_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>, String> {
        block_on(App::global().scheduler.list_tasks()).map(|list| list.into_iter().map(Into::into).collect())
    }

    fn add_scheduled_task(&self, request: ScheduledTaskRequest) -> Result<String, String> {
        block_on(App::global().scheduler.add_from_request(&request))
    }

    fn update_scheduled_task(&self, request: ScheduledTaskRequest) -> Result<(), String> {
        block_on(App::global().scheduler.update_from_request(&request))
    }

    fn delete_scheduled_task(&self, id: String) -> Result<(), String> {
        block_on(App::global().scheduler.remove_task(&id))
    }

    fn toggle_scheduled_task(&self, id: String, enabled: bool) -> Result<(), String> {
        block_on(App::global().scheduler.enable_task(&id, enabled))
    }

    fn run_scheduled_task_now(&self, id: String) -> Result<(), String> {
        block_on(App::global().scheduler.run_task_now(&id))
    }

    fn generate_scheduler_workspace(&self) -> Result<GeneratedWorkspace, String> {
        block_on(App::global().scheduler.generate_workspace())
    }
}

/// 在应用自建 runtime 上同步执行异步调度操作。
fn block_on<F: Future>(fut: F) -> F::Output {
    App::global().runtime.handle().block_on(fut)
}