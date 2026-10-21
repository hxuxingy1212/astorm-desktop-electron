//! 会话 DTO 镜像：napi 层类型 + 与 astorm-common DTO 的类型化转换（From）。

use astorm_common::session::req::{
    CreateSessionRequest as CoreCreateSessionRequest, MentionKind as CoreMentionKind,
    MentionRef as CoreMentionRef, QuoteRef as CoreQuoteRef,
    SessionSettings as CoreSessionSettings,
};
use astorm_common::session::types::{
    PermissionMode as CorePermissionMode, ReasoningEffort as CoreReasoningEffort,
    SessionMode as CoreSessionMode,
};
use astorm_common::types::record::{
    GoalState as CoreGoalState, MessageRecord as CoreMessageRecord,
    SessionRecord as CoreSessionRecord, SessionStatus as CoreSessionStatus,
    SessionType as CoreSessionType,
};
use llm_provider::TokenUsage;
use napi_derive::napi;
use serde::Serialize;

/// 会话运行状态（侧栏状态图标；snake_case 与后端落盘一致）。
#[napi(string_enum = "snake_case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// 空闲 / 未知
    Idle,
    /// 运行中
    Running,
    /// 等待审批
    AwaitingApproval,
    /// 等待回答
    AwaitingForm,
    /// 等待确认发送邮件
    AwaitingMailCompose,
    /// 正常结束
    Done,
    /// 异常结束
    Error,
    /// 用户取消
    Cancelled,
    /// 上次运行被中断
    Interrupted,
    /// 用户忽略了提问（可经 reopenInterrupt 继续填写）
    Ignored,
}

impl From<CoreSessionStatus> for SessionStatus {
    fn from(v: CoreSessionStatus) -> Self {
        match v {
            CoreSessionStatus::Idle => SessionStatus::Idle,
            CoreSessionStatus::Running => SessionStatus::Running,
            CoreSessionStatus::AwaitingApproval => SessionStatus::AwaitingApproval,
            CoreSessionStatus::AwaitingForm => SessionStatus::AwaitingForm,
            CoreSessionStatus::AwaitingMailCompose => SessionStatus::AwaitingMailCompose,
            CoreSessionStatus::Done => SessionStatus::Done,
            CoreSessionStatus::Error => SessionStatus::Error,
            CoreSessionStatus::Cancelled => SessionStatus::Cancelled,
            CoreSessionStatus::Interrupted => SessionStatus::Interrupted,
            CoreSessionStatus::Ignored => SessionStatus::Ignored,
        }
    }
}

/// 会话类型（这条会话从哪来；前端侧栏分组 + 类型徽标据此渲染）。
#[napi(string_enum = "snake_case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    /// 普通会话（用户手动新建）
    Normal,
    /// 定时任务触发创建的会话
    Scheduled,
    /// 项目会话（用户显式指定了工作目录）
    Project,
    /// 子会话（sub_session 工具委派）
    Child,
}

impl From<SessionType> for CoreSessionType {
    fn from(v: SessionType) -> Self {
        match v {
            SessionType::Normal => CoreSessionType::Normal,
            SessionType::Scheduled => CoreSessionType::Scheduled,
            SessionType::Project => CoreSessionType::Project,
            SessionType::Child => CoreSessionType::Child,
        }
    }
}

impl From<CoreSessionType> for SessionType {
    fn from(v: CoreSessionType) -> Self {
        match v {
            CoreSessionType::Normal => SessionType::Normal,
            CoreSessionType::Scheduled => SessionType::Scheduled,
            CoreSessionType::Project => SessionType::Project,
            CoreSessionType::Child => SessionType::Child,
        }
    }
}

/// 会话模式：计划 / 执行 / 自动。
#[napi(string_enum = "kebab-case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    Plan,
    Execute,
    Auto,
}

impl From<SessionMode> for CoreSessionMode {
    fn from(v: SessionMode) -> Self {
        match v {
            SessionMode::Plan => CoreSessionMode::Plan,
            SessionMode::Execute => CoreSessionMode::Execute,
            SessionMode::Auto => CoreSessionMode::Auto,
        }
    }
}

impl From<CoreSessionMode> for SessionMode {
    fn from(v: CoreSessionMode) -> Self {
        match v {
            CoreSessionMode::Plan => SessionMode::Plan,
            CoreSessionMode::Execute => SessionMode::Execute,
            CoreSessionMode::Auto => SessionMode::Auto,
        }
    }
}

