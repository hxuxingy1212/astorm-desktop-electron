//! 流中途传输中断的**端到端**回归（离线：脚本化假服务端，无需 LLM 凭据/网络）。
//!
//! 覆盖两层协作：
//! - provider：收到字节后被 RST → 自动重开一次（`llm_provider::LlmEvent::Restart`）；
//! - 引擎：`Restart` → 丢弃本轮已累积的正文/思考/工具调用，并向上发 `stream_reset`
//!   事件（前端据此清掉已渲染的半截内容重画）。
//!
//! 关键断言是**不拼接**：两次尝试的正文绝不能连在一起（这正是不发 Restart 时的故障形态）。
//!
//! 运行：`cargo test -p react-loop --test stream_reset`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use llm_provider::{ChatClient, ChatMessage, ChatRequest};
use react_loop::engine::{LlmResult, LoopContext, ReactEngine};
use react_loop::react_loop::ReactLoop;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

// ── 脚本化假服务端（逐连接返回原始 HTTP 字节，超出后循环用最后一条）─────

fn start_scripted(script: Vec<&'static str>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock 服务端绑定失败");
    let addr = listener.local_addr().expect("mock 地址");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let i = hits2.fetch_add(1, Ordering::SeqCst);
            serve_connection(&mut s, script[i.min(script.len() - 1)]);
        }
    });
    (format!("http://{addr}"), hits)
}

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
    let _ = s.write_all(resp.as_bytes());
    let _ = s.flush();
    // 写完立即关闭：截断脚本靠「声明到不了的长度」让客户端读到传输错误
}

/// 正常 SSE：一段正文 + [DONE]。
const SSE_OK: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Content-Length: 4000\r\n\
     \r\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"世界\"}}]}\r\n\r\n\
     data: [DONE]\r\n\r\n";

/// 流中截断：先给一条完整报文 + 半条，然后断连（Content-Length 未满足 → 传输错误）。
const SSE_TRUNCATED_MID: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Content-Length: 10000\r\n\
     \r\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\r\n\r\n\
     data: {\"choices\":[{\"del";

// ── 引擎 hooks 桩：只收集事件与落库的助手消息 ─────────────────────

#[derive(Default)]
struct CollectingHooks {
    events: Mutex<Vec<Value>>,
    appended: Mutex<Vec<ChatMessage>>,
}

#[async_trait]
impl ReactLoop for CollectingHooks {
    async fn before_loop(&self, _ctx: &mut LoopContext) -> Result<(), String> {
        Ok(())
    }
    async fn loop_start(&self, _ctx: &mut LoopContext) {}
    async fn after_llm(&self, _ctx: &mut LoopContext, _result: &LlmResult) {}
    fn loop_end(&self, _ctx: &mut LoopContext) {}
    async fn after_loop(&self, _ctx: &mut LoopContext) -> Result<(), String> {
        Ok(())
    }
    fn append_message(&self, _ctx: &LoopContext, message: ChatMessage) {
        self.appended.lock().unwrap().push(message);
    }
    fn emit_event(&self, _ctx: &LoopContext, event: Value) {
        self.events.lock().unwrap().push(event);
    }
}

impl CollectingHooks {
    fn event_types(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap_or("").to_string())
            .collect()
    }
    fn joined_text(&self) -> String {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "text")
            .filter_map(|e| e["content"].as_str())
            .collect()
    }
    fn assistant_contents(&self) -> Vec<String> {
        self.appended
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.role == "assistant")
            .filter_map(|m| m.content.clone())
            .collect()
    }
}

async fn run_engine(base: String) -> (Result<(), String>, Arc<CollectingHooks>) {
    let client = Arc::new(ChatClient::new(base, "test-key"));
    let hooks = Arc::new(CollectingHooks::default());
    let engine = ReactEngine::new(client, hooks.clone());
    let mut ctx = LoopContext::new("stream-reset-session", CancellationToken::new());
    ctx.chat_request = ChatRequest::new(vec![ChatMessage::user("ping")]).with_model("mock-model");
    let result = engine.run(&mut ctx).await;
    (result, hooks)
}

