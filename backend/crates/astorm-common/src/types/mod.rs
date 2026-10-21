//! 数据记录与存储契约层：供 astorm-storage（实现）与 astorm-core（使用）消费。
//!
//! - [`record`]：落盘记录（[`SessionRecord`] 富版 / [`MessageRecord`]）
//! - [`storage`]：存储契约（[`Storage`] / [`SearchIndex`] trait + [`StorageError`] + [`SearchHit`]）

pub mod agent;
pub mod deliverable;
pub mod plan;
pub mod record;
pub mod scheduled_task;
pub mod storage;
pub mod usage;

pub use agent::{AgentRecord, BUILTIN_MAIN_AGENT_ID};
pub use deliverable::DeliverableRow;
pub use plan::{PlanRecord, PlanStep};
pub use record::{MessageRecord, SessionRecord};
pub use scheduled_task::ScheduledTask;
pub use storage::{SearchHit, SearchIndex, Storage, StorageError};
pub use usage::{UsageEvent, UsageStatRow};