//! generate_scheduler_workspace：生成调度专用工作区。

use astorm_common::schedule::Schedule;
use napi_derive::napi;

use crate::schedule::dto::GeneratedWorkspace;

/// 生成调度专用工作区（uuid 目录）。
#[napi]
pub fn generate_scheduler_workspace() -> napi::Result<GeneratedWorkspace> {
    crate::app()?
        .schedule()
        .generate_scheduler_workspace()
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}