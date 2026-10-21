//! 组合存储管理器：[`StorageManager`]。
//!
//! 按职责路由到三类后端：
//! - 会话元数据 → 结构化后端（SQLite）
//! - 消息历史   → 追加式日志后端（JSONL）
//! - 全文检索   → 全文索引后端（Tantivy）

use astorm_common::types::{
    AgentRecord, DeliverableRow, MessageRecord, PlanRecord, ScheduledTask, SearchHit, SearchIndex,
    SessionRecord, Storage, StorageError, UsageEvent, UsageStatRow,
};

#[cfg(all(feature = "jsonl", feature = "sqlite", feature = "index"))]
use crate::backend::{JsonlStorage, SqliteStorage, TantivyIndex};
#[cfg(all(feature = "jsonl", feature = "sqlite", feature = "index"))]
use std::path::Path;

/// 组合存储管理器：持有元数据 / 消息日志 / 全文索引三类后端并路由。
pub struct StorageManager {
    metadata: Box<dyn Storage>,
    messages: Box<dyn Storage>,
    index: Box<dyn SearchIndex>,
}

impl StorageManager {
    /// 由三组后端构建（可注入任意实现，便于测试替换）。
    pub fn new(metadata: Box<dyn Storage>, messages: Box<dyn Storage>, index: Box<dyn SearchIndex>) -> Self {
        Self {
            metadata,
            messages,
            index,
        }
    }

    /// 默认组合：SQLite（元数据）+ JSONL（消息日志）+ Tantivy（全文索引）。
    ///
    /// 目录布局：
    /// ```text
    /// <base_dir>/
    ///   sqlite.db            # 会话元数据（SqliteStorage）
    ///   session/<sid>/       # 每会话一个目录（JsonlStorage + 摘要快照 / 剪支日志同目录）
    ///     message.jsonl      #   会话历史（活动日志）
    ///     message_NNN.jsonl  #   摘要备份段
    ///     summary_NNN.md     #   摘要快照（core 写入）
    ///   index/               # 全文索引（TantivyIndex）
    /// ```
    #[cfg(all(feature = "jsonl", feature = "sqlite", feature = "index"))]
    pub fn open_default(base_dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let base_dir = base_dir.as_ref();
        // 确保根目录存在（SqliteStorage 打开库文件需要父目录；JsonlStorage 内部会补建子目录）
        std::fs::create_dir_all(base_dir)?;
        let metadata = Box::new(SqliteStorage::open(base_dir.join("sqlite.db"))?);
        let messages = Box::new(JsonlStorage::new(base_dir.join("session"))?);
        let index = Box::new(TantivyIndex::open_or_create(base_dir.join("index"))?);
        Ok(Self::new(metadata, messages, index))
    }
}

impl Storage for StorageManager {
    fn upsert_session(&self, record: &SessionRecord) -> Result<(), StorageError> {
        self.metadata.upsert_session(record)
    }

    fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StorageError> {
        self.metadata.get_session(session_id)
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, StorageError> {
        self.metadata.list_sessions()
    }

    fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
        self.metadata.delete_session(session_id)?;
        self.messages.delete_session(session_id)?;
        self.index.delete_session(session_id)?;
        Ok(())
    }

