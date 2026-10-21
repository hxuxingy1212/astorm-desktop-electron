//! 会话标题生成：基于用户首条消息 / 摘要生成中英文双标题。
//!
//! 首次创建会话时（首条消息即创建消息）`title` 为空，会话层经 [`generate_titles`]
//! 用一次 LLM 调用同时提取中文与英文标题后写回记录，并经 [`title_event`] 发 `title`
//! 事件通知前端；摘要压缩完成后也重新生成一对标题并替换。
//! 失败仅告警降级（不阻塞会话创建、运行或摘要）。
//! 复用 `util::llm_util::complete_json`（流式 + 强制 json_object + 剥 metadata）。

use std::time::{SystemTime, UNIX_EPOCH};

use llm_provider::{ChatClient, ChatMessage, ChatRequest};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

/// 业务层专属事件类型：会话标题生成完成（与前端协议 `docs/开发规范.md` §8 对应；
/// 业务层事件由业务侧定义构造器，不进入 `react_loop::event`）。
pub const TITLE_EVENT: &str = "title";

/// 当前 unix 毫秒时间戳（事件时间，与前端 `Date.now()` 对齐；
/// `react_loop::event::now_ms` 为私有，此处按相同约定复刻）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 中英文双标题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTitles {
    pub zh: String,
    pub en: String,
}

/// 标题生成完成事件：`{ type, session_id, timestamp, title, title_zh, title_en }`。
/// `title` 保留中文版本兼容旧消费者；新前端按 `title_zh/title_en` 跟随语言偏好。
pub fn title_event(session_id: &str, titles: &SessionTitles) -> Value {
    json!({
        "type": TITLE_EVENT,
        "session_id": session_id,
        "timestamp": now_ms(),
        "title": titles.zh,
        "title_zh": titles.zh,
        "title_en": titles.en,
    })
}

/// 标题生成系统提示（纯静态，仅标题任务使用，不经会话主循环）。
const TITLE_SYSTEM_PROMPT: &str =
    "你是会话标题生成助手。根据用户消息或会话摘要，分别提取一个简短、准确的中文标题和自然英文标题，概括当前任务主题。";
/// 中文标题最大长度（防御性截断）。
const TITLE_MAX_CHARS_ZH: usize = 30;
/// 英文标题最大长度（英文按字符截断更宽松）。
const TITLE_MAX_CHARS_EN: usize = 60;

/// 基于用户首条消息 / 摘要生成中英文标题（单次 LLM 调用）。
///
/// 返回 `Ok(SessionTitles)` 或 `Err`（由调用方降级处理，不中断主流程）。
pub async fn generate_titles(
    client: &ChatClient,
    model: &str,
    source: &str,
    cancel: CancellationToken,
) -> Result<SessionTitles, String> {
    let messages = vec![
        ChatMessage::system(TITLE_SYSTEM_PROMPT),
        ChatMessage::user(format!(
            "请根据下面的用户消息/会话摘要生成中英文会话标题：\n\n{source}\n\n\
             严格输出 JSON：{{\"title_zh\": \"中文标题\", \"title_en\": \"English title\"}}。\
             英文标题要自然，不要逐字直译。"
        )),
    ];
    let request = ChatRequest::new(messages).with_model(model);
    let result = crate::util::llm_util::complete_json(client, request, cancel).await?;
    parse_titles(&result.value)
}

/// 解析 LLM 返回的双标题；缺失一侧时用另一侧兜底（保证两字段都有值）。
fn parse_titles(value: &Value) -> Result<SessionTitles, String> {
    let pick = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let zh = pick("title_zh")
        .or_else(|| pick("titleZh"))
        .or_else(|| pick("title"));
    let en = pick("title_en").or_else(|| pick("titleEn"));
    let (zh, en) = match (zh, en) {
        (Some(zh), Some(en)) => (zh, en),
        (Some(zh), None) => (zh.clone(), zh),
        (None, Some(en)) => (en.clone(), en),
        (None, None) => return Err(format!("标题解析失败，原始返回: {value}")),
    };
    Ok(SessionTitles {
        zh: truncate(&zh, TITLE_MAX_CHARS_ZH),
        en: truncate(&en, TITLE_MAX_CHARS_EN),
    })
}

/// UTF-8 安全截断（按字符）。
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_titles() {
        let titles = parse_titles(&json!({
            "title_zh": "季度复盘",
            "title_en": "Quarterly Review"
        }))
        .unwrap();
        assert_eq!(titles.zh, "季度复盘");
        assert_eq!(titles.en, "Quarterly Review");
    }

    #[test]
    fn missing_side_falls_back_to_other() {
        let only_zh = parse_titles(&json!({ "title_zh": "季度复盘" })).unwrap();
        assert_eq!(only_zh.zh, "季度复盘");
        assert_eq!(only_zh.en, "季度复盘");
        let only_en = parse_titles(&json!({ "title_en": "Quarterly Review" })).unwrap();
        assert_eq!(only_en.zh, "Quarterly Review");
        assert_eq!(only_en.en, "Quarterly Review");
        assert!(parse_titles(&json!({})).is_err());
    }

    #[test]
    fn legacy_or_camel_case_keys_still_parse() {
        // 模型没按新 schema 输出时，至少把单标题兜住（两侧都回退同一份）
        let legacy = parse_titles(&json!({ "title": "旧格式标题" })).unwrap();
        assert_eq!(legacy.zh, "旧格式标题");
        assert_eq!(legacy.en, "旧格式标题");
        let camel = parse_titles(&json!({ "titleZh": "驼峰标题", "titleEn": "Camel Title" })).unwrap();
        assert_eq!(camel.zh, "驼峰标题");
        assert_eq!(camel.en, "Camel Title");
    }

    #[test]
    fn truncates_by_chars() {
        let long = "一".repeat(TITLE_MAX_CHARS_ZH + 5);
        let titles = parse_titles(&json!({ "title_zh": long.clone(), "title_en": "x" })).unwrap();
        assert_eq!(titles.zh.chars().count(), TITLE_MAX_CHARS_ZH);
    }
}
