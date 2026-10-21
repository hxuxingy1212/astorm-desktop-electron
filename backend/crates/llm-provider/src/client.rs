//! ChatClient：OpenAI 兼容 SSE 流式 HTTP 客户端（主体 API）。
//!
//! 调用链：**熔断闸门** → **限流闸门** → HTTP 请求 → SSE 报文流式转发 → [`LlmEvent`] 投递。
//!
//! 报文处理遵循 OpenAI 标准 SSE 格式（`data: {json}` 行 + `data: [DONE]` 结束哨兵），
//! 每个报文即时解析并投递，不做私有协议（NDJSON）解析；流式 tool_calls 增量按
//! index 聚合（SSE 协议的标准处理），聚合完成后一次性投递完整 [`LlmEvent::ToolCalls`]。

use crate::circuit::{CircuitBreaker, CircuitState};
use crate::error::SseMeta;
use crate::rate_limit::TokenBucket;
use crate::structured::{correction_message, inject_structured_prompt, parse_and_validate};
use crate::{
    ChatRequest, Chunk, ClientOption, EndInfo, LlmError, LlmEvent, LlmStream, Protocol,
    StructuredOutput, TokenUsage, ToolCall,
};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

/// 文本/思考链累积冲刷间隔：高频小片段按 30ms 批量合并后转发，
/// 减少下游（引擎 → hook → UI）的事件数量；tool_calls 与 usage 始终完整解析后才投递。
const FLUSH_INTERVAL: Duration = Duration::from_millis(30);

/// SSE 首包等待超时：`chat_stream` 在返回前先健康检查首包（见 [`ChatClient::open_sse`]）。
/// 与 react-loop 引擎的 `LLM_STREAM_IDLE_TIMEOUT_SECS = 120` 看门狗对齐——引擎的看门狗
/// 只覆盖「拿到流之后」，首包前的停滞必须由本层兜底，避免客户端永久挂起。
const SSE_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(120);

/// 首包失败后的整体重试次数：0 字节到达（解码失败 / 连接在首包前断开）时无内容丢失，
/// 整体重发一次安全且能自愈偶发故障。
const STREAM_OPEN_RETRY_TIMES: u32 = 1;

/// **流中途**传输中断后的自动重试次数（收到过字节、已投递部分正文之后的失败）。
///
/// 为什么也重试：流式请求是无状态的（每轮整体重发 messages），重开一次与新发一条请求
/// 等价，唯一的代价是重算已生成的那部分 token。线上实测（会话 2026_09_14_00_22_47 之后
/// 的一次长思考）出现过收到 370KB / 1222 条 SSE 报文后被对端/中间设备 RST 的情况，
/// 旧实现直接作废整轮、用户白等数分钟。
///
/// 重试前必须投递 [`LlmEvent::Restart`] 让消费端丢弃已累积的半截内容，否则两次尝试的
/// 正文会被拼接成乱码。预算与首包重试独立计数：两类失败的恢复代价不同。
const STREAM_MID_RETRY_TIMES: u32 = 1;

/// 响应体读取的停滞上限（reqwest `read_timeout`：每次成功读取后重置）。
///
/// 取 180s **大于**引擎的空闲看门狗（`LLM_STREAM_IDLE_TIMEOUT_SECS = 120`）：看门狗语义
/// 更贴近业务（它能看到 SSE 事件粒度），让它先给出「无数据超时」；本项只兜住传输层
/// 「连接还在但永不吐字节」的情况，避免会话永久挂起。
const SSE_READ_TIMEOUT: Duration = Duration::from_secs(180);

/// 空流诊断的原始头部预览长度（首 ~96 字节，用于识别乱码 / 错误页）。
pub(crate) const HEAD_PREVIEW_BYTES: usize = 96;

/// SSE 响应体字节流（读循环消费）。
type ByteStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;

/// SSE 读循环的收尾结果：由调用方（[`ChatClient::drive_sse`]）决定「重试 / 上报 / 静默结束」。
///
/// 约定：读循环自己投递的错误（取消、空流诊断）直接返回 [`SseOutcome::Done`]；
/// 只有**流中传输中断**交由调用方处置，因为要不要重试是它的决定。
pub(crate) enum SseOutcome {
    /// 正常收尾（`[DONE]` / EOF / 已投递 End 或 Cancelled 或空流诊断错误）
    Done,
    /// 流中传输中断：**未**投递任何错误事件，诊断文本交调用方决定重试或上报
    Transport(String),
    /// 接收端已关闭（引擎超时/取消后 drop rx），无需再投递任何事件
    Abandoned,
}

/// 读循环种类（各端点的报文格式不同，重试驱动逻辑相同）。
#[derive(Clone, Copy)]
pub(crate) enum SseKind {
    Chat,
    Responses,
    Anthropic,
}

/// 单个 SSE 流打开结果：元数据 + 首个非空 chunk（喂给读循环作为种子，不丢首包）+ 余下字节流。
struct SseOpen {
    meta: SseMeta,
    first: bytes::Bytes,
    byte_stream: ByteStream,
}

