//! 后端实现：jsonl / sqlite / 全文索引，按 feature 门控。

#[cfg(feature = "jsonl")]
pub mod jsonl;
#[cfg(feature = "index")]
pub mod index;
#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "jsonl")]
pub use jsonl::JsonlStorage;
#[cfg(feature = "index")]
pub use index::TantivyIndex;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStorage;
