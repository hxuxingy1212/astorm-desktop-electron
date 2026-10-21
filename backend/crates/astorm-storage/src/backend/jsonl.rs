//! JSONL 追加式消息日志后端：[`JsonlStorage`]。
//!
//! 目录布局（每个会话一个目录，消息文件与摘要快照 / 剪支日志同目录）：
//! ```text
//! <base_dir>/
//!   <session_id>/
//!     message.jsonl        # 会话历史（活动日志，按 seq 追加）
//!     message_001.jsonl    # 备份段（摘要替换活动日志前的原样备份）
//!     message_002.jsonl
//!     summary_001.md       # 摘要快照（由 core 写入，与本后端同目录）
//!     compact_*.log        # 剪支归档的完整原文（由 core 写入）
//! ```
//!
//! 追加式写；`load_messages` **只读活动日志**——摘要把「系统提示词 + 摘要 + 保留消息」
//! 组装成新的活动日志后，活动日志即会话历史；备份段只作备份与检索（`search` 仍扫描全部
//! 文件），**不再参与回读**（否则被摘要掉的旧对话会被重新喂给模型）。
//! 活动文件内仍按记录 id 去重（后写覆盖先写）：就地更新的消息（如审批重放替换 tool 结果）
//! 以同 id、同 seq 追加新版本，回读即得最新版本，且不会把同一 tool_call 的响应重复喂给模型
//! （上游会 400）。会话元数据由 SQLite 承载（见 [`crate::manager::StorageManager`]）。
//! `search` 为朴素线性扫描（占位实现）。

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use astorm_common::types::{
    MessageRecord, PlanRecord, SearchHit, SessionRecord, Storage, StorageError,
};

/// 活动日志文件名（会话目录内；备份段为 `message_NNN.jsonl`）。
const LIVE_NAME: &str = "message.jsonl";

/// JSONL 追加式消息日志后端。
pub struct JsonlStorage {
    base_dir: PathBuf,
    /// 串行化追加写，避免并发交错。
    write_lock: Mutex<()>,
}

impl JsonlStorage {
    /// 创建（或打开）`base_dir` 下的 JSONL 存储，必要时创建目录。
    pub fn new(base_dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let base_dir = base_dir.as_ref().to_path_buf();
        fs::create_dir_all(&base_dir)?;
        Ok(Self {
            base_dir,
            write_lock: Mutex::new(()),
        })
    }