/// 主体 API：OpenAI 兼容 SSE 流式 HTTP 客户端。
///
/// 由 `base_url` + `api_key` 两元素定义供应商，每次调用通过 `chat_stream(model, ...)`
/// 指定模型名称。参数（超时 / 重试 / 限流 / 熔断等）通过 [`ClientOption`] 定制。
/// 引擎（[`crate::ReActLoop`]）直接持有 [`ChatClient`]；
/// 内置供应商限流（令牌桶）与熔断（连续失败自动熔断，熔断打开抛 [`LlmError::CircuitOpen`]）。
#[derive(Clone)]
pub struct ChatClient {
    http: reqwest::Client,
    /// OpenAI 兼容 API 地址
    base_url: String,
    /// Bearer 密钥
    api_key: String,
    /// 客户端参数（超时 / 重试 / 限流 / 熔断）
    option: ClientOption,
    /// 限流器（None = 不限流；Arc 共享，支持克隆）
    limiter: Option<Arc<TokenBucket>>,
    /// 熔断器
    breaker: Arc<CircuitBreaker>,
}

/// 默认 HTTP 客户端：面向**长连接流式**场景显式调参（裸 `Client::default()` 在流式下
/// 等于「无传输层保护」：没有 keepalive、没有读停滞上限，连接被中间设备静默回收时只能等
/// 对端 RST）。
///
/// - `tcp_keepalive` / `http2_keep_alive_*`：定时心跳，避免 NAT / 边缘网关按「空闲」回收
///   ——流式中途被 RST（`connection reset by peer`）最常见的就是这一类；
/// - `read_timeout`：响应体读取停滞上限（见 [`SSE_READ_TIMEOUT`]）；
/// - `pool_idle_timeout`：保持 reqwest 默认 90s，空闲连接池不长期持有被回收的连接。
fn default_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .tcp_keepalive(Duration::from_secs(15))
        .http2_keep_alive_interval(Duration::from_secs(15))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        .read_timeout(SSE_READ_TIMEOUT)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_default()
}

