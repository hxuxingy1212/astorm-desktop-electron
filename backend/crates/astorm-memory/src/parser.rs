//! Parser：会话 JSONL → 归一化 Markdown（写入 `raw/sessions/`）。
//!
//! 输入契约由本 crate 自己拥有（[`TranscriptMessage`] / [`SessionTranscript`]），**不依赖
//! `astorm-common`**，以保持模块解耦；字段名与 astorm 应用落盘的 `message.jsonl`（camelCase）
//! 一致，故应用侧的 `message.jsonl` 可**直接**解析，而 CLI 也能对任意同构目录运行。
//!
//! 归一化要点：
//! - 跳过 `system` 消息（静态提示词 + 智能体指引是无信息噪声）
//! - 用户消息尾部截掉**注入块**（`【会话环境】` 与 `【相关记忆】`，取最早切点）——
//!   既避免每轮的 workspace / 工具 / MCP 列表灌进记忆，也防止注入的记忆被再次提取（自我喂养）
//! - 每条消息打 **`[mNN]` 锚点**，供提取阶段的 `frag_ids` 引用（可回溯到具体消息）
//! - 工具结果截断（工具输出是最大噪声源）；工具调用渲染为一行
//! - 会话摘要里的 `user_infos` 提升为 `# 历史用户画像` 块（复用已有 LLM 摘要的产出）
//! - 某行 JSONL 解析失败**不中断**整个文件（只计数并跳过）

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 会话环境块起始标记（由 `agent/prompt.rs::ensure_user_prompt` 追加在用户消息尾部）。
const ENV_BLOCK_MARKER: &str = "【会话环境】";
/// 相关记忆块起始标记（同上，紧随环境块之后）。
///
/// **必须一并剥离**：那是应用注入给模型的记忆摘要，若留在 raw 里就会被下一次编译重新提取，
/// 记忆将开始「自我喂养」并在多轮后漂移。两个标记取**最早切点**，不依赖谁排在前面。
const MEMORY_BLOCK_MARKER: &str = "【相关记忆】";
/// `raw/` frontmatter 里的来源类型（为后续外部 agent 导入留位）。
const SOURCE_KIND: &str = "astorm-session";

/// 单条会话消息（字段名与 astorm 应用 `message.jsonl` 一致）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub session_id: String,
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// 工具调用数组（OpenAI 形态：`[{id, type, function: {name, arguments}}]`）。
    #[serde(default)]
    pub tool_calls: serde_json::Value,
    #[serde(default)]
    pub seq: i64,
    /// 创建时间（unix **秒**）。
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

impl TranscriptMessage {
    /// 是否为 `metadata.summary == true` 的摘要消息。
    pub fn is_summary(&self) -> bool {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("summary"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
}

/// 一段会话（归一化输入）。
#[derive(Debug, Clone, Default)]
pub struct SessionTranscript {
    pub session_id: String,
    /// 会话标题（可选；无则用 session id 短号）。
    pub title: Option<String>,
    pub messages: Vec<TranscriptMessage>,
}

impl SessionTranscript {
    pub fn new(session_id: impl Into<String>, messages: Vec<TranscriptMessage>) -> Self {
        Self {
            session_id: session_id.into(),
            title: None,
            messages,
        }
    }
}

/// JSONL 解析结果：消息 + 被跳过的坏行数。
#[derive(Debug, Clone, Default)]
pub struct ParsedJsonl {
    pub messages: Vec<TranscriptMessage>,
    /// 解析失败被跳过的行数（容错：不中断整个文件）。
    pub bad_lines: usize,
}

/// 流式逐行解析 JSONL：单行失败只计数跳过（空行忽略）。
pub fn parse_jsonl(content: &str) -> ParsedJsonl {
    let mut out = ParsedJsonl::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptMessage>(line) {
            Ok(msg) => out.messages.push(msg),
            Err(_) => out.bad_lines += 1,
        }
    }
    // 按 seq 稳定排序（应用落盘本就递增，容错保证）
    out.messages.sort_by_key(|m| m.seq);
    out
}

/// 归一化产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSession {
    /// 文件名主干（`{date}-{sid8}`）。
    pub stem: String,
    pub session_id: String,
    /// 归一化 Markdown（frontmatter + 正文）。
    pub markdown: String,
    /// 锚点 → 该消息 `createdAt`（unix 秒）；提取阶段据此推导 `observed_at`。
    pub anchors: BTreeMap<String, i64>,
    /// 原始会话 JSONL 内容的 sha256（增量状态用）。
    pub sha256: String,
    /// 对话消息条数（不含被跳过的 system）。
    pub message_count: usize,
    /// 会话时间范围 `(最早, 最晚)` unix 秒；无有效时间戳为 None。
    pub time_range: Option<(i64, i64)>,
}

