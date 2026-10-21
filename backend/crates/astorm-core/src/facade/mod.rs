//! 业务门面骨架：各应用模块接口的 stub 实现（方法统一返回"未实现"）。
//!
//! 接口契约定义在 [`astorm_common`]，此处仅占位并挂载到 `AppState`，
//! 业务实现按域后续补充（替换对应方法体即可）。

pub mod agent;
pub mod file;
pub mod git;
pub mod knowledge;
pub mod memory;
pub mod pty;
pub mod schedule;
pub mod skill;
pub mod taskplan;
pub mod terminal;
pub mod tool;

pub use agent::AgentImpl;
pub use file::FileImpl;
pub use git::GitImpl;
pub use knowledge::KnowledgeImpl;
pub use memory::{
    CompileReport, ConceptSummary, IssueLevel, LogEntry, MemoryCategoryStat, MemoryCompileRequest,
    MemoryConceptDetail, MemoryImpl, MemoryRawSource, MemorySourceRef, MemoryStatus, ValidateIssue,
    ValidateReport,
};
pub use pty::PtyManager;
pub use schedule::ScheduleImpl;
pub use skill::SkillImpl;
pub use taskplan::TaskPlanImpl;
pub use terminal::TerminalImpl;
pub use tool::ToolImpl;

/// 统一的"接口已定义、业务尚未实现"错误。
pub(crate) fn not_impl(module: &str, op: &str) -> String {
    format!("{module}.{op} 尚未实现（接口骨架）")
}