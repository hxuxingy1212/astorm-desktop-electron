//! 会话消息剪支：摘要前对**较旧且超长**的工具调用参数/结果做截断，原内容归档到磁盘，
//! 便于 LLM 读过记号后回溯原消息。
//!
//! - 触发：会话历史估算上下文字符数（纯字符 Σcontent.len + Σarguments.len）≥ `compact_threshold`。
//! - 范围：只处理最近 `compact_recent_rounds` 轮**之前**的 `tool` 参数 / 结果消息
//!   （近轮完整保留，避免裁剪正在进行的上下文）。
//! - 判据：初判整条消息字符数 > `compact_byte_count`，再对工具参数 / 工具结果各自
//!   > `compact_byte_count` 才做剪支。
//! - 替换：前 `compact_head_chars` 字符 + `...(truncated)` + 后 `compact_tail_chars` 字符，
//!   末尾追加 `[完整日志: <路径>]`；完整原文写入 `compact_{toolname}_{uuid}.log`。
//! - 约束：**不改持久化 jsonl**（追加式只写），仅改内存历史与发送载荷；重启后从存储恢复原样
//!   再重新触发；已剪支的消息在 `metadata.compact = true` 标记，之后跳过。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use llm_provider::ChatMessage;
use react_loop::LoopContext;

use super::session::Session;
use super::summary::{is_ephemeral_msg, session_data_dir, tail_len_by_rounds};
use crate::app::CompactConfig;

/// 剪支主入口（由 `ReactLoop::before_loop` 在 `repair_tool_calls` / 摘要之前调用）。
///
/// 返回 `true` 表示本次处理过消息；未达阈值 / 无可剪范围 / 无超长消息 / 失败返回 `false`（或上抛）。
pub(crate) async fn compact_if_needed(
    session: &Session,
    ctx: &mut LoopContext,
) -> Result<bool, String> {
    let cfg = session
        .compact
        .read()
        .map_err(|_| "剪支配置锁中毒".to_string())?
        .clone();
    let dir = session_data_dir(&ctx.session_id);
    let modified = {
        let mut history = session
            .history
            .lock()
            .map_err(|_| "会话历史锁中毒".to_string())?;
        if context_usage(&history) < cfg.compact_threshold {
            return Ok(false);
        }
        let kept = tail_len_by_rounds(&history, cfg.compact_recent_rounds);
        if kept == 0 {
            return Ok(false);
        }
        let up_to = history.len() - kept;
        compact_range(&mut history[..up_to], &dir, &cfg)
    };
    if modified == 0 {
        return Ok(false);
    }
    // 同步发送载荷：重建为「剪支后的历史 + 工具注入消息」。
    // 不按下标逐条覆盖——`repair_tool_calls`（loop_start 末尾）中插的占位 tool 消息
    // 会打乱下标；这些占位本轮 loop_start 末尾会按最终载荷重新生成，故此处直接丢弃重建。
    {
        let history = session
            .history
            .lock()
            .map_err(|_| "会话历史锁中毒".to_string())?
            .clone();
        let injected: Vec<ChatMessage> = ctx
            .chat_request
            .messages
            .iter()
            .filter(|m| is_ephemeral_msg(m))
            .cloned()
            .collect();
        let mut messages = history;
        messages.extend(injected);
        ctx.chat_request.messages = messages;
    }
    tracing::info!(
        "[session {}] 剪支完成: 处理 {} 条超长消息",
        ctx.session_id,
        modified,
    );
    Ok(true)
}

