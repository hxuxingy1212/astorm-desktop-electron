//! 工具接口契约：工具目录信息（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

/// 工具来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSource {
    /// 内置工具
    Builtin,
    /// 插件工具
    Plugin,
    /// MCP 服务器工具
    Mcp {
        /// MCP 服务器名
        server: String,
    },
}

/// 工具描述信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    /// 工具名
    pub name: String,
    /// 工具描述
    pub description: String,
    /// OpenAI JSON-schema 参数声明
    pub parameters_schema: serde_json::Value,
    /// 来源（builtin / plugin / mcp）
    pub source: ToolSource,
    /// 是否属「按需加载」动态工具：初始不下发 LLM，经 `tool_load` 加载后生效
    /// （清单真相在 `astorm-core agent::dynamic_tools::DYNAMIC_TOOLS`，此处仅展示标记）
    #[serde(default)]
    pub dynamic: bool,
}

/// 工具目录门面：由业务层（`astorm-core`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用。
pub trait Tools {
    /// 列出工具（可按类别过滤：builtin / plugin / mcp）。
    fn list_tools(&self, category: Option<String>) -> Result<Vec<ToolInfo>, String>;
    /// 列出指定 MCP 服务器的工具。
    fn list_mcp_server_tools(&self, server: String) -> Result<Vec<ToolInfo>, String>;
    /// 按关键词搜索工具。
    fn search_tools(&self, query: String, limit: Option<usize>) -> Result<Vec<ToolInfo>, String>;
}