//! 存储契约：统一门面 trait + 全文索引 trait + 记录类型 + 错误类型。
//!
//! 纯 serde，不依赖 napi / 具体后端，保持存储契约自洽、可独立复用。
//! 接口均为同步方法（与业务门面 [`crate::session::Session`] 的调用范式一致），
//! 阻塞 IO 由各后端内部处理。

use serde::{Deserialize, Serialize};

use crate::types::agent::AgentRecord;
use crate::types::deliverable::DeliverableRow;
use crate::types::plan::PlanRecord;
use crate::types::record::{MessageRecord, SessionRecord};
use crate::types::scheduled_task::ScheduledTask;
use crate::types::usage::{UsageEvent, UsageStatRow};

/// 全文检索命中结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// 命中消息所属会话。
    pub session_id: String,
    /// 命中消息 id。
    pub message_id: String,
    /// 相关度得分（后端相关度算法，越大越相关）。
    pub score: f32,
    /// 命中片段（可选，后端可省略）。
    pub snippet: Option<String>,
}

/// 存储层统一门面：会话元数据 + 消息历史 + 全文检索。
///
/// 由各后端（jsonl / sqlite / 组合管理器）实现；`astorm-core` 经 `Arc<dyn Storage>` 注入使用。
pub trait Storage: Send + Sync {
    /// 写入 / 更新会话元数据（按 id upsert）。
    fn upsert_session(&self, record: &SessionRecord) -> Result<(), StorageError>;
    /// 读取单个会话元数据。
    fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StorageError>;
    /// 列出全部会话元数据。
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, StorageError>;
    /// 删除会话（元数据 + 消息历史）。
    fn delete_session(&self, session_id: &str) -> Result<(), StorageError>;

    /// 追加一批消息（追加式日志，按 seq 排序）。
    fn append_messages(&self, session_id: &str, messages: &[MessageRecord]) -> Result<(), StorageError>;
    /// 加载会话消息历史（按 seq 升序）。**只读活动日志**（`<session_id>/message.jsonl`）：
    /// 摘要把「系统提示词 + 摘要 + 保留消息」写回活动文件后，活动文件即会话历史；
    /// 备份段 `message_NNN.jsonl` 只作备份与检索，不再参与回读
    /// （否则被摘要掉的旧对话会被重新喂给模型）。
    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageRecord>, StorageError>;
    /// 读取最近一次摘要**备份段**（`message_NNN.jsonl`）的展示回填尾部：
    /// 从倒数第 `max_user_turns` 条用户消息起（不足则从首条用户消息起）到最后归档点的消息。
    ///
    /// **仅供 UI 展示拼接**（`list_messages`）——提示词 / 重放仍只读活动日志，
    /// 否则被摘要掉的旧对话会被重新喂给模型。结构化后端（SQLite）摘要是整段替换、
    /// 无备份段，默认返回空即「不可回填」。
    fn load_archived_tail(
        &self,
        _session_id: &str,
        _max_user_turns: usize,
    ) -> Result<Vec<MessageRecord>, StorageError> {
        Ok(Vec::new())
    }
    /// 归档**分页**读取：最新备份段中 `seq < before_seq` 的消息，取末尾 `max_user_turns`
    /// 个用户轮次（不足则从该范围首条用户消息起）。供 UI「加载更早」逐批回看被压缩的前文；
    /// 返回空 = 已到归档起点。SQLite 后端无备份段，默认空。
    fn load_archived_before(
        &self,
        _session_id: &str,
        _before_seq: i64,
        _max_user_turns: usize,
    ) -> Result<Vec<MessageRecord>, StorageError> {
        Ok(Vec::new())
    }
    /// 用**压缩后的消息**替换活动日志，并把替换前的活动日志留作备份段
    /// `<session_id>/message_NNN.jsonl`（NNN=摘要次数，如 `message_001.jsonl`）。
    ///
    /// 实现须保证：备份先行落盘、活动文件原子替换——任一步崩溃都不会留下
    /// 「活动文件缺失 / 只写一半」的会话。结构化后端（SQLite）整段替换消息即可。
    fn rewrite_live_messages(
        &self,
        session_id: &str,
        index: u32,
        messages: &[MessageRecord],
    ) -> Result<(), StorageError>;

