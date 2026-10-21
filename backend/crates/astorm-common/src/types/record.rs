//! 落盘记录：会话元数据（富版）与消息历史。

use llm_provider::TokenUsage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::session::types::{PermissionMode, ReasoningEffort, SessionMode};

/// 会话运行状态（**会话级**元数据；与消息级 `metadata.interrupt` / `metadata.status` 各管一层）。
///
/// 唯一真相源在后端：侧栏状态图标据此渲染，前端不再自造终态。
/// 磁盘上的老记录没有该字段 → 反序列化为 `Idle`（无图标，与引入前的行为一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// 空闲 / 未知（老记录缺省值）
    #[default]
    Idle,
    /// 循环在跑（含排队待消费）
    Running,
    /// 等待用户审批（安全拦截）
    AwaitingApproval,
    /// 等待用户回答（chat_with_user 表单）
    AwaitingForm,
    /// 等待用户确认发送邮件（mail 工具 send：就地写邮件 UI）
    AwaitingMailCompose,
    /// 正常结束
    Done,
    /// 异常结束
    Error,
    /// 用户取消 / 暂停
    Cancelled,
    /// 上次运行被进程退出打断（冷启动把陈旧的 `running` 修正为此态）
    Interrupted,
    /// 用户忽略了 `chat_with_user` 的提问（`submitInterrupt` 的 `ignored` 决议）：
    /// 本轮不续跑、会话已停下，但那条提问仍可经 `reopenInterrupt` 重新填写（侧栏灰点）。
    Ignored,
}

/// 会话类型（**会话级**元数据：这条会话从哪来）。
///
/// 唯一真相源在后端：前端侧栏分组与类型徽标据此渲染，不再按工作区路径猜。
/// 磁盘上的老记录没有该字段 → 反序列化为 `Normal`（与引入前的行为一致，
/// 定时任务老会话由 [`crate::types::SessionRecord`] 的调用方一次性回填）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    /// 普通会话（用户手动新建；默认值 / 老记录缺省值）
    #[default]
    Normal,
    /// 定时任务触发创建的会话（`scheduler` 新建；`continuous_session` 后续轮次复用同一条）
    Scheduled,
    /// 项目会话（用户显式指定了工作目录）
    Project,
    /// 子会话（`sub_session` 工具委派创建；`pid` 指向父会话）
    Child,
}

/// 会话持久化记录（可序列化，供前端列表渲染 / 存储落盘）。
///
/// 统一为单一定义：`astorm-core` 的会话运行时持有它，`astorm-storage` 直接持久化整条记录。
/// 目标生命周期状态（`/goal`）：结束后保留记录供前端展示，只有用户删除才移除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    /// 目标循环仍在推进（含用户暂停）
    #[default]
    Active,
    /// 判定器确认目标已达成
    Completed,
    /// 触达预算 / 判定失败 / 无最终回复等，未达成而结束
    Stopped,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Completed => "completed",
            GoalStatus::Stopped => "stopped",
        }
    }
}

/// 会话「目标循环」状态（`/goal`）：`None` = 无目标。
///
/// 目标是一个特殊状态标记：设定后，外层循环在每轮正常结束后由单次 LLM 判定是否继续推进；
/// 判定结束或触达预算（token / 迭代）时**保留记录并把 [`GoalState::status`] 置为终态**，
/// 前端继续展示目标与“已完成 / 已结束”状态；删除目标才把整个 `goal` 清空。
/// `tokens_used`/`last_total` 用于统计本次目标运行期间的 token 消耗
/// （`last_total` = 上次记账时的会话累计用量基线）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalState {
    /// 目标文本（用户提供）。
    pub text: String,
    /// 界面语言（如 `zh-CN` / `en-US`）：决定判定提示词与续跑指令的语言。
    #[serde(default)]
    pub lang: String,
    /// 本次目标运行累计消耗 token（含判定调用）。
    #[serde(default)]
    pub tokens_used: u64,
    /// 上次记账时的 `SessionRecord.session_usage.total`（增量基线）。
    #[serde(default)]
    pub last_total: u64,
    /// 已执行的续跑迭代次数（判定为「继续」并注入用户消息的次数）。
    #[serde(default)]
    pub iterations: u32,
    /// 生命周期状态；旧记录缺省 = 仍在推进。
    #[serde(default)]
    pub status: GoalStatus,
}

