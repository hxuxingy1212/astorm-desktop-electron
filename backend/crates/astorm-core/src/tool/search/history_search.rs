//! `history_search` 工具：跨会话全文检索历史消息。
//!
//! 消息在追加落盘时即同步进全文索引（`StorageManager::append_messages` → Tantivy），
//! 此处直接经 `Storage::search` 检索，返回命中消息 id / 归属会话 / 片段。

use async_trait::async_trait;
use serde_json::json;

use crate::tool::{storage, ToolContext};

/// 单次结果上限（防御性夹紧）。
const MAX_LIMIT: usize = 50;

/// `history_search` 工具。
pub struct HistorySearchTool;

#[async_trait]
impl crate::tool::Tool for HistorySearchTool {
    fn name(&self) -> &str {
        "history_search"
    }

    fn description(&self) -> &str {
        "全文搜索所有会话的历史消息，返回命中消息 id / 归属会话 / 相关度 / 内容片段。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "limit": { "type": "integer", "description": "最大结果数（默认 10，上限 50）" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<String, String> {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.is_empty() {
            return Err("搜索关键词不能为空".into());
        }
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, MAX_LIMIT as u64) as usize;

        let store = storage().ok_or("App 存储未初始化，无法检索")?;
        let hits = store.search(query, limit).map_err(|e| format!("搜索失败: {e}"))?;

        if hits.is_empty() {
            return Ok("未找到匹配的历史消息".into());
        }

        let output: Vec<String> = hits
            .iter()
            .map(|h| {
                let snippet = h.snippet.as_deref().unwrap_or("");
                format!(
                    "- **{}**（会话 {}，score {:.4}）\n  {}",
                    h.message_id, h.session_id, h.score, snippet
                )
            })
            .collect();
        Ok(format!(
            "## 历史搜索结果\n\n找到 {} 条匹配:\n\n{}",
            hits.len(),
            output.join("\n")
        ))
    }
}