    /// 截断会话消息：删除 `seq >= keep_seq_exclusive` 的消息（撤回历史用）。
    /// 返回被删除的消息 id（供全文索引同步移除）。
    ///
    /// 与 [`Self::rewrite_live_messages`] 不同：**不留备份段**（撤回语义 = 丢弃）。
    /// 实现须保证原子性（临时文件 + rename / 单条 DELETE）。默认返回 `Unsupported`。
    fn truncate_messages(
        &self,
        _session_id: &str,
        _keep_seq_exclusive: i64,
    ) -> Result<Vec<String>, StorageError> {
        Err(StorageError::Unsupported("当前后端不支持消息截断".into()))
    }

    /// 全文检索：返回按相关度降序的命中结果（至多 `limit` 条）。
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError>;

    /// 写入 / 更新任务规划（按 session_id + task_id upsert；规划归会话所有）。
    fn upsert_plan(&self, session_id: &str, plan: &PlanRecord) -> Result<(), StorageError>;
    /// 读取单个任务规划。
    fn get_plan(&self, session_id: &str, task_id: &str) -> Result<Option<PlanRecord>, StorageError>;
    /// 删除任务规划（会话删除时也应级联清理）。
    fn delete_plan(&self, session_id: &str, task_id: &str) -> Result<(), StorageError>;
    /// 列出会话全部计划（按创建时间倒序；后端不支持时返回 Unsupported）。
    fn list_plans(&self, _session_id: &str) -> Result<Vec<PlanRecord>, StorageError> {
        unsupported_scheduled_tasks()
    }

    /// 列出全部定时任务（按创建时间倒序；后端不支持时返回 Unsupported）。
    fn list_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>, StorageError> {
        unsupported_scheduled_tasks()
    }
    /// 写入 / 更新定时任务（按 task_id upsert；后端不支持时返回 Unsupported）。
    fn upsert_scheduled_task(&self, _task: &ScheduledTask) -> Result<(), StorageError> {
        unsupported_scheduled_tasks()
    }
    /// 读取单个定时任务（后端不支持时返回 Unsupported）。
    fn get_scheduled_task(&self, _task_id: &str) -> Result<Option<ScheduledTask>, StorageError> {
        unsupported_scheduled_tasks()
    }
    /// 删除定时任务（后端不支持时返回 Unsupported）。
    fn delete_scheduled_task(&self, _task_id: &str) -> Result<(), StorageError> {
        unsupported_scheduled_tasks()
    }

    /// 列出全部智能体（内置卡片置前，其余按创建时间倒序；后端不支持时返回 Unsupported）。
    fn list_agents(&self) -> Result<Vec<AgentRecord>, StorageError> {
        unsupported_agents()
    }
    /// 读取单个智能体（后端不支持时返回 Unsupported）。
    fn get_agent(&self, _agent_id: &str) -> Result<Option<AgentRecord>, StorageError> {
        unsupported_agents()
    }
    /// 写入 / 更新智能体（按 agent_id upsert；后端不支持时返回 Unsupported）。
    fn upsert_agent(&self, _agent: &AgentRecord) -> Result<(), StorageError> {
        unsupported_agents()
    }
    /// 删除智能体（后端不支持时返回 Unsupported）。
    fn delete_agent(&self, _agent_id: &str) -> Result<(), StorageError> {
        unsupported_agents()
    }

    /// 追加一条 token 用量事件（逐轮落库；后端不支持时返回 Unsupported）。
    fn append_usage_event(&self, _event: &UsageEvent) -> Result<(), StorageError> {
        unsupported_usage()
    }
    /// 聚合 token 用量（本地日期 × 模型；`since_ts` 为 unix 秒下限；后端不支持时返回 Unsupported）。
    fn usage_stats(&self, _since_ts: i64) -> Result<Vec<UsageStatRow>, StorageError> {
        unsupported_usage()
    }

