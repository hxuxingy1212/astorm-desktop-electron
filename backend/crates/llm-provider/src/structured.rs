//! 结构化输出：声明 JSON Schema → 模型按 schema 输出 → 解析 + 校验 → 失败自动修正重试。
//!
//! 统一使用 `response_format = json_object` + 指令 prompt（跨供应商通用，
//! DeepSeek 的 json_object 模式要求 prompt 中出现 "json" 字样，已自动满足）；
//! 需要 OpenAI `json_schema` 严格模式的场景可自行用 [`ChatRequest::with_response_format`] 定制。
//!
//! 重试次数等配置见 [`crate::StructuredConfig`]（并入 [`crate::ClientOption`]）。

use crate::{ChatMessage, ChatRequest, TokenUsage};
use serde_json::Value;

/// 结构化输出结果。
#[derive(Debug, Clone)]
pub struct StructuredOutput {
    /// 校验通过的结构化结果
    pub value: Value,
    /// 累计 token 用量
    pub usage: Option<TokenUsage>,
    /// 结束原因
    pub finish_reason: String,
    /// 实际调用次数（含修正重试）
    pub attempts: u32,
}

/// 向请求注入结构化输出指令：
/// 1. `response_format = {"type": "json_object"}`
/// 2. 追加指令内容——若请求已有 system 消息，将指令**追加到其内容之后**
///    （保留原有指引，如角色设定、回答风格等），否则新建一条 system 消息
pub(crate) fn inject_structured_prompt(mut request: ChatRequest, schema: &Value) -> ChatRequest {
    let instruction = format!(
        "\n\n你必须输出一个符合以下 JSON Schema 的 JSON 对象，不要输出 markdown 代码块或任何解释文字：\n{}",
        schema,
    );
    request = request.with_response_format(serde_json::json!({ "type": "json_object" }));

    if let Some(system) = request.messages.iter_mut().find(|m| m.role == "system") {
        // 已有 system 提示词：追加到其后，不替换原有指引
        match &mut system.content {
            Some(content) => content.push_str(&instruction),
            None => system.content = Some(instruction),
        }
    } else {
        request.messages.push(ChatMessage::system(instruction));
    }
    request
}

/// 构造修正重试消息：把上一次的错误信息喂回 LLM，要求重新输出。
pub(crate) fn correction_message(errors: &str) -> ChatMessage {
    ChatMessage::system(format!(
        "你上一次的输出不符合要求，错误如下：\n{}\n请严格按要求重新输出符合 JSON Schema 的 JSON。",
        errors,
    ))
}

/// 从模型输出中提取 JSON 主体（参考 summary_agent 的 extract_json）：
/// 1. 剥离 ```json / ``` markdown 围栏
/// 2. 无围栏时，取第一个 `{` / `[` 到最后一个 `}` / `]` 之间的内容
pub(crate) fn extract_json(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    // ```json ... ``` 或 ``` ... ```
    if let Some(start) = trimmed.find("```") {
        let rest = &trimmed[start + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        if let Some(end) = rest.find("```") {
            return Some(rest[..end].trim());
        }
    }
    // 无围栏：取第一个 { / [ 到最后一个 } / ] 之间的内容
    let start = trimmed.find(['{', '[']);
    let end = trimmed.rfind(['}', ']']);
    match (start, end) {
        (Some(s), Some(e)) if s < e => Some(trimmed[s..=e].trim()),
        _ => None,
    }
}

/// 提取 + 解析 + 校验模型输出：成功返回解析结果，失败返回错误信息（用于修正重试）。
pub(crate) fn parse_and_validate(
    content: &str,
    validator: &jsonschema::Validator,
) -> Result<Value, String> {
    let json_text = extract_json(content)
        .ok_or_else(|| format!("未找到 JSON 主体（原始输出: {}）", truncate(content, 200)))?;
    let value: Value = serde_json::from_str(json_text)
        .map_err(|e| format!("JSON 解析失败: {}（原文: {}）", e, truncate(json_text, 200)))?;
    let errors: Vec<String> = validator.iter_errors(&value).map(|e| e.to_string()).collect();
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(format!("输出不符合 JSON Schema: {}", errors.join("; ")))
    }
}

/// 截断过长的原始输出（错误信息中保留摘要）。
pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}
