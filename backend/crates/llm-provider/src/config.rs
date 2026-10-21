//! ChatClient 客户端参数：http 客户端 / 超时 / 重试 / 限流 / 熔断等可定制项。

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 供应商 API 协议（wire 格式）。
///
/// - `ChatCompletions`：OpenAI 兼容 `/chat/completions`（默认，全供应商兼容）；
/// - `Responses`：OpenAI `/responses`（语义化事件，正文/推理分开）；
/// - `AnthropicMessages`：Anthropic `/v1/messages`（Claude 原生协议）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

/// ChatClient 客户端参数：一次定义超时 / 重试 / 限流 / 熔断等全部可定制项。
///
/// 通过 [`ChatClient::with_option`] 传入；默认值见 [`ClientOption::default`]。
#[derive(Debug, Clone)]
pub struct ClientOption {
    /// 自定义 http 客户端（连接池 / 代理 / TLS 配置；None = 使用默认客户端）
    pub http: Option<reqwest::Client>,
    /// 单次请求超时（默认 60s）
    pub timeout: Duration,
    /// 可恢复网络错误（连接失败 / 超时）的最大重试次数（默认 3；0 = 不重试）
    pub max_retries: u32,
    /// 重试退避基数（秒，线性退避：1x / 2x / 3x ...）
    pub retry_backoff_secs: u64,
    /// 限流配置（None = 不限流）
    pub rate_limit: Option<RateLimitConfig>,
    /// 熔断配置
    pub circuit: CircuitConfig,
    /// 结构化输出配置（解析/校验失败自动修正重试）
    pub structured: StructuredConfig,
    /// 供应商 API 协议（默认 `/chat/completions`）。
    pub protocol: Protocol,
}

impl Default for ClientOption {
    fn default() -> Self {
        Self {
            http: None,
            timeout: Duration::from_secs(60),
            max_retries: 3,
            retry_backoff_secs: 1,
            rate_limit: None,
            circuit: CircuitConfig::default(),
            structured: StructuredConfig::default(),
            protocol: Protocol::default(),
        }
    }
}

impl ClientOption {
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定供应商 API 协议。
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// 启用 OpenAI Responses API（默认关闭，走 /chat/completions）。
    pub fn with_responses_api(mut self) -> Self {
        self.protocol = Protocol::Responses;
        self
    }

    /// 注入自定义 http 客户端（连接池 / 代理 / TLS 配置）。
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// 设置单次请求超时（默认 60s）。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 设置可恢复网络错误的最大重试次数（0 = 不重试）。
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// 设置重试退避基数（秒，线性退避：1x / 2x / 3x ...）。
    pub fn with_retry_backoff_secs(mut self, secs: u64) -> Self {
        self.retry_backoff_secs = secs;
        self
    }

    /// 启用限流（令牌桶）。
    pub fn with_rate_limit(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit = Some(config);
        self
    }

    /// 覆盖熔断配置（默认：连续失败 5 次熔断 30s，半开探测 1 个）。
    pub fn with_circuit(mut self, config: CircuitConfig) -> Self {
        self.circuit = config;
        self
    }

    /// 覆盖结构化输出配置（默认：解析/校验失败自动修正重试，最多 2 次）。
    pub fn with_structured(mut self, config: StructuredConfig) -> Self {
        self.structured = config;
        self
    }
}

/// 结构化输出配置：解析/校验失败时自动修正重试的次数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuredConfig {
    /// 最多调用次数（含首次；默认 2，1 = 不重试）
    pub max_attempts: u32,
}

impl Default for StructuredConfig {
    fn default() -> Self {
        Self { max_attempts: 2 }
    }
}

impl StructuredConfig {
    pub fn new(max_attempts: u32) -> Self {
        Self { max_attempts }
    }
}

/// 限流配置（令牌桶）：`rps` 平滑限速 + `burst` 突发容量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitConfig {
    /// 每秒补充令牌数（请求速率上限）
    pub rps: f64,
    /// 桶容量（允许的突发请求数）
    pub burst: u32,
}

impl RateLimitConfig {
    pub fn new(rps: f64, burst: u32) -> Self {
        Self { rps, burst }
    }
}

/// 熔断配置：连续失败达到阈值后熔断，冷却后进入半开探测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitConfig {
    /// 连续失败达到该次数后熔断打开
    pub failure_threshold: u32,
    /// 熔断打开后的冷却时长，冷却结束进入半开
    pub open_duration: Duration,
    /// 半开状态下最多放行的探测请求数
    pub half_open_max: u32,
}

impl Default for CircuitConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            open_duration: Duration::from_secs(30),
            half_open_max: 1,
        }
    }
}