/// 会话归一化：返回 None 表示**无可用对话内容**（仅有 system / 空会话）。
pub fn normalize(
    transcript: &SessionTranscript,
    source_jsonl: &str,
    tool_result_chars: usize,
) -> Option<RawSession> {
    // ① 过滤 + 逐条渲染（锚点按**输出顺序**编号，保证正文里看到的锚点即有效锚点）
    let mut anchors: BTreeMap<String, i64> = BTreeMap::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut user_profile: Vec<String> = Vec::new();
    let mut timestamps: Vec<i64> = Vec::new();
    let mut counter = 0usize;

    for msg in &transcript.messages {
        let role = msg.role.trim().to_lowercase();
        if role == "system" {
            continue; // 静态提示词 + 智能体指引：无信息量
        }
        // 摘要消息：`user_infos` 提升为画像块（复用已有 LLM 摘要产出），正文渲染为**人类可读
        // 的 summary 文本**而非原始 JSON（原始 JSON 作为提取输入噪声过大，还会让同一批
        // user_infos 在正文里重复出现）。
        let is_summary = msg.is_summary();
        let mut rendered_role = role.clone();
        let body = if is_summary {
            rendered_role = "summary".to_string();
            user_profile.extend(extract_user_infos(&msg.content));
            match extract_summary_prose(&msg.content) {
                Some(prose) => format!("> 历史会话摘要（更早的对话已被压缩）\n\n{prose}"),
                // JSON 里没有可读摘要字段 → 退回原文（总比丢失信息好）
                None => render_message(msg, &role, tool_result_chars),
            }
        } else {
            render_message(msg, &role, tool_result_chars)
        };
        if body.trim().is_empty() {
            continue;
        }

        counter += 1;
        let anchor = format!("m{counter:02}");
        if msg.created_at > 0 {
            anchors.insert(anchor.clone(), msg.created_at);
            timestamps.push(msg.created_at);
        }
        blocks.push(format!("### [{anchor}] {rendered_role}\n\n{body}"));
    }

    if blocks.is_empty() {
        return None;
    }

    let message_count = blocks.len();
    let time_range = match (timestamps.iter().min(), timestamps.iter().max()) {
        (Some(min), Some(max)) => Some((*min, *max)),
        _ => None,
    };
    let sid8 = short_session_id(&transcript.session_id);
    let date = time_range
        .and_then(|(min, _)| iso_date(min))
        .unwrap_or_else(|| iso_date(now_secs()).unwrap_or_else(|| "0000-00-00".into()));
    let stem = format!("{date}-{sid8}");

    // ② 组装 Markdown
    let mut md = String::new();
    md.push_str("---\n");
    md.push_str(&format!("session_id: {}\n", yaml_str(&transcript.session_id)));
    md.push_str(&format!("source_kind: {SOURCE_KIND}\n"));
    if let Some(title) = transcript.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        md.push_str(&format!("title: {}\n", yaml_str(title)));
    }
    md.push_str(&format!("message_count: {message_count}\n"));
    if let Some((min, max)) = time_range {
        let start = iso_ts(min).unwrap_or_default();
        let end = iso_ts(max).unwrap_or_default();
        md.push_str(&format!("time_range: {start} ~ {end}\n"));
    }
    md.push_str(&format!("sha256: {}\n", sha256_hex(source_jsonl.as_bytes())));
    let parsed_at = iso_ts(now_secs()).unwrap_or_default();
    md.push_str(&format!("parsed_at: {parsed_at}\n"));
    md.push_str("---\n\n");

    let display_title = transcript
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| sid8.clone());
    md.push_str(&format!("# 会话 {display_title}\n"));

    if !user_profile.is_empty() {
        user_profile.sort();
        user_profile.dedup();
        md.push_str("\n# 历史用户画像\n\n");
        md.push_str("（来自历史会话摘要的 `user_infos` 沉淀，供提取用户偏好类记忆）\n\n");
        for item in &user_profile {
            md.push_str(&format!("- {item}\n"));
        }
    }

    md.push_str("\n# 对话\n\n");
    md.push_str(&blocks.join("\n\n"));
    md.push('\n');

    Some(RawSession {
        stem,
        session_id: transcript.session_id.clone(),
        markdown: md,
        anchors,
        sha256: sha256_hex(source_jsonl.as_bytes()),
        message_count,
        time_range,
    })
}