impl ChatClient {
    /// 创建客户端：base_url + api_key 两元素，使用默认参数（见 [`ClientOption::default`]）。
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_option(base_url, api_key, ClientOption::default())
    }

    /// 创建客户端并定制参数：http 客户端 / 超时 / 重试 / 限流 / 熔断（见 [`ClientOption`]）。
    pub fn with_option(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        option: ClientOption,
    ) -> Self {
        let base_url = base_url.into();
        let http = option.http.clone().unwrap_or_else(default_http_client);
        let limiter = option.rate_limit.map(|cfg| Arc::new(TokenBucket::new(cfg)));
        let breaker = Arc::new(CircuitBreaker::new(base_url.clone(), option.circuit));
        Self { http, base_url, api_key: api_key.into(), option, limiter, breaker }
    }

    /// 熔断器状态（观测用）。
    pub fn breaker_state(&self) -> CircuitState {
        self.breaker.state()
    }

    /// 按协议注入鉴权与 SSE 相关请求头：
    /// - OpenAI 系（chat / responses）：`Authorization: Bearer`
    /// - Anthropic Messages：`x-api-key` + `anthropic-version`
    fn apply_headers(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // 显式声明按 SSE 处理：部分网关据此走「不缓冲、不压缩」的转发路径
        let req = req
            .header("content-type", "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        match self.option.protocol {
            Protocol::AnthropicMessages => req
                .header("x-api-key", self.api_key.clone())
                .header("anthropic-version", "2023-06-01"),
            _ => req.header("authorization", format!("Bearer {}", self.api_key)),
        }
    }

    /// 发起流式对话请求（仅流式，无非流式接口）。
    ///
    /// `model` 为本次调用指定的模型名称（如 `deepseek-chat`）。
    /// 调用链：熔断闸门 → 限流闸门 → HTTP 请求 → SSE 报文流式转发。
    pub async fn chat_stream(
        &self,
        model: &str,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<LlmStream, LlmError> {
        // 1) 熔断闸门：熔断打开直接抛 E009 CircuitOpen（不发请求）
        self.breaker.allow()?;

        // 2) 限流闸门：令牌不足时等待，总等待超时抛 E010 RateLimited
        if let Some(limiter) = &self.limiter {
            limiter.acquire(&cancel).await?;
        }

        let base = self.base_url.trim_end_matches('/').to_string();

        // 3) Anthropic Messages（Claude 原生 /v1/messages）。
        if self.option.protocol == Protocol::AnthropicMessages {
            let url = anthropic_url(&base);
            let body = crate::anthropic::anthropic_request_body(&request, model);
            let SseOpen { meta, first, byte_stream } = match self.open_sse(&url, &body, &cancel).await {
                Ok(opened) => opened,
                Err(LlmError::Cancelled) => return Ok(cancelled_stream()),
                Err(e) => return Err(e),
            };
            let (tx, rx) = mpsc::unbounded_channel::<Result<LlmEvent, LlmError>>();
            let client = self.clone();
            let opened = SseOpen { meta, first, byte_stream };
            tokio::spawn(async move {
                client.drive_sse(SseKind::Anthropic, &url, &body, tx, cancel, opened).await;
            });
            return Ok(Box::pin(ChatStream { rx }));
        }

        // 4) 显式选择 Responses API：优先 /responses（语义化事件，正文/推理分开）；
        //    其任一失败（非成功状态 / 连接错误 / 重试耗尽）记录日志并回退 /chat/completions。
        if self.option.protocol == Protocol::Responses {
            let responses_url = format!("{base}/responses");
            let responses_body = crate::responses::responses_request_body(&request, model);
            match self.open_sse(&responses_url, &responses_body, &cancel).await {
                Ok(opened) => {
                    let (tx, rx) = mpsc::unbounded_channel::<Result<LlmEvent, LlmError>>();
                    let client = self.clone();
                    tokio::spawn(async move {
                        client
                            .drive_sse(
                                SseKind::Responses,
                                &responses_url,
                                &responses_body,
                                tx,
                                cancel,
                                opened,
                            )
                            .await;
                    });
                    return Ok(Box::pin(ChatStream { rx }));
                }
                // 等待首包期间被取消：按「已取消」语义返回（引擎依 ctx.cancel 收尾，
                // 不当错误上报），不回退旧通道重复发请求
                Err(LlmError::Cancelled) => return Ok(cancelled_stream()),
                Err(resp_err) => {
                    warn!("[ChatClient] /responses 不可用({resp_err})，回退 /chat/completions");
                }
            }
        }

        // 5) /chat/completions：构造请求 + 重试发送 + 首包健康检查 + 标准 SSE 转发。
        let url = format!("{base}/chat/completions");
        let mut body = serde_json::to_value(&request).map_err(LlmError::ParseError)?;
        body["model"] = serde_json::Value::String(model.to_string());
        body["stream"] = serde_json::Value::Bool(true);
        let SseOpen { meta, first, byte_stream } = match self.open_sse(&url, &body, &cancel).await {
            Ok(opened) => opened,
            Err(LlmError::Cancelled) => return Ok(cancelled_stream()),
            Err(e) => return Err(e),
        };
        let (tx, rx) = mpsc::unbounded_channel::<Result<LlmEvent, LlmError>>();
        let client = self.clone();
        let opened = SseOpen { meta, first, byte_stream };
        tokio::spawn(async move {
            client.drive_sse(SseKind::Chat, &url, &body, tx, cancel, opened).await;
        });
        Ok(Box::pin(ChatStream { rx }))
    }

    /// 驱动一条 SSE 流：读循环 + **流中途**传输中断的自动重试（[`STREAM_MID_RETRY_TIMES`]）。
    ///
    /// `opened` 是调用方在返回 [`LlmStream`] 之前已完成首包健康检查的那条连接（第 0 次尝试）；
    /// 后续每次重试都重新走 [`ChatClient::open_sse`]（其内部含自己的首包重试预算）。
    ///
    /// 重试前先投递 [`LlmEvent::Restart`]：消费端据此丢弃已累积的半截正文/思考，
    /// 且这次投递同时给引擎的空闲看门狗续命——退避 + 重开请求期间的「无事件窗口」
    /// 否则会被判成「LLM 流无数据超时」。
    async fn drive_sse(
        self,
        kind: SseKind,
        url: &str,
        body: &serde_json::Value,
        tx: mpsc::UnboundedSender<Result<LlmEvent, LlmError>>,
        cancel: CancellationToken,
        opened: SseOpen,
    ) {
        let mut next = Some(opened);
        for attempt in 0..=STREAM_MID_RETRY_TIMES {
            let opened = match next.take() {
                Some(o) => o,
                None => match self.open_sse(url, body, &cancel).await {
                    Ok(o) => o,
                    Err(e) => {
                        // 取消按「已取消」语义上报（引擎据 ctx.cancel 收尾，不当错误展示）
                        let _ = tx.send(Err(if cancel.is_cancelled() {
                            LlmError::Cancelled
                        } else {
                            e
                        }));
                        return;
                    }
                },
            };
            let SseOpen { meta, first, byte_stream } = opened;
            let outcome = match kind {
                SseKind::Chat => {
                    read_sse_loop(byte_stream, tx.clone(), self.breaker.clone(), &cancel, meta, first)
                        .await
                }
                SseKind::Responses => {
                    crate::responses::read_sse_responses_loop(
                        byte_stream,
                        tx.clone(),
                        self.breaker.clone(),
                        &cancel,
                        meta,
                        first,
                    )
                    .await
                }
                SseKind::Anthropic => {
                    crate::anthropic::read_sse_anthropic_loop(
                        byte_stream,
                        tx.clone(),
                        self.breaker.clone(),
                        &cancel,
                        meta,
                        first,
                    )
                    .await
                }
            };
            // 只有「流中传输中断」需要在这里决策：正常收尾与接收端已关闭都直接结束
            let SseOutcome::Transport(detail) = outcome else {
                return;
            };
            if cancel.is_cancelled() {
                let _ = tx.send(Err(LlmError::Cancelled));
                return;
            }
            if attempt >= STREAM_MID_RETRY_TIMES {
                error!("[ChatClient] SSE 流中传输中断，重试已耗尽：{detail}");
                self.breaker.on_failure();
                let retried = if attempt > 0 {
                    format!("，已自动重试 {attempt} 次")
                } else {
                    String::new()
                };
                let _ = tx.send(Err(LlmError::StreamError(format!("{detail}{retried}"))));
                return;
            }
            warn!(
                "[ChatClient] SSE 流中传输中断（{detail}），自动重试 ({}/{})",
                attempt + 1,
                STREAM_MID_RETRY_TIMES
            );
            if tx.send(Ok(LlmEvent::Restart)).is_err() {
                return; // 接收端已丢弃（引擎超时/取消）
            }
            if let Err(e) = self.sleep_before_retry(attempt, &cancel).await {
                let _ = tx.send(Err(e));
                return;
            }
        }
    }

    /// 打开一条 SSE 字节流：`send_with_retry` 成功 → 首包健康检查（首个非空 chunk），
    /// 首包前失败（传输错误 / 空响应体）时整体重发一次。
    ///
    /// **为何要等首包**：流式响应体解码失败（reqwest `Kind::Decode`，如边缘对 SSE 的 gzip
    /// 压缩在收尾时截断）或连接在首包前断开，会让整轮对话失败；此时 0 字节到达、没有
    /// 内容丢失，重发请求安全且能自愈偶发故障。返回的 `first` 作为读循环种子，保证首包不丢。
    ///
    /// 收到首包后的 mid-stream 失败由 [`ChatClient::drive_sse`] 处理：同样重开重发一次
    /// （先投递 [`LlmEvent::Restart`] 让消费端丢弃半截内容），重试耗尽才上报富化错误。
    async fn open_sse(
        &self,
        url: &str,
        body: &serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<SseOpen, LlmError> {
        'open: for attempt in 0..=STREAM_OPEN_RETRY_TIMES {
            let resp = self.send_with_retry(url, body, cancel).await?;
            if !resp.status().is_success() {
                // send_with_retry 仅对「端点未实现(404/405/501)」原样返回非成功响应；
                // 该响应体不是 SSE，直接报错（/responses 分支据此回退旧通道）
                let status = resp.status();
                let err_body = resp.text().await.unwrap_or_default();
                return Err(LlmError::ServerError(format!(
                    "HTTP {}: {}",
                    status.as_u16(),
                    friendly_api_error(&err_body)
                )));
            }
            let meta = SseMeta::from_response(url, &resp);
            let mut byte_stream = resp.bytes_stream();
            let first = 'peek: loop {
                let item = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                    item = tokio::time::timeout(SSE_FIRST_BYTE_TIMEOUT, byte_stream.next()) => item,
                };
                match item {
                    // 首包等待超时：供应商连接成功但迟迟不吐字节（与引擎空闲看门狗同语义）
                    Err(_) => return Err(LlmError::Timeout),
                    Ok(Some(Ok(bytes))) if !bytes.is_empty() => break 'peek bytes,
                    Ok(Some(Ok(_))) => {} // 空 chunk（罕见），继续等待
                    Ok(Some(Err(e))) => {
                        self.breaker.on_failure();
                        let msg = meta.describe_transport_err(&e, 0, 0);
                        if attempt == STREAM_OPEN_RETRY_TIMES {
                            return Err(LlmError::StreamError(msg));
                        }
                        warn!(
                            "[ChatClient] SSE 首包传输失败（{msg}），整体重试 ({}/{})",
                            attempt + 1,
                            STREAM_OPEN_RETRY_TIMES
                        );
                        self.sleep_before_retry(attempt, cancel).await?;
                        continue 'open;
                    }
                    Ok(None) => {
                        // 空响应体（HTTP 200 但 0 字节 EOF）：整体重试一次后报错
                        if attempt == STREAM_OPEN_RETRY_TIMES {
                            return Err(LlmError::StreamError(
                                meta.describe_empty_stream(0, &[]),
                            ));
                        }
                        warn!(
                            "[ChatClient] SSE 响应体为空，整体重试 ({}/{})",
                            attempt + 1,
                            STREAM_OPEN_RETRY_TIMES
                        );
                        self.sleep_before_retry(attempt, cancel).await?;
                        continue 'open;
                    }
                }
            };
            return Ok(SseOpen { meta, first, byte_stream: Box::pin(byte_stream) });
        }
        unreachable!("重试循环内必返回")
    }

    /// 整体重试前的线性退避（受取消中断与 [`MAX_RETRY_BACKOFF`] 上限约束）。
    async fn sleep_before_retry(
        &self,
        attempt: u32,
        cancel: &CancellationToken,
    ) -> Result<(), LlmError> {
        let backoff = Duration::from_secs(self.option.retry_backoff_secs * (attempt as u64 + 1))
            .min(MAX_RETRY_BACKOFF);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(LlmError::Cancelled),
            _ = tokio::time::sleep(backoff) => Ok(()),
        }
    }

    /// 发送一次流式请求并处理可恢复错误的重试。
    ///
    /// 可恢复错误 = 网络错误（send 失败 / 超时）+ HTTP 429 / 5xx（按 max_retries 线性退避）；
    /// 429 优先读取 Retry-After 响应头决定退避时长；端点未实现(404/405/501)原样返回
    /// 响应（body 未消费），由调用方决定是否回退。
    ///
    /// 429 的退避受 [`MAX_RETRY_BACKOFF`] 限制：用量/额度类限制(如 opencode weekly limit)
    /// 常返回天级的 Retry-After，重试没有意义，直接报错交由上层提示，避免会话挂起数日。
    async fn send_with_retry(
        &self,
        url: &str,
        body: &serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, LlmError> {
        let mut response = None;
        let mut backoff = Duration::ZERO;
        for attempt in 0..=self.option.max_retries {
            if attempt > 0 {
                warn!(
                    "[ChatClient] 请求失败，{:?} 后重试 ({}/{})",
                    backoff, attempt, self.option.max_retries,
                );
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                    _ = tokio::time::sleep(backoff) => {}
                }
            }
            // 流式请求显式声明不接受压缩编码：SSE 是逐帧推送的长连接，边缘节点/网关
            // 对它的 gzip 压缩会在流收尾或连接取消时截断，造成偶发「响应体解码失败」
            // （reqwest Kind::Decode → E005）。identity 让合规节点不做任何内容编码。
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                result = tokio::time::timeout(
                    self.option.timeout,
                    self.apply_headers(self.http.post(url))
                        .json(body)
                        .send(),
                ) => result,
            };
            let linear = (Duration::from_secs(self.option.retry_backoff_secs * attempt as u64))
                .min(MAX_RETRY_BACKOFF);
            match result {
                Ok(Ok(resp)) => {
                    let status = resp.status();
                    if status.is_success() {
                        response = Some(resp);
                        break;
                    }
                    // 端点未实现(404/405/501)：body 未消费，原样返回由调用方决定回退
                    if crate::responses::should_fallback(status) {
                        response = Some(resp);
                        break;
                    }
                    // 状态码错误：429 / 5xx 可重试（受 max_retries 约束），其余 4xx 直接返回
                    let status_code = status.as_u16();
                    let retry_after = if status_code == 429 {
                        resp.headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.trim().parse::<u64>().ok())
                    } else {
                        None
                    };
                    let err_body = resp.text().await.unwrap_or_default();
                    self.breaker.on_failure();
                    let err = if status_code == 429 {
                        LlmError::RateLimited(format!("HTTP 429: {}", friendly_api_error(&err_body)))
                    } else {
                        LlmError::ServerError(format!(
                            "HTTP {}: {}",
                            status_code,
                            friendly_api_error(&err_body)
                        ))
                    };
                    // 用量/额度限制(Retry-After 天级)或已到重试上限 → 不再重试
                    let retry_too_far = retry_after
                        .map(|secs| Duration::from_secs(secs) > MAX_RETRY_BACKOFF)
                        .unwrap_or(false);
                    if !is_retryable_status(status_code)
                        || attempt == self.option.max_retries
                        || retry_too_far
                    {
                        return Err(err);
                    }
                    // 429 优先使用 Retry-After 退避（受上限约束），否则线性退避
                    backoff = retry_after
                        .map(Duration::from_secs)
                        .map(|d| d.min(MAX_RETRY_BACKOFF))
                        .unwrap_or(linear);
                }
                Ok(Err(e)) => {
                    self.breaker.on_failure();
                    if attempt == self.option.max_retries {
                        return Err(LlmError::RequestFailed(e.to_string()));
                    }
                    backoff = linear;
                }
                Err(_) => {
                    self.breaker.on_failure();
                    if attempt == self.option.max_retries {
                        return Err(LlmError::Timeout);
                    }
                    backoff = linear;
                }
            }
        }
        Ok(response.expect("重试循环内必返回"))
    }

    /// 列出供应商可用模型。
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .get(&url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        if !response.status().is_success() {
            return Err(LlmError::ServerError(format!("HTTP {}", response.status().as_u16())));
        }
        let text = response
            .text()
            .await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(LlmError::ParseError)?;
        let models = json["data"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default();
        Ok(models)
    }

    /// 结构化输出：声明 JSON Schema → 模型按 schema 输出 → 解析 + 校验，
    /// 失败自动修正重试（次数由 [`ClientOption`] 的 `structured` 配置，默认最多 2 次）。
    ///
    /// 内部仍走流式调用（熔断 / 限流闸门照常生效），对外返回完整结构化结果。
    /// 自动注入 `response_format = json_object` 与含 schema 的指令消息；
    /// 校验失败时把错误列表喂回 LLM 重新生成。
    pub async fn chat_structured(
        &self,
        model: &str,
        request: ChatRequest,
        schema: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<StructuredOutput, LlmError> {
        let max_attempts = self.option.structured.max_attempts.max(1);
        // 编译 schema（校验错误可序列化，用于修正重试）
        let validator = jsonschema::validator_for(&schema)
            .map_err(|e| LlmError::StructuredError(format!("JSON Schema 编译失败: {}", e)))?;

        // 注入结构化指令：response_format = json_object + schema 指令消息
        let mut request = inject_structured_prompt(request, &schema);
        let mut total_usage: Option<TokenUsage> = None;
        let mut finish_reason = String::new();

        for attempt in 1..=max_attempts {
            // 流式收集完整文本
            let mut stream = self
                .chat_stream(model, request.clone(), cancel.child_token())
                .await?;
            let mut content = String::new();
            while let Some(event) = stream.next().await {
                match event? {
                    LlmEvent::Start => {}
                    LlmEvent::Chunk(c) => content.push_str(&c.content),
                    LlmEvent::ReasoningChunk(_) => {} // 结构化输出只关心最终文本
                    LlmEvent::ToolCalls(_) => {}      // 结构化模式不期望工具调用
                    // 流中传输中断后自动重开：此前累积的文本作废（否则两次尝试会拼在一起）
                    LlmEvent::Restart => content.clear(),
                    LlmEvent::End(e) => {
                        finish_reason = e.finish_reason;
                        if let Some(u) = e.usage {
                            if let Some(acc) = &mut total_usage {
                                acc.accumulate(&u);
                            } else {
                                total_usage = Some(u);
                            }
                        }
                        break;
                    }
                    LlmEvent::Error(msg) => return Err(LlmError::ServerError(msg)),
                }
            }

            // 提取 + 解析 + 校验
            match parse_and_validate(&content, &validator) {
                Ok(value) => {
                    return Ok(StructuredOutput {
                        value,
                        usage: total_usage,
                        finish_reason,
                        attempts: attempt,
                    });
                }
                Err(err_msg) => {
                    if attempt == max_attempts {
                        return Err(LlmError::StructuredError(format!(
                            "第 {} 次尝试失败：{}",
                            attempt, err_msg,
                        )));
                    }
                    // 未耗尽：把错误喂回 LLM，下一轮修正重试
                    warn!(
                        "[ChatClient] 结构化输出不达标（{}/{}）：{}",
                        attempt, max_attempts, err_msg,
                    );
                    request.messages.push(correction_message(&err_msg));
                }
            }
        }
        unreachable!("重试循环内必返回")
    }
}

/// 可重试的 HTTP 状态码：429（供应商限速，退避后重试）与 5xx（服务端瞬时错误）。
/// 其余 4xx（400/401/404 等请求错误）不重试——重试无意义。
fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// Anthropic Messages 端点 URL：base 已含 `/v1` 时用 `/messages`，否则补 `/v1/messages`
/// （兼容用户填 `https://api.anthropic.com` 或 `https://api.anthropic.com/v1`）。
fn anthropic_url(base: &str) -> String {
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

/// 重试退避上限：限制 429 Retry-After / 线性退避的最长等待，防止用量/额度类
/// 限制（天级 Retry-After）把会话挂起数日而不给用户任何反馈。
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(30);

/// 从供应商错误响应体提取人类可读消息（OpenAI 风格 `error.message`，或顶层 `message`）；
/// 非 JSON / 缺字段时原样返回原文。
fn friendly_api_error(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| extract_error_message(&v))
        .unwrap_or_else(|| body.to_string())
}

/// 从错误 JSON 里取 `error.message`（OpenAI 兼容）或顶层 `message` 字段文本。
fn extract_error_message(v: &serde_json::Value) -> Option<String> {
    v.get("error")
        .and_then(|e| e.get("message"))
        .or_else(|| v.get("message"))
        .and_then(|m| m.as_str())
        .map(String::from)
}

/// SSE 报文读取循环：按行切分报文（跨 chunk 缓冲），解析 `data:` 报文并投递事件。
///
/// 投递策略（累积 + 定时冲刷）：
/// - **think / content**：高频小片段累积到 30ms 缓冲，到期批量合并后转发
///   （一个流中可能多次交替出现，每次冲刷时思考链先于文本）
/// - **tool_calls**：增量按 index 聚合，完整解析后才一次性投递
/// - **usage**：随流结束的 `End` 事件一次投递
/// - 流中传输中断 → 先冲刷半截文本，再以 [`SseOutcome::Transport`] 把富化诊断交调用方
///   （由 [`ChatClient::drive_sse`] 决定自动重试或上报；本函数**不**投递错误、**不**记熔断）
/// - 收到字节却无任何有效报文 → 空流诊断错误（不再静默产出空回复）；正常结束记录熔断成功
///
/// `first` 为 [`ChatClient::open_sse`] 首包健康检查留下的首个 chunk（作为初始缓冲，不丢首包）；
/// `meta` 携带响应 url / 协议版本 / 编码头，供错误富化。
async fn read_sse_loop(
    mut byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    tx: mpsc::UnboundedSender<Result<LlmEvent, LlmError>>,
    breaker: Arc<CircuitBreaker>,
    cancel: &CancellationToken,
    meta: SseMeta,
    first: bytes::Bytes,
) -> SseOutcome {
    let mut buf = Vec::new();
    // 首包作为初始缓冲喂入：与「首包随网络到达」走同一解析路径，不丢数据
    buf.extend_from_slice(&first);
    let mut finish_reason = String::new();
    let mut usage: Option<TokenUsage> = None;
    // 流式 tool_calls 增量按 index 聚合（SSE 协议的标准处理）
    let mut tool_calls: HashMap<usize, AccumToolCall> = HashMap::new();
    // 30ms 累积缓冲：content / reasoning 高频片段批量合并后转发
    let mut pending_text = String::new();
    let mut pending_reasoning = String::new();
    // 流进度计数（错误富化 / 空流诊断用）：已收字节、有效 data: 报文数、原始头部预览
    let mut received_bytes = first.len();
    let mut sse_lines = 0usize;
    let mut raw_head: Vec<u8> = Vec::new();
    capture_head(&mut raw_head, &first);
    let mut flush = tokio::time::interval(FLUSH_INTERVAL);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    flush.tick().await; // 消费 interval 的首次立即 tick

    'outer: loop {
        if cancel.is_cancelled() {
            // 用户取消：冲刷半截文本后投递 Cancelled（非正常 End），引擎据此按
            // 「被取消」收尾；tool_calls 丢弃（取消后不会执行），熔断不计数
            if flush_pending(&tx, &mut pending_text, &mut pending_reasoning).is_err() {
                return SseOutcome::Abandoned;
            }
            let _ = tx.send(Err(LlmError::Cancelled));
            return SseOutcome::Done;
        }
        // 处理缓冲区内完整行（累积到 pending / tool_calls，不 await）
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if line.is_empty() {
                continue;
            }
            // 剥离 SSE data: 前缀
            let is_data_line = line.starts_with("data:");
            let payload = line.strip_prefix("data:").unwrap_or(&line).trim();
            if payload == "[DONE]" {
                // [DONE] 本身即一条有效 data: 报文，先计数再退出（空流诊断依赖此计数）
                if is_data_line {
                    sse_lines += 1;
                }
                break 'outer;
            }
            if payload.is_empty() {
                continue;
            }
            let chunk: SseChunk = match serde_json::from_str(payload) {
                Ok(c) => c,
                Err(_) => {
                    // data: 携带的非 JSON 报文（注释行 / 未知扩展）仍是有效 SSE 信封；
                    // 裸行（无 data: 前缀）且非 JSON 的不计数
                    if is_data_line {
                        sse_lines += 1;
                    }
                    continue;
                }
            };
            // 成功解析的 JSON 报文：有效 SSE 信封 +1
            sse_lines += 1;
            if let Some(u) = chunk.usage {
                usage = Some(u.into_token_usage());
            }
            if let Some(choice) = chunk.choices.first() {
                if let Some(content) = &choice.delta.content {
                    pending_text.push_str(content);
                }
                if let Some(reasoning) = &choice.delta.reasoning_content {
                    pending_reasoning.push_str(reasoning);
                }
                for delta in &choice.delta.tool_calls {
                    let acc = tool_calls.entry(delta.index.unwrap_or(0)).or_default();
                    if let Some(id) = &delta.id {
                        acc.id = id.clone();
                    }
                    if let Some(f) = &delta.function {
                        if let Some(name) = &f.name {
                            acc.name = name.clone();
                        }
                        if let Some(args) = &f.arguments {
                            acc.arguments.push_str(args);
                        }
                    }
                }
                if let Some(fr) = &choice.finish_reason {
                    finish_reason = fr.clone();
                }
            }
        }
        // 等待：30ms 冲刷定时器（biased 优先，避免被网络数据饿死）或网络数据
        tokio::select! {
            biased;
            _ = flush.tick() => {
                if flush_pending(&tx, &mut pending_text, &mut pending_reasoning).is_err() {
                    return SseOutcome::Abandoned;
                }
            }
            result = byte_stream.next() => {
                match result {
                    Some(Ok(bytes)) => {
                        received_bytes += bytes.len();
                        capture_head(&mut raw_head, &bytes);
                        buf.extend_from_slice(&bytes);
                    }
                    Some(Err(e)) => {
                        // 流中传输错误（解码失败 / 连接被 RST）：先冲刷半截累积文本（避免
                        // ≤30ms 未冲刷的碎片随错误一起丢失），再把富化诊断交给调用方——
                        // 由它决定「自动重试」还是「上报错误」。熔断计数也在那边（重试成功
                        // 不该记一次失败）。
                        if flush_pending(&tx, &mut pending_text, &mut pending_reasoning).is_err() {
                            return SseOutcome::Abandoned;
                        }
                        return SseOutcome::Transport(
                            meta.describe_transport_err(&e, received_bytes, sse_lines),
                        );
                    }
                    None => break, // EOF
                }
            }
        }
    }

    // 空流诊断：收到字节但无任何有效 data: 报文（被网关强制压缩后的乱码 / HTML
    // 错误页 / 非 SSE 端点等）。旧行为会静默「成功」产出空回复，这里转为带上下文的
    // 显式错误；取消场景（is_cancelled）不误报，按既有取消收尾路径处理。
    // 非传输中断（body 正常读到 EOF）→ 不重试，直接上报。
    if received_bytes > 0 && sse_lines == 0 && !cancel.is_cancelled() {
        breaker.on_failure();
        let _ = tx.send(Err(LlmError::StreamError(
            meta.describe_empty_stream(received_bytes, &raw_head),
        )));
        return SseOutcome::Done;
    }
    // 流结束：先冲刷剩余累积文本，再投递聚合后的完整工具调用与 End
    if flush_pending(&tx, &mut pending_text, &mut pending_reasoning).is_err() {
        return SseOutcome::Abandoned;
    }
    if !tool_calls.is_empty() {
        let mut indices: Vec<usize> = tool_calls.keys().copied().collect();
        indices.sort_unstable();
        let calls: Vec<ToolCall> = indices
            .into_iter()
            .map(|i| {
                let acc = &tool_calls[&i];
                ToolCall::new(acc.id.clone(), acc.name.clone(), acc.arguments.clone())
            })
            .collect();
        if tx.send(Ok(LlmEvent::ToolCalls(calls))).is_err() {
            return SseOutcome::Abandoned;
        }
    }
    let _ = tx.send(Ok(LlmEvent::End(EndInfo { finish_reason, usage })));
    breaker.on_success();
    SseOutcome::Done
}

