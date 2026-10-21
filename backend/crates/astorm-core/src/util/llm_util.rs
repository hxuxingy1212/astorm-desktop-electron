//! llm_util：LLM 流式工具（仅流式，阻塞到完整输出，内容限定 JSON）。

use futures::StreamExt;
use llm_provider::{ChatClient, ChatMessage, ChatRequest, LlmEvent, TokenUsage};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

/// 注入到 system 提示的 JSON 输出要求。
///
/// DeepSeek 的 `json_object` 模式要求 prompt 含 "json" 字样，故显式注入。
const JSON_PROMPT_HINT: &str = "请以 JSON 格式输出最终结果。";

/// LLM 流式调用结果（阻塞到最终结果完整输出后返回）。
pub struct LlmJsonResult {
    /// 解析后的 JSON 内容
    pub value: Value,
    /// token 用量
    pub usage: TokenUsage,
    /// 结束原因（stop / length ...）
    pub finish_reason: String,
}

/// 阻塞流式调用 LLM 至完整输出，将内容解析为 JSON 返回。
///
/// - 仅使用流式接口 [`ChatClient::chat_stream`]，阻塞到 `End` 事件（完整输出）后返回
/// - 强制 `response_format = {"type":"json_object"}`，并向 system 提示注入
///   「请以 JSON 格式输出最终结果。」（DeepSeek 的 json_object 模式要求 prompt 含 "json" 字样）
/// - 剥离消息 `metadata`（内部状态不上送 LLM，与引擎约定一致）
pub async fn complete_json(
    client: &ChatClient,
    request: ChatRequest,
    cancel: CancellationToken,
) -> Result<LlmJsonResult, String> {
    if request.model.is_empty() {
        return Err("ChatRequest 缺少模型名（model 为空）".into());
    }

    // ① 强制 JSON 输出模式 + 注入 system 提示（覆盖调用方 response_format）
    let request = inject_json_hint(request);
    // ② 剥离消息 metadata（内部状态不上送）
    let request = strip_message_metadata(request);

    // ③ 仅流式：阻塞聚合到 End（完整输出）
    let model = request.model.clone();
    let mut stream = client
        .chat_stream(&model, request, cancel.child_token())
        .await
        .map_err(|e| format!("LLM 请求失败: {e}"))?;

    let mut content = String::new();
    let mut usage = TokenUsage::default();
    let mut finish_reason = String::new();
    while let Some(item) = stream.next().await {
        let event = item.map_err(|e| format!("LLM 流错误: {e}"))?;
        match event {
            LlmEvent::Chunk(c) => content.push_str(&c.content),
            // 流中传输中断后自动重开：作废重试前累积的半截文本（否则拼接成乱码）
            LlmEvent::Restart => content.clear(),
            LlmEvent::End(e) => {
                finish_reason = e.finish_reason;
                if let Some(u) = e.usage {
                    usage = u;
                }
                break;
            }
            LlmEvent::Start | LlmEvent::ReasoningChunk(_) | LlmEvent::ToolCalls(_) => {}
            LlmEvent::Error(msg) => return Err(format!("LLM 服务错误: {msg}")),
        }
    }

    // ④ 提取并解析 JSON
    let value = extract_json(&content).ok_or_else(|| {
        let shown: String = content.chars().take(200).collect();
        format!("LLM 输出不是合法 JSON: {shown}")
    })?;

    Ok(LlmJsonResult {
        value,
        usage,
        finish_reason,
    })
}

/// 强制 JSON 输出模式并注入「请以 JSON 格式输出」的 system 提示：
/// 已有 system 消息则追加到其末尾，否则前置一条新 system 消息。
fn inject_json_hint(mut request: ChatRequest) -> ChatRequest {
    request = request.with_response_format(json!({ "type": "json_object" }));
    let mut messages = request.messages;
    match messages.first_mut() {
        Some(first) if first.role == "system" => match first.content.as_mut() {
            Some(content) => content.push_str(&format!("\n\n{JSON_PROMPT_HINT}")),
            None => first.content = Some(JSON_PROMPT_HINT.to_string()),
        },
        _ => messages.insert(0, ChatMessage::system(JSON_PROMPT_HINT)),
    }
    request.messages = messages;
    request
}

/// 剥离每条消息的 `metadata`（内部状态不上送 LLM，与引擎约定一致）。
fn strip_message_metadata(mut request: ChatRequest) -> ChatRequest {
    for message in &mut request.messages {
        message.metadata = None;
    }
    request
}

/// 从 LLM 原始输出中提取 JSON：
/// 去 ` ```json ` 围栏后直接解析，失败回退取首 `{` 至末 `}` 的子串。
fn extract_json(content: &str) -> Option<Value> {
    let trimmed = content.trim();
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);

    serde_json::from_str(stripped).ok().or_else(|| {
        let start = stripped.find('{')?;
        let end = stripped.rfind('}')?;
        serde_json::from_str(&stripped[start..=end]).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plain_json() {
        let v = extract_json(r#"{"a": 1, "b": [true, null]}"#).unwrap();
        assert_eq!(v, json!({"a": 1, "b": [true, null]}));
    }

    #[test]
    fn extract_fenced_json() {
        let v = extract_json("```json\n{\"ok\": true}\n```").unwrap();
        assert_eq!(v, json!({"ok": true}));
    }

    #[test]
    fn extract_json_with_surrounding_text() {
        let v = extract_json("好的，结果如下：{\"msg\": \"hi\"} 以上。").unwrap();
        assert_eq!(v, json!({"msg": "hi"}));
    }

    #[test]
    fn extract_invalid_returns_none() {
        assert!(extract_json("这不是 JSON").is_none());
    }
}