/// 对切片内消息做剪支（纯逻辑，便于单测）：返回处理后消息条数。
fn compact_range(messages: &mut [ChatMessage], dir: &Path, cfg: &CompactConfig) -> usize {
    // 建立 tool_call_id → 工具名映射（tool 消息结果需反查归属工具）。
    let mut name_by_call: HashMap<String, String> = HashMap::new();
    for m in messages.iter() {
        if let Some(tcs) = &m.tool_calls {
            for tc in tcs {
                name_by_call.insert(tc.id.clone(), tc.function.name.clone());
            }
        }
    }

    let mut modified = 0usize;
    for m in messages.iter_mut() {
        // 已剪支的消息不再判断和处理（防重复）。
        if is_compacted(m) {
            continue;
        }
        if m.role == "assistant" {
            // 细判：工具参数（arguments）超长才剪。
            let mut cut = false;
            if let Some(tcs) = &mut m.tool_calls {
                for tc in tcs.iter_mut() {
                    if tc.function.arguments.len() > cfg.compact_byte_count {
                        let full = std::mem::take(&mut tc.function.arguments);
                        let name = sanitize_name(&tc.function.name);
                        let path = write_backup(dir, &name, &full);
                        tc.function.arguments = append_log(truncate_content(&full, cfg), &path);
                        cut = true;
                        modified += 1;
                    }
                }
            }
            if cut {
                mark_compacted(m);
            }
        } else if m.role == "tool" {
            // 细判：工具结果（content）超长才剪。
            let over = m
                .content
                .as_ref()
                .map(|c| c.len() > cfg.compact_byte_count)
                .unwrap_or(false);
            if over {
                let full = m.content.clone().unwrap_or_default();
                let name = m
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| name_by_call.get(id))
                    .cloned()
                    .unwrap_or_else(|| "unknown".into());
                let path = write_backup(dir, &sanitize_name(&name), &full);
                m.content = Some(append_log(truncate_content(&full, cfg), &path));
                mark_compacted(m);
                modified += 1;
            }
        }
    }
    modified
}

/// 估算会话历史上下文字符数（纯字符：Σcontent.len + Σarguments.len）。
fn context_usage(history: &[ChatMessage]) -> usize {
    history
        .iter()
        .map(|m| {
            let content = m.content.as_deref().map(|s| s.len()).unwrap_or(0);
            let args: usize = m
                .tool_calls
                .as_ref()
                .map(|tcs| tcs.iter().map(|tc| tc.function.arguments.len()).sum())
                .unwrap_or(0);
            content + args
        })
        .sum()
}

/// 取整段内容的前后字符（纯字符，无换行）：`前 head_chars 字符 + ...(truncated) + 后 tail_chars 字符`。
fn truncate_content(full: &str, cfg: &CompactConfig) -> String {
    let chars: Vec<char> = full.chars().collect();
    // 尾部起点（留出尾部保留区），头部截断到该点之前，避免头尾重叠。
    let tail_from = chars.len().saturating_sub(cfg.compact_tail_chars);
    let head_end = cfg.compact_head_chars.min(tail_from);
    let mut out = String::new();
    out.extend(chars[..head_end].iter());
    out.push_str("...(truncated)");
    out.extend(chars[tail_from..].iter());
    out
}

/// 截断正文末尾追加完整日志路径标记。
fn append_log(truncated: String, path: &Path) -> String {
    format!("{truncated}\n[完整日志: {}]", path.display())
}

/// 写归档原文件：`<dir>/compact_{toolname}_{uuid}.log`，返回其路径（失败仅告警，不阻断）。
fn write_backup(dir: &Path, toolname: &str, content: &str) -> PathBuf {
    let id = uuid::Uuid::new_v4();
    let path = dir.join(format!("compact_{toolname}_{id}.log"));
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("[compact] 创建归档目录失败: {e}");
        return path;
    }
    if let Err(e) = std::fs::write(&path, content) {
        tracing::warn!("[compact] 写归档文件失败: {e}");
    }
    path
}

/// 是否已做过剪支（`metadata.compact == true`）。
fn is_compacted(m: &ChatMessage) -> bool {
    m.metadata
        .as_ref()
        .and_then(|v| v.get("compact").and_then(|c| c.as_bool()))
        .unwrap_or(false)
}