/// 权限模式：手动 / 自动授权。
#[napi(string_enum = "kebab-case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Manual,
    Auto,
}

impl From<PermissionMode> for CorePermissionMode {
    fn from(v: PermissionMode) -> Self {
        match v {
            PermissionMode::Manual => CorePermissionMode::Manual,
            PermissionMode::Auto => CorePermissionMode::Auto,
        }
    }
}

impl From<CorePermissionMode> for PermissionMode {
    fn from(v: CorePermissionMode) -> Self {
        match v {
            CorePermissionMode::Manual => PermissionMode::Manual,
            CorePermissionMode::Auto => PermissionMode::Auto,
        }
    }
}

/// 思考强度：无 / 低 / 中 / 高。
#[napi(string_enum = "kebab-case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

impl From<ReasoningEffort> for CoreReasoningEffort {
    fn from(v: ReasoningEffort) -> Self {
        match v {
            ReasoningEffort::None => CoreReasoningEffort::None,
            ReasoningEffort::Low => CoreReasoningEffort::Low,
            ReasoningEffort::Medium => CoreReasoningEffort::Medium,
            ReasoningEffort::High => CoreReasoningEffort::High,
        }
    }
}

impl From<CoreReasoningEffort> for ReasoningEffort {
    fn from(v: CoreReasoningEffort) -> Self {
        match v {
            CoreReasoningEffort::None => ReasoningEffort::None,
            CoreReasoningEffort::Low => ReasoningEffort::Low,
            CoreReasoningEffort::Medium => ReasoningEffort::Medium,
            CoreReasoningEffort::High => ReasoningEffort::High,
        }
    }
}

/// 提及类型（`@` / `$` 快速引用）。
#[napi(string_enum = "snake_case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionKind {
    /// 快捷命令（`/`；后端暂不处理）
    Command,
    /// 技能（`$name`）
    Skill,
    /// 子智能体（`$agent/<id>`）
    Agent,
    /// 文件引用（`@<相对路径>`）
    File,
}

impl From<MentionKind> for CoreMentionKind {
    fn from(v: MentionKind) -> Self {
        match v {
            MentionKind::Command => CoreMentionKind::Command,
            MentionKind::Skill => CoreMentionKind::Skill,
            MentionKind::Agent => CoreMentionKind::Agent,
            MentionKind::File => CoreMentionKind::File,
        }
    }
}

/// 结构化提及（napi 镜像，见 core `MentionRef`）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionRef {
    /// 提及类型
    pub kind: MentionKind,
    /// 标识：技能名 / 智能体 id（裸）/ 文件相对路径 / 命令 id
    pub id: String,
    /// 原文 token
    pub token: String,
    /// 展示名（可选）
    pub label: Option<String>,
}

impl From<MentionRef> for CoreMentionRef {
    fn from(m: MentionRef) -> Self {
        CoreMentionRef {
            kind: m.kind.into(),
            id: m.id,
            token: m.token,
            label: m.label,
        }
    }
}

/// 对话正文引用（napi 镜像，见 core `QuoteRef`）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRef {
    /// 选中正文原文（完整）
    pub text: String,
    /// 来源消息序号（`#seq`；可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    /// 来源角色：`assistant` / `user`（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl From<QuoteRef> for CoreQuoteRef {
    fn from(q: QuoteRef) -> Self {
        CoreQuoteRef {
            text: q.text,
            seq: q.seq,
            role: q.role,
        }
    }
}