impl GoalState {
    /// 是否仍参与目标循环（终态只展示，不再续跑）。
    pub fn is_active(&self) -> bool {
        self.status == GoalStatus::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: Uuid,
    /// 父会话 id（子会话工具创建时携带；普通会话为 None）。子会话工作区落在
    /// `父会话工作区/.astorm/subagent_{id}/`，且不允许再开子会话。
    pub pid: Option<Uuid>,
    /// 会话类型（侧栏分组 / 类型徽标；`serde(default)` 兼容老记录 → `Normal`）
    #[serde(default)]
    pub session_type: SessionType,
    /// 产生本条会话的定时任务 id（普通 / 子会话为 None；`session_type == Scheduled` 时有值）。
    ///
    /// 归属真相只在会话侧：一个任务多次 `new_session` 触发 → 多条会话都指回同一任务，
    /// 前端据此在任务卡片下按时间列出执行记录。
    #[serde(default)]
    pub scheduled_task_id: Option<String>,
    /// 智能体 id（创建时由请求解析而来：未指定 → 内置默认主智能体）。
    /// 会话的 `tool_list` / `skill_list` 即按该卡片物化；卡片被删除后冻结为当时的快照。
    #[serde(default)]
    pub agent_type: Option<String>,
    pub title: Option<String>,
    /// 中文标题（LLM 双标题生成；旧记录缺省，前端回退 `title`）
    #[serde(default)]
    pub title_zh: Option<String>,
    /// English title（LLM 双标题生成；旧记录缺省，前端回退 `title`）
    #[serde(default)]
    pub title_en: Option<String>,
    /// 会话使用的模型 / 供应商(请求级持久化;会话信息卡片展示)。
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,

    pub workspace: Option<String>,
    pub session_mode: SessionMode,
    pub reasoning_effort: ReasoningEffort,
    pub permission_mode: PermissionMode,
    /// 动态加载的 skill/tool/mcp 列表
    pub skill_list: Vec<String>,
    pub tool_list: Vec<String>,
    pub mcp_list: Vec<String>,
    /// 经 `tool_load` 显式加载的工具（LLM 实际可用集 = `tool_list ∪ loaded_tool_list`；
    /// `tool_list` 保持「智能体卡片物化的可见工具集」语义，由冷启动自愈覆写，这里**不**随自愈重算）。
    /// `serde(default)` 兼容老记录；历史消息重建 / subsession 分叉亦不受影响。
    #[serde(default)]
    pub loaded_tool_list: Vec<String>,

    /// 会话 token 用量（**累计**：跨轮次累加）
    pub session_usage: TokenUsage,
    /// 子会话专用：**已经累加到父会话**的 token 累计（用于计算增量、避免重复累加）。
    /// 普通会话恒为默认值。
    #[serde(default)]
    pub contributed_to_parent: TokenUsage,
    /// 父会话聚合的**所有子会话** token 用量（子会话每轮把增量累加到这里）。
    /// 展示总量取 `session_usage + subagent_usage`；`/goal` 预算也据此把子 agent 开销计入。
    #[serde(default)]
    pub subagent_usage: TokenUsage,
    /// 当前上下文占用的真实 token 数（**上一轮** LLM 请求的 `prompt_tokens`；
    /// 会话信息「上下文容量」进度条与摘要阈值判定使用；provider 未返回用量时为 0）
    #[serde(default)]
    pub context_tokens: u32,
    /// 摘要次数
    pub summary_times: i32,
    /// 用户提问次数（累计：每追加一条用户消息 +1；撤回历史时回退到保留条数）
    #[serde(default)]
    pub question_times: i32,
    /// 当前轮次（累计 LLM 调用轮数）
    pub current_round: i32,
    /// 创建时间（unix 秒）
    pub created_at: i64,
    /// 更新时间（unix 秒）
    pub updated_at: i64,
    /// 会话运行状态（侧栏状态图标；`serde(default)` 兼容老记录）
    #[serde(default)]
    pub status: SessionStatus,
    /// 状态最后变更时间（unix 秒；0 = 从未记录）
    #[serde(default)]
    pub status_updated_at: i64,
    /// 目标循环状态（`/goal`；`None` = 无目标）。
    #[serde(default)]
    pub goal: Option<GoalState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 老记录（无 status / statusUpdatedAt 字段）必须能无损反序列化 → 默认 Idle。
    /// 样本里的 `gitBranch` 是**已删除的历史字段**：老记录里仍带着它，必须被忽略而不是报错。
    #[test]
    fn 会话记录向后兼容缺省状态() {
        let legacy = r#"{
            "id": "018f0e2a-0000-7000-8000-000000000001",
            "pid": null, "title": "旧会话", "workspace": null, "gitBranch": null,
            "sessionMode": "execute", "reasoningEffort": "medium", "permissionMode": "manual",
            "skillList": [], "toolList": [], "mcpList": [],
            "sessionUsage": { "prompt": 0, "completion": 0, "total": 0 }, "summaryTimes": 0, "currentRound": 3,
            "createdAt": 1, "updatedAt": 2
        }"#;
        let record: SessionRecord = serde_json::from_str(legacy).expect("老记录应可反序列化");
        assert_eq!(record.status, SessionStatus::Idle);
        assert_eq!(record.status_updated_at, 0);
        assert!(record.agent_type.is_none(), "老记录无智能体字段 → None（默认主智能体语义）");
        assert_eq!(record.session_type, SessionType::Normal, "老记录无会话类型 → Normal");
        assert!(record.scheduled_task_id.is_none(), "老记录无定时任务归属 → None");
    }

