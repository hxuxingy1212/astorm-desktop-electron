//! memory_show_concept：读取单个概念详情（含 Markdown 原文）。

use napi_derive::napi;

use crate::memory::dto::MemoryConceptDetail;

/// 读取概念详情（`conceptId` 为相对 `wiki/` 去 `.md` 的路径，如 `decisions/use-jwt`）。
#[napi]
pub fn memory_show_concept(concept_id: String) -> napi::Result<MemoryConceptDetail> {
    crate::app()?
        .memory()
        .show_concept(&concept_id)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
