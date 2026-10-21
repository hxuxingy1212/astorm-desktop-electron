//! 交付物记录：`submit_deliverables` 工具每轮上报的落库行。
//!
//! 由 astorm-core 的交付工具在每轮对话收尾时写入（同轮重调覆盖），
//! 前端在会话结束后按会话拉取，渲染「交付物 / 文件变更 / 有用记忆」卡片。
//! 两个清单为 `serde_json::Value` 数组：元素形状由写入方（交付工具富集）定义，
//! 与前端 `ArtifactDeliverable` / `ArtifactChange` 对齐，读侧不做结构解释。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一轮对话的交付物记录（sqlite `session_deliverables` 表行，按 `session_id + round` 唯一）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableRow {
    /// 会话 id。
    pub session_id: String,
    /// 轮次 = 写入时会话的 `question_times`（本会话第几条用户消息，1 起；
    /// 决议续跑不新增用户消息，与当轮共用同一轮次，重调覆盖）。
    pub round: i32,
    /// 会话标题快照（写入时取，标题后补生成不影响已存行）。
    pub session_title: String,
    /// 当轮用户消息的 jsonl seq（前端把行精确 join 到对话轮次用；
    /// 直发消息在前端无 seq，回退按 `user_message` 文本匹配）。
    pub user_msg_seq: i64,
    /// 当轮用户消息文本快照（截断保护）。
    pub user_message: String,
    /// 交付产物清单：`[{path, name, size?}]`（仅保留磁盘真实存在的文件）。
    #[serde(default)]
    pub deliverables: Vec<Value>,
    /// 修改 / 新增文件清单：`[{path, name, dir?, status?, additions, deletions}]`（合并 git 差异统计）。
    #[serde(default)]
    pub changed_files: Vec<Value>,
    /// 本轮真实有用的记忆 concept_id 列表（模型自报，去重截断）。
    #[serde(default)]
    pub memory_ids: Vec<String>,
    /// 首次写入时间（unix 秒）。
    pub created_at: i64,
    /// 最近覆盖时间（unix 秒）。
    pub updated_at: i64,
}
