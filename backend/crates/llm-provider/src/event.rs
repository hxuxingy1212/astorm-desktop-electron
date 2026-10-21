//! 流式 LLM 事件与 token 用量统计。

use crate::request::ToolCall;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

/// 流式输出中的文本片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub content: String,
}

impl Chunk {
    pub fn new(content: impl Into<String>) -> Self {
        Self { content: content.into() }
    }
}

/// 流结束信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndInfo {
    /// 结束原因（stop / tool_calls / length ...）
    pub finish_reason: String,
    pub usage: Option<TokenUsage>,
}

/// token 用量。循环引擎会跨轮次累计到 [`crate::LoopState::stats`]。
///
/// 标记 `#[napi(object)]`：可跨语言传递（会话记录 [`astorm_core::SessionRecord`] 引用它）。
#[napi(object)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
    #[serde(default)]
    pub cache_hit: u32,
    #[serde(default)]
    pub cache_miss: u32,
}

impl TokenUsage {
    /// 由 prompt / completion 构造，total 自动求和。
    pub fn new(prompt: u32, completion: u32) -> Self {
        Self {
            prompt,
            completion,
            total: prompt + completion,
            cache_hit: 0,
            cache_miss: 0,
        }
    }

    /// 将另一份用量累加到自身（用于跨轮次累计）。
    pub fn accumulate(&mut self, other: &TokenUsage) {
        self.prompt += other.prompt;
        self.completion += other.completion;
        self.total += other.total;
        self.cache_hit += other.cache_hit;
        self.cache_miss += other.cache_miss;
    }

    /// 相对 `previous` 的增量（逐字段饱和相减；用于子会话把新增用量上报父会话）。
    pub fn delta_from(&self, previous: &TokenUsage) -> TokenUsage {
        TokenUsage {
            prompt: self.prompt.saturating_sub(previous.prompt),
            completion: self.completion.saturating_sub(previous.completion),
            total: self.total.saturating_sub(previous.total),
            cache_hit: self.cache_hit.saturating_sub(previous.cache_hit),
            cache_miss: self.cache_miss.saturating_sub(previous.cache_miss),
        }
    }

    /// 是否为零用量。
    pub fn is_zero(&self) -> bool {
        self.prompt == 0
            && self.completion == 0
            && self.total == 0
            && self.cache_hit == 0
            && self.cache_miss == 0
    }
}

/// 一次流式 LLM 调用的事件序列。
///
/// 典型序列：`Start → (Chunk | ReasoningChunk | ToolCalls)* → End`，
/// 异常时以 `Error` 结束。
///
/// 流中传输中断且自动重试时，序列里会出现一次 `Restart`：**它之前**投递的
/// `Chunk` / `ReasoningChunk` 全部作废（消费端必须丢弃已累积的半截内容），
/// 之后是新一轮请求的完整事件序列。
#[derive(Debug, Clone)]
pub enum LlmEvent {
    Start,
    Chunk(Chunk),
    ReasoningChunk(Chunk),
    ToolCalls(Vec<ToolCall>),
    /// 流中传输中断后自动重开：消费端须丢弃本轮此前累积的正文/思考/工具调用。
    Restart,
    End(EndInfo),
    Error(String),
}

#[cfg(test)]
mod usage_tests {
    use super::TokenUsage;

    #[test]
    fn delta_and_zero() {
        let prev = TokenUsage::new(100, 40);
        let now = TokenUsage { prompt: 150, completion: 60, total: 210, cache_hit: 5, cache_miss: 7 };
        let d = now.delta_from(&prev);
        assert_eq!((d.prompt, d.completion, d.total), (50, 20, 70));
        assert!(!d.is_zero());
        // 未增长 → 零增量
        assert!(now.delta_from(&now).is_zero());
        // 回退（异常）饱和到 0，不 panic
        assert!(prev.delta_from(&now).is_zero());
    }
}
