//! Anthropic Messages API 支持（Claude 原生 `/v1/messages`）。
//!
//! 与 OpenAI 的两套通道（`/chat/completions`、`/responses`）并存，由
//! [`Protocol::AnthropicMessages`](crate::Protocol) 选择。
//!
//! 事件 → [`LlmEvent`] 映射：
//! - `content_block_delta`(`text_delta`)      → `Chunk`(正文)
//! - `content_block_delta`(`thinking_delta`)  → `ReasoningChunk`(思考链)
//! - `content_block_start`/`input_json_delta` → 聚合工具调用，`message_stop` 时作 `ToolCalls`
//! - `message_stop`                           → `End`(含 usage)
//! - `error`                                  → `Error`

use crate::error::SseMeta;
use crate::event::{Chunk, EndInfo, TokenUsage};
use crate::{ChatMessage, ChatRequest, ContentPart, LlmError, LlmEvent, ToolCall};
use futures::{Stream, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Anthropic 请求的默认 `max_tokens`（该 API 必填；会话轮次由引擎自管，这里取宽松上限）。
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 8192;

/// 构造 `/v1/messages` 请求体：system 提到顶层，消息 → messages（tool 结果并入 user）。
pub(crate) fn anthropic_request_body(request: &ChatRequest, model: &str) -> serde_json::Value {
    let mut system = String::new();
    let mut messages: Vec<serde_json::Value> = Vec::new();
    let mut last_was_tool_result = false;

    for m in &request.messages {
        match m.role.as_str() {
            "system" => {
                let text = m.content.clone().unwrap_or_default();
                if !text.is_empty() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
                last_was_tool_result = false;
            }
            "tool" => {
                // 连续工具结果并入同一条 user 消息（Anthropic 要求紧跟对应 tool_use）
                let block = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content.clone().unwrap_or_default(),
                });
                if last_was_tool_result
                    && let Some(last) = messages.last_mut()
                    && let Some(blocks) = last["content"].as_array_mut()
                {
                    blocks.push(block);
                } else {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [block],
                    }));
                    last_was_tool_result = true;
                }
            }
            "assistant" => {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if let Some(text) = m.content.as_ref().filter(|t| !t.is_empty()) {
                    blocks.push(serde_json::json!({ "type": "text", "text": text }));
                }
                if let Some(calls) = &m.tool_calls {
                    for tc in calls {
                        let input: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| serde_json::json!({}));
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                }
                if blocks.is_empty() {
                    blocks.push(serde_json::json!({ "type": "text", "text": "" }));
                }
                messages.push(serde_json::json!({ "role": "assistant", "content": blocks }));
                last_was_tool_result = false;
            }
            _ => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": anthropic_content(m),
                }));
                last_was_tool_result = false;
            }
        }
    }

    let tools: Vec<serde_json::Value> = request
        .tools
        .iter()
        .flatten()
        .map(|t| {
            serde_json::json!({
                "name": t.function.name,
                "description": t.function.description,
                "input_schema": t.function.parameters,
            })
        })
        .collect();

    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::Value::String(model.to_string()));
    body.insert("max_tokens".into(), serde_json::Value::from(DEFAULT_MAX_TOKENS));
    body.insert("stream".into(), serde_json::Value::Bool(true));
    body.insert("messages".into(), serde_json::Value::Array(messages));
    if !system.is_empty() {
        body.insert("system".into(), serde_json::Value::String(system));
    }
    if !tools.is_empty() {
        body.insert("tools".into(), serde_json::Value::Array(tools));
    }
    serde_json::Value::Object(body)
}

/// 消息内容 → Anthropic `content`（纯文本为字符串；多模态为 block 数组）。
fn anthropic_content(m: &ChatMessage) -> serde_json::Value {
    let Some(parts) = m.content_parts.as_ref().filter(|p| !p.is_empty()) else {
        return serde_json::Value::String(m.content.clone().unwrap_or_default());
    };
    let mut blocks: Vec<serde_json::Value> = Vec::with_capacity(parts.len() + 1);
    if let Some(text) = m.content.as_ref().filter(|t| !t.is_empty()) {
        blocks.push(serde_json::json!({ "type": "text", "text": text }));
    }
    for part in parts {
        match part {
            ContentPart::Text { text } => {
                blocks.push(serde_json::json!({ "type": "text", "text": text }));
            }
            ContentPart::ImageUrl { image_url } => {
                if let Some(block) = anthropic_image(&image_url.url) {
                    blocks.push(block);
                }
            }
        }
    }
    serde_json::Value::Array(blocks)
}

