//! 工具 DTO 镜像。

use astorm_common::tool::{ToolInfo as CoreToolInfo, ToolSource as CoreToolSource};
use napi_derive::napi;
use serde::Serialize;

/// 工具来源（napi 表示：`{ type, server? }`）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSource {
    /// 来源类型：builtin / plugin / mcp
    pub r#type: String,
    /// MCP 服务器名（type=mcp 时有值）
    pub server: Option<String>,
}

impl From<CoreToolSource> for ToolSource {
    fn from(v: CoreToolSource) -> Self {
        match v {
            CoreToolSource::Builtin => ToolSource {
                r#type: "builtin".into(),
                server: None,
            },
            CoreToolSource::Plugin => ToolSource {
                r#type: "plugin".into(),
                server: None,
            },
            CoreToolSource::Mcp { server } => ToolSource {
                r#type: "mcp".into(),
                server: Some(server),
            },
        }
    }
}

/// 工具描述信息。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
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
    /// 是否「按需加载」动态工具（初始不下发 LLM，经 tool_load 加载后生效）
    pub dynamic: bool,
}

impl From<CoreToolInfo> for ToolInfo {
    fn from(v: CoreToolInfo) -> Self {
        ToolInfo {
            name: v.name,
            description: v.description,
            parameters_schema: v.parameters_schema,
            source: v.source.into(),
            dynamic: v.dynamic,
        }
    }
}