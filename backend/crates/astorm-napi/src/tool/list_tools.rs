//! list_tools：列出工具（可按类别过滤）。

use astorm_common::tool::Tools;
use napi_derive::napi;

use crate::tool::dto::ToolInfo;

/// 列出工具（可按类别过滤：builtin / plugin / mcp）。
#[napi]
pub fn list_tools(category: Option<String>) -> napi::Result<Vec<ToolInfo>> {
    crate::app()?
        .tool()
        .list_tools(category)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}