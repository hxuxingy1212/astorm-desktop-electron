//! 智能体记录：智能体卡片（名称 / 描述 / 额外用户指引 / 工具集 / 技能集）的落盘模型
//! （sqlite `agents` 表 `data` 列 / Storage trait 通用）。
//!
//! 全局唯一（不归会话所有）：会话经 `SessionRecord.agent_type` 引用其 `agent_id`，
//! 创建时按卡片物化出会话的 `tool_list` / `skill_list`。

use serde::{Deserialize, Serialize};

use crate::session::types::{PermissionMode, SessionMode};

/// 智能体的执行模式：**创建会话时**决定会话模式（`SessionMode`），不引入新的执行机制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecMode {
    /// 只读：会话为 `plan` 模式（Plan 的写禁令在工具层硬拒，`permission = auto` 也不放宽）。
    ReadOnly,
    /// 读写：会话为 `execute` 模式（可直接改写文件）。
    ReadWrite,
    /// 自动：会话为 `auto` 模式（自动判断 + 可审批项自动同意，仍可写）。
    Auto,
    /// 继承：不干预——沿用请求 / 父会话（子会话）/ 缺省值。
    #[default]
    Inherit,
}

impl AgentExecMode {
    /// 映射到会话模式；`继承` → `None`（不干预，沿用请求 / 父会话 / 缺省）。
    pub fn to_session(self) -> Option<SessionMode> {
        match self {
            AgentExecMode::ReadOnly => Some(SessionMode::Plan),
            AgentExecMode::ReadWrite => Some(SessionMode::Execute),
            AgentExecMode::Auto => Some(SessionMode::Auto),
            AgentExecMode::Inherit => None,
        }
    }
}

impl AgentExecMode {
    /// 是否为只读（能力上限）。
    pub fn is_read_only(&self) -> bool {
        matches!(self, AgentExecMode::ReadOnly)
    }
}

/// 智能体的权限模式：约束其会话的**审批行为**（手动弹卡 / 自动同意）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissionMode {
    /// 手动授权（弹审批卡）。
    Manual,
    /// 自动授权（可审批项自动同意；不影响危险命令等硬规则）。
    Auto,
    /// 继承：不干预（按请求 → 父会话 → 缺省解析）。
    #[default]
    Inherit,
}

impl AgentPermissionMode {
    /// 映射到会话权限模式；`继承` → `None`（不干预）。
    pub fn to_session(self) -> Option<PermissionMode> {
        match self {
            AgentPermissionMode::Manual => Some(PermissionMode::Manual),
            AgentPermissionMode::Auto => Some(PermissionMode::Auto),
            AgentPermissionMode::Inherit => None,
        }
    }
}

/// 内置「默认主智能体」id：会话未指定 `agent_type` 时解析到它。
///
/// 语义 = **不做任何限制**（全量工具、技能不受限、无额外指引），与引入智能体前的行为一致；
/// 其卡片不可编辑 / 删除。
pub const BUILTIN_MAIN_AGENT_ID: &str = "builtin-main";

/// 内置「Explore」智能体 id：快速只读探索代理。
pub const BUILTIN_EXPLORE_AGENT_ID: &str = "builtin-explore";

/// 内置「General」智能体 id：通用执行代理（可写，适合并行的工作单元）。
pub const BUILTIN_GENERAL_AGENT_ID: &str = "builtin-general";

/// 智能体卡片：可命名的能力配置（工具集 / 技能集 / 额外系统指引）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    /// 智能体 id（自定义 = uuid v4；内置 = [`BUILTIN_MAIN_AGENT_ID`]）。
    pub agent_id: String,
    /// 名称（列表展示 / 委派时供模型识别）。
    pub name: String,
    /// 描述（供模型判断该委派给谁）。
    #[serde(default)]
    pub description: String,
    /// 额外的用户指引：非空则拼进**系统提示词之后**（同一条 system 消息，见 agent/prompt.rs）。
    #[serde(default)]
    pub guidance: String,
    /// 工具集：仅这些内置工具对该智能体的会话可见（**空 = 无工具**）。
    /// 内置主智能体忽略该字段（= 全量）。
    #[serde(default)]
    pub tool_ids: Vec<String>,
    /// 技能集：仅这些技能列进会话环境提示词（**空 = 不列任何技能**）；
    /// **不限制 `skills` 工具加载**（任意技能都能 load），故为引导性配置。
    /// 内置主智能体忽略该字段（= 不列技能，但不限加载，与引入前一致）。
    #[serde(default)]
    pub skill_ids: Vec<String>,
    /// 执行模式：只读 / 读写 / 继承（见 [`AgentExecMode`]）。
    #[serde(default)]
    pub mode: AgentExecMode,
    /// 权限模式：手动 / 自动 / 继承（见 [`AgentPermissionMode`]）。
    #[serde(default)]
    pub permission: AgentPermissionMode,
    /// 是否内置（内置不可编辑 / 删除）。
    #[serde(default)]
    pub builtin: bool,
    /// 创建时间（unix 秒）。
    pub created_at: i64,
    /// 更新时间（unix 秒）。
    pub updated_at: i64,
}

