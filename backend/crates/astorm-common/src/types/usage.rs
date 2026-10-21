//! Token 用量事件：逐轮落库，供「使用统计」按天/按模型聚合。

use serde::{Deserialize, Serialize};

/// 一条 token 用量事件（一次 LLM 轮次的增量用量）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    /// 所属会话 id。
    pub session_id: String,
    /// 该轮所用模型（会话记录里的模型名；未知为空串）。
    pub model: String,
    /// 该轮所用供应商（未知为空串）。
    pub provider: String,
    /// 事件时间（unix 秒）。
    pub ts: i64,
    pub prompt: u32,
    pub completion: u32,
    #[serde(default)]
    pub cache_hit: u32,
    #[serde(default)]
    pub cache_miss: u32,
    pub total: u32,
}

/// 按「本地日期 + 模型」聚合后的用量行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatRow {
    /// 本地日期（`YYYY-MM-DD`）。
    pub day: String,
    /// 模型名（空串归为「未知」）。
    pub model: String,
    pub prompt: u64,
    pub completion: u64,
    pub cache_hit: u64,
    pub cache_miss: u64,
    pub total: u64,
}
