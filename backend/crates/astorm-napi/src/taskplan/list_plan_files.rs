//! list_plan_files：列出会话的任务规划文件。

use astorm_common::taskplan::TaskPlan;
use napi_derive::napi;

use crate::taskplan::dto::PlanSummary;

/// 列出会话的任务规划文件。
#[napi]
pub fn list_plan_files(session_id: String) -> napi::Result<Vec<PlanSummary>> {
    crate::app()?
        .taskplan()
        .list_plan_files(session_id)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}