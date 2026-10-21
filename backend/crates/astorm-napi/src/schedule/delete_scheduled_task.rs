//! delete_scheduled_task：删除定时任务。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

/// 删除定时任务。
#[napi]
pub fn delete_scheduled_task(id: String) -> napi::Result<()> {
    crate::app()?
        .schedule()
        .delete_scheduled_task(id)
        .map_err(|e| napi::Error::from_reason(e))?;
    // 定义变了 → 广播全局事件
    super::emit_scheduled_tasks_changed();
    Ok(())
}