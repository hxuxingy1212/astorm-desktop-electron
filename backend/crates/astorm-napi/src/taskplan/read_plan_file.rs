//! read_plan_file：读取单个规划文件。

use astorm_common::taskplan::TaskPlan;
use napi_derive::napi;

use crate::taskplan::dto::{PlanFile, ReadPlanFileRequest};

/// 读取单个规划文件。
#[napi]
pub fn read_plan_file(request: ReadPlanFileRequest) -> napi::Result<PlanFile> {
    crate::app()?
        .taskplan()
        .read_plan_file(request.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}