/// 冲刷 30ms 累积缓冲：思考链先于文本转发；无累积时为空操作。
fn flush_pending(
    tx: &mpsc::UnboundedSender<Result<LlmEvent, LlmError>>,
    pending_text: &mut String,
    pending_reasoning: &mut String,
) -> Result<(), ()> {
    if !pending_reasoning.is_empty() {
        let s = std::mem::take(pending_reasoning);
        if tx.send(Ok(LlmEvent::ReasoningChunk(Chunk::new(s)))).is_err() {
            return Err(()); // 接收端已关闭
        }
    }
    if !pending_text.is_empty() {
        let s = std::mem::take(pending_text);
        if tx.send(Ok(LlmEvent::Chunk(Chunk::new(s)))).is_err() {
            return Err(()); // 接收端已关闭
        }
    }
    Ok(())
}

/// 采集空流诊断用的原始头部预览（首个 [`HEAD_PREVIEW_BYTES`] 字节，跨 chunk 累积）。
pub(crate) fn capture_head(head: &mut Vec<u8>, bytes: &[u8]) {
    if head.len() >= HEAD_PREVIEW_BYTES {
        return;
    }
    let take = (HEAD_PREVIEW_BYTES - head.len()).min(bytes.len());
    head.extend_from_slice(&bytes[..take]);
}

