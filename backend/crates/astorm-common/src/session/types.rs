//! 会话模式枚举（serde camelCase 契约，供请求体与持久化记录复用）。

use serde::{Deserialize, Serialize};

/// 会话模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    /// 计划模式
    #[default]
    Plan,
    /// 执行模式
    Execute,
    /// 自动模式
    Auto,
}

/// 权限模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// 手动授权
    #[default]
    Manual,
    /// 自动授权
    Auto,
}

/// 思考强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// 无
    #[default]
    None,
    /// 低
    Low,
    /// 中
    Medium,
    /// 高
    High,
}

impl ReasoningEffort {
    /// 转换为 provider 请求的 `reasoning_effort` 字符串（None = 不上送）。
    pub fn as_str(&self) -> Option<&'static str> {
        match self {
            ReasoningEffort::None => None,
            ReasoningEffort::Low => Some("low"),
            ReasoningEffort::Medium => Some("medium"),
            ReasoningEffort::High => Some("high"),
        }
    }
}

/// 会话事件（业务层专用事件通道的类型标记）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionEvent {
    LoopEvent,
}