    fn append_messages(&self, session_id: &str, messages: &[MessageRecord]) -> Result<(), StorageError> {
        self.messages.append_messages(session_id, messages)?;
        self.index.index_messages(messages)?;
        Ok(())
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageRecord>, StorageError> {
        self.messages.load_messages(session_id)
    }

    /// 替换活动日志（摘要压缩结果）。**不重建检索索引**：压缩结果里的消息此前已索引，
    /// 重写会为同一内容生成新 id 而重复入索引；旧内容仍在备份段里可被 `search` 命中。
    fn rewrite_live_messages(
        &self,
        session_id: &str,
        index: u32,
        messages: &[MessageRecord],
    ) -> Result<(), StorageError> {
        self.messages.rewrite_live_messages(session_id, index, messages)
    }

    fn load_archived_tail(
        &self,
        session_id: &str,
        max_user_turns: usize,
    ) -> Result<Vec<MessageRecord>, StorageError> {
        self.messages.load_archived_tail(session_id, max_user_turns)
    }

    fn load_archived_before(
        &self,
        session_id: &str,
        before_seq: i64,
        max_user_turns: usize,
    ) -> Result<Vec<MessageRecord>, StorageError> {
        self.messages.load_archived_before(session_id, before_seq, max_user_turns)
    }

    /// 撤回截断：先删消息日志，再从全文索引移除被删 id（与 `append_messages` 的同步口径对称）。
    fn truncate_messages(
        &self,
        session_id: &str,
        keep_seq_exclusive: i64,
    ) -> Result<Vec<String>, StorageError> {
        let removed = self.messages.truncate_messages(session_id, keep_seq_exclusive)?;
        if !removed.is_empty() {
            self.index.delete_messages(&removed)?;
        }
        Ok(removed)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError> {
        self.index.search(query, limit)
    }

    fn upsert_plan(&self, session_id: &str, plan: &PlanRecord) -> Result<(), StorageError> {
        self.metadata.upsert_plan(session_id, plan)
    }

    fn get_plan(&self, session_id: &str, task_id: &str) -> Result<Option<PlanRecord>, StorageError> {
        self.metadata.get_plan(session_id, task_id)
    }

    fn delete_plan(&self, session_id: &str, task_id: &str) -> Result<(), StorageError> {
        self.metadata.delete_plan(session_id, task_id)
    }

    fn list_plans(&self, session_id: &str) -> Result<Vec<PlanRecord>, StorageError> {
        self.metadata.list_plans(session_id)
    }

    fn list_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>, StorageError> {
        self.metadata.list_scheduled_tasks()
    }

    fn upsert_scheduled_task(&self, task: &ScheduledTask) -> Result<(), StorageError> {
        self.metadata.upsert_scheduled_task(task)
    }

    fn get_scheduled_task(&self, task_id: &str) -> Result<Option<ScheduledTask>, StorageError> {
        self.metadata.get_scheduled_task(task_id)
    }

    fn delete_scheduled_task(&self, task_id: &str) -> Result<(), StorageError> {
        self.metadata.delete_scheduled_task(task_id)
    }

    fn list_agents(&self) -> Result<Vec<AgentRecord>, StorageError> {
        self.metadata.list_agents()
    }

    fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StorageError> {
        self.metadata.get_agent(agent_id)
    }

    fn upsert_agent(&self, agent: &AgentRecord) -> Result<(), StorageError> {
        self.metadata.upsert_agent(agent)
    }

    fn delete_agent(&self, agent_id: &str) -> Result<(), StorageError> {
        self.metadata.delete_agent(agent_id)
    }

    fn append_usage_event(&self, event: &UsageEvent) -> Result<(), StorageError> {
        self.metadata.append_usage_event(event)
    }

    fn usage_stats(&self, since_ts: i64) -> Result<Vec<UsageStatRow>, StorageError> {
        self.metadata.usage_stats(since_ts)
    }

    fn upsert_deliverable(&self, row: &DeliverableRow) -> Result<(), StorageError> {
        self.metadata.upsert_deliverable(row)
    }

    fn list_deliverables(&self, session_id: &str) -> Result<Vec<DeliverableRow>, StorageError> {
        self.metadata.list_deliverables(session_id)
    }

    fn list_all_deliverables(&self, limit: i64) -> Result<Vec<DeliverableRow>, StorageError> {
        self.metadata.list_all_deliverables(limit)
    }

    fn delete_deliverables_from(&self, session_id: &str, min_round: i32) -> Result<(), StorageError> {
        self.metadata.delete_deliverables_from(session_id, min_round)
    }
}