impl AgentRecord {
    /// 是否内置主智能体（不做工具 / 技能限制、无额外指引）。
    pub fn is_builtin_main(&self) -> bool {
        self.builtin && self.agent_id == BUILTIN_MAIN_AGENT_ID
    }
}

/// 内置默认主智能体卡片（启动时播种；工具集 / 技能集留空 = 不限制）。
pub fn builtin_main_agent(now: i64) -> AgentRecord {
    AgentRecord {
        agent_id: BUILTIN_MAIN_AGENT_ID.to_string(),
        name: "默认主智能体".into(),
        description: "系统内置：全部工具与技能可用，无额外指引。会话未指定智能体时使用。".into(),
        guidance: String::new(),
        tool_ids: Vec::new(),
        skill_ids: Vec::new(),
        // 不干预模式与权限：会话保持请求 / 缺省解析结果（与引入智能体前一致）
        mode: AgentExecMode::Inherit,
        permission: AgentPermissionMode::Inherit,
        builtin: true,
        created_at: now,
        updated_at: now,
    }
}

/// 内置 General 智能体卡片：**通用执行代理**（启动时播种）。
///
/// 与 Explore 相对：拥有完整工具访问权限（**除任务规划 `task_list`**），因此能改文件、
/// 跑命令、驱动浏览器与页签——适合并行铺开多个独立工作单元（父会话一次委派多个）。
/// 执行模式与权限模式都是**继承**：写不写、弹不弹审批卡随父会话（子会话继承父记录）。
///
/// 工具集不含 `sub_session`：子会话不能再开子会话（深度 = 1），带上它只会得到报错。
/// `browser` / `tabs` 与父会话**共享同一个浏览器实例与同一套页签**（按根会话归属，见
/// `astorm-core/src/tool/mod.rs::root_session_id`）。
pub fn builtin_general_agent(now: i64) -> AgentRecord {
    AgentRecord {
        agent_id: BUILTIN_GENERAL_AGENT_ID.to_string(),
        name: "General".into(),
        description: "一个通用代理，拥有完整的工具访问权限（除任务规划），因此可以在需要时修改文件，适合并行执行多个工作单元。".into(),
        guidance: "你是通用执行代理：可读写文件、执行命令、驱动浏览器与页签，独立完成交给你的工作单元。按需自主推进，不要中途停下等确认（需要用户决策时才用 chat_with_user 提问）；结束时用简明 Markdown 汇报做了什么——改了哪些文件、跑了什么命令、结论与遗留风险。不要给自己再派子会话（子会话不能再开子会话）。".into(),
        tool_ids: vec![
            "file_operation".into(),
            "bash".into(),
            "chat_with_user".into(),
            "search".into(),
            "read_images".into(),
            "skills".into(),
            "schedule_task".into(),
            "browser".into(),
            "office".into(),
            "tabs".into(),
            "memory".into(),
        ],
        skill_ids: Vec::new(),
        // 执行 / 审批都继承：写权限与审批行为随会话（子会话继承父记录），不由卡片另设
        mode: AgentExecMode::Inherit,
        permission: AgentPermissionMode::Inherit,
        builtin: true,
        created_at: now,
        updated_at: now,
    }
}

/// 内置 Explore 智能体卡片：**快速只读探索代理**（启动时播种）。
///
/// 只读由**工具集**决定：仅文件工具（`file_operation`）/ 搜索工具（`search`）/
/// 图片读取（`read_images`）——没有 shell，也没有
/// 任何"只能写"的工具；额外指引要求它只读不改，不做工具层硬拦截。
pub fn builtin_explore_agent(now: i64) -> AgentRecord {
    AgentRecord {
        agent_id: BUILTIN_EXPLORE_AGENT_ID.to_string(),
        name: "Explore".into(),
        description: "一个快速的只读代理，用于探索代码库，无法修改文件。适合快速查找文件、搜索代码关键字或回答关于代码库的问题。".into(),
        guidance: "你是探索型只读代理：只读取与检索，不修改任何文件。回答关于代码库的问题时先定位相关文件与代码，再给出结论，并附上文件路径与行号等可核查的引用；找不到时明确说明未找到，不要猜测。".into(),
        tool_ids: vec![
            "file_operation".into(),
            "search".into(),
            "read_images".into(),
            "memory".into(),
        ],
        skill_ids: Vec::new(),
        // 只读：会话落 Plan 模式（工具层硬拒写）；自动审批：探索期读空间外不弹卡
        // （Plan 只读是硬规则，自动审批不会放宽写禁令）
        mode: AgentExecMode::ReadOnly,
        permission: AgentPermissionMode::Auto,
        builtin: true,
        created_at: now,
        updated_at: now,
    }
}
