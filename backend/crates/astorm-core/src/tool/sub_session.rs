//! `sub_session` 工具：把独立子任务**委派给子会话**执行，并同步等待其最终回答。
//!
//! 子会话是**普通会话**（同一条会话链路：落盘 / 侧栏展示 / 事件流向一致），差别只有三点：
//! - 记录带 `pid`（父会话 id）与 `agent_type`（本次委派指定的智能体）；
//! - 工作区落在 `父会话工作区/.astorm/subagent_{子会话id}/`（见 `agent::session::create_session`）；
//! - **不允许再开子会话**（深度 = 1，防递归爆炸）。
//!
//! 权限 / 模式 / 模型 / 供应商由 `create_session` 从父会话记录继承（`pid` 存在时）。
//! 子会话若触发审批（手动权限模式），其审批卡在前端**它自己的会话卡片**上正常处理，
//! 本工具只是继续等待（收到 `interrupt` 事件不结束等待）——父会话表现为「工具执行中」。

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use astorm_common::session::req::CreateSessionRequest;
use astorm_common::session::Session as SessionFacade;
use astorm_common::types::record::{SessionRecord, SessionStatus, SessionType};

use crate::tool::{session_record, storage, ToolContext};
use react_loop::event::ty;

/// 子会话等待期间的单次事件等待上限：仅用于周期性检查父会话取消（不结束等待）。
const WAIT_TICK: Duration = Duration::from_secs(1);

/// 工具参数。
#[derive(Debug, Deserialize)]
struct SubSessionArgs {
    /// 交给子会话的任务描述（作为子会话的首条用户消息）。
    task: String,
    /// 子会话使用的智能体 id（必填；`# 可用智能体` 块里给出）。
    agent_type: String,
}

/// `sub_session` 工具。
pub struct SubSessionTool;

/// 子会话的最终回答：最后一条**无工具调用**的 assistant 文本消息。
///
/// 取落盘历史而非拼接流式分片：多轮循环里正文可能被工具调用切分，落盘的最后一条
/// assistant 文本才是「结论」；工具调用轮次的消息 `tool_calls` 非空，跳过。
fn final_answer(session_id: &Uuid) -> Option<String> {
    let records = storage()?.load_messages(&session_id.to_string()).ok()?;
    records
        .iter()
        .rev()
        .find(|r| {
            r.role == "assistant"
                && r.tool_calls.is_null()
                && !r.content.trim().is_empty()
        })
        .map(|r| r.content.trim().to_string())
}

/// 子会话结束态（由事件流判定）。
enum ChildExit {
    /// 正常跑完
    Done,
    /// 异常结束（附事件里的 message / error 文本）
    Error(String),
    /// 被用户取消
    Cancelled,
}

/// 由落盘状态兜底判定子会话是否已结束（事件滞后 / 丢失时的保险）。
///
/// 创建时状态即为 `Running`，故终态只可能来自循环收尾（`agent::run` 的终态块）：
/// `done` / `error` / `cancelled` 三态落盘，表单「忽略」落 `Ignored`。
/// `awaiting_*`（等审批 / 等回答）**不算结束**：等用户在前端子会话卡片上处理。
/// `Ignored` 与 `Idle` 同判为已结束（与引入该状态前的行为一致）——子会话的提问被忽略后
/// 本轮已停，父会话若继续等会永久挂起（那条提问可经 `reopenInterrupt` 继续，届时子会话会再跑一轮）。
fn exit_from_status(record: &SessionRecord) -> Option<ChildExit> {
    match record.status {
        SessionStatus::Done | SessionStatus::Idle | SessionStatus::Ignored => {
            Some(ChildExit::Done)
        }
        SessionStatus::Error => Some(ChildExit::Error("会话异常结束".into())),
        SessionStatus::Cancelled => Some(ChildExit::Cancelled),
        SessionStatus::Running
        | SessionStatus::AwaitingApproval
        | SessionStatus::AwaitingForm
        | SessionStatus::AwaitingMailCompose
        | SessionStatus::Interrupted => None,
    }
}