// ── 用例 ───────────────────────────────────────────────────────

#[tokio::test]
async fn mid_stream_reset_discards_partial_and_finishes() {
    // 第 1 次：吐出「半截」后连接被截断 → provider 自动重开；第 2 次：完整返回「世界」
    let (base, hits) = start_scripted(vec![SSE_TRUNCATED_MID, SSE_OK]);
    let (result, hooks) = run_engine(base).await;

    assert!(result.is_ok(), "自动重试后整轮应正常结束: {result:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 2, "应自动重开一次请求");
    assert_eq!(
        hooks
            .event_types()
            .iter()
            .filter(|t| *t == "stream_reset")
            .count(),
        1,
        "重开前必须发一次 stream_reset: {:?}",
        hooks.event_types()
    );
    // 事件流里两次尝试的正文都出现过（半截在前，前端收到 stream_reset 后丢弃）
    assert_eq!(hooks.joined_text(), "半截世界");
    // 落库的助手消息只含最后一次尝试的内容——绝不能是「半截世界」拼接体
    assert_eq!(
        hooks.assistant_contents(),
        vec!["世界".to_string()],
        "落库正文只能来自最后一次尝试"
    );
}

#[tokio::test]
async fn mid_stream_reset_exhausted_reports_error_without_appending() {
    // 脚本只有一条 → 两次尝试都在同一位置截断 → 重试耗尽，本轮以错误结束
    let (base, hits) = start_scripted(vec![SSE_TRUNCATED_MID]);
    let (result, hooks) = run_engine(base).await;

    assert_eq!(hits.load(Ordering::SeqCst), 2, "应自动重开一次后放弃");
    let err = result.expect_err("重试耗尽应返回错误");
    assert!(err.contains("E005"), "应为流错误: {err}");
    assert!(err.contains("已自动重试 1 次"), "错误应说明已重试: {err}");
    assert!(err.contains("url="), "错误应带 url: {err}");
    assert!(err.contains("http_version="), "错误应带 HTTP 版本: {err}");
    // 半截内容不落库（避免把截断的回复写进后续上下文）
    assert!(
        hooks.assistant_contents().is_empty(),
        "失败轮不应落库助手消息: {:?}",
        hooks.assistant_contents()
    );
}

/// 重复工具回复：无正文仍注入警告；下一轮持续重复则停止，并补齐整批响应。
#[tokio::test]
async fn repeated_tool_batches_receive_warning_and_stop_with_paired_results() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for round in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 4096];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") { break pos + 4; }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let len: usize = headers.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
            }).unwrap();
            while bytes.len() < header_end + len {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            requests.push(serde_json::from_slice::<Value>(&bytes[header_end..header_end + len]).unwrap());
            let calls: Vec<_> = (0..3).map(|i| serde_json::json!({
                "index": i, "id": format!("call-{round}-{i}"), "type":"function",
                "function":{"name":"missing", "arguments":"{}"}
            })).collect();
            let chunk = serde_json::json!({"choices":[{"delta":{"tool_calls":calls},"finish_reason":null}]});
            let body = format!("data: {chunk}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n");
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        requests
    });
    let (result, hooks) = run_engine(base).await;
    assert!(result.unwrap_err().contains("工具循环已停止"));
    let requests = server.join().unwrap();
    let messages = requests[1]["messages"].as_array().unwrap();
    assert!(messages.last().unwrap()["content"].as_str().unwrap().contains("工具循环警告"));
    assert_eq!(messages.iter().filter(|m| m["role"] == "tool").count(), 3);
    let appended = hooks.appended.lock().unwrap();
    assert_eq!(appended.iter().filter(|m| m.role == "tool").count(), 6);
    assert!(!appended.iter().any(|m| m.content.as_deref().unwrap_or("").contains("工具循环警告")));
}
