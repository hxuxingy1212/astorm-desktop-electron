//! 会话请求体。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::session::types::{PermissionMode, ReasoningEffort, SessionMode};
use crate::types::record::{SessionRecord, SessionType};

/// 用户消息里结构化提及的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionKind {
    /// 快捷命令（`/`；当前后端不处理，仅为契约占位）
    Command,
    /// 技能（`$name`）
    Skill,
    /// 子智能体（`$agent/<id>`）
    Agent,
    /// 文件引用（`@<相对路径>`）
    File,
}

/// 用户消息中的结构化提及（快速引用 `@` / `$` 的机器可读形式）。
///
/// 由前端在发送时从输入框 chip 收集；后端据此生成给模型的**引用说明**。
/// 正文只保存用户可见的「原文」（文件路径 / 智能体名 / 元素编号标签）；说明写进消息 metadata，
/// **不渲染**，喂模型与冷启动重建时再拼回 user 消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionRef {
    /// 提及类型
    pub kind: MentionKind,
    /// 标识：技能名 / 智能体 id（裸，不含 `agent/` 前缀）/ 文件工作区相对路径 / 命令 id
    pub id: String,
    /// 原文 token（如 `$agent/builtin-explore`、`@src/foo.ts`），仅用于对齐与展示
    pub token: String,
    /// 展示名（技能名 / 智能体名 / 文件名；可选）
    #[serde(default)]
    pub label: Option<String>,
}

/// 用户消息里对既有会话正文的引用（对话「添加到当前任务」选中的一段）。
///
/// 与 [`MentionRef`] 同源：由前端在发送时收集。正文只保留「原文」的截断预览，
/// 这里携带**完整选中文本 + 来源消息**，后端据此生成引用说明（写进消息 metadata，
/// 不渲染；喂模型 / 冷启动重建时拼回 user 消息）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRef {
    /// 选中正文原文（完整）
    pub text: String,
    /// 来源消息序号（`#seq`；可选）
    #[serde(default)]
    pub seq: Option<i64>,
    /// 来源角色：`assistant` / `user`（可选）
    #[serde(default)]
    pub role: Option<String>,
}

