//! 应用存储层：会话持久化的三类后端实现 + 组合管理器。
//!
//! - 记录与存储契约（[`SessionRecord`] / [`MessageRecord`] / [`Storage`] / [`StorageError`] 等）
//!   统一定义于 [`astorm_common::types`]，本 crate 仅 re-export，保持下游路径可用。
//! - [`JsonlStorage`] 追加式 JSONL 日志（消息历史）、[`SqliteStorage`] 结构化元数据
//!   （整条记录 JSON 落单列）、[`TantivyIndex`] 全文检索，按 feature 门控。
//! - [`StorageManager`] 组合三者，作为应用门面的统一存储入口。

mod backend;
mod manager;

#[cfg(feature = "jsonl")]
pub use backend::JsonlStorage;
#[cfg(feature = "index")]
pub use backend::TantivyIndex;
#[cfg(feature = "sqlite")]
pub use backend::SqliteStorage;

pub use astorm_common::types::{
    MessageRecord, SearchHit, SearchIndex, SessionRecord, Storage, StorageError,
};
pub use manager::StorageManager;