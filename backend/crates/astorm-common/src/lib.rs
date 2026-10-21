//! astorm-common：跨 crate 公共数据模型与接口契约（纯 serde，不含 napi）。
//!
//! - [`session`]：会话 API 契约（门面 trait / 请求 / 枚举）——供 core 与 napi 用
//! - [`agent`]：智能体管理契约（门面 trait + 新建 / 更新请求）——供 core 与 napi 用
//! - [`file`] / [`git`] / [`tool`] / [`skill`] / [`schedule`] / [`taskplan`]：
//!   应用模块接口契约（门面 trait + 请求/类型），实现待 core 补充
//! - [`types`]：存储记录与存储契约（[`SessionRecord`] / [`MessageRecord`] / [`Storage`] /
//!   [`StorageError`] 等）——供 storage 与 core 用

pub mod agent;
pub mod file;
pub mod fsx;
pub mod git;
pub mod knowledge;
pub mod schedule;
pub mod session;
pub mod skill;
pub mod taskplan;
pub mod tool;
pub mod types;

pub use agent::Agent;
pub use file::File;
pub use git::Git;
pub use knowledge::Knowledge;
pub use schedule::Schedule;
pub use session::Session;
pub use skill::Skill;
pub use taskplan::TaskPlan;
pub use tool::Tools;
pub use types::{MessageRecord, SearchHit, SearchIndex, SessionRecord, Storage, StorageError};