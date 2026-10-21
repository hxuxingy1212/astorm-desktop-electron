//! DeepSeek 三协议联调（需真实 key，默认 `#[ignore]`）。
//!
//! 运行：
//! ```bash
//! DEEPSEEK_API_KEY=sk-xxx cargo test -p llm-provider --test deepseek_live -- --ignored --nocapture
//! ```
//!
//! 覆盖 chat_completions / responses / anthropic_messages 三条通道的：
//! - 纯文本流式（正文 + End 收尾）
//! - 工具调用（ToolCalls 事件）

use futures::StreamExt;
use llm_provider::{
    ChatClient, ChatMessage, ChatRequest, ClientOption, LlmEvent, Protocol, ToolDef,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const OPENAI_BASE: &str = "https://api.deepseek.com";
const ANTHROPIC_BASE: &str = "https://api.deepseek.com/anthropic";
const MODEL: &str = "deepseek-flash";

const PROTOCOLS: [Protocol; 3] = [
    Protocol::ChatCompletions,
    Protocol::Responses,
    Protocol::AnthropicMessages,
];

#[derive(Default)]
struct Outcome {
    content: String,
    reasoning: String,
    tool_calls: usize,
    ended: bool,
    error: Option<String>,
}

fn api_key() -> String {
    std::env::var("DEEPSEEK_API_KEY").expect("请设置 DEEPSEEK_API_KEY 环境变量")
}

async fn run(protocol: Protocol, request: ChatRequest) -> Outcome {
    let base = if protocol == Protocol::AnthropicMessages {
        ANTHROPIC_BASE
    } else {
        OPENAI_BASE
    };
    let client = ChatClient::with_option(
        base,
        api_key(),
        ClientOption::default()
            .with_timeout(Duration::from_secs(90))
            .with_protocol(protocol),
    );
    let mut stream = client
        .chat_stream(MODEL, request, CancellationToken::new())
        .await
        .expect("chat_stream 打开失败");

    let mut out = Outcome::default();
    while let Some(item) = stream.next().await {
        match item {
            Ok(LlmEvent::Chunk(c)) => out.content.push_str(&c.content),
            Ok(LlmEvent::ReasoningChunk(c)) => out.reasoning.push_str(&c.content),
            Ok(LlmEvent::ToolCalls(c)) => out.tool_calls += c.len(),
            Ok(LlmEvent::Restart) => {
                out.content.clear();
                out.reasoning.clear();
            }
            Ok(LlmEvent::End(_)) => out.ended = true,
            Ok(LlmEvent::Error(m)) => out.error = Some(m),
            Ok(LlmEvent::Start) => {}
            Err(e) => out.error = Some(e.to_string()),
        }
    }
    out
}

#[tokio::test]
#[ignore]
async fn deepseek_three_protocols_text() {
    for protocol in PROTOCOLS {
        let request = ChatRequest::new(vec![ChatMessage::user("Reply with exactly: OK")]);
        let out = run(protocol, request).await;
        println!(
            "[text][{protocol:?}] content={:?} reasoning_len={} ended={} err={:?}",
            out.content,
            out.reasoning.len(),
            out.ended,
            out.error
        );
        assert!(out.error.is_none(), "{protocol:?} 报错: {:?}", out.error);
        assert!(out.ended, "{protocol:?} 未收到 End");
        assert!(!out.content.trim().is_empty(), "{protocol:?} 正文为空");
    }
}

#[tokio::test]
#[ignore]
async fn deepseek_three_protocols_tool_call() {
    for protocol in PROTOCOLS {
        let request = ChatRequest::new(vec![ChatMessage::user(
            "What is the weather in San Francisco? Call the get_weather tool.",
        )])
        .with_tools(vec![ToolDef::new(
            "get_weather",
            "Get the current weather for a city",
            serde_json::json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
        )]);
        let out = run(protocol, request).await;
        println!(
            "[tool][{protocol:?}] tool_calls={} content={:?} ended={} err={:?}",
            out.tool_calls,
            out.content,
            out.ended,
            out.error
        );
        assert!(out.error.is_none(), "{protocol:?} 报错: {:?}", out.error);
        assert!(out.tool_calls >= 1, "{protocol:?} 未返回工具调用");
    }
}
