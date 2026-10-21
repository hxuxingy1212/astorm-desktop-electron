//! update_scheduled_task：更新定时任务。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

use crate::schedule::dto::ScheduledTaskRequest;

/// 更新定时任务。
#[napi]
pub fn update_scheduled_task(request: ScheduledTaskRequest) -> napi::Result<()> {
    crate::app()?
        .schedule()
        .update_scheduled_task(request.into())
        .map_err(|e| napi::Error::from_reason(e))?;
    // 定义变了 → 广播全局事件（改名 / 改 cron / 改指令都走这里）
    super::emit_scheduled_tasks_changed();
    Ok(())
}