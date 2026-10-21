//! LoopEvent：工具 → 业务的自定义事件通道（mpsc）。
//!
//! 业务创建 `mpsc::unbounded_channel::<LoopEvent>()` 后把发送端放入
//! [`LoopContext::event_tx`](crate::engine::LoopContext::event_tx)，
//! 引擎将其挂到 [`ToolContext`](crate::tool::ToolContext) 上；
//! 工具经 `emit` 推送事件，业务在接收端按具体类型 downcast 消费。

use std::any::Any;
use std::sync::Arc;

/// 工具侧事件（经 mpsc 通道推送给业务接收方）。
pub enum LoopEvent {
    /// 自定义类型事件（接收方按具体类型 downcast 使用）。
    Custom(Arc<dyn Any + Send + Sync>),
    /// 原始 JSON 透传事件。
    Json(serde_json::Value),
}

impl LoopEvent {
    /// 构造自定义类型事件。
    pub fn custom<T: Any + Send + Sync>(data: T) -> Self {
        LoopEvent::Custom(Arc::new(data))
    }

    /// 下行转型为具体类型（仅 `Custom` 变体可转）。
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        match self {
            LoopEvent::Custom(v) => v.downcast_ref::<T>(),
            LoopEvent::Json(_) => None,
        }
    }
}
