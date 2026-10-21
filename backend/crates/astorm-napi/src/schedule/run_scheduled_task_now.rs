//! run_scheduled_task_now：立即执行一次定时任务。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

/// 立即执行一次定时任务。
#[napi]
pub fn run_scheduled_task_now(id: String) -> napi::Result<()> {
    crate::app()?
        .schedule()
        .run_scheduled_task_now(id)
        .map_err(|e| napi::Error::from_reason(e))?;
    // 执行会把新会话 id 回写进任务定义（任务卡片下的执行记录按它归集）→ 也广播一次
    super::emit_scheduled_tasks_changed();
    Ok(())
}