/// 标记消息已剪支（合并进已有 metadata，不覆盖其它键）。
fn mark_compacted(m: &mut ChatMessage) {
    let meta = m
        .metadata
        .get_or_insert_with(|| serde_json::json!({}));
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("compact".into(), serde_json::Value::Bool(true));
    }
}

/// 工具名清理为安全文件名字符（字母/数字/下划线/连字符，其余替换为下划线）。
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CompactConfig {
        CompactConfig {
            compact_threshold: 32_768,
            compact_recent_rounds: 5,
            compact_byte_count: 10_240,
            compact_head_chars: 200,
            compact_tail_chars: 400,
        }
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astorm_compact_{}_{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn truncate_content_keeps_head_tail() {
        let c = cfg();
        // 构造 > 10240 字符的内容
        let full = "x".repeat(10241);
        let t = truncate_content(&full, &c);
        assert!(t.contains("...(truncated)"));
        assert!(t.starts_with("x".repeat(c.compact_head_chars).as_str()));
        assert!(t.ends_with("x".repeat(c.compact_tail_chars).as_str()));
        // 长度 = 头部字符 + "...(truncated)" + 尾部字符（纯字符，无换行）
        let marker = "...(truncated)";
        assert_eq!(
            t.len(),
            c.compact_head_chars + marker.len() + c.compact_tail_chars
        );
    }

    #[test]
    fn context_usage_counts_content_and_args() {
        let mut m = ChatMessage::user("hello");
        m.tool_calls = Some(vec![llm_provider::ToolCall::new("c1", "bash", "{\"a\":1}")]);
        let history = vec![m];
        // content "hello"=5 + arguments "{\"a\":1}"=7 → 12
        assert_eq!(context_usage(&history), 12);
    }

    #[test]
    fn compact_range_replaces_and_backs_up() {
        let dir = tmp_dir("range");
        let c = cfg();
        let full_args = "a".repeat(10241);
        let full_content = "b".repeat(10241);

        let mut messages = vec![
            // assistant 带超长 arguments
            ChatMessage::assistant("").with_tool_calls(vec![
                llm_provider::ToolCall::new("c1", "bash", full_args.clone()),
            ]),
            // 对应 tool 结果（超长）
            ChatMessage { role: "tool".into(), content: Some(full_content.clone()), tool_call_id: Some("c1".into()), ..ChatMessage::user("") },
            // 未超长，应不动
            ChatMessage::tool("short", "c2"),
        ];

        let n = compact_range(&mut messages, &dir, &c);
        assert_eq!(n, 2);

        // assistant 参数被替换为截断 + 日志标记
        let args = messages[0].tool_calls.as_ref().unwrap()[0].function.arguments.clone();
        assert!(args.contains("...(truncated)"));
        assert!(args.contains("[完整日志:"));
        // metadata 标记
        assert!(is_compacted(&messages[0]));
        // 备份文件生成于 data/session/<sid> 同级目录（此处为 dir）
        let mut writes: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        writes.sort();
        assert_eq!(writes.len(), 2);
        // tool 结果也被替换并标记
        assert!(messages[1].content.as_deref().unwrap().contains("[完整日志:"));
        assert!(is_compacted(&messages[1]));
        // 未超长消息保持原样、无标记
        assert_eq!(messages[2].content.as_deref(), Some("short"));
        assert!(!is_compacted(&messages[2]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_range_skips_already_compacted() {
        let dir = tmp_dir("skip");
        let c = cfg();
        let mut m = ChatMessage::user("").with_tool_calls(vec![
            llm_provider::ToolCall::new("c1", "bash", "a".repeat(10241)),
        ]);
        mark_compacted(&mut m);
        let mut messages = vec![m];
        let n = compact_range(&mut messages, &dir, &c);
        assert_eq!(n, 0, "已标记消息不应重复处理");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
