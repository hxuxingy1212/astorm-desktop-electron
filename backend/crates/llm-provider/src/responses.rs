//! OpenAI Responses API 支持：语义化流式事件（正文 / 推理分开、工具调用显式）。
//!
//! 背景(2026-08 实测, deepseek-v4-flash)：`/chat/completions` 在 opencode zen 上对
//! 带工具请求存在按客户端指纹的「仅思考链、正文恒空」分裂；官方 deepseek 直连正常。
//! `/responses` 通道对两类客户端均稳定返回 `response.output_text.delta`(正文增量)
//! + `response.reasoning_text.delta`(推理增量) + `response.completed`。
//!
//! 使用：**非默认**——`chat_stream` 默认走 `/chat/completions`；仅当
//! [`ClientOption::with_responses_api`] 开启时优先 `/responses`(任一失败自动回退旧通道)。
//!
//! 事件 → [`LlmEvent`] 映射与旧通道一致，引擎无需改动：
//! - `response.output_text.delta`          → `Chunk`(正文)
//! - `response.reasoning_text.delta`       → `ReasoningChunk`(思考链)
//! - `response.output_item.done`(function_call) → 聚合后作为 `ToolCalls`
//! - `response.completed`                  → `End`(含 usage)
//! - `response.error` / `response.failed`  → `Error`

use crate::error::SseMeta;
use crate::event::{Chunk, EndInfo, TokenUsage};
use crate::{ChatMessage, ChatRequest, ContentPart, LlmError, LlmEvent, ToolCall};
use futures::{Stream, StreamExt};
use reqwest::StatusCode;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 构造 `/responses` 请求体：消息 → input 项，chat 工具定义 → Responses 扁平格式。
pub(crate) fn responses_request_body(request: &ChatRequest, model: &str) -> serde_json::Value {
    let input: Vec<serde_json::Value> = request
        .messages
        .iter()
        // tool 消息（function_call_output）不过滤：content 为空也要上送——它是
        // assistant function_call 的配对载体，少一条上游会报调用无响应（400）
        .filter(|m| m.role == "tool" || m.content.is_some() || m.content_parts.is_some() || m.tool_calls.is_some())
        .flat_map(|m| {
            let mut items = Vec::new();
            match m.role.as_str() {
                "tool" => items.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": m.content.clone().unwrap_or_default(),
                })),
                _ => {
                    let mut item = serde_json::json!({
                        "role": m.role,
                        "content": responses_content(m),
                    });
                    // 思考模式：上一轮 assistant 的 reasoning 必须回传（多端点实测：
                    // zen(Console Go) 与 deepseek 的 /responses 均要求；缺失会 400
                    // `reasoning_text in the thinking mode must be passed back`）。
                    // 三种字段名全带以提高兼容性（deepseek 全部接受，zen 接受 reasoning_text）。
                    if let Some(r) = &m.reasoning_content
                        && !r.is_empty()
                    {
                        item["reasoning_text"] = serde_json::Value::String(r.clone());
                        item["reasoning"] = serde_json::Value::String(r.clone());
                        item["reasoning_content"] = serde_json::Value::String(r.clone());
                    }
                    items.push(item);
                    // assistant 携带的工具调用 → 独立的 function_call 输入项
                    if let Some(calls) = &m.tool_calls {
                        for tc in calls {
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.function.name,
                                "arguments": tc.function.arguments,
                            }));
                        }
                    }
                }
            }
            items
        })
        .collect();

    let tools: Vec<serde_json::Value> = request
        .tools
        .iter()
        .flatten()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.function.name,
                "description": t.function.description,
                "parameters": t.function.parameters,
            })
        })
        .collect();

    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::Value::String(model.to_string()));
    body.insert("stream".into(), serde_json::Value::Bool(true));
    body.insert("input".into(), serde_json::Value::Array(input));
    if !tools.is_empty() {
        body.insert("tools".into(), serde_json::Value::Array(tools));
    }
    serde_json::Value::Object(body)
}

/// 消息内容 → Responses API 的 `content` 形态。
///
/// 与 `/chat/completions` 不同：多模态 block 用 `input_text` / `input_image`
/// （`input_image` 的 `image_url` 是**字符串**而非 `{url}` 对象），纯文本仍是字符串。
fn responses_content(m: &ChatMessage) -> serde_json::Value {
    let Some(parts) = m.content_parts.as_ref().filter(|p| !p.is_empty()) else {
        return serde_json::Value::String(m.content.clone().unwrap_or_default());
    };
    let mut blocks: Vec<serde_json::Value> = Vec::with_capacity(parts.len() + 1);
    if let Some(text) = m.content.as_ref().filter(|t| !t.is_empty()) {
        blocks.push(serde_json::json!({ "type": "input_text", "text": text }));
    }
    for part in parts {
        match part {
            ContentPart::Text { text } => {
                blocks.push(serde_json::json!({ "type": "input_text", "text": text }));
            }
            ContentPart::ImageUrl { image_url } => {
                let mut block =
                    serde_json::json!({ "type": "input_image", "image_url": image_url.url });
                if let Some(detail) = &image_url.detail {
                    block["detail"] = serde_json::Value::String(detail.clone());
                }
                blocks.push(block);
            }
        }
    }
    serde_json::Value::Array(blocks)
}

