//! 会话接口契约：门面 trait + 请求 / 类型定义（纯 serde，不依赖 napi）。

pub mod interrupt;
pub mod req;
pub mod types;

pub use interrupt::{InterruptResolution, InterruptType};
pub use req::{CreateSessionRequest, SessionSettings};
pub use types::{PermissionMode, ReasoningEffort, SessionMode};

use uuid::Uuid;

use crate::types::record::{MessageRecord, SessionRecord};

/// 会话管理门面：由业务层（`astorm-core` 的 `SessionImpl`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用，不直接依赖本 trait 的实现类型。
pub trait Session {
    /// 创建会话：生成 session_id（Uuid），初始化会话记录与会话运行时，返回 session_id。
    fn create_session(&self, request: CreateSessionRequest) -> Result<Uuid, String>;
    /// 列出全部会话记录。
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, String>;
    /// 读取会话历史消息（jsonl,按 seq 排序；前端打开会话时回灌）。
    fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, String>;
    /// 归档**分页**读取（UI「加载更早」）：最新摘要备份段中 `seq < before_seq` 的消息，
    /// 取末尾 `max_user_turns` 个用户轮次（不足则从该范围首条用户消息起）。
    /// 返回空 = 已到归档起点（按钮收起）。
    fn archived_messages_before(
        &self,
        session_id: &Uuid,
        before_seq: i64,
        max_user_turns: u32,
    ) -> Result<Vec<MessageRecord>, String>;
    /// 读取会话**系统提示词**（静态人设 + AGENTS.md 约定 + 智能体指引）。
    ///
    /// 系统提示词是运行时按当前配置拼装的，**不落盘**（仅摘要重写后活动日志里可能留一条，
    /// 且可能过期）；故这里按需重建返回。供「会话轨迹」等只读视图展示。
    fn system_prompt(&self, session_id: &Uuid) -> Result<String, String>;
    /// 删除会话（取消其运行时任务并移除注册表）。
    fn delete_session(&self, session_id: &Uuid) -> Result<(), String>;
    /// 追加一条用户请求（含前端对话框设置）并驱动 ReAct 循环（会话正忙时排队）。
    fn send_message(&self, session_id: &Uuid, request: CreateSessionRequest) -> Result<(), String>;
    /// 「启动」：以**已有历史**驱动一轮 ReAct，**不追加新的用户消息**。
    ///
    /// 用于「仅创建」（`auto_run = false`）的会话 —— 那条指令在创建时就已写进历史末尾，
    /// 这里直接把它当本轮请求（看板「待办」卡片上的「启动」）。
    /// 正在运行、历史为空、或历史末尾不是用户消息时返回错误。
    fn start_session(&self, session_id: &Uuid) -> Result<(), String>;
    /// 更新会话级设置（模式 / 审批 / 思考强度 / 模型 / 供应商）：**不必发消息**。
    ///
    /// 设置是会话的持久化属性；`settings` 里缺省的字段保持原值。写的是内存里的活记录
    /// （工具策略每个工具调用、LLM 每轮都读它），故**下一个工具调用 / 下一轮即生效**，
    /// 不需要打断当前循环。已在等待用户决定的审批卡不会被这条调用改写。
    fn update_settings(&self, session_id: &Uuid, settings: SessionSettings) -> Result<(), String>;
    /// 设置 / 修改 / 删除会话目标（`/goal`；不必发消息）。
    ///
    /// `goal = Some(非空文本)` 写入新目标并重置预算（token 基线 = 当前累计用量、迭代 0）；
    /// `goal = None`（或纯空白）删除目标。成功后广播 `session_info`。
    fn update_goal(
        &self,
        session_id: &Uuid,
        goal: Option<String>,
        goal_lang: Option<String>,
    ) -> Result<(), String>;
    /// 取消会话当前循环（cancel 令牌）。
    fn cancel_session(&self, session_id: &Uuid) -> Result<(), String>;
    /// 撤回历史：保留前 `keep_user_messages` 条用户消息，删除第 N 条及其后全部消息
    /// （内存 `history` 与磁盘活动日志同步截断）。
    ///
    /// 语义 = 立即中断当前循环；不得跨会话摘要（目标用户消息必须在最新摘要记录之后），
    /// 跨摘要则返回错误。`keep_user_messages` 从 0 开始计数。
    fn rewind_to_user_message(&self, session_id: &Uuid, keep_user_messages: u32)
        -> Result<(), String>;
}