//! 工具目录门面实现：`list_tools` / `search_tools` 来自 [`crate::tool::ToolRegistry`]
//! 的内置工具快照 + 已连接的 MCP 服务器工具（[`crate::mcp::McpManager`]）。

use astorm_common::tool::{ToolInfo, ToolSource, Tools};

use crate::app::App;

/// 工具目录门面（`AppState.tool`）。
pub struct ToolImpl;

impl Tools for ToolImpl {
    fn list_tools(&self, category: Option<String>) -> Result<Vec<ToolInfo>, String> {
        let app = App::global();
        let mut infos = app.tools.infos();
        // MCP 工具（仅已连接的服务器；未连接 = 工具未发现）
        infos.extend(app.mcp().tool_infos());
        let list: Vec<ToolInfo> = match category.as_deref() {
            Some("plugin") => infos
                .into_iter()
                .filter(|t| matches!(t.source, ToolSource::Plugin))
                .collect(),
            Some("mcp") => infos
                .into_iter()
                .filter(|t| matches!(t.source, ToolSource::Mcp { .. }))
                .collect(),
            // 无过滤 / "builtin"：内置 + MCP（插件系统未接入后端）
            _ => infos,
        };
        Ok(list)
    }

    fn search_tools(&self, query: String, limit: Option<usize>) -> Result<Vec<ToolInfo>, String> {
        let app = App::global();
        let q = query.to_lowercase();
        let mut infos = app.tools.infos();
        infos.extend(app.mcp().tool_infos());
        let mut out: Vec<ToolInfo> = infos
            .into_iter()
            .filter(|t| {
                t.name.to_lowercase().contains(&q) || t.description.to_lowercase().contains(&q)
            })
            .collect();
        if let Some(n) = limit {
            out.truncate(n);
        }
        Ok(out)
    }

    fn list_mcp_server_tools(&self, server: String) -> Result<Vec<ToolInfo>, String> {
        Ok(App::global().mcp().list_tools(&server))
    }
}