/// 流式 tool_calls 增量聚合器。
#[derive(Default)]
struct AccumToolCall {
    id: String,
    name: String,
    arguments: String,
}

// ═════════════════════════════════════════════════════════
// OpenAI 兼容 SSE 报文结构（正常报文格式，非私有协议）
// ═════════════════════════════════════════════════════════

/// 兼容反序列化：显式 `null` 视为空列表。
/// 部分网关（如 opencode-go / zen 路由）在无工具调用时仍在每个 delta
/// 里显式输出 `"tool_calls": null`——`#[serde(default)]` 只处理字段缺失，
/// 显式 null 会让 Vec 反序列化直接失败，进而导致整条分片（含 content /
/// reasoning）被 `Err(..) => continue` 丢弃（deepseek 官方省略该字段不受影响）。
fn deserialize_nullable_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let opt = Option::<Vec<T>>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// 单个 SSE 报文：`data: {json}`。
#[derive(Debug, Default, Deserialize)]
struct SseChunk {
    #[serde(default, deserialize_with = "deserialize_nullable_vec")]
    choices: Vec<SseChoice>,
    #[serde(default)]
    usage: Option<SseUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct SseChoice {
    #[serde(default)]
    delta: SseDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SseDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_vec")]
    tool_calls: Vec<SseToolCallDelta>,
}

/// 流式 tool_calls 增量片段（arguments 按 index 跨报文累加）。
#[derive(Debug, Deserialize)]
struct SseToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SseFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct SseFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// DeepSeek 缓存明细：`usage.prompt_tokens_details.cached_tokens`（v3.2 起官方
/// 把缓存 token 放这里；OpenAI 风格则是顶层 `prompt_cache_hit_tokens`）。
#[derive(Debug, Default, Deserialize)]
struct SsePromptDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

/// OpenAI 兼容 usage 报文（含 DeepSeek 的 cache 字段，缺失字段默认 0）。
#[derive(Debug, Default, Deserialize)]
struct SseUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
    prompt_cache_hit_tokens: Option<u32>,
    prompt_cache_miss_tokens: Option<u32>,
    #[serde(default)]
    prompt_tokens_details: Option<SsePromptDetails>,
}

