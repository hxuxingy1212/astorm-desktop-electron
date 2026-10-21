//! 任务规划请求体（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

/// 读取规划文件请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadPlanFileRequest {
    /// 所属会话 id
    pub session_id: String,
    /// 规划 id
    pub task_id: String,
}