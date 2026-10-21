//! `submit_deliverables` 工具：对话收尾的交付上报。
//!
//! 系统提示词要求：**仅当本轮确有交付产物**时，在给出最终答复前调用一次——
//! 上报关键交付产物（`deliverables`）、本轮实际改动 / 新增的文件（`changed_files`）、
//! 本轮真实取用且有帮助的记忆 concept_id（`memory_ids`）；没有交付产物则无需调用。
//!
//! 工具就地富集（存在性过滤防幻觉路径、合并 git 差异统计）后写入 sqlite
//! `session_deliverables` 表（按 `session_id + round` upsert，同轮重调覆盖）；
//! 前端在会话结束后经 `list_session_deliverables` 拉表渲染「交付物 / 文件变更 /
//! 有用记忆」卡片。落库即最终交付，不再经事件推送。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use astorm_common::git::Git;
use astorm_common::types::DeliverableRow;

use crate::agent::session::unix_now;
use crate::tool::{resolve_workspace, session_record, storage, Tool, ToolContext};

/// 单类最多保留条数（防模型刷屏 / 行体过大）。
const MAX_ITEMS: usize = 50;
/// 用户消息文本快照的截断上限（字符数）。
const USER_MSG_MAX_CHARS: usize = 2000;
/// 回执里 `dropped`（被丢弃的无效路径）最多列出的条数。
const DROPPED_PREVIEW: usize = 10;

/// `submit_deliverables` 工具。
pub struct DeliveryTool;

/// 宽松收参数组：接受字符串数组，也兼容模型直接给单个字符串（非字符串项忽略）。
fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t]
            }
        }
        _ => Vec::new(),
    }
}

/// 去重（保序）+ 截断。
fn dedupe_cap(mut list: Vec<String>, cap: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    list.retain(|p| seen.insert(p.clone()));
    list.truncate(cap);
    list
}

/// 按字符边界截断文本快照。
fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 工具参数。
#[derive(Debug, Deserialize)]
struct DeliveryArgs {
    /// 关键交付产物（相对工作区路径）。
    #[serde(default)]
    deliverables: Option<Value>,
    /// 本轮实际改动 / 新增的文件（相对工作区路径）。
    #[serde(default)]
    changed_files: Option<Value>,
    /// 本轮真实有用的记忆 concept_id（memory 工具结果行首的 id）。
    #[serde(default)]
    memory_ids: Option<Value>,
}

#[async_trait]
impl Tool for DeliveryTool {
    fn name(&self) -> &str {
        "submit_deliverables"
    }