    /// 写入 / 更新一轮交付物（按 session_id + round upsert，同轮重调覆盖；后端不支持时返回 Unsupported）。
    fn upsert_deliverable(&self, _row: &DeliverableRow) -> Result<(), StorageError> {
        unsupported_deliverables()
    }
    /// 列出会话全部交付物行（按轮次升序；后端不支持时返回 Unsupported）。
    fn list_deliverables(&self, _session_id: &str) -> Result<Vec<DeliverableRow>, StorageError> {
        unsupported_deliverables()
    }
    /// 删除会话中 `round >= min_round` 的交付物行（撤回历史回退轮次后同步清理）。
    fn delete_deliverables_from(&self, _session_id: &str, _min_round: i32) -> Result<(), StorageError> {
        unsupported_deliverables()
    }
    /// 跨会话列出交付物行（按更新时间倒序限量；「交付物」页跨会话分组展示用）。
    fn list_all_deliverables(&self, _limit: i64) -> Result<Vec<DeliverableRow>, StorageError> {
        unsupported_deliverables()
    }
}

/// 定时任务能力缺失时的统一错误（JSONL 等轻量后端直接继承默认实现）。
fn unsupported_scheduled_tasks<T>() -> Result<T, StorageError> {
    Err(StorageError::Unsupported("当前后端不支持 scheduled_tasks 表".into()))
}

/// 智能体能力缺失时的统一错误（JSONL 等轻量后端直接继承默认实现）。
fn unsupported_agents<T>() -> Result<T, StorageError> {
    Err(StorageError::Unsupported("当前后端不支持 agents 表".into()))
}

/// 用量统计能力缺失时的统一错误（JSONL 等轻量后端直接继承默认实现）。
fn unsupported_usage<T>() -> Result<T, StorageError> {
    Err(StorageError::Unsupported("当前后端不支持 usage_events 表".into()))
}

/// 交付物能力缺失时的统一错误（JSONL 等轻量后端直接继承默认实现）。
fn unsupported_deliverables<T>() -> Result<T, StorageError> {
    Err(StorageError::Unsupported("当前后端不支持 session_deliverables 表".into()))
}

/// 全文索引门面：面向消息的倒排索引，供检索后端（Tantivy 等）实现。
pub trait SearchIndex: Send + Sync {
    /// 索引一批消息（新增 / 更新，按消息 id 去重）。
    fn index_messages(&self, messages: &[MessageRecord]) -> Result<(), StorageError>;
    /// 按消息 id 批量移除索引（撤回截断后同步）。默认空实现（无索引后端）。
    fn delete_messages(&self, _ids: &[String]) -> Result<(), StorageError> {
        Ok(())
    }
    /// 移除某会话的全部消息索引。
    fn delete_session(&self, session_id: &str) -> Result<(), StorageError>;
    /// 全文检索：返回按相关度降序的命中结果（至多 `limit` 条）。
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError>;
}

/// 存储层错误：带错误码，便于前端展示与重试策略判断。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// 底层 IO 错误（文件读写 / 目录创建）。
    #[error("[S001] IO error: {0}")]
    Io(#[from] std::io::Error),
    /// 序列化 / 反序列化错误。
    #[error("[S002] Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// 数据库错误（SQLite 等结构化后端）。
    #[error("[S003] Database error: {0}")]
    Database(String),
    /// 全文索引错误（Tantivy 等检索后端）。
    #[error("[S004] Index error: {0}")]
    Index(String),
    /// 资源不存在。
    #[error("[S005] Not found: {0}")]
    NotFound(String),
    /// 后端不支持该操作（能力缺失时返回）。
    #[error("[S006] Unsupported operation: {0}")]
    Unsupported(String),
    /// 其他错误。
    #[error("[S999] {0}")]
    Other(String),
}

impl StorageError {
    /// 错误码（S001 ~ S006 / S999）。
    pub fn code(&self) -> &str {
        match self {
            Self::Io(_) => "S001",
            Self::Serialization(_) => "S002",
            Self::Database(_) => "S003",
            Self::Index(_) => "S004",
            Self::NotFound(_) => "S005",
            Self::Unsupported(_) => "S006",
            Self::Other(_) => "S999",
        }
    }

    /// 错误类型名（与变体名一致）。
    pub fn type_name(&self) -> &str {
        match self {
            Self::Io(_) => "Io",
            Self::Serialization(_) => "Serialization",
            Self::Database(_) => "Database",
            Self::Index(_) => "Index",
            Self::NotFound(_) => "NotFound",
            Self::Unsupported(_) => "Unsupported",
            Self::Other(_) => "Other",
        }
    }

    /// 是否可恢复（可安全重试）。
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::NotFound(_) | Self::Unsupported(_))
    }
}