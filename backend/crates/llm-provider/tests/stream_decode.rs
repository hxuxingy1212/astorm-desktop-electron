//! 流式响应解码的离线回归测试（无需真实 LLM / key / 网络）。
//!
//! 背景：历史上流式路径零离线覆盖，真实 e2e 全部 `#[ignore]` 且需 key。偶发的
//! 「[E005] Stream error: error decoding response body」（响应体解码失败）与各类
//! 「静默空回复」只能靠线上复现。这里用脚本化假服务端（std TcpListener，逐连接按
//! 脚本返回原始 HTTP 字节）覆盖：
//! - 正常 SSE 报文的解析与 End 收尾；
//! - 首包前连接损坏（Content-Length 声明大于实际）→ 自动整体重试一次后成功；
//! - 空响应体 → 自动整体重试一次后成功；
//! - 假 `Content-Encoding: gzip` + 非 SSE 乱码 body → 不再静默空回复，
//!   报带 url / content-encoding / 头部预览上下文的富化错误；
//! - 流中截断（部分报文已到达后连接损坏）→ 先冲刷半截文本再报富化错误；
//! - `/responses` 端点 404 → 回退 `/chat/completions` 成功。

use futures::StreamExt;
use llm_provider::{ChatClient, ChatMessage, ChatRequest, ClientOption, LlmEvent, Protocol};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ── 脚本化假服务端 ─────────────────────────────────────────────

/// 启动脚本化 HTTP/1.1 假服务端：第 i 个连接返回 `script[i]` 原始响应
/// （超出连接数后循环使用最后一条）。返回 (base_url, 已受理连接数)。
fn start_scripted(script: Vec<&'static str>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock 服务端绑定失败");
    let addr = listener.local_addr().expect("mock 地址");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = hits.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let i = hits2.fetch_add(1, Ordering::SeqCst);
            let resp = script[i.min(script.len() - 1)];
            serve_connection(&mut s, resp);
        }
    });
    (format!("http://{addr}"), hits)
}

/// 单连接处理：读请求头（至空行）→ 按 Content-Length 排空请求体（避免提前关闭
/// 导致客户端写失败）→ 写脚本化响应 → 关闭连接。
fn serve_connection(s: &mut TcpStream, resp: &str) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 2048];
    let mut hdrs: Vec<u8> = Vec::new();
    while hdrs.len() < 64 * 1024 && !hdrs.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => hdrs.extend_from_slice(&buf[..n]),
        }
    }
    if let Ok(text) = std::str::from_utf8(&hdrs) {
        if let Some(cl) = header_value(text, "content-length")
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            let mut left = cl;
            while left > 0 {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => left = left.saturating_sub(n),
                }
            }
        }
    }
    let _ = s.write_all(resp.as_bytes());
    let _ = s.flush();
    // drop 即关闭连接：响应体由 Content-Length 界定，或读至 EOF
}

/// 从请求头文本取单值头（小写名称匹配）。
fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case(name) {
            Some(v.trim())
        } else {
            None
        }
    })
}

// ── 响应脚本 ───────────────────────────────────────────────────

/// 正常 SSE：两段 content + [DONE]。
const SSE_OK: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Connection: close\r\n\
     \r\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\r\n\r\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"世界\"}}]}\r\n\r\n\
     data: [DONE]\r\n\r\n";

/// 首包前损坏：声明 500B 但立即断连（模拟解码失败 / 传输截断）。
const SSE_BROKEN_FIRST: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Content-Length: 500\r\n\
     Connection: close\r\n\
     \r\n";

/// 空响应体：Content-Length: 0。
const SSE_EMPTY_BODY: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Content-Length: 0\r\n\
     Connection: close\r\n\
     \r\n";

/// 假 gzip 编码头 + 非 SSE 乱码正文（真实错误页形态，非 JSON / 非 data:）。
const SSE_FAKE_GZIP_GARBAGE: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Content-Encoding: gzip\r\n\
     Connection: close\r\n\
     \r\n\
     <html><body>502 gateway error page, definitely not gzip</body></html>\n\
     upstream connect failed; pool exhausted; retry later\n";

