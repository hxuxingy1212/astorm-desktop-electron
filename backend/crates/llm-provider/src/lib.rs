//! LLM 提供方层：消息类型、事件、错误与具体实现。
//!
//! - [`ChatMessage`] 定义消息体、[`LlmEvent`] 定义流式事件、[`LlmError`] 定义错误码。
//! - [`ChatClient`] 是主体 API（OpenAI 兼容 SSE 流式 HTTP 客户端），引擎直接持有它；
//!   内置供应商限流（[`TokenBucket`]）与熔断（[`CircuitBreaker`]）。

/// 熔断器：[`CircuitBreaker`] 状态机（Closed → Open → HalfOpen），
/// 连续失败自动熔断、冷却后半开探测，熔断打开抛 [`LlmError::CircuitOpen`]。
pub mod circuit;

/// ChatClient 主体 API：OpenAI 兼容 SSE 流式 HTTP 客户端，
/// 由 base_url + api_key 两元素定义供应商，模型名在每次 chat_stream 调用时指定；
/// 内置 30ms 累积冲刷（think/content 批量转发，tool/usage 完整后投递）。
pub mod client;

/// ChatClient 客户端参数：[`ClientOption`] 一次定义 http 客户端 / 超时 / 重试 / 限流 / 熔断 / 结构化输出。
pub mod config;

/// LLM 错误体系：[`LlmError`] 错误码（E001~E010 / E999）+ code / type_name / is_recoverable。
pub mod error;

/// 流式事件：[`LlmEvent`] / [`Chunk`] / [`EndInfo`] / [`TokenUsage`]。
pub mod event;

/// 令牌桶限流：[`TokenBucket`]，按 rps 平滑限速 + burst 突发容量，等待超时抛 [`LlmError::RateLimited`]。
pub mod rate_limit;

/// 请求与消息类型：[`ChatMessage`] 消息体 / [`ToolCall`] / [`ToolDef`] / [`ChatRequest`]。
pub mod request;

/// OpenAI Responses API 支持：语义化流式事件（正文/推理分开、工具调用显式），
/// chat_stream 优先走该通道，端点未实现(404/405/501)时回退 /chat/completions。
pub mod responses;

/// Anthropic Messages API 支持（Claude 原生 `/v1/messages`）：请求体构造 + SSE 事件解析。
pub mod anthropic;

/// 结构化输出：[`StructuredOutput`] 结果类型与解析校验辅助，JSON Schema 驱动 + 失败修正重试。
pub mod structured;

pub use circuit::{CircuitBreaker, CircuitState};
pub use client::ChatClient;
pub use config::{CircuitConfig, ClientOption, Protocol, RateLimitConfig, StructuredConfig};
pub use error::LlmError;
pub use event::{Chunk, EndInfo, LlmEvent, TokenUsage};
pub use rate_limit::TokenBucket;
pub use request::{
    ChatMessage, ChatRequest, ContentPart, FunctionCall, FunctionDef, ImageUrlContent, ToolCall,
    ToolDef,
};
pub use structured::StructuredOutput;

use futures::Stream;
use std::pin::Pin;

/// 流式 LLM 响应。
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>>;