    fn description(&self) -> &str {
        "对话收尾交付上报：仅当本轮确有交付产物（报告/文档/数据/代码等文件）时，在给出最终答复之前调用一次。上报本轮的关键交付产物、实际改动/新增的文件、以及真实取用且有帮助的记忆 id，系统据此在对话结束后生成交付卡片；没有交付产物则不必调用。"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "deliverables": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "本轮交付的关键产物文件（报告/文档/数据/代码等），相对会话工作区的路径，只列真实存在的关键文件；没有则传空数组"
                },
                "changed_files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "本轮实际改动/新增的全部文件，相对会话工作区的路径；没有则传空数组"
                },
                "memory_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "本轮从记忆中取用且对完成任务确有帮助的记忆 concept_id（memory 工具结果每行开头的 id）；没有用到记忆则传空数组，不要编造 id"
                }
            }
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        let args: DeliveryArgs =
            serde_json::from_value(params).map_err(|e| format!("参数错误: {e}"))?;
        let session_id = ctx.session_id.clone();
        let storage = storage().ok_or("App 存储未初始化，无法记录交付物")?;

        // 会话信息：轮次 = question_times（本会话第几条用户消息，1 起；决议续跑共用同一轮次）
        let record = session_record(&session_id);
        let round = record
            .as_ref()
            .map(|r| r.question_times.max(1))
            .unwrap_or(1);
        let session_title = record
            .as_ref()
            .and_then(|r| r.title.clone())
            .unwrap_or_default();

        // 当轮用户消息快照：历史里最后一条 user 记录（seq 供前端精确 join 到对话轮次）
        let last_user = storage
            .load_messages(&session_id)
            .ok()
            .and_then(|msgs| msgs.into_iter().rev().find(|m| m.role == "user"));
        let (user_msg_seq, user_message) = last_user
            .map(|m| (m.seq, truncate_chars(&m.content, USER_MSG_MAX_CHARS)))
            .unwrap_or((0, String::new()));

        // 路径富集：交付物做存在性过滤（防幻觉死卡片），变更合并 git 差异统计
        let workspace = resolve_workspace(&session_id);
        let raw_deliverables = dedupe_cap(string_list(args.deliverables.as_ref()), MAX_ITEMS);
        let raw_changes = dedupe_cap(string_list(args.changed_files.as_ref()), MAX_ITEMS);
        let deliverables = deliverable_values(&workspace, &raw_deliverables);
        let changed_files = change_values(&workspace, &raw_changes);
        let memory_ids = dedupe_cap(string_list(args.memory_ids.as_ref()), MAX_ITEMS);

        // 被丢弃的无效路径（模型报了但磁盘 / git 都查不到），回执里点名让模型下次改对。
        // 交付物输出的 path 是原始路径；变更输出的 path 是仓库相对路径，分别比对。
        let dlv_paths: HashSet<&str> = deliverables
            .iter()
            .filter_map(|v| v.get("path").and_then(|p| p.as_str()))
            .collect();
        let chg_paths: HashSet<String> = changed_files
            .iter()
            .filter_map(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
            .collect();
        let dropped: Vec<String> = raw_deliverables
            .iter()
            .filter(|p| !dlv_paths.contains(p.as_str()))
            .cloned()
            .chain(
                raw_changes
                    .iter()
                    .filter(|p| !chg_paths.contains(&repo_rel(p, &workspace)))
                    .cloned(),
            )
            .collect();

        let now = unix_now();
        let row = DeliverableRow {
            session_id: session_id.clone(),
            round,
            session_title,
            user_msg_seq,
            user_message,
            deliverables,
            changed_files,
            memory_ids,
            created_at: now,
            updated_at: now,
        };
        storage
            .upsert_deliverable(&row)
            .map_err(|e| format!("保存交付物失败: {e}"))?;

        // 短回执给模型：确认已记录 + 无效路径点名（正文不再重复清单）
        let mut receipt = json!({
            "ok": true,
            "round": round,
            "deliverables": row.deliverables.len(),
            "changed_files": row.changed_files.len(),
            "memory_ids": row.memory_ids.len(),
        });
        if !dropped.is_empty() {
            receipt["dropped"] = json!(dropped.iter().take(DROPPED_PREVIEW).collect::<Vec<_>>());
            if dropped.len() > DROPPED_PREVIEW {
                receipt["dropped_more"] = json!(dropped.len() - DROPPED_PREVIEW);
            }
        }
        Ok(receipt.to_string())
    }
}

// ---------- 路径富集：交付物带体积、变更合并 git 增删统计（与前端卡片渲染字段对齐） ----------

/// 交付物：仅保留磁盘上真实存在的文件（防幻觉路径出现死卡片），带字节数。
fn deliverable_values(workspace: &Path, paths: &[String]) -> Vec<Value> {
    paths
        .iter()
        .filter_map(|raw| {
            let abs = resolve_path(workspace, raw);
            let meta = std::fs::metadata(&abs).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some(json!({
                "path": raw,
                "name": file_name(raw),
                "size": meta.len(),
            }))
        })
        .collect()
}

/// 变更：合并 git 差异摘要（增删计数 / 状态字）；摘要里没有但磁盘存在的按「未跟踪改动」兜底，
/// 两边都查不到的（幻觉 / 已清理）丢弃。
fn change_values(workspace: &Path, paths: &[String]) -> Vec<Value> {
    let summary = git_summary(workspace);
    paths
        .iter()
        .filter_map(|raw| {
            let rel = repo_rel(raw, workspace);
            let entry = summary
                .iter()
                .find(|(p, _, _, _)| p == &rel)
                .or_else(|| summary.iter().find(|(p, _, _, _)| p.ends_with(&format!("/{rel}"))));
            match entry {
                Some((_, status, additions, deletions)) => Some(json!({
                    "path": rel,
                    "name": file_name(&rel),
                    "dir": dir_of(&rel),
                    "status": status,
                    "additions": additions,
                    "deletions": deletions,
                })),
                None => {
                    // 不在 git 摘要里（非仓库工作区 / 未跟踪漏网）：文件仍在磁盘 → 按「新增」
                    // 处理，新增行数 = 文件行数（新增文件也要显示 +N，而不是空白）；
                    // 文件不存在则丢弃（防幻觉路径出现死项）。
                    let abs = resolve_path(workspace, raw);
                    if !abs.is_file() {
                        return None;
                    }
                    let content = std::fs::read_to_string(&abs).unwrap_or_default();
                    Some(json!({
                        "path": rel,
                        "name": file_name(&rel),
                        "dir": dir_of(&rel),
                        "status": "?",
                        "additions": text_lines(&content),
                        "deletions": 0,
                    }))
                }
            }
        })
        .collect()
}

