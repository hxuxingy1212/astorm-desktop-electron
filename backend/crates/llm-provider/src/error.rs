//! LLM 提供方错误：带错误码，便于前端展示与重试策略判断。

/// LLM 调用错误。
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("[E001] Timeout")]
    Timeout,
    #[error("[E002] Connection refused: {0}")]
    ConnectionRefused(String),
    #[error("[E003] Connection lost: {0}")]
    ConnectionLost(String),
    #[error("[E004] Request failed: {0}")]
    RequestFailed(String),
    #[error("[E005] Stream error: {0}")]
    StreamError(String),
    #[error("[E006] Parse error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("[E007] Server error: {0}")]
    ServerError(String),
    #[error("[E008] Cancelled")]
    Cancelled,
    /// 熔断器打开：供应商连续失败触发熔断后抛出的自定义异常
    #[error("[E009] Circuit breaker open: {0}")]
    CircuitOpen(String),
    /// 限流拒绝：令牌不足且等待超时
    #[error("[E010] Rate limit exceeded: {0}")]
    RateLimited(String),
    /// 结构化输出失败（JSON 解析失败 / 不符合声明的 JSON Schema）
    #[error("[E011] Structured output failed: {0}")]
    StructuredError(String),
    #[error("[E999] {0}")]
    Other(String),
}

impl LlmError {
    /// 错误码（E001 ~ E011 / E999）。
    pub fn code(&self) -> &str {
        match self {
            Self::Timeout => "E001",
            Self::ConnectionRefused(_) => "E002",
            Self::ConnectionLost(_) => "E003",
            Self::RequestFailed(_) => "E004",
            Self::StreamError(_) => "E005",
            Self::ParseError(_) => "E006",
            Self::ServerError(_) => "E007",
            Self::Cancelled => "E008",
            Self::CircuitOpen(_) => "E009",
            Self::RateLimited(_) => "E010",
            Self::StructuredError(_) => "E011",
            Self::Other(_) => "E999",
        }
    }

    /// 错误类型名（与变体名一致）。
    pub fn type_name(&self) -> &str {
        match self {
            Self::Timeout => "Timeout",
            Self::ConnectionRefused(_) => "ConnectionRefused",
            Self::ConnectionLost(_) => "ConnectionLost",
            Self::RequestFailed(_) => "RequestFailed",
            Self::StreamError(_) => "StreamError",
            Self::ParseError(_) => "ParseError",
            Self::ServerError(_) => "ServerError",
            Self::Cancelled => "Cancelled",
            Self::CircuitOpen(_) => "CircuitOpen",
            Self::RateLimited(_) => "RateLimited",
            Self::StructuredError(_) => "StructuredError",
            Self::Other(_) => "Other",
        }
    }

    /// 是否可恢复（可安全重试）。
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Cancelled | Self::CircuitOpen(_) | Self::RateLimited(_)
        )
    }
}

/// 展开错误的 source 链（至多 3 层）供错误富化：reqwest 的 Display 只展示 kind 文案
/// （如 `error decoding response body`），真实原因（如 `invalid gzip header` /
/// `unexpected end of file`）在 source 链里，漏掉会让「只看 UI 错误」的人无从定位。
pub(crate) fn source_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = Vec::new();
    let mut cur = e.source();
    for _ in 0..3 {
        let Some(s) = cur else { break };
        parts.push(s.to_string());
        cur = s.source();
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("，底层原因: {}", parts.join(" → "))
    }
}

/// SSE 流响应元数据：读循环错误富化所需的上下文，在消费响应体之前从响应头捕获后随读循环携带。
///
/// 富化的动机：偶发的流式解码失败（如边缘节点对 SSE 的 gzip 压缩在流收尾/取消时截断，
/// reqwest 报 `Kind::Decode`；或传输中途被 RST）若只转发 `e.to_string()`，UI 与日志均无
/// URL / 编码头 / 流进度 / 协议版本可查，只能盲猜网关；带上后下次出现即自解释。
#[derive(Debug, Clone)]
pub(crate) struct SseMeta {
    /// 请求完整 URL（/chat/completions 或 /responses）
    pub url: String,
    /// `Content-Encoding` 头原文（空 = 服务端未声明压缩）
    pub content_encoding: String,
    /// `Content-Type` 头原文
    pub content_type: String,
    /// `Content-Length` 头原文（分块 SSE 通常为空 → 显示 (无)）
    pub content_length: String,
    /// 实际协商到的 HTTP 版本（`HTTP/2.0` / `HTTP/1.1`）——
    /// 区分「h2 连接被 RST」与「h1 边缘节点回收连接」的关键证据
    pub http_version: String,
    /// 本连接的起算点（拿响应头时）——elapsed 反映流已存活多久
    pub started: std::time::Instant,
}

impl SseMeta {
    /// 从响应头捕获元数据（必须在读取/消费 body 之前调用）。
    pub(crate) fn from_response(url: &str, resp: &reqwest::Response) -> Self {
        let header = |name: reqwest::header::HeaderName| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        Self {
            url: url.to_string(),
            content_encoding: header(reqwest::header::CONTENT_ENCODING),
            content_type: header(reqwest::header::CONTENT_TYPE),
            content_length: header(reqwest::header::CONTENT_LENGTH),
            http_version: format!("{:?}", resp.version()),
            started: std::time::Instant::now(),
        }
    }

    /// 富化流传输错误：reqwest 原文 + source 链 + URL / 协议版本 / 存活时长 / 编码头 / 流进度。
    ///
    /// `received` 是**下限**：触发错误的那一段字节没能交付，未计入。
    pub(crate) fn describe_transport_err(
        &self,
        e: &reqwest::Error,
        received: usize,
        sse_lines: usize,
    ) -> String {
        format!(
            "{}{}（url={}，http_version={}，elapsed={}s，received={}B，sse_lines={}，\
             content-encoding={}，content-type={}，content-length={}）",
            e,
            source_chain(e),
            self.url,
            self.http_version,
            self.started.elapsed().as_secs(),
            received,
            sse_lines,
            Self::na(&self.content_encoding),
            Self::na(&self.content_type),
            Self::na(&self.content_length),
        )
    }

    /// 空流诊断文案：收到字节但无任何 `data:` 报文（被网关强制压缩后的乱码 /
    /// HTML 错误页等），旧行为会静默产出空回复，这里转为显式错误。
    pub(crate) fn describe_empty_stream(&self, received: usize, head: &[u8]) -> String {
        let preview: String = String::from_utf8_lossy(head).escape_debug().take(200).collect();
        format!(
            "响应体无任何有效 SSE 报文（received={received}B，url={}，content-encoding={}，content-type={}）；\
             疑似被网关压缩或返回了非 SSE 错误页。原始头部预览: {preview:?}",
            self.url,
            Self::na(&self.content_encoding),
            Self::na(&self.content_type),
        )
    }

    /// 空字符串显示为 (无)，避免错误文案里出现空字段。
    fn na(v: &str) -> &str {
        if v.is_empty() {
            "(无)"
        } else {
            v
        }
    }
}
