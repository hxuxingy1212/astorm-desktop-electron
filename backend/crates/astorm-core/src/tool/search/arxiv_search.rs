//! `arxiv_search` 工具：arXiv OpenSearch API 学术论文搜索。
//!
//! 请求 `export.arxiv.org/api/query`（HTTPS），返回 Atom XML，经 quick-xml 反序列化；
//! 摘要截断 200 字符。

use std::time::Duration;

use async_trait::async_trait;
use quick_xml::de;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

/// 请求超时与单次结果上限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: usize = 20;

/// 工具参数。
#[derive(Debug, Deserialize)]
struct ArxivArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

/// Atom feed（仅取 entry 列表）。
#[derive(Debug, Deserialize)]
struct ArxivFeed {
    #[serde(default)]
    entry: Vec<ArxivEntry>,
}

#[derive(Debug, Deserialize)]
struct ArxivEntry {
    title: String,
    summary: String,
    #[serde(default)]
    published: Option<String>,
    #[serde(default)]
    author: Vec<ArxivAuthor>,
}

#[derive(Debug, Deserialize)]
struct ArxivAuthor {
    name: String,
}

/// `arxiv_search` 工具。
pub struct ArxivSearchTool;

#[async_trait]
impl crate::tool::Tool for ArxivSearchTool {
    fn name(&self) -> &str {
        "arxiv_search"
    }

    fn description(&self) -> &str {
        "搜索 arXiv 学术论文，返回标题 / 作者 / 发布日期 / 摘要。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "max_results": { "type": "integer", "description": "最大结果数（默认 5，上限 20）" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &crate::tool::ToolContext) -> Result<String, String> {
        let args: ArxivArgs = serde_json::from_value(params).map_err(|e| format!("参数错误: {e}"))?;
        if args.query.trim().is_empty() {
            return Err("query 不能为空".into());
        }
        let max = args.max_results.unwrap_or(5).min(MAX_RESULTS).max(1);
        let q = args.query.split_whitespace().collect::<Vec<_>>().join("+");
        let url = format!(
            "https://export.arxiv.org/api/query?search_query=all:{q}&start=0&max_results={max}"
        );

        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent("astorm-tool/0.1")
            .build()
            .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;
        let xml = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("请求失败: {e}"))?
            .text()
            .await
            .map_err(|e| format!("读取失败: {e}"))?;

        let feed: ArxivFeed = de::from_str(&xml).map_err(|e| format!("XML 解析失败: {e}"))?;
        if feed.entry.is_empty() {
            return Ok("未找到匹配的 arXiv 论文".into());
        }

        let mut results = Vec::new();
        for entry in feed.entry.iter().take(max) {
            let authors: Vec<&str> = entry.author.iter().map(|a| a.name.as_str()).collect();
            let summary = entry.summary.trim().chars().take(200).collect::<String>();
            results.push(format!(
                "- **{}**\n  {} ({})\n  {}",
                entry.title.trim(),
                authors.join(", "),
                entry.published.as_deref().unwrap_or(""),
                summary
            ));
        }
        Ok(results.join("\n\n"))
    }
}