/// 流中截断：先给一条完整报文 + 半条，然后断连（Content-Length 未满足）。
const SSE_TRUNCATED_MID: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Content-Length: 10000\r\n\
     \r\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\r\n\r\n\
     data: {\"choices\":[{\"del";

/// /responses 端点未实现（404 JSON 错误体，正文 25B）。
const RESPONSES_404: &str = "HTTP/1.1 404 Not Found\r\n\
     Content-Type: application/json\r\n\
     Content-Length: 25\r\n\
     Connection: close\r\n\
     \r\n\
     {\"error\":\"no /responses\"}";

/// /responses 语义事件 SSE：正文增量 + completed（含 usage）。
const RESPONSES_OK: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Connection: close\r\n\
     \r\n\
     data: {\"type\":\"response.output_text.delta\",\"delta\":\"语义你好\"}\r\n\
     data: {\"type\":\"response.completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4,\"total_tokens\":7}}\r\n";

// ── 测试工具 ───────────────────────────────────────────────────

/// 以定制参数打开流并消耗至结束，返回 (正文文本, 是否收到 End, 最终错误文案, Restart 次数)。
///
/// 遵循消费端契约：收到 `Restart`（流中传输中断后自动重开）即丢弃此前累积的正文，
/// 否则两次尝试的文本会被拼接——引擎与结构化输出路径都是这么实现的。
async fn drain(base: String, option: ClientOption) -> (String, bool, Option<String>, usize) {
    let client = ChatClient::with_option(base, "test-key", option);
    let request = ChatRequest::new(vec![ChatMessage::user("ping")]);
    let mut stream = client
        .chat_stream("mock-model", request, CancellationToken::new())
        .await
        .expect("chat_stream 打开失败");

    let mut content = String::new();
    let mut ended = false;
    let mut err: Option<String> = None;
    let mut restarts = 0usize;
    let collect = async {
        while let Some(item) = stream.next().await {
            match item {
                Ok(LlmEvent::Chunk(c)) => content.push_str(&c.content),
                Ok(LlmEvent::Restart) => {
                    restarts += 1;
                    content.clear(); // 契约：重开即作废此前半截内容
                }
                Ok(LlmEvent::End(_)) => ended = true,
                Ok(LlmEvent::Error(msg)) => err = Some(msg),
                Ok(_) => {} // ReasoningChunk / ToolCalls / Start
                Err(e) => err = Some(e.to_string()),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), collect)
        .await
        .expect("流消费超时（服务端未按脚本关闭？）");
    (content, ended, err, restarts)
}

/// 测试默认参数：10s 超时 + 关闭 send 层重试（连接均能建立成功，避免干扰脚本序号计数）。
fn base_option() -> ClientOption {
    ClientOption::default()
        .with_timeout(Duration::from_secs(10))
        .with_max_retries(0)
}

// ── 用例 ───────────────────────────────────────────────────────

#[tokio::test]
async fn happy_path_sse_delivers_chunks_and_end() {
    let (base, hits) = start_scripted(vec![SSE_OK]);
    let (content, ended, err, restarts) = drain(base, base_option()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1, "正常流应只发一次请求");
    assert_eq!(restarts, 0, "正常流不应出现 Restart");
    assert!(err.is_none(), "不应报错: {err:?}");
    assert!(ended, "应以 End 收尾");
    assert_eq!(content, "你好世界");
}

#[tokio::test]
async fn broken_first_packet_retries_once_then_succeeds() {
    let (base, hits) = start_scripted(vec![SSE_BROKEN_FIRST, SSE_OK]);
    let (content, ended, err, restarts) = drain(base, base_option()).await;
    // 首包前损坏（0 字节到达）→ 整体重发一次 → 第二次成功
    assert_eq!(hits.load(Ordering::SeqCst), 2, "应自动整体重试一次");
    assert_eq!(restarts, 0, "首包前重试无内容可作废，不应发 Restart");
    assert!(err.is_none(), "重试后不应报错: {err:?}");
    assert!(ended, "应以 End 收尾");
    assert_eq!(content, "你好世界");
}

#[tokio::test]
async fn empty_response_body_retries_once_then_succeeds() {
    let (base, hits) = start_scripted(vec![SSE_EMPTY_BODY, SSE_OK]);
    let (content, ended, err, restarts) = drain(base, base_option()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2, "空响应体应整体重试一次");
    assert_eq!(restarts, 0, "首包前的重试尚未产生内容，不需 Restart");
    assert!(err.is_none(), "重试后不应报错: {err:?}");
    assert!(ended, "应以 End 收尾");
    assert_eq!(content, "你好世界");
}

#[tokio::test]
async fn garbage_body_with_fake_gzip_reports_rich_error() {
    let (base, hits) = start_scripted(vec![SSE_FAKE_GZIP_GARBAGE]);
    let (content, _ended, err, restarts) = drain(base, base_option()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1, "空流诊断非传输中断，不应重试");
    assert_eq!(restarts, 0);
    assert_eq!(content, "", "乱码不应产出「成功」的空正文");
    let msg = err.expect("应报显式错误而非静默空回复");
    // 富化上下文：编码头 / URL / 原始头部预览
    assert!(msg.contains("content-encoding=gzip"), "错误应带编码头: {msg}");
    assert!(msg.contains("chat/completions"), "错误应带 url: {msg}");
    assert!(msg.contains("无任何有效 SSE"), "应为空流诊断: {msg}");
}

#[tokio::test]
async fn truncated_mid_stream_retries_once_then_errors_with_rich_context() {
    // 脚本只有一条 → 每次连接都在同一位置截断：第 1 次中断 → 自动重开第 2 次 → 仍中断 →
    // 重试耗尽后上报富化错误（不再像旧实现那样第 1 次就整轮作废）
    let (base, hits) = start_scripted(vec![SSE_TRUNCATED_MID]);
    let (content, _ended, err, restarts) = drain(base, base_option()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2, "流中中断应自动重开一次");
    assert_eq!(restarts, 1, "重开前必须投递 Restart（消费端据此丢弃半截）");
    // 关键：正文只来自**最后一次**尝试（第 1 次的半截已被 Restart 丢弃，没有拼成「半截半截」）
    assert_eq!(content, "半截", "Restart 后正文应只来自最后一次尝试: {content}");
    let msg = err.expect("重试耗尽应报错误");
    assert!(msg.contains("E005"), "应为流错误: {msg}");
    assert!(msg.contains("url="), "错误应带 url: {msg}");
    assert!(msg.contains("received="), "错误应带已收字节数: {msg}");
    assert!(msg.contains("http_version="), "错误应带 HTTP 版本（判定 h2/h1）: {msg}");
    assert!(msg.contains("elapsed="), "错误应带连接存活时长: {msg}");
    assert!(msg.contains("已自动重试 1 次"), "错误应说明已重试: {msg}");
}

#[tokio::test]
async fn truncated_mid_stream_retries_once_then_succeeds() {
    // 第 1 次流中截断（已吐出「半截」）→ 自动重开 → 第 2 次完整返回
    let (base, hits) = start_scripted(vec![SSE_TRUNCATED_MID, SSE_OK]);
    let (content, ended, err, restarts) = drain(base, base_option()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2, "应自动重开一次");
    assert_eq!(restarts, 1, "应投递一次 Restart");
    assert!(err.is_none(), "重试成功后不应报错: {err:?}");
    assert!(ended, "应以 End 收尾");
    // 关键：最终正文只含第 2 次尝试的内容，不含第 1 次的半截（无拼接污染）
    assert_eq!(content, "你好世界", "重试后正文应只来自最后一次尝试: {content}");
}

#[tokio::test]
async fn responses_404_falls_back_to_chat_completions() {
    let (base, hits) = start_scripted(vec![RESPONSES_404, SSE_OK]);
    let (content, ended, err, restarts) = drain(base, base_option().with_responses_api()).await;
    assert_eq!(restarts, 0, "正常路径不应出现 Restart");
    assert_eq!(hits.load(Ordering::SeqCst), 2, "/responses 404 后应回退一次");
    assert!(err.is_none(), "回退后不应报错: {err:?}");
    assert!(ended, "应以 End 收尾");
    assert_eq!(content, "你好世界");
}

#[tokio::test]
async fn responses_semantic_events_stream_ok() {
    let (base, hits) = start_scripted(vec![RESPONSES_OK]);
    let (content, ended, err, restarts) = drain(base, base_option().with_responses_api()).await;
    assert_eq!(restarts, 0, "正常路径不应出现 Restart");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(err.is_none(), "不应报错: {err:?}");
    assert!(ended, "response.completed 应以 End 收尾");
    assert_eq!(content, "语义你好");
}

/// Anthropic Messages SSE：`event:` + `data:` 双行，正文增量 + message_stop。
const ANTHROPIC_OK: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Connection: close\r\n\
     \r\n\
     event: message_start\r\n\
     data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\r\n\r\n\
     event: content_block_delta\r\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你好\"}}\r\n\r\n\
     event: content_block_delta\r\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"世界\"}}\r\n\r\n\
     event: message_delta\r\n\
     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\r\n\r\n\
     event: message_stop\r\n\
     data: {\"type\":\"message_stop\"}\r\n\r\n";

#[tokio::test]
async fn anthropic_messages_stream_ok() {
    let (base, hits) = start_scripted(vec![ANTHROPIC_OK]);
    let (content, ended, err, restarts) =
        drain(base, base_option().with_protocol(Protocol::AnthropicMessages)).await;
    assert_eq!(restarts, 0, "正常路径不应出现 Restart");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(err.is_none(), "不应报错: {err:?}");
    assert!(ended, "message_stop 应以 End 收尾");
    assert_eq!(content, "你好世界");
}
