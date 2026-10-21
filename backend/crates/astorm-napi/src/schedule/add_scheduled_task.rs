//! add_scheduled_task：新增定时任务。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

use crate::schedule::dto::ScheduledTaskRequest;

/// 新增定时任务，返回任务 id。
#[napi]
pub fn add_scheduled_task(request: ScheduledTaskRequest) -> napi::Result<String> {
    let id = crate::app()?
        .schedule()
        .add_scheduled_task(request.into())
        .map_err(|e| napi::Error::from_reason(e))?;
    // 定义变了 → 广播全局事件：定时任务页重拉列表、看板刷新归属徽标（不必等下次进入页面）
    super::emit_scheduled_tasks_changed();
    Ok(id)
}