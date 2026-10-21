//! memory_search：中文全文检索（BM25；索引缺失时退化为子串扫描）。

use napi_derive::napi;

use crate::memory::dto::MemoryConcept;

/// 检索概念（标题 / 描述 / 标签 / 正文）；`limit` 省略时取 20，上限 200。
#[napi]
pub fn memory_search(query: String, limit: Option<i32>) -> napi::Result<Vec<MemoryConcept>> {
    let limit = limit.filter(|n| *n > 0).map(|n| n as usize).unwrap_or(20);
    crate::app()?
        .memory()
        .search(&query, limit)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
