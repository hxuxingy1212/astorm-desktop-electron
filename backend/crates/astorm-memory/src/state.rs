//! 增量同步状态（`.okfstate.json`）：记录已处理源会话的指纹，避免重复编译。
//!
//! 判定分两级，避免每次都算哈希：
//! 1. **快路径**：`size` + `mtime` 与记录一致 ⇒ 直接跳过（不读盘、不算哈希）
//! 2. **慢路径**：指纹变了才算 sha256，与记录一致 ⇒ 仍然跳过（内容未变，只是被 touch）
//!
//! 源文件为会话活动日志 `session/<sid>/message.jsonl`（摘要重写会改变其内容与指纹）。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 源会话轻量指纹（`size` + `mtime` 秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceStamp {
    pub size: u64,
    /// 最后修改时间（unix 秒；取不到时为 0，此时快路径不生效，退化为算哈希）。
    pub mtime: i64,
}

impl SourceStamp {
    pub fn new(size: u64, mtime: i64) -> Self {
        Self { size, mtime }
    }

    /// 从文件元数据构造（元数据不可读时 `(0, 0)`）。
    pub fn of_file(path: &Path) -> Self {
        use std::time::UNIX_EPOCH;
        let Ok(meta) = std::fs::metadata(path) else {
            return Self::new(0, 0);
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self::new(meta.len(), mtime)
    }
}

/// 增量判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incremental {
    /// 指纹完全一致 ⇒ 跳过（无需读文件）
    Skip,
    /// 指纹变化 ⇒ 需算 sha256 再定（内容可能只是被 touch）
    NeedHash,
}

/// 单个已处理源会话的记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRecord {
    pub session_id: String,
    /// 源文件内容 sha256（十六进制）。
    pub sha256: String,
    pub size: u64,
    /// 最后修改时间（unix 秒）。
    #[serde(default)]
    pub mtime: i64,
    /// 处理时间（RFC3339）。
    pub processed_at: String,
    /// 最近一次**编译**（产出概念）的时间；仅 `parse` 过而没编译的会话为 `None`。
    ///
    /// 没有这个字段就会出问题：`parse` 也会写状态，若 `compile` 只看指纹，则
    /// 「先 parse 再 compile」会把这些会话全部跳过、一个概念都产不出来。
    #[serde(default)]
    pub compiled_at: Option<String>,
    #[serde(default)]
    pub concepts_created: Vec<String>,
    #[serde(default)]
    pub concepts_updated: Vec<String>,
}

/// 增量同步状态（落盘 `.okfstate.json`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    /// `session_id` → 处理记录。
    #[serde(default)]
    pub processed_sessions: BTreeMap<String, FileRecord>,
    /// 上次同步时间（RFC3339）。
    #[serde(default)]
    pub last_sync: Option<String>,
}

impl SyncState {
    /// 从磁盘读取（缺失 / 损坏 → 空状态，不阻断编译）。
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 原子落盘（tmp + rename，避免写一半损坏状态文件）。
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| format!("序列化记忆状态失败: {e}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建状态目录失败({}): {e}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("写入记忆状态失败({}): {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("提交记忆状态失败({}): {e}", path.display()))
    }

    /// 快路径判定：指纹是否与记录一致。
    ///
    /// `mtime == 0` 表示取不到修改时间（旧记录 / 特殊文件系统）⇒ 快路径不可信，退化为算哈希。
    pub fn check(&self, session_id: &str, stamp: SourceStamp) -> Incremental {
        match self.processed_sessions.get(session_id) {
            Some(rec) if rec.mtime != 0 && rec.mtime == stamp.mtime && rec.size == stamp.size => {
                Incremental::Skip
            }
            _ => Incremental::NeedHash,
        }
    }

    /// 是否**编译过**（`parse` 只做归一化，不算编译）。
    pub fn is_compiled(&self, session_id: &str) -> bool {
        self.processed_sessions
            .get(session_id)
            .map(|rec| rec.compiled_at.is_some())
            .unwrap_or(false)
    }

