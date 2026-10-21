//! list_scheduled_tasks：列出全部定时任务。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

use crate::schedule::dto::ScheduledTask;

/// 列出全部定时任务。
#[napi]
pub fn list_scheduled_tasks() -> napi::Result<Vec<ScheduledTask>> {
    crate::app()?
        .schedule()
        .list_scheduled_tasks()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}