impl SseUsage {
    fn into_token_usage(self) -> TokenUsage {
        // 缓存命中优先取 DeepSeek 明细；未上报命中时按 0。
        let hit = self
            .prompt_tokens_details
            .and_then(|d| d.cached_tokens)
            .or(self.prompt_cache_hit_tokens)
            .unwrap_or(0);
        // 未命中:供应商未上报时由 prompt - hit 推导(DeepSeek 只给命中,便于缓存率计算)
        let miss = self
            .prompt_cache_miss_tokens
            .unwrap_or_else(|| self.prompt_tokens.unwrap_or(0).saturating_sub(hit));
        TokenUsage {
            prompt: self.prompt_tokens.unwrap_or(0),
            completion: self.completion_tokens.unwrap_or(0),
            total: self.total_tokens.unwrap_or(0),
            cache_hit: hit,
            cache_miss: miss,
        }
    }
}

/// 流式响应：mpsc receiver 适配为 [`futures::Stream`]。
///
/// 接收端被 drop 后，读取循环下一次 send 失败即退出（避免任务悬挂）。
struct ChatStream {
    rx: mpsc::UnboundedReceiver<Result<LlmEvent, LlmError>>,
}

/// 构造「已被取消」的流：立即投递 Cancelled 错误项后关闭。
///
/// 请求建立 / 首包等待期间被取消时，[`ChatClient::chat_stream`] 用它代替直接返回 `Err`——
/// 引擎对流内 `Err(LlmError::Cancelled)` 按「被取消收尾」处理（取消分支），
/// 直接报错反而会把用户取消误报成「LLM 请求失败」。
fn cancelled_stream() -> LlmStream {
    let (tx, rx) = mpsc::unbounded_channel::<Result<LlmEvent, LlmError>>();
    let _ = tx.send(Err(LlmError::Cancelled));
    Box::pin(ChatStream { rx })
}

impl Stream for ChatStream {
    type Item = Result<LlmEvent, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}