    /// 是否需要**编译**：指纹有变化 **或** 从未编译过（含只 `parse` 过的情况）。
    pub fn needs_compile(&self, session_id: &str, stamp: SourceStamp) -> bool {
        self.check(session_id, stamp) == Incremental::NeedHash || !self.is_compiled(session_id)
    }

    /// 慢路径判定：内容哈希是否与记录一致（`check` 返回 `NeedHash` 时调用）。
    pub fn matches_hash(&self, session_id: &str, sha256: &str) -> bool {
        self.processed_sessions
            .get(session_id)
            .map(|rec| rec.sha256 == sha256)
            .unwrap_or(false)
    }

    pub fn get(&self, session_id: &str) -> Option<&FileRecord> {
        self.processed_sessions.get(session_id)
    }

    /// 写入（覆盖）一条记录。
    pub fn record(&mut self, rec: FileRecord) {
        self.processed_sessions.insert(rec.session_id.clone(), rec);
    }

    /// 删除一条记录（会话被删除时清理）。
    pub fn remove(&mut self, session_id: &str) {
        self.processed_sessions.remove(session_id);
    }

    pub fn is_empty(&self) -> bool {
        self.processed_sessions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.processed_sessions.len()
    }

    /// 待编译会话数：指纹已变、从未处理、或只 `parse` 未编译的会话个数。
    /// **不读文件内容、不算哈希**。
    pub fn count_pending(&self, sources: &[(String, SourceStamp)]) -> usize {
        sources
            .iter()
            .filter(|(sid, stamp)| self.needs_compile(sid, *stamp))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(sid: &str, sha: &str, size: u64, mtime: i64) -> SyncState {
        let mut st = SyncState::default();
        st.record(FileRecord {
            session_id: sid.into(),
            sha256: sha.into(),
            size,
            mtime,
            processed_at: "2026-09-25T10:00:00Z".into(),
            compiled_at: Some("2026-09-25T10:05:00Z".into()),
            concepts_created: vec!["decisions/x".into()],
            concepts_updated: Vec::new(),
        });
        st
    }

    #[test]
    fn unknown_session_needs_compile() {
        let st = SyncState::default();
        assert_eq!(st.check("s1", SourceStamp::new(10, 100)), Incremental::NeedHash);
        assert!(!st.matches_hash("s1", "any"));
    }

    #[test]
    fn identical_stamp_skips_without_hashing() {
        let st = state_with("s1", "abc", 10, 100);
        assert_eq!(st.check("s1", SourceStamp::new(10, 100)), Incremental::Skip);
        assert!(st.matches_hash("s1", "abc"));
    }

    #[test]
    fn changed_stamp_needs_hash() {
        let st = state_with("s1", "abc", 10, 100);
        // 追加了新消息：size 变了
        assert_eq!(st.check("s1", SourceStamp::new(20, 200)), Incremental::NeedHash);
        // 同样 size 但 mtime 变了（touch）→ 也需算哈希才能判定
        assert_eq!(st.check("s1", SourceStamp::new(10, 200)), Incremental::NeedHash);
    }

    /// 被 touch 但内容未变：慢路径应判为一致（不重复编译）。
    #[test]
    fn touched_file_with_same_hash_is_unchanged() {
        let st = state_with("s1", "abc", 10, 100);
        assert_eq!(st.check("s1", SourceStamp::new(10, 999)), Incremental::NeedHash);
        assert!(st.matches_hash("s1", "abc"), "哈希一致 → 应跳过");
        assert!(!st.matches_hash("s1", "def"), "哈希不同 → 应编译");
    }

    /// mtime 取不到（0）时快路径不生效，退化为算哈希——不会误判为已处理。
    #[test]
    fn zero_mtime_never_fast_path_skips() {
        let st = state_with("s1", "abc", 10, 0);
        assert_eq!(st.check("s1", SourceStamp::new(10, 0)), Incremental::NeedHash);
        let st2 = state_with("s1", "abc", 10, 100);
        assert_eq!(st2.check("s1", SourceStamp::new(10, 0)), Incremental::NeedHash);
    }

    #[test]
    fn count_pending_only_counts_changed() {
        let st = state_with("s1", "abc", 10, 100);
        let sources = vec![
            ("s1".to_string(), SourceStamp::new(10, 100)), // 未变
            ("s2".to_string(), SourceStamp::new(5, 50)),   // 新增
            ("s3".to_string(), SourceStamp::new(7, 70)),   // 新增
        ];
        assert_eq!(st.count_pending(&sources), 2);
        assert_eq!(SyncState::default().count_pending(&sources), 3);
    }

    #[test]
    fn round_trip_and_atomic_save() {
        let dir = std::env::temp_dir().join(format!("astorm_mem_state_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join(".okfstate.json");

        let mut st = state_with("s1", "abc", 10, 100);
        st.last_sync = Some("2026-09-25T10:00:00Z".into());
        st.save(&file).unwrap();

        let back = SyncState::load(&file);
        assert_eq!(back, st);
        assert!(!dir.join(".okfstate.json.tmp").exists(), "临时文件应已被改名");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_or_corrupt_returns_empty() {
        let dir = std::env::temp_dir().join(format!("astorm_mem_state_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(SyncState::load(&dir.join("nope.json")).is_empty());

        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ 这不是 JSON").unwrap();
        assert!(SyncState::load(&bad).is_empty(), "损坏文件应回退空状态而非报错");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_overwrites_and_remove_clears() {
        let mut st = state_with("s1", "abc", 10, 100);
        st.record(FileRecord {
            session_id: "s1".into(),
            sha256: "def".into(),
            size: 20,
            mtime: 200,
            processed_at: "2026-09-26T10:00:00Z".into(),
            compiled_at: None,
            concepts_created: Vec::new(),
            concepts_updated: vec!["decisions/x".into()],
        });
        assert_eq!(st.len(), 1, "同一会话应覆盖而非追加");
        assert_eq!(st.get("s1").unwrap().sha256, "def");

        st.remove("s1");
        assert!(st.is_empty());
        assert_eq!(st.check("s1", SourceStamp::new(20, 200)), Incremental::NeedHash);
    }

    /// 只 `parse` 过（compiled_at 为 None）的会话仍应算「待编译」。
    #[test]
    fn parsed_but_not_compiled_still_counts_as_pending() {
        let mut st = SyncState::default();
        st.record(FileRecord {
            session_id: "s1".into(),
            sha256: "abc".into(),
            size: 10,
            mtime: 100,
            processed_at: "t".into(),
            compiled_at: None, // 仅归一化
            concepts_created: Vec::new(),
            concepts_updated: Vec::new(),
        });
        // 指纹一致，但没编译过 → 仍需编译
        assert_eq!(st.check("s1", SourceStamp::new(10, 100)), Incremental::Skip);
        assert!(!st.is_compiled("s1"));
        assert!(st.needs_compile("s1", SourceStamp::new(10, 100)));
        assert_eq!(st.count_pending(&[("s1".into(), SourceStamp::new(10, 100))]), 1);

        // 标记为已编译后 → 不再待编译
        let mut rec = st.get("s1").unwrap().clone();
        rec.compiled_at = Some("t2".into());
        st.record(rec);
        assert!(st.is_compiled("s1"));
        assert!(!st.needs_compile("s1", SourceStamp::new(10, 100)));
        assert_eq!(st.count_pending(&[("s1".into(), SourceStamp::new(10, 100))]), 0);
        // 但内容变了仍需编译
        assert!(st.needs_compile("s1", SourceStamp::new(20, 200)));
    }

    /// compiled_at 缺省（旧状态文件）→ 视为未编译，宁可多跑一次也不漏。
    #[test]
    fn missing_compiled_at_field_is_backward_compatible() {
        let json = r#"{"processedSessions":{"s1":{"sessionId":"s1","sha256":"a","size":1,"mtime":2,"processedAt":"t"}}}"#;
        let st: SyncState = serde_json::from_str(json).unwrap();
        assert!(!st.is_compiled("s1"), "旧状态缺少 compiledAt 应视为未编译");
        assert!(st.needs_compile("s1", SourceStamp::new(1, 2)));
    }

    #[test]
    fn stamp_of_missing_file_is_zero() {
        let stamp = SourceStamp::of_file(Path::new("/definitely/not/here.jsonl"));
        assert_eq!(stamp, SourceStamp::new(0, 0));
    }
}
