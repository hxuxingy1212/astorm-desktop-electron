//! memory_list_concepts：列出全部概念（直接读盘，不依赖索引）。

use napi_derive::napi;

use crate::memory::dto::MemoryConcept;

/// 列出全部概念（按 concept ID 升序；索引未建也能列出）。
#[napi]
pub fn memory_list_concepts() -> napi::Result<Vec<MemoryConcept>> {
    crate::app()?
        .memory()
        .list_concepts()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
