//! 长期记忆（memory_*）接口：状态 / 概念浏览 / 检索 / 校验 / 变更日志 / 来源 / 编译触发。
//!
//! 全部委托 [`astorm_core::app::AppState::memory`] 门面；记忆语义在 `astorm-memory` crate，
//! core 只做「应用设定 → 记忆库」的集成适配，此处仅做 napi 胶水。
//!
//! 进度与结果经全局事件总线推送（`type === "memory"`，`session_id` 为空），
//! 事件形状见 `astorm_core::facade::memory`。

mod cancel;
mod compile;
mod dto;
mod list_concepts;
mod list_raw_sources;
mod read_log;
mod read_raw_source;
mod reindex;
mod root;
mod search;
mod show_concept;
mod status;
mod validate;

pub use cancel::memory_cancel;
pub use compile::memory_compile;
pub use dto::{
    MemoryCategoryStat, MemoryConcept, MemoryConceptDetail, MemoryIssue, MemoryLogEntry,
    MemoryRawSource, MemorySourceRef, MemoryStatus, MemoryValidateReport,
};
pub use list_concepts::memory_list_concepts;
pub use list_raw_sources::memory_list_raw_sources;
pub use read_log::memory_read_log;
pub use read_raw_source::memory_read_raw_source;
pub use reindex::memory_reindex;
pub use root::memory_root;
pub use search::memory_search;
pub use show_concept::memory_show_concept;
pub use status::memory_status;
pub use validate::memory_validate;