/// 累积中的 function_call 工具调用。
#[derive(Default)]
struct AccumResponsesCall {
    id: String,
    name: String,
    arguments: String,
}

/// Responses 语义事件 SSE 解析循环（与 `read_sse_loop` 同一制造形状：换解析器）。
///
/// 工具调用按 `output_item.done`(function_call) 聚合完成；正文/推理增量即时转发。
/// `first` 为 [`crate::client::ChatClient`] 首包健康检查留下的首个 chunk（不丢首包）；
/// `meta` 携带响应 url / 协议版本 / 编码头，供流内错误富化。
///
/// 收尾语义与 `read_sse_loop` 一致（见 [`crate::client::SseOutcome`]）：流中传输中断
/// 不自行报错，交调用方决定重试或上报；其余终态（完成 / 取消 / 空流诊断 / 服务端错误）
/// 自行投递后返回 [`crate::client::SseOutcome::Done`]。
pub(crate) async fn read_sse_responses_loop(
    mut byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    tx: mpsc::UnboundedSender<Result<LlmEvent, LlmError>>,
    breaker: Arc<crate::circuit::CircuitBreaker>,
    cancel: &CancellationToken,
    meta: SseMeta,
    first: bytes::Bytes,
) -> crate::client::SseOutcome {
    let mut buf = Vec::new();
    // 首包作为初始缓冲喂入，与「首包随网络到达」走同一解析路径，不丢数据
    buf.extend_from_slice(&first);
    let mut calls: Vec<AccumResponsesCall> = Vec::new();
    let mut has_any_call = false;
    // 流进度计数（错误富化 / 空流诊断用）：已收字节、有效 data: 报文数、原始头部预览
    let mut received_bytes = first.len();
    let mut sse_lines = 0usize;
    let mut raw_head: Vec<u8> = Vec::new();
    crate::client::capture_head(&mut raw_head, &first);

    'outer: loop {
        if cancel.is_cancelled() {
            // 用户取消：投递 Cancelled 而非静默关流（引擎据此按「被取消」收尾）
            let _ = tx.send(Err(LlmError::Cancelled));
            return crate::client::SseOutcome::Done;
        }
        // 处理缓冲区内完整行
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
            let ev: serde_json::Value = match serde_json::from_str(payload) {
                Ok(v) => v,
                Err(_) => {
                    // data: 携带的非 JSON 报文仍是有效 SSE 信封；裸行非 JSON 不计数
                    if is_data_line {
                        sse_lines += 1;
                    }
                    continue;
                }
            };
            // 成功解析的 JSON 报文：有效 SSE 信封 +1
            sse_lines += 1;
            let type_ = ev["type"].as_str().unwrap_or("");
            match type_ {
                "response.output_text.delta" => {
                    if let Some(delta) = ev["delta"].as_str() {
                        if !delta.is_empty()
                            && tx.send(Ok(LlmEvent::Chunk(Chunk::new(delta.to_string())))).is_err()
                        {
                            return crate::client::SseOutcome::Abandoned;
                        }
                    }
                }
                "response.reasoning_text.delta" => {
                    if let Some(delta) = ev["delta"].as_str() {
                        if !delta.is_empty()
                            && tx
                                .send(Ok(LlmEvent::ReasoningChunk(Chunk::new(delta.to_string()))))
                                .is_err()
                        {
                            return crate::client::SseOutcome::Abandoned;
                        }
                    }
                }
                "response.output_item.added" => {
                    // 新 function_call 项：先登记 id + name，参数经 delta 增量累积
                    if ev["item"]["type"] == "function_call" {
                        calls.push(AccumResponsesCall {
                            id: ev["item"]["id"].as_str().unwrap_or("").to_string(),
                            name: ev["item"]["name"].as_str().unwrap_or("").to_string(),
                            arguments: String::new(),
                        });
                        has_any_call = true;
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(delta) = ev["delta"].as_str() {
                        let item_id = ev["item_id"].as_str().unwrap_or("");
                        if !item_id.is_empty() {
                            if let Some(call) = calls.iter_mut().find(|c| c.id == item_id) {
                                call.arguments.push_str(delta);
                            }
                        } else if let Some(idx) = ev["output_index"].as_u64() {
                            // 无 item_id 时按 output_index 兜底(仅限最后一个参与中的调用)
                            let len = calls.len();
                            if len > 0 && idx as usize >= len - 1 {
                                if let Some(call) = calls.last_mut() {
                                    call.arguments.push_str(delta);
                                }
                            }
                        }
                    }
                }
                "response.output_item.done" => {
                    // 服务端可能一次性给全 arguments(非增量)；覆盖累积值
                    if ev["item"]["type"] == "function_call" {
                        let id = ev["item"]["id"].as_str().unwrap_or("");
                        let mut merged = false;
                        if let Some(call) = calls.iter_mut().find(|c| c.id == id) {
                            if let Some(args) = ev["item"]["arguments"].as_str() {
                                call.arguments = args.to_string();
                            }
                            if let Some(name) = ev["item"]["name"].as_str() {
                                call.name = name.to_string();
                            }
                            merged = true;
                        }
                        if !merged {
                            calls.push(AccumResponsesCall {
                                id: id.to_string(),
                                name: ev["item"]["name"].as_str().unwrap_or("").to_string(),
                                arguments: ev["item"]["arguments"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string(),
                            });
                            has_any_call = true;
                        }
                    }
                }
                "response.completed" => {
                    // 先投递聚合完成的工具调用，再发 End
                    if has_any_call {
                        let completed: Vec<ToolCall> = calls
                            .iter()
                            .map(|c| ToolCall::new(c.id.clone(), c.name.clone(), c.arguments.clone()))
                            .collect();
                        if tx.send(Ok(LlmEvent::ToolCalls(completed))).is_err() {
                            return crate::client::SseOutcome::Abandoned;
                        }
                    }
                    let usage = usage_from_completed(&ev["usage"]);
                    breaker.on_success();
                    let _ = tx.send(Ok(LlmEvent::End(EndInfo {
                        finish_reason: "stop".to_string(),
                        usage,
                    })));
                    return crate::client::SseOutcome::Done;
                }
                "response.error" | "response.failed" => {
                    breaker.on_failure();
                    let msg = ev["message"].as_str().unwrap_or("Responses 流式错误");
                    let _ = tx.send(Err(LlmError::ServerError(msg.to_string())));
                    return crate::client::SseOutcome::Done;
                }
                _ => { /* response.created / in_progress / content_part.* / text.done / ping 等忽略 */ }
            }
        }
        if cancel.is_cancelled() {
            // 用户取消：投递 Cancelled 而非静默关流（引擎据此按「被取消」收尾）
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
                // 流中传输中断：诊断交调用方（由它决定自动重试还是上报），熔断计数也在那边
                return crate::client::SseOutcome::Transport(
                    meta.describe_transport_err(&e, received_bytes, sse_lines),
                );
            }
            None => {
                // EOF 未收到 completed：若收到字节却无任何有效 data: 报文（乱码 /
                // 错误页等），转显式空流诊断错误，避免静默产出空回复
                if received_bytes > 0 && sse_lines == 0 && !cancel.is_cancelled() {
                    breaker.on_failure();
                    let _ = tx.send(Err(LlmError::StreamError(
                        meta.describe_empty_stream(received_bytes, &raw_head),
                    )));
                    return crate::client::SseOutcome::Done;
                }
                // 正常 EOF 收尾
                if has_any_call {
                    let completed: Vec<ToolCall> = calls
                        .iter()
                        .map(|c| ToolCall::new(c.id.clone(), c.name.clone(), c.arguments.clone()))
                        .collect();
                    let _ = tx.send(Ok(LlmEvent::ToolCalls(completed)));
                }
                let _ = tx.send(Ok(LlmEvent::End(EndInfo {
                    finish_reason: "stop".to_string(),
                    usage: None,
                })));
                breaker.on_success();
                return crate::client::SseOutcome::Done;
            }
        }
    }
    // [DONE] 哨兵到达：与 EOF 同样做空流诊断（收到字节却无任何有效 data: 报文时
    // 报错而非静默成功——如乱码后被网关补发一个 [DONE] 的病理流）
    if received_bytes > 0 && sse_lines == 0 && !cancel.is_cancelled() {
        breaker.on_failure();
        let _ = tx.send(Err(LlmError::StreamError(
            meta.describe_empty_stream(received_bytes, &raw_head),
        )));
        return crate::client::SseOutcome::Done;
    }
    breaker.on_success();
    crate::client::SseOutcome::Done
}

/// 解析 `response.completed` 的 usage 块(input/output/total tokens)。
fn usage_from_completed(usage: &serde_json::Value) -> Option<TokenUsage> {
    if usage.is_null() {
        return None;
    }
    let prompt = usage["input_tokens"].as_u64().unwrap_or(0) as u32;
    let completion = usage["output_tokens"].as_u64().unwrap_or(0) as u32;
    let total = usage["total_tokens"].as_u64().unwrap_or(0) as u32;
    let mut u = TokenUsage::new(prompt, completion);
    if total != 0 {
        u.total = total;
    }
    Some(u)
}

/// `StatusCode → 是否为回退条件(端点未实现 / 方法不允许)`。
pub(crate) fn should_fallback(status: StatusCode) -> bool {
    matches!(status, StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED)
}