/// 读取子会话当前落盘状态并判定是否结束。
fn poll_child_exit(child_id: &Uuid) -> Option<ChildExit> {
    session_record(&child_id.to_string()).and_then(|record| exit_from_status(&record))
}

#[async_trait]
impl crate::tool::Tool for SubSessionTool {
    fn name(&self) -> &str {
        "sub_session"
    }

    fn description(&self) -> &str {
        "把独立子任务委派给子会话执行：新建一个子会话（继承父会话的模型/供应商/工作区，工作区在父会话工作区的 .astorm/subagent_<会话id>/ 下），等待它跑完并返回子会话的最终回答。模式与权限默认继承父会话，但**目标智能体卡片可覆盖**（如只读智能体强制 plan，无法改动文件）。适合可独立完成、上下文自足的调研/实现/验证类任务。子会话不能再开子会话；一次委派就是一次完整会话，耗时与 token 都不低，不要用于琐碎步骤。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "子任务描述（作为子会话的首条用户消息，需自足：交代目标、范围、交付物与验收标准）"
                },
                "agent_type": {
                    "type": "string",
                    "description": "子会话使用的智能体 id（见环境提示词「可用智能体」块）"
                }
            },
            "required": ["task", "agent_type"]
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<String, String> {
        let args: SubSessionArgs = serde_json::from_value(params)
            .map_err(|e| format!("参数解析失败（需要 task / agent_type）: {e}"))?;
        if args.task.trim().is_empty() {
            return Err("task 不能为空".into());
        }
        let app = crate::app::App::try_global()
            .ok_or_else(|| "应用未初始化，无法创建子会话".to_string())?;
        let parent = session_record(&ctx.session_id).ok_or_else(|| {
            format!("读取父会话失败（会话不存在）: {}", ctx.session_id)
        })?;
        // 深度 = 1：子会话不允许再开子会话（`pid` 存在即已是子会话）
        if parent.pid.is_some() {
            return Err("子会话不允许再创建子会话（委派深度仅 1 层）".into());
        }
        // 目标智能体必须存在（缺省不兜底：委派必须明确指定）
        let card = app.agents.resolve(Some(&args.agent_type))?;
        if card.is_builtin_main() {
            // 显式委派给主智能体是允许的（等价默认能力），此处仅提示便于排查
            tracing::info!("[session {}] sub_session 委派给内置主智能体", ctx.session_id);
        }

        // **先订阅再创建**：事件总线是 broadcast 且无回放，先建子会话可能丢掉早期事件
        let mut rx = app.subscribe();
        let request = CreateSessionRequest {
            // 模型 / 供应商 / 模式 / 权限 / 思考强度留空 → 由 create_session 从父会话继承
            model: String::new(),
            provider: None,
            pid: Some(parent.id),
            agent_type: Some(card.agent_id.clone()),
            // 子会话类型（create_session 亦按 pid 强制归一，这里显式声明意图）
            session_type: Some(SessionType::Child),
            // 子会话不归属定时任务
            scheduled_task_id: None,
            workspace: None,
            session_mode: None,
            permission_mode: None,
            reasoning_effort: None,
            user_message: args.task.clone(),
            attachments: None,
            // 子会话必须执行：工具在等它的终态事件（idle 会被 exit_from_status 当作已结束）
            auto_run: Some(true),
            // 子会话任务文本来自模型，无前端提及 / 无元素 / 无目标
            mentions: None,
            elements: None,
            office_refs: None,
            quotes: None,
            goal: None,
            goal_lang: None,
        };
        let child_id = app.session().create_session(request)?;
        let child_id_str = child_id.to_string();
        tracing::info!(
            "[session {}] sub_session 已创建子会话 {child_id_str}（智能体: {}）",
            ctx.session_id,
            card.name
        );

        // 等待子会话结束：只认它自己的终态事件；`interrupt`（子会话等审批）继续等；
        // 父会话取消 → 传播取消给子会话并返回。
        let exit = loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    let _ = app.session().cancel_session(&child_id);
                    return Err(format!(
                        "父会话已取消，子会话 {child_id_str} 已一并取消（未取到结果）"
                    ));
                }
                received = tokio::time::timeout(WAIT_TICK, rx.recv()) => {
                    match received {
                        // 本 tick 内无事件：轮询落盘状态兜底（事件可能因广播滞后被跳过）
                        Err(_) => {
                            if let Some(exit) = poll_child_exit(&child_id) {
                                break exit;
                            }
                            continue;
                        }
                        Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped))) => {
                            tracing::warn!("[sub_session] 事件流滞后，跳过 {skipped} 条事件");
                            if let Some(exit) = poll_child_exit(&child_id) {
                                break exit;
                            }
                            continue;
                        }
                        Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                            return Err("事件总线已关闭，无法等待子会话结果".into());
                        }
                        Ok(Ok(event)) => {
                            if event.get("session_id").and_then(|v| v.as_str()) != Some(&child_id_str) {
                                continue;
                            }
                            match event.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
                                ty::DONE => break ChildExit::Done,
                                ty::CANCELLED => break ChildExit::Cancelled,
                                ty::ERROR => {
                                    let detail = event
                                        .get("message")
                                        .or_else(|| event.get("error"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("未知错误")
                                        .to_string();
                                    break ChildExit::Error(detail);
                                }
                                // interrupt（等审批 / 等回答）与其他流式事件：继续等
                                _ => continue,
                            }
                        }
                    }
                }
            }
        };

        let answer = final_answer(&child_id)
            .unwrap_or_else(|| "（子会话无文本输出）".to_string());
        let head = match exit {
            ChildExit::Done => format!("[子会话 {child_id_str}] 已完成。"),
            ChildExit::Error(e) => format!("[子会话 {child_id_str}] 异常结束（{e}）。"),
            ChildExit::Cancelled => format!("[子会话 {child_id_str}] 被用户取消。"),
        };
        Ok(format!("{head}\n\n{answer}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astorm_common::types::record::SessionStatus;

    fn record_with(status: SessionStatus) -> SessionRecord {
        SessionRecord {
            id: Uuid::new_v4(),
            pid: None,
            session_type: SessionType::Normal,
            scheduled_task_id: None,
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
            mcp_list: vec![],
            loaded_tool_list: vec![],
            session_usage: Default::default(),
            contributed_to_parent: Default::default(),
            subagent_usage: Default::default(),
            context_tokens: 0,
            summary_times: 0,
            question_times: 0,
            current_round: 0,
            created_at: 0,
            updated_at: 0,
            status,
            status_updated_at: 0,
            goal: None,
        }
    }

    /// 终态判定：只有 done / error / cancelled 结束等待；等审批 / 等回答 / 运行中都不算。
    #[test]
    fn 终态判定覆盖审批与运行中() {
        assert!(matches!(exit_from_status(&record_with(SessionStatus::Done)), Some(ChildExit::Done)));
        // 表单「忽略」落 Ignored，与 Idle 一样视为已结束
        assert!(matches!(exit_from_status(&record_with(SessionStatus::Idle)), Some(ChildExit::Done)));
        assert!(matches!(
            exit_from_status(&record_with(SessionStatus::Ignored)),
            Some(ChildExit::Done)
        ));
        assert!(matches!(
            exit_from_status(&record_with(SessionStatus::Error)),
            Some(ChildExit::Error(_))
        ));
        assert!(matches!(
            exit_from_status(&record_with(SessionStatus::Cancelled)),
            Some(ChildExit::Cancelled)
        ));
        for pending in [SessionStatus::Running, SessionStatus::AwaitingApproval, SessionStatus::AwaitingForm] {
            assert!(
                exit_from_status(&record_with(pending)).is_none(),
                "{pending:?} 不应结束等待（等用户在前端子会话卡片上处理）"
            );
        }
    }
}