/// 单条消息 → Markdown 正文块。
///
/// 渲染以「**人看简洁、模型也读得懂**」为准：结构化 payload（工具参数 / 工具结果）折叠成
/// 代码块，正文里的长 JSON 单行包成围栏块（避免展开成一堵墙），其余保持原样 Markdown。
fn render_message(msg: &TranscriptMessage, role: &str, tool_result_chars: usize) -> String {
    let mut out = String::new();

    // 工具调用（assistant 消息）：折叠为每条一行的代码块，避免参数里的 JSON 混进正文
    for call in tool_calls_of(msg) {
        let args = shorten(&call.arguments, 400);
        out.push_str(&format!("调用 `{}` `{}`\n", call.name, args));
    }
    if !out.is_empty() {
        out = format!("```\n{}\n```", out.trim_end_matches('\n'));
    }

    let content = match role {
        "user" => strip_injected_blocks(&msg.content).trim().to_string(),
        "tool" => {
            let truncated = truncate_chars(msg.content.trim(), tool_result_chars);
            let id = msg.tool_call_id.as_deref().unwrap_or("-");
            let inner = block_or_indent(&truncated);
            return format!("**工具结果** `{id}`\n\n{inner}");
        }
        _ => msg.content.trim().to_string(),
    };
    if !content.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&content);
    }
    out.trim_end().to_string()
}

/// 结构化 payload 的块级包裹：多行或像 JSON/代码时用围栏代码块，否则按行加引用前缀。
fn block_or_indent(s: &str) -> String {
    let looks_structured = s.contains('\n') || starts_like_json(s);
    if looks_structured {
        let lang = if starts_like_json(s) { "json" } else { "" };
        format!("```{lang}\n{s}\n```")
    } else {
        s.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
    }
}

/// 行首是否为 JSON 容器（对象 / 数组）。用于把紧凑 JSON 包成围栏而不是任其撑成长行。
fn starts_like_json(s: &str) -> bool {
    matches!(s.trim_start().chars().next(), Some('{') | Some('['))
}

