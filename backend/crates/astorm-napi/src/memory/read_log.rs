//! memory_read_log：变更历史（`wiki/log.md`，倒序）。

use napi_derive::napi;

use crate::memory::dto::MemoryLogEntry;

/// 读取变更历史（倒序）；`limit` 省略时取 200，上限 2000。
#[napi]
pub fn memory_read_log(limit: Option<i32>) -> napi::Result<Vec<MemoryLogEntry>> {
    let limit = limit.filter(|n| *n > 0).map(|n| n as usize).unwrap_or(200);
    Ok(crate::app()?
        .memory()
        .read_log(limit)
        .into_iter()
        .map(Into::into)
        .collect())
}