/// 读取工作区的 git 差异摘要（未暂存，含未跟踪）；非仓库 / git 不可用 → 空。
fn git_summary(workspace: &Path) -> Vec<(String, String, u32, u32)> {
    let Some(app) = crate::app::App::try_global() else {
        return Vec::new();
    };
    let dir = workspace.to_string_lossy().into_owned();
    match app.git().get_git_diff_summary(dir, false) {
        Ok(entries) => entries
            .into_iter()
            .map(|e| (e.path, e.status, e.additions, e.deletions))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 原始路径 → 绝对路径（相对路径以工作区为基准）。
fn resolve_path(workspace: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(raw)
    }
}

/// 归一化为**仓库相对**的展示路径：反斜杠转正斜杠、去 `./`、去掉工作区绝对前缀。
fn repo_rel(raw: &str, workspace: &Path) -> String {
    let mut p = raw.replace('\\', "/");
    while let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    let ws = workspace.to_string_lossy().replace('\\', "/");
    let ws = ws.trim_end_matches('/');
    if !ws.is_empty() {
        if let Some(rest) = p.strip_prefix(&format!("{ws}/")) {
            return rest.to_string();
        }
        if p == ws {
            return String::new();
        }
    }
    p
}

/// 路径最后一段（文件名）。
fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// 路径目录部分（含尾部分隔符，与审查页签展示一致）；无目录返回空串。
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..=i].to_string(),
        None => String::new(),
    }
}

/// 文本行数（空串 = 0；与 `facade::git` 同口径，末尾换行不额外算一行）。
fn text_lines(content: &str) -> u32 {
    if content.is_empty() {
        0
    } else {
        content.lines().count() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_list_accepts_array_and_single_string() {
        let v = json!(["a.md", " b.rs ", "", 3]);
        assert_eq!(string_list(Some(&v)), vec!["a.md", "b.rs"]);
        assert_eq!(string_list(Some(&json!("docs/x.md"))), vec!["docs/x.md"]);
        assert!(string_list(Some(&json!({"k": 1}))).is_empty());
        assert!(string_list(None).is_empty());
    }

    #[test]
    fn dedupe_cap_keeps_order() {
        let out = dedupe_cap(
            vec!["a".into(), "b".into(), "a".into(), "c".into()],
            2,
        );
        assert_eq!(out, vec!["a", "b"]);
    }

    #[test]
    fn truncate_chars_is_char_safe() {
        assert_eq!(truncate_chars("你好世界", 3), "你好世");
        assert_eq!(truncate_chars("", 10), "");
    }

    #[test]
    fn change_values_falls_back_to_line_count_outside_git() {
        // 非仓库工作区：git 摘要为空 → 新文件按「新增」兜底，行数 = 文件行数（不是 0）
        let dir = std::env::temp_dir().join(format!("astorm_delivery_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        std::fs::write(dir.join("deck.json"), "a\nb\nc\n").expect("写文件失败");

        let changes = change_values(&dir, &["deck.json".to_string()]);
        assert_eq!(changes.len(), 1, "存在的文件应保留");
        assert_eq!(changes[0]["status"], json!("?"));
        assert_eq!(changes[0]["additions"], json!(3));
        assert_eq!(changes[0]["deletions"], json!(0));

        // 不存在的文件（幻觉路径）丢弃
        assert!(change_values(&dir, &["nope.rs".to_string()]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deliverable_values_drops_missing_files() {
        let dir = std::env::temp_dir().join(format!("astorm_delivery_dlv_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        std::fs::write(dir.join("report.md"), "# hi").expect("写文件失败");

        let out = deliverable_values(&dir, &["report.md".to_string(), "ghost.md".to_string()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], json!("report.md"));
        assert_eq!(out[0]["size"], json!(4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_name_and_dir() {
        assert_eq!(file_name("a/b/c.rs"), "c.rs");
        assert_eq!(dir_of("a/b/c.rs"), "a/b/");
        assert_eq!(dir_of("c.rs"), "");
    }
}