    /// 读取**最近一个备份段**（`message_NNN.jsonl`，NNN 最大；活动文件不在内）：
    /// 文件内按 id 去重（后写覆盖先写）后按 seq 升序。无备份段（未摘要）→ None。
    fn read_latest_backup(
        &self,
        session_id: &str,
    ) -> Result<Option<Vec<MessageRecord>>, StorageError> {
        let dir = self.session_dir(session_id)?;
        let latest = fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_string();
                let order = backup_order(&name)?;
                (order != u32::MAX).then_some((order, e.path()))
            })
            .max_by_key(|(order, _)| *order);
        let Some((_, path)) = latest else {
            return Ok(None);
        };
        let Ok(file) = fs::File::open(&path) else {
            return Ok(None);
        };
        let mut by_id: HashMap<String, MessageRecord> = HashMap::new();
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<MessageRecord>(&line) {
                by_id.insert(record.id.clone(), record);
            }
        }
        let mut out: Vec<MessageRecord> = by_id.into_values().collect();
        out.sort_by_key(|m| m.seq);
        Ok(Some(out))
    }

    /// 校验会话 id 并返回其数据目录 `base_dir/<session_id>`（消息日志与摘要快照 / 剪支日志
    /// 同居于此，与 core 的 `session_data_dir` 同一路径）。
    fn session_dir(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        if session_id.is_empty() || session_id.contains(['\\', '/', '.', '_']) {
            return Err(StorageError::Other(format!("非法的会话 id: {session_id}")));
        }
        Ok(self.base_dir.join(session_id))
    }

    /// 活动日志 `<session_id>/message.jsonl`（会话历史）。
    fn live_file(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        Ok(self.session_dir(session_id)?.join(LIVE_NAME))
    }

    /// 会话目录内全部消息文件：备份段 `message_NNN.jsonl`（NNN 升序）+ 活动文件
    /// `message.jsonl`（排序键最大）。仅供**消息文件清理**（删除会话）使用；
    /// 目录内的摘要快照 / 剪支日志由 core 负责清理。
    fn session_files(&self, session_id: &str) -> Result<Vec<PathBuf>, StorageError> {
        let dir = self.session_dir(session_id)?;
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut files: Vec<(u32, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?.to_string();
                backup_order(&name).map(|order| (order, p))
            })
            .collect();
        // 备份序号升序；活动文件 order = u32::MAX → 排在最后（最新写入）
        files.sort_by_key(|(order, _)| *order);
        Ok(files.into_iter().map(|(_, p)| p).collect())
    }

    fn append_line(&self, path: &Path, line: &str) -> Result<(), StorageError> {
        let _guard = self.write_lock.lock().map_err(|_| StorageError::Other("写锁中毒".into()))?;
        // 会话目录按需创建（首条消息即建目录）
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{line}")?;
        file.flush()?;
        Ok(())
    }

    /// 朴素线性检索：大小写不敏感子串匹配（扫描 base_dir/<sid>/ 下全部会话消息文件）。
    fn naive_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError> {
        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        let Ok(entries) = fs::read_dir(&self.base_dir) else {
            return Ok(hits);
        };
        // 会话目录层（正常布局）+ base_dir 下的零散文件（容错）都在扫描范围内
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(inner) = fs::read_dir(&path) {
                    files.extend(inner.flatten().map(|e| e.path()));
                }
            } else {
                files.push(path);
            }
        }
        for path in files {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let file = fs::File::open(&path)?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(record) = serde_json::from_str::<MessageRecord>(&line) else {
                    continue;
                };
                if record.content.to_lowercase().contains(&needle) {
                    hits.push(SearchHit {
                        session_id: record.session_id.clone(),
                        message_id: record.id.clone(),
                        score: 1.0,
                        snippet: snippet(&record.content, &needle),
                    });
                }
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }
}

impl Storage for JsonlStorage {
    fn upsert_session(&self, _record: &SessionRecord) -> Result<(), StorageError> {
        // 会话元数据由 SQLite 承载；JSONL 仅存消息历史，不落元数据。
        Err(StorageError::Unsupported("jsonl 仅作消息历史后端，会话元数据由 SQLite 承载".into()))
    }

    fn get_session(&self, _session_id: &str) -> Result<Option<SessionRecord>, StorageError> {
        Err(StorageError::Unsupported("jsonl 仅作消息历史后端，会话元数据由 SQLite 承载".into()))
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, StorageError> {
        Err(StorageError::Unsupported("jsonl 仅作消息历史后端，会话元数据由 SQLite 承载".into()))
    }

    fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
        // 清理该会话目录内的全部消息文件（活动 + 备份段，尽力而为）；
        // 目录内的摘要快照 / 剪支日志由 core 的 cleanup_summary_dir 负责。
        if let Ok(files) = self.session_files(session_id) {
            for file in files {
                let _ = fs::remove_file(file);
            }
        }
        // 消息文件已清空 → 目录若已空顺手移除（非空则留给 core 后续清理）
        if let Ok(dir) = self.session_dir(session_id) {
            let _ = fs::remove_dir(dir);
        }
        Ok(())
    }

    fn append_messages(&self, session_id: &str, messages: &[MessageRecord]) -> Result<(), StorageError> {
        let path = self.live_file(session_id)?;
        let mut buf = String::new();
        for msg in messages {
            buf.push_str(&serde_json::to_string(msg)?);
            buf.push('\n');
        }
        self.append_line(&path, &buf)
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageRecord>, StorageError> {
        // 只读**活动日志**：它就是会话历史（摘要已把「系统提示词 + 摘要 + 保留消息」写回），
        // 备份段 `message_NNN.jsonl` 仅作备份/检索，不参与回读——否则被摘要掉的旧对话会被重新喂给模型。
        // 文件内仍按 id 去重（后写覆盖先写）：审批重放替换 tool 结果会以同 id 追加新版本，
        // 重复的 tool 响应会让上游 LLM 直接 400。
        let live = self.live_file(session_id)?;
        let file = match fs::File::open(&live) {
            Ok(f) => f,
            Err(_) => return Ok(Vec::new()),
        };
        let mut by_id: HashMap<String, MessageRecord> = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<MessageRecord>(&line) {
                by_id.insert(record.id.clone(), record);
            }
        }
        let mut messages: Vec<MessageRecord> = by_id.into_values().collect();
        messages.sort_by_key(|m| m.seq);
        Ok(messages)
    }