/// 图片引用 → Anthropic image block（`data:` 走 base64，http(s) 走 url）。
fn anthropic_image(url: &str) -> Option<serde_json::Value> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        let media_type = meta.split(';').next().unwrap_or("image/png");
        return Some(serde_json::json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(serde_json::json!({
            "type": "image",
            "source": { "type": "url", "url": url },
        }));
    }
    None
}

/// 累积中的 tool_use 调用（按 content block index 聚合）。
#[derive(Default)]
struct AccumAnthropicCall {
    id: String,
    name: String,
    arguments: String,
    seen: bool,
}

/// Anthropic SSE 解析循环（与 [`crate::responses`] 同一制造形状：换解析器）。
pub(crate) async fn read_sse_anthropic_loop(
    mut byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    tx: mpsc::UnboundedSender<Result<LlmEvent, LlmError>>,
    breaker: Arc<crate::circuit::CircuitBreaker>,
    cancel: &CancellationToken,
    meta: SseMeta,
    first: bytes::Bytes,
) -> crate::client::SseOutcome {
    let mut buf = Vec::new();
    buf.extend_from_slice(&first);
    let mut calls: Vec<AccumAnthropicCall> = Vec::new();
    let mut finish_reason = String::new();
    let mut prompt_tokens: u32 = 0;
    let mut completion_tokens: u32 = 0;
    let mut received_bytes = first.len();
    let mut sse_lines = 0usize;
    let mut raw_head: Vec<u8> = Vec::new();
    crate::client::capture_head(&mut raw_head, &first);

    loop {
        if cancel.is_cancelled() {
            let _ = tx.send(Err(LlmError::Cancelled));
            return crate::client::SseOutcome::Done;
        }
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if line.is_empty() {
                continue;
            }
            // Anthropic 用 `event:` + `data:` 两行；这里只认 data: 的 JSON（其内含 type）
            if line.starts_with("event:") {
                continue;
            }
            let is_data_line = line.starts_with("data:");
            let payload = line.strip_prefix("data:").unwrap_or(&line).trim();
            if payload.is_empty() || payload == "[DONE]" {
                if is_data_line {
                    sse_lines += 1;
                }
                continue;
            }
            let ev: serde_json::Value = match serde_json::from_str(payload) {
                Ok(v) => v,
                Err(_) => {
                    if is_data_line {
                        sse_lines += 1;
                    }
                    continue;
                }
            };
            sse_lines += 1;
            let type_ = ev["type"].as_str().unwrap_or("");
            match type_ {
                "message_start" => {
                    prompt_tokens = ev["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
                }
                "content_block_start" => {
                    if ev["content_block"]["type"] == "tool_use" {
                        let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                        ensure_call(&mut calls, idx);
                        let call = &mut calls[idx];
                        call.id = ev["content_block"]["id"].as_str().unwrap_or("").to_string();
                        call.name = ev["content_block"]["name"].as_str().unwrap_or("").to_string();
                        call.seen = true;
                    }
                }
                "content_block_delta" => {
                    let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                    match ev["delta"]["type"].as_str().unwrap_or("") {
                        "text_delta" => {
                            if let Some(delta) = ev["delta"]["text"].as_str() {
                                if !delta.is_empty()
                                    && tx
                                        .send(Ok(LlmEvent::Chunk(Chunk::new(delta.to_string()))))
                                        .is_err()
                                {
                                    return crate::client::SseOutcome::Abandoned;
                                }
                            }
                        }
                        "thinking_delta" => {
                            if let Some(delta) = ev["delta"]["thinking"].as_str() {
                                if !delta.is_empty()
                                    && tx
                                        .send(Ok(LlmEvent::ReasoningChunk(Chunk::new(delta.to_string()))))
                                        .is_err()
                                {
                                    return crate::client::SseOutcome::Abandoned;
                                }
                            }
                        }
                        "input_json_delta" => {
                            if let Some(partial) = ev["delta"]["partial_json"].as_str() {
                                ensure_call(&mut calls, idx);
                                calls[idx].arguments.push_str(partial);
                            }
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    if let Some(reason) = ev["delta"]["stop_reason"].as_str() {
                        finish_reason = map_stop_reason(reason);
                    }
                    if let Some(out) = ev["usage"]["output_tokens"].as_u64() {
                        completion_tokens = out as u32;
                    }
                }
                "message_stop" => {
                    emit_tool_calls(&tx, &calls);
                    let usage = TokenUsage::new(prompt_tokens, completion_tokens);
                    breaker.on_success();
                    let _ = tx.send(Ok(LlmEvent::End(EndInfo {
                        finish_reason: if finish_reason.is_empty() {
                            "stop".to_string()
                        } else {
                            finish_reason.clone()
                        },
                        usage: Some(usage),
                    })));
                    return crate::client::SseOutcome::Done;
                }
                "error" => {
                    breaker.on_failure();
                    let msg = ev["error"]["message"]
                        .as_str()
                        .or_else(|| ev["message"].as_str())
                        .unwrap_or("Anthropic 流式错误");
                    let _ = tx.send(Err(LlmError::ServerError(msg.to_string())));
                    return crate::client::SseOutcome::Done;
                }
                _ => { /* ping / content_block_stop / message_delta(无字段) 等忽略 */ }
            }
        }
        if cancel.is_cancelled() {
            let _ = tx.send(Err(LlmError::Cancelled));
            return crate::client::SseOutcome::Done;
        }
        match byte_stream.next().await {
            Some(Ok(bytes)) => {
                received_bytes += bytes.len();
                crate::client::capture_head(&mut raw_head, &bytes);
                buf.extend_from_slice(&bytes);
            }
            Some(Err(e)) => {
                return crate::client::SseOutcome::Transport(
                    meta.describe_transport_err(&e, received_bytes, sse_lines),
                );
            }
            None => {
                if received_bytes > 0 && sse_lines == 0 && !cancel.is_cancelled() {
                    breaker.on_failure();
                    let _ = tx.send(Err(LlmError::StreamError(
                        meta.describe_empty_stream(received_bytes, &raw_head),
                    )));
                    return crate::client::SseOutcome::Done;
                }
                emit_tool_calls(&tx, &calls);
                let usage = TokenUsage::new(prompt_tokens, completion_tokens);
                let _ = tx.send(Ok(LlmEvent::End(EndInfo {
                    finish_reason: if finish_reason.is_empty() {
                        "stop".to_string()
                    } else {
                        finish_reason.clone()
                    },
                    usage: Some(usage),
                })));
                breaker.on_success();
                return crate::client::SseOutcome::Done;
            }
        }
    }
}

/// 保证 `calls[idx]` 存在。
fn ensure_call(calls: &mut Vec<AccumAnthropicCall>, idx: usize) {
    while calls.len() <= idx {
        calls.push(AccumAnthropicCall::default());
    }
}

/// 投递聚合完成的工具调用（仅 seen 的项）。
fn emit_tool_calls(
    tx: &mpsc::UnboundedSender<Result<LlmEvent, LlmError>>,
    calls: &[AccumAnthropicCall],
) {
    let completed: Vec<ToolCall> = calls
        .iter()
        .filter(|c| c.seen)
        .map(|c| ToolCall::new(c.id.clone(), c.name.clone(), c.arguments.clone()))
        .collect();
    if !completed.is_empty() {
        let _ = tx.send(Ok(LlmEvent::ToolCalls(completed)));
    }
}

/// Anthropic `stop_reason` → OpenAI 风格 finish_reason（引擎按后者判定）。
fn map_stop_reason(reason: &str) -> String {
    match reason {
        "tool_use" => "tool_calls".to_string(),
        "max_tokens" => "length".to_string(),
        "end_turn" | "stop_sequence" => "stop".to_string(),
        other => other.to_string(),
    }
}