/// 创建会话请求参数（napi 镜像）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// 模型引用 id（必填）
    pub model: String,
    /// 供应商名（可选）：缺省用默认供应商
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 智能体 id（可选）：缺省 = 内置默认主智能体。
    /// 决定本会话可见的工具集（强制过滤）、技能集（仅提示词）与额外系统指引。
    /// 注意：父子关系（`pid`）由后端子会话工具设置，不在此暴露。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// 会话类型（可选）：缺省 = 普通会话；前端选了目录时传 `project`。
    /// 注意：子会话类型后端按 `pid` 强制归一，不在此声明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_type: Option<SessionType>,
    /// 工作空间（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// 模式（可选）：计划 / 执行 / 自动
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SessionMode>,
    /// 权限（可选）：手动 / 自动授权
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionMode>,
    /// 思考强度（可选）：无 / 低 / 中 / 高
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 文字消息
    pub message: String,
    /// 附件（可选，列表）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
    /// 创建后是否立即执行（可选）：缺省/`true` = 现在的流程（起首轮 ReAct 循环）；
    /// `false` = 只创建（落标题 + 首条消息进历史，状态 `idle`，不起循环）—— 看板「仅创建」按钮用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_run: Option<bool>,
    /// 结构化提及（可选）：`@` / `$` 快速引用，缺省 = 无。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<MentionRef>>,
    /// 浏览器拾取元素的围栏原文（可选）：每项一段 ```element 围栏，缺省 = 无。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<String>>,
    /// office 预览拾取的围栏原文（可选）：每项是一段 ```office-ref 围栏，缺省 = 无。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub office_refs: Option<Vec<String>>,
    /// 对话正文引用（可选）：对话「添加到当前任务」选中的一段，缺省 = 无。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quotes: Option<Vec<QuoteRef>>,
    /// 目标（可选，`/goal`）：非空时设置会话目标循环
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// 目标语言（可选，`/goal`；如 `zh-CN` / `en-US`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_lang: Option<String>,
}

impl From<CreateSessionRequest> for CoreCreateSessionRequest {
    fn from(r: CreateSessionRequest) -> Self {
        CoreCreateSessionRequest {
            model: r.model,
            provider: r.provider,
            // 父会话 id 不在 napi DTO 暴露（子会话只由后端 sub_session 工具创建）
            pid: None,
            agent_type: r.agent_type,
            session_type: r.session_type.map(Into::into),
            // 归属定时任务只由后端调度器注入，不在 napi DTO 暴露（前端不可设）
            scheduled_task_id: None,
            workspace: r.workspace,
            session_mode: r.mode.map(Into::into),
            permission_mode: r.permission.map(Into::into),
            reasoning_effort: r.reasoning_effort.map(Into::into),
            user_message: r.message,
            attachments: r.attachments,
            auto_run: r.auto_run,
            mentions: r.mentions.map(|ms| ms.into_iter().map(Into::into).collect()),
            elements: r.elements,
            office_refs: r.office_refs,
            quotes: r.quotes.map(|qs| qs.into_iter().map(Into::into).collect()),
            goal: r.goal,
            goal_lang: r.goal_lang,
        }
    }
}

/// 目标循环状态（napi 镜像，见 core `GoalState`）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalState {
    /// 目标文本
    pub text: String,
    /// 目标语言（判定提示词 / 续跑指令语言）
    pub lang: String,
    /// 本次目标运行累计消耗 token
    pub tokens_used: f64,
    /// 已执行的续跑迭代次数
    pub iterations: u32,
    /// 生命周期状态：active / completed / stopped
    pub status: String,
}

impl From<CoreGoalState> for GoalState {
    fn from(g: CoreGoalState) -> Self {
        GoalState {
            text: g.text,
            lang: g.lang,
            tokens_used: g.tokens_used as f64,
            iterations: g.iterations,
            status: g.status.as_str().to_string(),
        }
    }
}

/// 更新会话级设置（napi 镜像）：模式 / 审批 / 思考强度 / 模型 / 供应商。
///
/// 这些是**会话的持久化属性**，无需发消息即可改写；字段缺省 = 保持原值
/// （全缺省为空操作）。改写的是内存活记录，工具策略下一个工具调用即读到新值。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSessionSettingsRequest {
    /// 模式（可选）：计划 / 执行 / 自动
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SessionMode>,
    /// 权限（可选）：手动 / 自动授权
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionMode>,
    /// 思考强度（可选）：无 / 低 / 中 / 高
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 模型引用 id（可选）：空串视为未给出
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 供应商名（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl From<UpdateSessionSettingsRequest> for CoreSessionSettings {
    fn from(r: UpdateSessionSettingsRequest) -> Self {
        CoreSessionSettings {
            session_mode: r.mode.map(Into::into),
            permission_mode: r.permission.map(Into::into),
            reasoning_effort: r.reasoning_effort.map(Into::into),
            model: r.model,
            provider: r.provider,
        }
    }
}