    fn load_archived_tail(
        &self,
        session_id: &str,
        max_user_turns: usize,
    ) -> Result<Vec<MessageRecord>, StorageError> {
        let Some(mut archived) = self.read_latest_backup(session_id)? else {
            return Ok(Vec::new());
        };
        if archived.is_empty() {
            return Ok(Vec::new());
        }
        // 剪除与**当前活动日志尾部**重合的后缀：备份 = 整份旧日志，其中「保留进活动日志的
        // 尾部消息」也在内（摘要重写会给非 tool 消息换新 id，无法按 id 去重——按
        // role+content+tool_call_id 从尾到头逐一比对，重合部分整段丢弃）。
        // 活动日志里的 system 与摘要消息不是旧日志的成员，比对前先排除。
        let active = self.load_messages(session_id)?;
        let active_tail: Vec<&MessageRecord> = active
            .iter()
            .filter(|r| {
                r.role != "system"
                    && !r
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("summary"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
            })
            .collect();
        let same = |a: &MessageRecord, b: &MessageRecord| {
            a.role == b.role && a.content == b.content && a.tool_call_id == b.tool_call_id
        };
        let mut overlap = 0usize;
        while overlap < archived.len()
            && overlap < active_tail.len()
            && same(&archived[archived.len() - 1 - overlap], active_tail[active_tail.len() - 1 - overlap])
        {
            overlap += 1;
        }
        archived.truncate(archived.len() - overlap);
        Ok(take_last_turns(archived, max_user_turns))
    }

    fn load_archived_before(
        &self,
        session_id: &str,
        before_seq: i64,
        max_user_turns: usize,
    ) -> Result<Vec<MessageRecord>, StorageError> {
        let Some(segment) = self.read_latest_backup(session_id)? else {
            return Ok(Vec::new());
        };
        // 取游标之前的记录，再切「末尾 max_user_turns 个用户轮次」（不足则从首条用户起）
        let before: Vec<MessageRecord> = segment.into_iter().filter(|r| r.seq < before_seq).collect();
        Ok(take_last_turns(before, max_user_turns))
    }

    fn rewrite_live_messages(
        &self,
        session_id: &str,
        index: u32,
        messages: &[MessageRecord],
    ) -> Result<(), StorageError> {
        let _guard = self.write_lock.lock().map_err(|_| StorageError::Other("写锁中毒".into()))?;
        let dir = self.session_dir(session_id)?;
        fs::create_dir_all(&dir)?;
        let live = dir.join(LIVE_NAME);
        // ① 备份先行：现有活动日志**复制**（不是改名）为 message_NNN.jsonl，
        //    保证活动文件在整个过程中始终存在（崩溃也不会出现「历史丢失」）。
        if live.exists() {
            let backup = dir.join(format!("message_{index:03}.jsonl"));
            // 目标已存在（重复归档）先移除再复制，避免 Windows 下覆盖失败
            if backup.exists() {
                fs::remove_file(&backup)?;
            }
            fs::copy(&live, &backup)?;
        }
        // ② 新活动日志先写临时文件，再原子改名替换：写一半崩溃仍是压缩前的完整日志。
        //    临时文件名 `message.jsonl.tmp` 不匹配任何消息文件模式（见 backup_order），
        //    残留也不会被回读；下次重写覆盖它。
        let tmp = dir.join("message.jsonl.tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            let mut buf = String::new();
            for msg in messages {
                buf.push_str(&serde_json::to_string(msg)?);
                buf.push('\n');
            }
            file.write_all(buf.as_bytes())?;
            file.flush()?;
        }
        fs::rename(&tmp, &live)?;
        Ok(())
    }

    /// 撤回截断：删除 `seq >= keep_seq_exclusive` 的消息，原子重写活动日志（**不留备份段**）。
    fn truncate_messages(
        &self,
        session_id: &str,
        keep_seq_exclusive: i64,
    ) -> Result<Vec<String>, StorageError> {
        let _guard = self.write_lock.lock().map_err(|_| StorageError::Other("写锁中毒".into()))?;
        let all = self.load_messages(session_id)?;
        let (keep, removed): (Vec<MessageRecord>, Vec<MessageRecord>) =
            all.into_iter().partition(|m| m.seq < keep_seq_exclusive);
        if removed.is_empty() {
            return Ok(Vec::new());
        }
        let dir = self.session_dir(session_id)?;
        fs::create_dir_all(&dir)?;
        let live = dir.join(LIVE_NAME);
        // 先写临时文件再原子改名：写一半崩溃仍是截断前的完整日志（与摘要重写同范式）
        let tmp = dir.join("message.jsonl.tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            let mut buf = String::new();
            for msg in &keep {
                buf.push_str(&serde_json::to_string(msg)?);
                buf.push('\n');
            }
            file.write_all(buf.as_bytes())?;
            file.flush()?;
        }
        fs::rename(&tmp, &live)?;
        Ok(removed.into_iter().map(|m| m.id).collect())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError> {
        self.naive_search(query, limit)
    }

    fn upsert_plan(&self, _session_id: &str, _plan: &PlanRecord) -> Result<(), StorageError> {
        // 规划数据由结构化后端（SQLite）承载；JSONL 日志后端不落规划，避免日志与库双写。
        Err(StorageError::Unsupported("jsonl 后端不支持规划存储，请使用 SQLite 装配".into()))
    }

    fn get_plan(&self, _session_id: &str, _task_id: &str) -> Result<Option<PlanRecord>, StorageError> {
        Err(StorageError::Unsupported("jsonl 后端不支持规划存储，请使用 SQLite 装配".into()))
    }

    fn delete_plan(&self, _session_id: &str, _task_id: &str) -> Result<(), StorageError> {
        Err(StorageError::Unsupported("jsonl 后端不支持规划存储，请使用 SQLite 装配".into()))
    }
}

/// 取「末尾 max_user_turns 个用户轮次」：切点为倒数第 N 条用户消息（不足则从首条用户
/// 消息起）；无用户消息返回空。归档回填与分页共用。
fn take_last_turns(records: Vec<MessageRecord>, max_user_turns: usize) -> Vec<MessageRecord> {
    let user_idx: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r.role == "user")
        .map(|(i, _)| i)
        .collect();
    if user_idx.is_empty() {
        return Vec::new();
    }
    let n = max_user_turns.max(1);
    let start = if user_idx.len() >= n {
        user_idx[user_idx.len() - n]
    } else {
        user_idx[0]
    };
    records[start..].to_vec()
}

/// 会话目录内的消息文件名 → **写入时序**（排序键）：
/// - `message.jsonl`（活动日志，最新）→ `u32::MAX`
/// - `message_NNN.jsonl`（备份段，NNN 越大越新）→ `NNN`
/// - 其他（摘要快照 / 剪支日志 / `.tmp` 残留）→ None（不是消息文件）
fn backup_order(name: &str) -> Option<u32> {
    if name == LIVE_NAME {
        return Some(u32::MAX);
    }
    let digits = name.strip_prefix("message_")?.strip_suffix(".jsonl")?;
    (digits.len() == 3 && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// 截取命中片段（命中词前后约 40 字符）。
fn snippet(content: &str, needle: &str) -> Option<String> {
    let lower = content.to_lowercase();
    let idx = lower.find(needle)?;
    let start = idx.saturating_sub(40);
    let end = (idx + needle.len() + 40).min(content.len());
    Some(content[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试记录（指定 role；`record()` 缺省 user —— 见其定义）
    fn record_typed(id: &str, seq: i64, content: &str, role: &str) -> MessageRecord {
        let mut r = record(id, seq, content);
        r.role = role.to_string();
        r
    }

    fn record(id: &str, seq: i64, content: &str) -> MessageRecord {
        MessageRecord {
            id: id.into(),
            session_id: "s1".into(),
            role: "tool".into(),
            content: content.into(),
            tool_calls: serde_json::Value::Null,
            seq,
            created_at: 1,
            reasoning_content: None,
            tool_call_id: Some("call_1".into()),
            metadata: None,
        }
    }

    /// 就地更新（同 id 后写）在追加式日志上表现为「后写覆盖先写」，且不产生重复记录。
    #[test]
    fn load_messages_dedups_same_id_last_write_wins() {
        let dir = std::env::temp_dir().join(format!("astorm_jsonl_dedup_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = JsonlStorage::new(&dir).unwrap();
        assert!(store.append_messages("s1", &[record("m1", 0, "第一次")]).is_ok());
        assert!(store.append_messages("s1", &[record("m1", 0, "第二次")]).is_ok());
        assert!(store.append_messages("s1", &[record("m2", 1, "另一条")]).is_ok());
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 2, "同 id 应只回读一条: {loaded:?}");
        assert_eq!(loaded[0].content, "第二次", "应取后写版本");
        assert_eq!(loaded[1].content, "另一条");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 展示回填：归档尾部 = 备份段去掉「保留进活动日志的尾部重合」后，最近 2 轮用户消息起。
    #[test]
    fn load_archived_tail_returns_last_two_turns_without_overlap() {
        let dir = std::env::temp_dir().join(format!("astorm_jsonl_archtail_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = JsonlStorage::new(&dir).unwrap();
        // 4 轮（每轮 user + assistant），seq 0..7
        let mut msgs = Vec::new();
        for (i, ch) in ["U1", "A1", "U2", "A2", "U3", "A3", "U4", "A4"].iter().enumerate() {
            let role = if ch.starts_with('U') { "user" } else { "assistant" };
            msgs.push(record_typed(&format!("m{i}"), i as i64, ch, role));
        }
        assert!(store.append_messages("s1", &msgs).is_ok());
        // 摘要重写：保留末轮（U4/A4）进活动日志（新 id、大 seq），旧日志整份留作备份
        let kept = vec![
            record_typed("s:sum", 100, "摘要", "assistant"),
            record_typed("new-u4", 101, "U4", "user"),
            record_typed("new-a4", 102, "A4", "assistant"),
        ];
        assert!(store.rewrite_live_messages("s1", 1, &kept).is_ok());

        let tail = store.load_archived_tail("s1", 2).unwrap();
        let contents: Vec<&str> = tail.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(
            contents,
            vec!["U2", "A2", "U3", "A3"],
            "应为前 2 轮且不含与活动日志重合的 U4/A4: {contents:?}"
        );
        // 无备份段（未摘要 / 结构化后端语义）返回空
        let dir2 = std::env::temp_dir().join(format!("astorm_jsonl_archtail2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir2);
        let store2 = JsonlStorage::new(&dir2).unwrap();
        assert!(store2.append_messages("s1", &msgs[..2]).is_ok());
        assert!(store2.load_archived_tail("s1", 2).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    /// 归档分页：游标之前的记录，取末尾 N 个用户轮次；到起点返回空。
    #[test]
    fn load_archived_before_pages_backwards_by_cursor() {
        let dir = std::env::temp_dir().join(format!("astorm_jsonl_archbefore_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = JsonlStorage::new(&dir).unwrap();
        let mut msgs = Vec::new();
        for (i, ch) in ["U1", "A1", "U2", "A2", "U3", "A3", "U4", "A4"].iter().enumerate() {
            let role = if ch.starts_with('U') { "user" } else { "assistant" };
            msgs.push(record_typed(&format!("m{i}"), i as i64, ch, role));
        }
        assert!(store.append_messages("s1", &msgs).is_ok());
        assert!(store
            .rewrite_live_messages("s1", 1, &[record_typed("new-u4", 101, "U4", "user")])
            .is_ok());

        // 游标 = 已展示区最小 seq(=6，U4 的旧副本)。取 seq<6 的末尾 2 轮 → U2/A2/U3/A3
        let page = store.load_archived_before("s1", 6, 2).unwrap();
        let contents: Vec<&str> = page.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents, vec!["U2", "A2", "U3", "A3"], "{contents:?}");
        // 再往前一页：游标 = 该页最小 seq(=2) → U1/A1
        let page2 = store.load_archived_before("s1", 2, 2).unwrap();
        let contents2: Vec<&str> = page2.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents2, vec!["U1", "A1"], "{contents2:?}");
        // 到起点：空
        assert!(store.load_archived_before("s1", 0, 2).unwrap().is_empty());
        // 无备份段（未摘要）：空
        let dir2 = std::env::temp_dir().join(format!("astorm_jsonl_archbefore2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir2);
        let store2 = JsonlStorage::new(&dir2).unwrap();
        assert!(store2.append_messages("s1", &msgs[..2]).is_ok());
        assert!(store2.load_archived_before("s1", 99, 2).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    /// 备份段不参与回读：摘要替换活动日志后，回读只看到压缩结果（备份仅落在磁盘）。
    #[test]
    fn load_messages_ignores_backup_segments() {
        let dir = std::env::temp_dir().join(format!("astorm_jsonl_backup_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = JsonlStorage::new(&dir).unwrap();
        assert!(store.append_messages("s1", &[record("m1", 0, "旧版本")]).is_ok());
        // 摘要：用压缩结果（摘要 + 保留消息）替换活动日志，原日志留作 _001 备份
        assert!(store
            .rewrite_live_messages("s1", 1, &[record("m1", 10, "旧版本"), record("m2", 11, "保留")])
            .is_ok());
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 2, "只应回读活动日志: {loaded:?}");
        assert_eq!(loaded[1].content, "保留");

        // 消息文件都落在会话目录内（与摘要快照 / 剪支日志同目录）
        let session_dir = dir.join("s1");
        assert!(session_dir.join("message.jsonl").exists(), "活动日志应在 s1/ 目录内");
        let backup = session_dir.join("message_001.jsonl");
        assert!(backup.exists(), "备份段应在 s1/ 目录内");
        let text = std::fs::read_to_string(&backup).unwrap();
        assert!(text.contains("旧版本"), "备份段应含压缩前内容: {text}");
        // 目录层不应再有扁平消息文件残留
        assert!(
            !dir.join("s1.jsonl").exists() && !dir.join("s1_001.jsonl").exists(),
            "不应再产生 base_dir 下的扁平文件"
        );

        // 备份段新增内容不影响回读（活动日志已替换，旧内容不再回灌）
        assert!(store.append_messages("s1", &[record("m3", 12, "压缩后新增")]).is_ok());
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 3, "{loaded:?}");
        assert_eq!(loaded[2].content, "压缩后新增");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 活动日志缺失时不写备份（首次摘要前的空会话），且原子替换后不留临时文件。
    #[test]
    fn rewrite_without_live_file_creates_no_backup() {
        let dir = std::env::temp_dir().join(format!("astorm_jsonl_rewrite_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = JsonlStorage::new(&dir).unwrap();
        assert!(store
            .rewrite_live_messages("s1", 1, &[record("m1", 0, "摘要")])
            .is_ok());
        let session_dir = dir.join("s1");
        assert!(
            !session_dir.join("message_001.jsonl").exists(),
            "无活动日志时不应产生备份段"
        );
        assert!(
            !session_dir.join("message.jsonl.tmp").exists(),
            "临时文件应已被改名"
        );
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "摘要");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 删除会话：清掉消息文件（活动 + 备份），目录空后顺手移除。
    #[test]
    fn delete_session_removes_message_files() {
        let dir = std::env::temp_dir().join(format!("astorm_jsonl_delete_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = JsonlStorage::new(&dir).unwrap();
        assert!(store.append_messages("s1", &[record("m1", 0, "一")]).is_ok());
        assert!(store.rewrite_live_messages("s1", 1, &[record("m2", 5, "二")]).is_ok());
        assert!(store.delete_session("s1").is_ok());
        assert!(store.load_messages("s1").unwrap().is_empty());
        assert!(!dir.join("s1").exists(), "消息文件清空后目录应被移除");
        // 其他会话不受影响
        assert!(store.append_messages("s2", &[record("m1", 0, "别的会话")]).is_ok());
        assert!(store.delete_session("s1").is_ok());
        assert_eq!(store.load_messages("s2").unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 文件名 → 写入时序（活动文件最新；备份段按序号；非消息文件忽略）。
    #[test]
    fn backup_order_ranks_live_last() {
        assert_eq!(backup_order("message.jsonl"), Some(u32::MAX));
        assert_eq!(backup_order("message_001.jsonl"), Some(1));
        assert_eq!(backup_order("message_012.jsonl"), Some(12));
        assert_eq!(backup_order("message_01.jsonl"), None);
        assert_eq!(backup_order("summary_001.md"), None);
        assert_eq!(backup_order("compact_browser_x.log"), None);
        assert_eq!(backup_order("message.jsonl.tmp"), None);
    }
}
