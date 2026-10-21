//! 定时任务接口契约：列表 / 增删改 / 启停 / 立即执行（纯 serde，不依赖 napi）。

pub mod req;
pub mod types;

pub use req::ScheduledTaskRequest;
pub use types::{GeneratedWorkspace, ScheduledTask};

/// 定时任务门面：由业务层（`astorm-core`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用。
pub trait Schedule {
    /// 列出全部定时任务。
    fn list_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>, String>;
    /// 新增定时任务，返回任务 id。
    fn add_scheduled_task(&self, request: ScheduledTaskRequest) -> Result<String, String>;
    /// 更新定时任务。
    fn update_scheduled_task(&self, request: ScheduledTaskRequest) -> Result<(), String>;
    /// 删除定时任务。
    fn delete_scheduled_task(&self, id: String) -> Result<(), String>;
    /// 启停定时任务。
    fn toggle_scheduled_task(&self, id: String, enabled: bool) -> Result<(), String>;
    /// 立即执行一次定时任务。
    fn run_scheduled_task_now(&self, id: String) -> Result<(), String>;
    /// 生成调度专用工作区（uuid 目录）。
    fn generate_scheduler_workspace(&self) -> Result<GeneratedWorkspace, String>;
}