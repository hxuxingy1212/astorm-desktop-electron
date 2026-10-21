//! toggle_scheduled_task：启停定时任务。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

/// 启停定时任务。
#[napi]
pub fn toggle_scheduled_task(id: String, enabled: bool) -> napi::Result<()> {
    crate::app()?
        .schedule()
        .toggle_scheduled_task(id, enabled)
        .map_err(|e| napi::Error::from_reason(e))?;
    // 启用位变了 → 广播全局事件
    super::emit_scheduled_tasks_changed();
    Ok(())
}