//! 任务规划接口契约：规划文件列表 / 读取（纯 serde，不依赖 napi）。

pub mod req;
pub mod types;

pub use req::ReadPlanFileRequest;
pub use types::{PlanFile, PlanStep, PlanSummary};

/// 任务规划门面：由业务层（`astorm-core`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用。
pub trait TaskPlan {
    /// 列出会话的任务规划文件。
    fn list_plan_files(&self, session_id: String) -> Result<Vec<PlanSummary>, String>;
    /// 读取单个规划文件。
    fn read_plan_file(&self, request: ReadPlanFileRequest) -> Result<PlanFile, String>;
}