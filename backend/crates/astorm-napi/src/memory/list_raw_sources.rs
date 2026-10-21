//! memory_list_raw_sources：源会话列表（`raw/sessions/`）。

use napi_derive::napi;

use crate::memory::dto::MemoryRawSource;

/// 列出归一化后的源会话（按文件名倒序 = 新的在前）。
#[napi]
pub fn memory_list_raw_sources() -> napi::Result<Vec<MemoryRawSource>> {
    crate::app()?
        .memory()
        .list_raw_sources()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
