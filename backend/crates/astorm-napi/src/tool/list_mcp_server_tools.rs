//! list_mcp_server_tools：列出指定 MCP 服务器的工具。

use astorm_common::tool::Tools;
use napi_derive::napi;

use crate::tool::dto::ToolInfo;

/// 列出指定 MCP 服务器的工具。
#[napi]
pub fn list_mcp_server_tools(server: String) -> napi::Result<Vec<ToolInfo>> {
    crate::app()?
        .tool()
        .list_mcp_server_tools(server)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}