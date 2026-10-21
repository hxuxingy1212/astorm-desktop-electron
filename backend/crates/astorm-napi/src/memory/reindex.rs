//! memory_reindex：重建全文索引（wiki 是真相，索引纯派生）。

use napi_derive::napi;

/// 由 `wiki/` 重建全文索引，返回重建的概念数。
#[napi]
pub fn memory_reindex() -> napi::Result<u32> {
    crate::app()?
        .memory()
        .reindex()
        .map(|n| n as u32)
        .map_err(|e| napi::Error::from_reason(e))
}