/// 创建会话的请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// 模型引用 id（供应商模型列表中的 `id`；未匹配时原样透传，允许自定义模型名直连）
    pub model: String,
    /// 供应商名（可选；缺省用默认供应商）
    #[serde(default)]
    pub provider: Option<String>,
    /// 父会话 id（可选）：**仅后端子会话工具**创建子会话时携带。
    /// 携带时工作区落在 `父会话工作区/.astorm/subagent_{本会话id}/`（显式 workspace 优先），
    /// 且该会话不允许再创建子会话（深度 = 1）。
    #[serde(default)]
    pub pid: Option<Uuid>,
    /// 智能体 id（可选）：缺省 = 内置默认主智能体。
    /// 决定本会话可见的工具集（强制过滤）、技能集（仅提示词）与额外系统指引。
    #[serde(default)]
    pub agent_type: Option<String>,
    /// 会话类型（可选）：缺省 [`SessionType::Normal`]。
    /// 调用方声明"这条会话从哪来"（定时任务 / 项目 / 子会话）；携带 `pid` 时**强制**
    /// 归一为 [`SessionType::Child`]（见 `SessionImpl::create_session`）。
    #[serde(default)]
    pub session_type: Option<SessionType>,
    /// 产生本条会话的定时任务 id（仅后端起调度器注入；前端不暴露）。
    #[serde(default)]
    pub scheduled_task_id: Option<String>,
    /// 工作空间（可选）
    pub workspace: Option<String>,
    /// 模式（计划 / 执行 / 自动）；未携带时不改动会话既有模式（运行时切换跟随最新请求）
    #[serde(default)]
    pub session_mode: Option<SessionMode>,
    /// 权限（手动 / 自动授权）；未携带时不改动会话既有权限
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// 思考强度（可选）：无 / 低 / 中 / 高
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 文字消息（首条用户输入）
    pub user_message: String,
    /// 附件（可选）：本地**绝对路径**列表（前端主进程处理后的路径）。按文件内容分两类：
    /// - **图片**（JPEG/PNG/GIF/WebP，按魔数判定）：复制进会话工作区 `.astorm/attachment/`，
    ///   作为多模态 user 消息随本条消息上送；单条 ≤8 张、单张 ≤20MB、总量 ≤30MB；
    /// - **其他文件**：**不读内容、不复制**，只把原始路径追加进用户消息文本（模型用
    ///   自行读取（PDF/Office 用 astorm-office 技能、图片用 `read_images`、文本用
    ///   `file_operation`）），数量 ≤20 个。
    ///
    /// 两类都把描述写进消息 metadata（前端缩略图 / 文件胶囊 / 冷启动重建据此恢复）；
    /// 超限或非法项逐项忽略并在文本中注明。
    pub attachments: Option<Vec<String>>,
    /// 创建后是否**立即执行**（缺省 = 执行，保持既有语义）。
    ///
    /// `Some(false)`（「仅创建」）：建记录 → 落标题 → 把 `user_message` 写进会话历史 → **不起
    /// 循环**（状态落 `idle`，前端看板归「待办」）。用于「先建任务、稍后再跑」这一类入口。
    /// 定时任务调度器与 `sub_session` 工具必须保持执行（前者要跑任务，后者的退出判定把
    /// `idle` 当作已结束）。
    #[serde(default)]
    pub auto_run: Option<bool>,
    /// 结构化提及（可选）：`@` / `$` 快速引用的机器可读形式，见 [`MentionRef`]。
    ///
    /// 缺省 = 无提及，行为与引入前完全一致。后端据此生成「引用说明」：**喂模型**的副本尾部追加
    /// 指令块（加载技能 / 委派子智能体 / 文件路径提示）；原样正文落盘，说明写进消息 metadata
    /// （**不渲染**），冷启动重建历史时再拼回 user 消息。
    #[serde(default)]
    pub mentions: Option<Vec<MentionRef>>,
    /// 浏览器拾取元素的围栏原文（可选）：每项是一段 ```element 围栏。
    ///
    /// 与 [`Self::mentions`] 同源——由前端输入框 chip 收集。元素标签（如 `2. a.cover`）作为用户
    /// 可见正文落盘，围栏数据作为「说明」进 metadata（**不渲染**），喂模型 / 重建时拼回 user 消息。
    #[serde(default)]
    pub elements: Option<Vec<String>>,
    /// office 预览拾取的围栏原文（可选）：每项是一段 ```office-ref 围栏。
    ///
    /// 与 [`Self::elements`] 同源——office 预览面板「选择路径」模式下点击元素，由输入框
    /// chip 收集。chip 标签（如 `1. 第 2 段`）作为用户可见正文落盘，围栏（文件 + CLI 树形
    /// 路径）作为「说明」进 metadata（**不渲染**），喂模型 / 重建时拼回 user 消息。
    #[serde(default)]
    pub office_refs: Option<Vec<String>>,
    /// 对话正文引用（可选）：对话「添加到当前任务」选中的一段，见 [`QuoteRef`]。
    #[serde(default)]
    pub quotes: Option<Vec<QuoteRef>>,
    /// 目标（可选，`/goal`）：非空时把本会话标记为「目标循环」。随本条消息原子设置，
    /// 后端在跑循环前写入 `SessionRecord.goal`（`None` = 不改动既有目标）。
    #[serde(default)]
    pub goal: Option<String>,
    /// 目标语言（可选，`/goal`；如 `zh-CN` / `en-US`）：决定判定提示词与续跑指令的语言。
    #[serde(default)]
    pub goal_lang: Option<String>,
}

/// 会话级设置（模式 / 审批 / 思考强度 / 模型 / 供应商）。
///
/// 这五项都是**会话的持久化属性**（落在 [`SessionRecord`] 里）：
/// - 创建会话时由首条请求写入；
/// - 之后可由前端经 `update_session_settings` **异步改写**（不必发消息），
///   工具策略每个工具调用、LLM 每轮都读活记录，故**立即生效**；
/// - 发消息时**不再需要携带**（缺省 = 保持），避免「UI 里旧值把会话改回去」。
///
/// 全部字段可选：**缺省 = 保持原值**，不回落默认值（全缺省视为无操作）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettings {
    /// 模式：计划 / 执行 / 自动
    #[serde(default)]
    pub session_mode: Option<SessionMode>,
    /// 权限：手动 / 自动授权
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// 思考强度：无 / 低 / 中 / 高
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 模型引用 id（空串视为缺省 = 保持；会话必须有模型，不支持清空）
    #[serde(default)]
    pub model: Option<String>,
    /// 供应商名（缺省 = 保持）
    #[serde(default)]
    pub provider: Option<String>,
}