/// 会话记录（napi 镜像，供列表渲染）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    /// 父会话 id（子会话才有值）
    pub pid: Option<String>,
    /// 会话类型（普通 / 定时任务 / 项目 / 子会话）
    pub session_type: SessionType,
    /// 产生本条会话的定时任务 id（定时任务会话才有值；普通 / 子会话为 null）
    pub scheduled_task_id: Option<String>,
    /// 智能体 id（未指定时是内置默认主智能体）
    pub agent_type: Option<String>,
    pub title: Option<String>,
    /// 中文标题（LLM 双标题生成）
    pub title_zh: Option<String>,
    /// English title（LLM 双标题生成）
    pub title_en: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub workspace: Option<String>,
    pub session_mode: SessionMode,
    pub reasoning_effort: ReasoningEffort,
    pub permission_mode: PermissionMode,
    pub skill_list: Vec<String>,
    pub tool_list: Vec<String>,
    pub mcp_list: Vec<String>,
    /// 经 tool_load 加载的延迟工具（LLM 实际可用集 = toolList ∪ loadedToolList；见 agent::dynamic_tools）
    pub loaded_tool_list: Vec<String>,
    pub session_usage: TokenUsage,
    /// 父会话聚合的子会话 token 用量（展示总量 = session_usage + subagent_usage；冷启动回填）
    pub subagent_usage: TokenUsage,
    /// 当前上下文占用的真实 token 数（上一轮请求的 prompt_tokens；会话信息进度条用）
    pub context_tokens: u32,
    pub summary_times: i32,
    /// 用户提问次数（累计追加的用户消息条数；撤回历史时回退）
    pub question_times: i32,
    pub current_round: i32,
    pub created_at: i64,
    pub updated_at: i64,
    /// 运行状态（前端侧栏状态图标；冷启动据此恢复）
    pub status: SessionStatus,
    /// 状态最后变更时间（unix 秒；0 = 从未记录）
    pub status_updated_at: i64,
    /// 目标循环状态（`/goal`；null = 无目标）
    pub goal: Option<GoalState>,
}

impl From<CoreSessionRecord> for SessionRecord {
    fn from(m: CoreSessionRecord) -> Self {
        SessionRecord {
            id: m.id.to_string(),
            pid: m.pid.map(|p| p.to_string()),
            session_type: m.session_type.into(),
            scheduled_task_id: m.scheduled_task_id,
            agent_type: m.agent_type,
            title: m.title,
            title_zh: m.title_zh,
            title_en: m.title_en,
            model: m.model,
            provider: m.provider,
            workspace: m.workspace,
            session_mode: m.session_mode.into(),
            reasoning_effort: m.reasoning_effort.into(),
            permission_mode: m.permission_mode.into(),
            skill_list: m.skill_list,
            tool_list: m.tool_list,
            mcp_list: m.mcp_list,
            loaded_tool_list: m.loaded_tool_list,
            session_usage: m.session_usage,
            subagent_usage: m.subagent_usage,
            context_tokens: m.context_tokens,
            summary_times: m.summary_times,
            question_times: m.question_times,
            current_round: m.current_round,
            created_at: m.created_at,
            updated_at: m.updated_at,
            status: m.status.into(),
            status_updated_at: m.status_updated_at,
            goal: m.goal.map(Into::into),
        }
    }
}

/// 消息记录（napi 镜像，供历史回读渲染）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    /// 工具调用载荷（assistant 消息；null = 非工具消息）
    pub tool_calls: Option<serde_json::Value>,
    pub seq: i64,
    /// unix 秒（前端乘 1000 展示）
    pub created_at: i64,
    /// 思考链（assistant 消息）
    pub reasoning_content: Option<String>,
    /// 工具结果消息关联的 tool_call id
    pub tool_call_id: Option<String>,
    /// 业务元数据（工具耗时等）
    pub metadata: Option<serde_json::Value>,
}

impl From<CoreMessageRecord> for MessageRecord {
    fn from(m: CoreMessageRecord) -> Self {
        let tool_calls = (!m.tool_calls.is_null()).then_some(m.tool_calls);
        MessageRecord {
            id: m.id,
            session_id: m.session_id,
            role: m.role,
            content: m.content,
            tool_calls,
            seq: m.seq,
            created_at: m.created_at,
            reasoning_content: m.reasoning_content,
            tool_call_id: m.tool_call_id,
            metadata: m.metadata,
        }
    }
}
