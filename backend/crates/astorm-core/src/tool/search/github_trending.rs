//! `github_trending` 工具：抓取 GitHub Trending 页面获取热门仓库列表。
//!
//! 支持按编程语言与时间范围（daily / weekly / monthly）过滤；使用 scraper 解析 HTML，
//! 至多返回 25 条。

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::json;

/// 请求超时（秒）与结果上限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: usize = 25;

/// `github_trending` 工具。
pub struct GithubTrendingTool;

#[async_trait]
impl crate::tool::Tool for GithubTrendingTool {
    fn name(&self) -> &str {
        "github_trending"
    }

    fn description(&self) -> &str {
        "获取 GitHub Trending 仓库列表（支持按语言与时间范围过滤），返回热门仓库名称与简介。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "language": { "type": "string", "description": "编程语言过滤（可选）" },
                "since": { "type": "string", "enum": ["daily", "weekly", "monthly"], "description": "时间范围（默认 daily）" }
            }
        })
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &crate::tool::ToolContext) -> Result<String, String> {
        let lang = params.get("language").and_then(|v| v.as_str()).unwrap_or("");
        let since = params.get("since").and_then(|v| v.as_str()).unwrap_or("daily");
        if !["daily", "weekly", "monthly"].contains(&since) {
            return Err("since 必须是 daily / weekly / monthly".into());
        }

        let url = if lang.is_empty() {
            format!("https://github.com/trending?since={since}")
        } else {
            format!("https://github.com/trending/{lang}?since={since}")
        };

        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent("astorm-tool/0.1")
            .build()
            .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;
        let body = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("请求失败: {e}"))?
            .text()
            .await
            .map_err(|e| format!("读取失败: {e}"))?;

        let doc = Html::parse_document(&body);
        let article_sel = Selector::parse("article").map_err(|e| format!("解析选择器失败: {e}"))?;
        let h2_sel = Selector::parse("h2 a").map_err(|e| format!("解析选择器失败: {e}"))?;
        let p_sel = Selector::parse("p").map_err(|e| format!("解析选择器失败: {e}"))?;

        let mut results = Vec::new();
        for (i, article) in doc.select(&article_sel).enumerate() {
            if i >= MAX_RESULTS {
                break;
            }
            // 仓库名称：h2 a 文本取最后一段（"owner / repo" 形式）
            let title = article
                .select(&h2_sel)
                .next()
                .map(|a| {
                    let text: String = a.text().collect();
                    text.split_whitespace().last().unwrap_or("unknown").to_string()
                })
                .unwrap_or_else(|| "unknown".into());
            let desc = article
                .select(&p_sel)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            results.push(format!("{}. {} - {}", i + 1, title, desc));
        }

        if results.is_empty() {
            return Ok("GitHub Trending 无结果".into());
        }
        Ok(results.join("\n"))
    }
}