impl SessionSettings {
    /// 是否一个字段都没给出（调用方可据此跳过落盘与广播）。
    pub fn is_empty(&self) -> bool {
        self.session_mode.is_none()
            && self.permission_mode.is_none()
            && self.reasoning_effort.is_none()
            && self.model.as_deref().map(str::trim).unwrap_or("").is_empty()
            && self.provider.is_none()
    }

    /// 并入会话记录，返回是否发生变化（**缺省字段保持原值**）。
    ///
    /// 唯一的合并实现：`update_session_settings` 与发消息时的
    /// `sync_mode_permission` 都走这里，避免两套语义漂移。
    pub fn apply_to(&self, record: &mut SessionRecord) -> bool {
        let mut changed = false;
        if let Some(m) = self.session_mode {
            if record.session_mode != m {
                record.session_mode = m;
                changed = true;
            }
        }
        if let Some(p) = self.permission_mode {
            if record.permission_mode != p {
                record.permission_mode = p;
                changed = true;
            }
        }
        if let Some(e) = self.reasoning_effort {
            if record.reasoning_effort != e {
                record.reasoning_effort = e;
                changed = true;
            }
        }
        if let Some(m) = self.model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if record.model.as_deref() != Some(m) {
                record.model = Some(m.to_string());
                changed = true;
            }
        }
        if let Some(p) = self.provider.as_deref() {
            if record.provider.as_deref() != Some(p) {
                record.provider = Some(p.to_string());
                changed = true;
            }
        }
        changed
    }
}

impl From<&CreateSessionRequest> for SessionSettings {
    /// 发消息携带的设置（历史行为：UI 每条消息随发）。空模型名视为未给出。
    fn from(r: &CreateSessionRequest) -> Self {
        Self {
            session_mode: r.session_mode,
            permission_mode: r.permission_mode,
            reasoning_effort: r.reasoning_effort,
            model: Some(r.model.trim().to_string()).filter(|m| !m.is_empty()),
            provider: r.provider.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> SessionRecord {
        serde_json::from_str(
            r#"{"id":"00000000-0000-0000-0000-000000000001","pid":null,"title":null,
                "workspace":null,"sessionMode":"plan","reasoningEffort":"none",
                "permissionMode":"manual","skillList":[],"toolList":[],"mcpList":[],
                "sessionUsage":{"prompt":0,"completion":0,"total":0},"summaryTimes":0,
                "currentRound":0,"createdAt":1,"updatedAt":1}"#,
        )
        .expect("会话记录样本反序列化失败")
    }

    #[test]
    fn empty_settings_keep_everything() {
        let mut rec = record();
        assert!(SessionSettings::default().is_empty());
        assert!(!SessionSettings::default().apply_to(&mut rec));
        assert_eq!(rec.session_mode, SessionMode::Plan);
        assert_eq!(rec.permission_mode, PermissionMode::Manual);
        assert!(rec.model.is_none());
    }

    #[test]
    fn given_fields_apply_and_absent_fields_keep() {
        let mut rec = record();
        rec.model = Some("old-model".into());
        let s = SessionSettings {
            session_mode: Some(SessionMode::Auto),
            model: Some("new-model".into()),
            ..Default::default()
        };
        assert!(!s.is_empty());
        assert!(s.apply_to(&mut rec));
        assert_eq!(rec.session_mode, SessionMode::Auto);
        assert_eq!(rec.model.as_deref(), Some("new-model"));
        // 未给出 → 保持
        assert_eq!(rec.permission_mode, PermissionMode::Manual);
        assert_eq!(rec.reasoning_effort, ReasoningEffort::None);
        // 同值再写一次 → 视为无变化(幂等,不落盘不广播)
        assert!(!s.apply_to(&mut rec));
    }

    #[test]
    fn blank_model_is_treated_as_absent() {
        let mut rec = record();
        rec.model = Some("keep-me".into());
        let s = SessionSettings { model: Some("   ".into()), ..Default::default() };
        assert!(s.is_empty());
        assert!(!s.apply_to(&mut rec));
        assert_eq!(rec.model.as_deref(), Some("keep-me"));
    }
}