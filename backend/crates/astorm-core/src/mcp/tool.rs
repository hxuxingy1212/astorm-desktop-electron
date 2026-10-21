//! MCP 工具接入：把服务器工具包装成 react-loop 的 [`Tool`]（执行转发到 [`McpManager::call`]）。

use async_trait::async_trait;
use react_loop::{Tool, ToolContext};
use serde_json::Value;

use super::McpManager;

/// 单个 MCP 工具的循环侧句柄。
///
/// `name` = 对外暴露名（LLM 的 `function.name`；与内置工具重名时带 `{server}__` 前缀），
/// `raw` = 服务器侧原名（`tools/call` 用）。
pub(crate) struct McpToolHandle {
    pub(crate) server: String,
    pub(crate) name: String,
    pub(crate) raw: String,
    pub(crate) description: String,
    pub(crate) schema: Value,
}

#[async_trait]
impl Tool for McpToolHandle {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        let app = crate::app::App::try_global().ok_or("应用未初始化")?;
        // 用户「停止」即取消：调用进行中也可中断（MCP 请求本身不可撤销，连接保留）
        let manager: &McpManager = app.mcp();
        tokio::select! {
            r = manager.call(&self.server, &self.raw, params) => r,
            _ = ctx.cancel.cancelled() => Err("已取消".to_string()),
        }
    }
}