/// 从 `tool_calls` JSON 提取 (name, arguments)。
fn tool_calls_of(msg: &TranscriptMessage) -> Vec<ToolCallView> {
    let Some(array) = msg.tool_calls.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|call| {
            let function = call.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let arguments = function
                .get("arguments")
                .map(|a| match a {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(ToolCallView { name, arguments })
        })
        .collect()
}

struct ToolCallView {
    name: String,
    arguments: String,
}

/// 截掉用户消息尾部的**注入块**（`【会话环境】` / `【相关记忆】`）及其前导空行。
///
/// 取两个标记中**最早出现**的那个作为切点：顺序无关，谁在前面都切得对
/// （早先只按环境标记切，恰好因为记忆块排在它后面才没出问题 —— 那是顺序巧合，不是规则）。
fn strip_injected_blocks(content: &str) -> &str {
    let cut = [ENV_BLOCK_MARKER, MEMORY_BLOCK_MARKER]
        .iter()
        .filter_map(|marker| content.find(marker))
        .min();
    match cut {
        Some(idx) => content[..idx].trim_end_matches(['\n', '\r', ' ', '\t']),
        None => content,
    }
}

/// 从摘要消息内容里抽取人类可读的摘要正文（优先 `summary`，兼容 `summary_text`）。
///
/// 摘要消息落盘的是 `SessionSummary` 的 JSON；直接把 JSON 喂给提取器噪声大且会让
/// `user_infos` 在正文里重复出现，故只取摘要正文。
fn extract_summary_prose(content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(unwrap_json_fence(content)).ok()?;
    for key in ["summary", "summary_text", "summaryText"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// 去掉可能的 ```json 围栏。
fn unwrap_json_fence(content: &str) -> &str {
    let trimmed = content.trim();
    trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim().trim_end_matches("```").trim())
        .unwrap_or(trimmed)
}

/// 从摘要消息内容里抽取 `user_infos`（JSON 解析失败则返回空）。
fn extract_user_infos(content: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        user_infos: Vec<String>,
    }
    serde_json::from_str::<Probe>(unwrap_json_fence(content))
        .map(|p| {
            p.user_infos
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 由锚点集合 + `frag_ids` 推导观察日期（取最早时间戳的 UTC 日期）。
pub fn observed_at_from(anchors: &BTreeMap<String, i64>, frag_ids: &[String]) -> Option<String> {
    let earliest = frag_ids
        .iter()
        .filter_map(|id| anchors.get(id.trim()))
        .copied()
        .min()?;
    iso_date(earliest)
}

/// 会话 ID → 8 位短号（仅保留字母数字）。
fn short_session_id(session_id: &str) -> String {
    let alnum: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if alnum.is_empty() {
        "unknown0".to_string()
    } else {
        alnum.chars().take(8).collect()
    }
}

/// YAML 标量的安全引用（含特殊字符时用双引号包裹并转义）。
fn yaml_str(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value.chars().any(|c| {
            c.is_whitespace() || matches!(c, ':' | '#' | '\'' | '"' | '{' | '}' | '[' | ']' | ',' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`')
        });
    if needs_quote {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// 内容 sha256（十六进制）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// unix 秒 → UTC 日期（`YYYY-MM-DD`）。
pub fn iso_date(secs: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// unix 秒 → RFC3339（UTC）。
pub fn iso_ts(secs: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// 当前 unix 秒。
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 当前时间 RFC3339（UTC，秒精度）。
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// 按字符截断（UTF-8 边界安全），超出时追加省略标记。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}\n…（已截断，原文 {} 字符）", s.chars().count())
}

/// 单行压缩到指定字符数（工具参数等）。
fn shorten(s: &str, max_chars: usize) -> String {
    let one_line = s.replace(['\n', '\r'], " ");
    if one_line.chars().count() <= max_chars {
        one_line
    } else {
        let head: String = one_line.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str, seq: i64, created_at: i64) -> TranscriptMessage {
        TranscriptMessage {
            id: format!("id-{seq}"),
            session_id: "018f0e2a-0000-7000-8000-000000000001".into(),
            role: role.into(),
            content: content.into(),
            tool_calls: serde_json::Value::Null,
            seq,
            created_at,
            tool_call_id: None,
            metadata: None,
        }
    }

    fn transcript(messages: Vec<TranscriptMessage>) -> SessionTranscript {
        SessionTranscript::new("018f0e2a-0000-7000-8000-000000000001", messages)
    }

    #[test]
    fn normalizes_basic_conversation() {
        let t = transcript(vec![
            msg("system", "你是 astorm……", 0, 1_760_000_000),
            msg("user", "帮我看看认证方案", 1, 1_760_000_100),
            msg("assistant", "建议用 JWT。", 2, 1_760_000_200),
        ]);
        let raw = normalize(&t, "raw-jsonl", 1_200).unwrap();

        assert_eq!(raw.message_count, 2, "system 应被跳过");
        assert_eq!(raw.stem, "2025-10-09-018f0e2a");
        assert!(raw.markdown.contains("# 对话"));
        assert!(raw.markdown.contains("### [m01] user"));
        assert!(raw.markdown.contains("### [m02] assistant"));
        assert!(!raw.markdown.contains("你是 astorm"), "system 内容不得进入正文");
        assert!(raw.markdown.contains("帮我看看认证方案"));
        // 锚点映射到时间戳
        assert_eq!(raw.anchors.get("m01"), Some(&1_760_000_100));
        assert_eq!(raw.anchors.get("m02"), Some(&1_760_000_200));
        assert_eq!(raw.time_range, Some((1_760_000_100, 1_760_000_200)));
        // frontmatter 记录来源与指纹
        assert!(raw.markdown.starts_with("---\nsession_id: 018f0e2a-0000-7000-8000-000000000001\n"));
        assert!(raw.markdown.contains("source_kind: astorm-session"));
        assert!(raw.markdown.contains(&format!("sha256: {}", sha256_hex(b"raw-jsonl"))));
        assert_eq!(raw.sha256, sha256_hex(b"raw-jsonl"));
    }

    /// 用户消息尾部的【会话环境】块必须截掉，否则每轮的 workspace/工具列表都会灌进记忆。
    #[test]
    fn strips_session_env_block_from_user_messages() {
        let content = "帮我修个 bug\n\n【会话环境】\n- 会话模式：execute\n- 工作区：/w/x\n- 可用工具：bash, read_file";
        let t = transcript(vec![msg("user", content, 1, 1_760_000_100)]);
        let raw = normalize(&t, "x", 1_200).unwrap();

        assert!(raw.markdown.contains("帮我修个 bug"));
        assert!(!raw.markdown.contains("【会话环境】"), "环境块标记应被截掉");
        assert!(!raw.markdown.contains("/w/x"), "工作区噪声不得进入记忆");
        assert!(!raw.markdown.contains("可用工具"));
    }

    /// 注入的【相关记忆】块同样必须截掉 —— 否则记忆会「自我喂养」（注入内容被下次编译再提取）。
    #[test]
    fn strips_injected_memory_block_from_user_messages() {
        // 常态：环境块在前，记忆块紧随其后（app 侧 `ensure_user_prompt` 的追加顺序）
        let content = "我们之前定过认证选型吗\n\n【会话环境】\n- 会话模式：plan\n\n\
                       【相关记忆】\n以下是跨会话沉淀的记忆：\n- decisions/认证方案选型 [Decision] 用 JWT";
        let t = transcript(vec![msg("user", content, 1, 1_760_000_100)]);
        let raw = normalize(&t, "x", 1_200).unwrap();

        assert!(raw.markdown.contains("我们之前定过认证选型吗"), "{}", raw.markdown);
        assert!(!raw.markdown.contains("【相关记忆】"), "记忆块标记应被截掉");
        assert!(!raw.markdown.contains("以下是跨会话沉淀的记忆"), "注入文案不得进入 raw");
        assert!(!raw.markdown.contains("decisions/认证方案选型"), "注入的记忆不得回流成来源");
        assert!(!raw.markdown.contains("【会话环境】"));
    }

    /// 顺序无关：只按最早切点截断（早先只认环境标记，恰好因为记忆块排在它后面才没出问题）。
    #[test]
    fn injected_blocks_are_stripped_regardless_of_order() {
        let memory_first = "问题\n\n【相关记忆】\n- a\n\n【会话环境】\n- 模式：plan";
        let t = transcript(vec![msg("user", memory_first, 1, 1_760_000_100)]);
        let raw = normalize(&t, "x", 1_200).unwrap();
        assert!(raw.markdown.contains("问题"));
        assert!(!raw.markdown.contains("【相关记忆】"));
        assert!(!raw.markdown.contains("模式：plan"), "{}", raw.markdown);

        // 只有记忆块（假设某天环境块不再注入）也要截掉
        let only_memory = "问题\n\n【相关记忆】\n- b";
        let t2 = transcript(vec![msg("user", only_memory, 1, 1_760_000_100)]);
        let raw2 = normalize(&t2, "x", 1_200).unwrap();
        assert!(raw2.markdown.contains("问题"));
        assert!(!raw2.markdown.contains("【相关记忆】"));
    }

    #[test]
    fn truncates_long_tool_results() {
        let long = "行".repeat(5_000);
        let mut tool = msg("tool", &long, 3, 1_760_000_300);
        tool.tool_call_id = Some("call_1".into());
        let t = transcript(vec![
            msg("user", "查日志", 1, 1_760_000_100),
            tool,
        ]);
        let raw = normalize(&t, "x", 100).unwrap();

        assert!(raw.markdown.contains("**工具结果** `call_1`"));
        assert!(raw.markdown.contains("已截断"), "超长工具结果应被截断");
        // 正文里不应出现完整 5000 字符
        assert!(!raw.markdown.contains(&long), "不得写入完整工具输出");
    }

    #[test]
    fn renders_tool_calls_as_code_block() {
        let mut assistant = msg("assistant", "我来查一下", 2, 1_760_000_200);
        assistant.tool_calls = serde_json::json!([
            {"id": "call_1", "type": "function",
             "function": {"name": "bash", "arguments": "{\"command\":\"ls -la\"}"}},
        ]);
        let t = transcript(vec![msg("user", "列出文件", 1, 1_760_000_100), assistant]);
        let raw = normalize(&t, "x", 1_200).unwrap();

        assert!(raw.markdown.contains("调用 `bash`"), "{}", raw.markdown);
        assert!(raw.markdown.contains("ls -la"));
        assert!(raw.markdown.contains("我来查一下"));
        // 工具调用应折叠进代码块，而不是散在正文里
        assert!(raw.markdown.contains("```\n调用 `bash`"), "{}", raw.markdown);
    }

    /// 紧凑 JSON 工具结果应被包成围栏代码块，避免渲染成一堵长墙。
    #[test]
    fn wraps_json_tool_result_in_fence() {
        let mut tool = msg("tool", "{\"stdout\":\"ok\",\"returncode\":0}", 3, 1_760_000_300);
        tool.tool_call_id = Some("call_9".into());
        let t = transcript(vec![msg("user", "跑命令", 1, 1_760_000_100), tool]);
        let raw = normalize(&t, "x", 1_200).unwrap();

        assert!(raw.markdown.contains("**工具结果** `call_9`"), "{}", raw.markdown);
        // 前后各有围栏：`json 开头，` 结尾
        assert!(raw.markdown.contains("```json\n{\"stdout\""), "{}", raw.markdown);
        assert!(raw.markdown.contains("\"returncode\":0}\n```"), "{}", raw.markdown);
    }

    /// 纯文本工具结果单行 → 引用行；多行 → 围栏块。
    #[test]
    fn formats_plain_tool_result() {
        let mut single = msg("tool", "no diff", 3, 1_760_000_300);
        single.tool_call_id = Some("call_a".into());
        let mut multi = msg("tool", "line1\nline2", 4, 1_760_000_400);
        multi.tool_call_id = Some("call_b".into());
        let t = transcript(vec![msg("user", "跑命令", 1, 1_760_000_100), single, multi]);
        let raw = normalize(&t, "x", 1_200).unwrap();

        assert!(raw.markdown.contains("> no diff"), "{}", raw.markdown);
        assert!(raw.markdown.contains("```\nline1\nline2\n```"), "{}", raw.markdown);
    }

    /// 摘要消息的 user_infos 应提升为独立的用户画像块（复用已有 LLM 摘要产出）。
    #[test]
    fn lifts_user_infos_from_summary_messages() {
        let summary = serde_json::json!({
            "summary": "用户在做 electron 项目",
            "user_infos": ["偏好中文注释", "习惯先规划后执行", "偏好中文注释"]
        })
        .to_string();
        let mut summary_msg = msg("user", &summary, 1, 1_760_000_100);
        summary_msg.metadata = Some(serde_json::json!({"summary": true}));
        let t = transcript(vec![summary_msg, msg("user", "继续", 2, 1_760_000_200)]);
        let raw = normalize(&t, "x", 1_200).unwrap();

        assert!(raw.markdown.contains("# 历史用户画像"), "{}", raw.markdown);
        // 画像块内去重
        let profile = raw
            .markdown
            .split("# 历史用户画像")
            .nth(1)
            .expect("应有画像块")
            .split("# 对话")
            .next()
            .unwrap();
        assert!(profile.contains("- 偏好中文注释"), "{profile}");
        assert!(profile.contains("- 习惯先规划后执行"), "{profile}");
        assert_eq!(profile.matches("偏好中文注释").count(), 1, "画像块内应去重: {profile}");

        // 摘要消息渲染为人类可读正文，而非原始 JSON
        assert!(raw.markdown.contains("### [m01] summary"), "{}", raw.markdown);
        assert!(raw.markdown.contains("用户在做 electron 项目"), "{}", raw.markdown);
        let dialogue = raw.markdown.split("# 对话").nth(1).expect("应有对话段");
        assert!(!dialogue.contains("user_infos"), "原始 JSON 不得进入对话正文: {dialogue}");
        assert!(!dialogue.contains('{'), "对话正文不应含 JSON 花括号: {dialogue}");
        assert!(!dialogue.contains("偏好中文注释"), "user_infos 不应在对话段重复出现: {dialogue}");
    }

    #[test]
    fn tolerates_non_summary_json_content() {
        let mut m = msg("user", "{\"not\": \"a summary\"}", 1, 1_760_000_100);
        m.metadata = Some(serde_json::json!({"summary": true}));
        let t = transcript(vec![m]);
        let raw = normalize(&t, "x", 1_200).unwrap();
        assert!(!raw.markdown.contains("# 历史用户画像"));
        assert!(raw.markdown.contains("not"));
    }

    #[test]
    fn empty_or_system_only_session_returns_none() {
        assert!(normalize(&transcript(vec![]), "x", 1_200).is_none());
        assert!(normalize(&transcript(vec![msg("system", "只有提示词", 0, 1)]), "x", 1_200).is_none());
        // 空白正文也算空
        assert!(normalize(&transcript(vec![msg("assistant", "   \n  ", 1, 1)]), "x", 1_200).is_none());
    }

    #[test]
    fn jsonl_parsing_tolerates_bad_lines() {
        let jsonl = concat!(
            "{\"role\":\"user\",\"content\":\"你好\",\"seq\":1,\"createdAt\":100}\n",
            "{ 这不是合法 JSON\n",
            "\n",
            "{\"role\":\"assistant\",\"content\":\"你好\",\"seq\":2,\"createdAt\":200}\n",
        );
        let parsed = parse_jsonl(jsonl);
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.bad_lines, 1, "坏行只计数不中断");
        assert_eq!(parsed.messages[0].role, "user");
    }

    #[test]
    fn jsonl_parsing_sorts_by_seq_and_defaults_missing_fields() {
        let jsonl = concat!(
            "{\"role\":\"assistant\",\"content\":\"后\",\"seq\":9}\n",
            "{\"role\":\"user\",\"content\":\"先\",\"seq\":1}\n",
        );
        let parsed = parse_jsonl(jsonl);
        assert_eq!(parsed.messages[0].seq, 1, "应按 seq 排序");
        assert_eq!(parsed.messages[0].created_at, 0, "缺省字段回退默认");
        assert!(parsed.messages[0].tool_calls.is_null());
    }

    /// 真实应用落盘的一行（字段齐全）应能直接解析。
    #[test]
    fn parses_real_app_message_record_shape() {
        let line = r#"{"id":"018f0e2a-0000-7000-8000-000000000001:tool:call_1","sessionId":"018f0e2a-0000-7000-8000-000000000001","role":"tool","content":"file listing...","toolCalls":null,"seq":5,"createdAt":1730000001,"reasoningContent":null,"toolCallId":"call_1","metadata":{"status":"ok","started_at":1730000001456,"ended_at":1730000002000}}"#;
        let parsed = parse_jsonl(line);
        assert_eq!(parsed.bad_lines, 0, "应用真实记录必须可解析");
        let m = &parsed.messages[0];
        assert_eq!(m.role, "tool");
        assert_eq!(m.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(m.created_at, 1730000001);
        assert_eq!(m.session_id, "018f0e2a-0000-7000-8000-000000000001");
    }

    #[test]
    fn observed_at_is_derived_from_earliest_fragment() {
        let mut anchors = BTreeMap::new();
        anchors.insert("m01".to_string(), 1_760_000_100); // 2025-10-09
        anchors.insert("m02".to_string(), 1_760_086_500); // 2025-10-10
        assert_eq!(
            observed_at_from(&anchors, &["m02".into(), "m01".into()]),
            Some("2025-10-09".into()),
            "应取最早时间戳的日期"
        );
        assert_eq!(observed_at_from(&anchors, &[]), None);
        assert_eq!(observed_at_from(&anchors, &["m99".into()]), None, "无效锚点应被忽略");
    }

    #[test]
    fn anchors_are_numbered_by_output_order() {
        let t = transcript(vec![
            msg("system", "x", 0, 10),
            msg("user", "第一条", 1, 100),
            msg("user", "第二条", 2, 200),
        ]);
        let raw = normalize(&t, "x", 1_200).unwrap();
        assert!(raw.markdown.contains("### [m01] user"));
        assert!(raw.markdown.contains("### [m02] user"));
        assert_eq!(raw.anchors.len(), 2);
        assert!(!raw.anchors.contains_key("m00"), "锚点从 m01 起编号");
    }

    #[test]
    fn stem_falls_back_when_no_timestamps() {
        let t = transcript(vec![msg("user", "无时间戳", 1, 0)]);
        let raw = normalize(&t, "x", 1_200).unwrap();
        assert!(raw.time_range.is_none());
        // 无时间戳时用当天日期，短号仍取自 session id
        assert!(raw.stem.ends_with("-018f0e2a"), "{}", raw.stem);
        assert_eq!(raw.anchors.len(), 0, "createdAt=0 不建锚点时间映射");
    }

    #[test]
    fn session_without_alnum_id_falls_back() {
        let t = SessionTranscript::new("----", vec![msg("user", "hi", 1, 1_760_000_100)]);
        let raw = normalize(&t, "x", 1_200).unwrap();
        assert!(raw.stem.ends_with("-unknown0"), "{}", raw.stem);
    }

    #[test]
    fn yaml_quoting_handles_special_chars() {
        assert_eq!(yaml_str("simple"), "simple");
        assert_eq!(yaml_str("has space"), "\"has space\"");
        assert_eq!(yaml_str("has: colon"), "\"has: colon\"");
        assert_eq!(yaml_str(""), "\"\"");
        assert_eq!(yaml_str("中文标题"), "中文标题");

        // 带特殊字符的标题不破坏 frontmatter（整体可被 YAML 解析）
        let mut t = transcript(vec![msg("user", "内容", 1, 1_760_000_100)]);
        t.title = Some("标题: 带冒号 # 与井号".into());
        let raw = normalize(&t, "x", 1_200).unwrap();
        let (yaml, _) = crate::concept::split_frontmatter(&raw.markdown).unwrap();
        let parsed: BTreeMap<String, serde_yaml::Value> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            parsed.get("title").and_then(|v| v.as_str()),
            Some("标题: 带冒号 # 与井号")
        );
    }

    #[test]
    fn raw_markdown_frontmatter_is_parseable() {
        let mut t = transcript(vec![msg("user", "问", 1, 1_760_000_100)]);
        t.title = Some("鉴权讨论".into());
        let raw = normalize(&t, "x", 1_200).unwrap();
        let (yaml, body) = crate::concept::split_frontmatter(&raw.markdown).unwrap();
        let parsed: BTreeMap<String, serde_yaml::Value> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.get("title").and_then(|v| v.as_str()), Some("鉴权讨论"));
        assert_eq!(parsed.get("message_count").and_then(|v| v.as_i64()), Some(1));
        assert!(body.contains("# 对话"));
    }

    #[test]
    fn truncate_and_shorten_helpers() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert!(truncate_chars("abcde", 2).starts_with("ab"));
        assert!(truncate_chars("abcde", 2).contains("已截断"));
        assert_eq!(shorten("a\nb", 10), "a b");
        assert!(shorten("abcdef", 3).ends_with('…'));
    }

    #[test]
    fn iso_helpers() {
        assert_eq!(iso_date(1_760_000_100).as_deref(), Some("2025-10-09"));
        assert_eq!(iso_ts(1_760_000_100).as_deref(), Some("2025-10-09T08:55:00Z"));
        assert_eq!(iso_date(i64::MAX), None, "非法时间戳应返回 None");
        assert!(now_rfc3339().ends_with('Z'));
    }
}