    /// 序列化名与前端契约一致（snake_case 字符串，camelCase 字段名）。
    #[test]
    fn 状态字段序列化契约() {
        let json = serde_json::to_value(SessionStatus::AwaitingApproval).unwrap();
        assert_eq!(json, serde_json::json!("awaiting_approval"));
        assert_eq!(
            serde_json::to_value(SessionStatus::Interrupted).unwrap(),
            serde_json::json!("interrupted")
        );
        let record = SessionRecord {
            id: Uuid::nil(),
            pid: None,
            session_type: SessionType::Scheduled,
            scheduled_task_id: Some("task-1".into()),
            agent_type: None,
            title: None,
            title_zh: None,
            title_en: None,
            model: None,
            provider: None,
            workspace: None,
            session_mode: Default::default(),
            reasoning_effort: Default::default(),
            permission_mode: Default::default(),
            skill_list: vec![],
            tool_list: vec![],
            loaded_tool_list: vec![],
            mcp_list: vec![],
            session_usage: Default::default(),
            contributed_to_parent: Default::default(),
            subagent_usage: Default::default(),
            context_tokens: 0,
            summary_times: 0,
            question_times: 0,
            current_round: 0,
            created_at: 0,
            updated_at: 0,
            status: SessionStatus::Done,
            status_updated_at: 42,
            goal: None,
        };
        let v = serde_json::to_value(&record).unwrap();
        assert_eq!(v["status"], serde_json::json!("done"));
        assert_eq!(v["statusUpdatedAt"], serde_json::json!(42));
        assert_eq!(v["sessionType"], serde_json::json!("scheduled"));
        assert_eq!(v["scheduledTaskId"], serde_json::json!("task-1"));
    }

    /// 目标状态契约：旧记录缺省 = active（继续推进）；终态只展示、不再续跑。
    #[test]
    fn 目标状态缺省与序列化契约() {
        let mut goal: GoalState =
            serde_json::from_value(serde_json::json!({ "text": "目标" })).unwrap();
        assert!(goal.is_active());
        assert_eq!(serde_json::to_value(&goal).unwrap()["status"], "active");
        goal.status = GoalStatus::Completed;
        assert!(!goal.is_active());
        assert_eq!(serde_json::to_value(&goal).unwrap()["status"], "completed");
    }

    /// 会话类型取值契约（前端按这四个字符串判分组与徽标）。
    #[test]
    fn 会话类型序列化契约() {
        for (t, name) in [
            (SessionType::Normal, "normal"),
            (SessionType::Scheduled, "scheduled"),
            (SessionType::Project, "project"),
            (SessionType::Child, "child"),
        ] {
            assert_eq!(serde_json::to_value(t).unwrap(), serde_json::json!(name));
        }
        assert_eq!(SessionType::default(), SessionType::Normal);
    }
}

/// 消息持久化记录：会话内追加式消息流。
/// 富字段（reasoning / tool_call_id / metadata）带默认值,旧 jsonl 行可无损读取。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    /// 消息唯一 id。
    pub id: String,
    /// 所属会话 id。
    pub session_id: String,
    /// 角色：user / assistant / tool / system。
    pub role: String,
    /// 消息正文。
    pub content: String,
    /// 工具调用载荷（JSON，可选）。
    #[serde(default)]
    pub tool_calls: Value,
    /// 会话内递增序号（用于排序 / 断点续传）。
    pub seq: i64,
    /// 创建时间（unix 秒）。
    pub created_at: i64,
    /// 思考链（assistant 消息；历史回读渲染 think 块）。
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// 工具结果消息关联的 tool_call id（assistant tool_calls 的 id）。
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// 业务元数据（工具耗时 / 错误等；发送给 LLM 前剥离,仅用于回读展示）。
    #[serde(default)]
    pub metadata: Option<Value>,
}