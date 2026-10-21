//! search_tools：按关键词搜索工具。

use astorm_common::tool::Tools;
use napi_derive::napi;

use crate::tool::dto::ToolInfo;

/// 按关键词搜索工具。
#[napi]
pub fn search_tools(query: String, limit: Option<i32>) -> napi::Result<Vec<ToolInfo>> {
    crate::app()?
        .tool()
        .search_tools(query, limit.map(|n| n as usize))
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}