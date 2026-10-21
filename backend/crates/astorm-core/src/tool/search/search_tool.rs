//! `search` 工具：统一搜索路由器（对外门面）。
//!
//! 本目录内唯一对外暴露的工具：根据 `action` 参数将请求分发到
//! 目录内子工具实现 `websearch` / `webfetch` / `github` / `arxiv` / `history`。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::ToolContext;

/// `search` 工具。
pub struct SearchTool;

#[async_trait]
impl crate::tool::Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "统一搜索路由器：用 action 选择子能力——websearch 网络搜索（query）/ webfetch 抓取指定 URL（url）/ github GitHub Trending（language、since）/ arxiv 学术论文（query）/ history 会话历史（query）"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["websearch", "webfetch", "github", "arxiv", "history"],
                    "description": "动作：websearch 网络搜索 / webfetch 抓取指定 URL / github GitHub Trending / arxiv 学术论文 / history 会话历史"
                },
                "query": { "type": "string", "description": "搜索关键词（websearch / arxiv / history 使用；github 忽略）" },
                "url": { "type": "string", "description": "目标 URL（webfetch 使用）" },
                "max_results": { "type": "integer", "description": "最大结果数（websearch 默认 5 上限 10；arxiv 默认 5 上限 20）" },
                "max_length": { "type": "integer", "description": "返回文本最大长度（webfetch 默认 8000，上限 20000）" },
                "read": { "type": "integer", "description": "websearch：读取前 N 条结果正文（0-3，默认 0；逐条渲染取正文并清洗，较慢）" },
                "language": { "type": "string", "description": "编程语言过滤（github 使用，可选）" },
                "since": { "type": "string", "enum": ["daily", "weekly", "monthly"], "description": "时间范围（github 使用，默认 daily）" },
                "limit": { "type": "integer", "description": "最大结果数（history 默认 10，上限 50）" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<String, String> {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("websearch");
        match action {
            "websearch" => super::websearch::WebSearchTool.execute(params, ctx).await,
            "webfetch" => super::webfetch::WebFetchTool.execute(params, ctx).await,
            "github" => super::github_trending::GithubTrendingTool.execute(params, ctx).await,
            "arxiv" => super::arxiv_search::ArxivSearchTool.execute(params, ctx).await,
            "history" => super::history_search::HistorySearchTool.execute(params, ctx).await,
            _ => Err(format!(
                "未知 action: {action}（可用: websearch / webfetch / github / arxiv / history）"
            )),
        }
    }

    /// 转发到子工具的结构化元数据通道（目前仅 websearch 产出 `web_results`）。
    async fn execute_with_meta(
        &self,
        params: Value,
        ctx: &ToolContext,
    ) -> Result<(String, Option<Value>), String> {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("websearch");
        if action == "websearch" {
            return super::websearch::WebSearchTool.execute_with_meta(params, ctx).await;
        }
        self.execute(params, ctx).await.map(|text| (text, None))
    }
}