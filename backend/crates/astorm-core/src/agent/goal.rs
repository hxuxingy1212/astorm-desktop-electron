//! 会话「目标循环」（`/goal`）：在 ReAct 循环之外再套一层循环。
//!
//! 设定目标后，每轮正常结束（无后续、无待决中断、末条为无工具调用的助手消息）时，
//! 由一个**单次 LLM 判定器**依据「目标 + 助手最近一次最终回复」决定是否继续；需要继续时
//! 生成一条下一步用户指令注入会话，推动再跑一轮；判定为完成、或触达预算（token / 迭代，见
//! [`crate::app::config::GoalConfig`]）时把目标置为终态并收尾（记录保留供前端展示）。
//! 判定提示词随目标语言（界面语言）选中文/英文。

use llm_provider::{ChatMessage, ChatRequest, TokenUsage};
use serde::Deserialize;
use serde_json::{json, Value};

use astorm_common::session::req::CreateSessionRequest;
use astorm_common::types::record::{GoalState, GoalStatus};

use super::session::{session_info_event, unix_now, Session, SessionInput};

/// 业务侧事件：目标循环注入了一条续跑用户消息（前端据此即时补一条用户气泡）。
pub const GOAL_CONTINUE_EVENT: &str = "goal_continue";

/// 判定器输出（严格 JSON）。
#[derive(Debug, Clone, Deserialize)]
struct GoalDecision {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    message: String,
}

/// 判定提示词（中文）。
const JUDGE_SYSTEM_ZH: &str = r#"你是本会话「目标推进」的完成判定器。输入为【会话目标】与【助手最近一次无工具调用的最终回复】。
判断目标是否已经完成；未完成则给出一条能最好推进目标的下一步用户指令。

规则：
- 只依据当前真实状态判断；计划、意图、部分进展或间接信号（测试通过、有清单、有方案、已付出努力）本身都不算完成，除非覆盖了目标的每一项要求。
- 有任何缺失、未完成或未验证之处，一律视为未完成；不确定即未完成。
- 避免重复已完成的工作。
- 指令要具体、自包含，并与【会话目标】同一种语言。
- 你只做判断与指令，不亲自执行。
只输出 JSON：{ "done": boolean, "message": string }
- done=true 时 message 为空串；done=false 时 message 为下一步用户指令（非空）。"#;

/// 判定提示词（英文）。
const JUDGE_SYSTEM_EN: &str = r#"You are the completion judge for this session's goal loop. Input: the session GOAL and the assistant's most recent final reply (latest assistant message with no tool calls).
Decide whether the goal is fully achieved; if not, produce a single next user instruction that best advances it.

Rules:
- Judge against the actual current state only. Plans, intentions, partial progress, or proxy signals (passing tests, a manifest, a proposed plan, effort) do not count as completion unless they cover every requirement of the goal.
- If anything is missing, incomplete, or unverified, treat it as NOT done; uncertainty means not achieved.
- Avoid repeating work already done.
- The instruction must be concrete, self-contained, and in the same language as the goal.
- You only decide and instruct; you do not perform the work.
Output JSON only: { "done": boolean, "message": string }
- done=true -> message: ""; done=false -> message: the next user instruction (non-empty)."#;

fn is_zh(lang: &str) -> bool {
    lang.to_lowercase().starts_with("zh")
}

fn judge_system_prompt(lang: &str) -> &'static str {
    if is_zh(lang) {
        JUDGE_SYSTEM_ZH
    } else {
        JUDGE_SYSTEM_EN
    }
}

/// 目标判定输入的用户内容（目标 + 末条最终回复）。
fn judge_user_prompt(lang: &str, goal: &str, last_final: &str) -> String {
    if is_zh(lang) {
        format!("【会话目标】\n{goal}\n\n【助手最近一次无工具调用的最终回复】\n{last_final}")
    } else {
        format!("GOAL:\n{goal}\n\nASSISTANT'S LAST FINAL REPLY (no tool calls):\n{last_final}")
    }
}

/// 历史里最后一条「无工具调用的助手消息」正文（排除摘要记录）。
pub(crate) fn last_final_assistant(history: &[ChatMessage]) -> Option<&ChatMessage> {
    history.iter().rev().find(|m| {
        m.role == "assistant"
            && m.tool_calls.as_ref().map_or(true, |v| v.is_empty())
            && !is_summary_msg(m)
    })
}

fn is_summary_msg(msg: &ChatMessage) -> bool {
    msg.metadata
        .as_ref()
        .and_then(|m| m.get("summary"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 目标续跑事件：`{ type, session_id, timestamp, content }`。
fn goal_continue_event(session_id: &str, content: &str) -> Value {
    json!({
        "type": GOAL_CONTINUE_EVENT,
        "session_id": session_id,
        "timestamp": unix_now() * 1000,
        "content": content,
    })
}

impl Session {
    /// 判定一次是否继续目标推进；需要继续则注入一条用户消息并返回 `true`。
    ///
    /// 调用方须保证：本轮正常结束、未取消、无待决中断、且队列为空（否则用户消息优先）。
    pub(crate) async fn goal_continue(&self, session_id: &str) -> bool {
        if self.is_deleted() {
            return false;
        }
        // 读取目标（终态目标只展示，不再参与续跑）
        let Some(goal) = self.record.lock().ok().and_then(|r| r.goal.clone()) else {
            return false;
        };
        if !goal.is_active() {
            return false;
        }
        let cfg = self
            .goal_cfg
            .read()
            .map(|c| c.clone())
            .unwrap_or_default();

        // 记账：把自上次记账以来会话累计用量增量并入目标消耗
        let (tokens_used, iterations) = {
            let mut rec = match self.record.lock() {
                Ok(r) => r,
                Err(_) => return false,
            };
            // 目标消耗 = 自身用量 + 子 agent 聚合用量（子会话开销计入预算）
            let total = rec.session_usage.total as u64 + rec.subagent_usage.total as u64;
            if let Some(g) = rec.goal.as_mut() {
                // 判定/记账期间用户可能已设置新目标（同一会话只保留一个 goal）：
                // 旧目标的判定结果不能记到新目标上。
                if !g.is_active() || g.text != goal.text {
                    return false;
                }
                g.tokens_used = g.tokens_used.saturating_add(total.saturating_sub(g.last_total));
                g.last_total = total;
                (g.tokens_used, g.iterations)
            } else {
                return false;
            }
        };

        // 预算判定：达到上限即停止推进并保留终态
        if iterations >= cfg.max_iterations || tokens_used >= cfg.max_tokens {
            tracing::info!(
                "[session {session_id}] 目标循环达到预算（iterations={iterations}, tokens_used={tokens_used}），停止推进"
            );
            self.finish_goal_and_persist(session_id, &goal.text, GoalStatus::Stopped);
            return false;
        }

        // 末条无工具调用的助手消息（判定的第二输入）
        let last_final = {
            let history = match self.history.lock() {
                Ok(h) => h,
                Err(_) => return false,
            };
            match last_final_assistant(&history).and_then(|m| m.content.clone()) {
                Some(c) => c,
                None => {
                    // 没有最终回复（末轮以工具/中断收尾）：无法判定，停止并保留终态
                    drop(history);
                    self.finish_goal_and_persist(session_id, &goal.text, GoalStatus::Stopped);
                    return false;
                }
            }
        };

        // 单次判定
        let (decision, usage) = match judge_goal(self, &goal, &last_final).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[session {session_id}] 目标判定失败: {e}");
                self.finish_goal_and_persist(session_id, &goal.text, GoalStatus::Stopped);
                return false;
            }
        };

        let message = decision.message.trim().to_string();
        if decision.done {
            self.finish_goal_and_persist(session_id, &goal.text, GoalStatus::Completed);
            return false;
        }
        if message.is_empty() {
            self.finish_goal_and_persist(session_id, &goal.text, GoalStatus::Stopped);
            return false;
        }

        // 迭代 +1、记账判定用量，持久化目标状态
        let snapshot = {
            let mut rec = match self.record.lock() {
                Ok(r) => r,
                Err(_) => return false,
            };
            if let Some(g) = rec.goal.as_mut() {
                // 判定期间目标已被替换 / 删除：丢弃本次旧目标的续跑
                if !g.is_active() || g.text != goal.text {
                    return false;
                }
                g.tokens_used = g.tokens_used.saturating_add(usage.total as u64);
                g.iterations = g.iterations.saturating_add(1);
            } else {
                return false;
            }
            rec.updated_at = unix_now();
            rec.clone()
        };
        let _ = self.storage.upsert_session(&snapshot);
        let _ = self.event_tx.send(session_info_event(session_id, &snapshot));

        // 构造续跑用户消息（不携带新目标；模型/供应商沿用会话记录）
        let (model, provider) = {
            let rec = match self.record.lock() {
                Ok(r) => r,
                Err(_) => return false,
            };
            (rec.model.clone().unwrap_or_default(), rec.provider.clone())
        };
        let request = CreateSessionRequest {
            model,
            provider,
            pid: None,
            agent_type: None,
            session_type: None,
            scheduled_task_id: None,
            workspace: None,
            session_mode: None,
            permission_mode: None,
            reasoning_effort: None,
            user_message: message.clone(),
            attachments: None,
            auto_run: Some(true),
            mentions: None,
            elements: None,
            office_refs: None,
            quotes: None,
            goal: None,
            goal_lang: None,
        };
        match self.queue.lock() {
            Ok(mut q) => q.push_back(SessionInput::Message(request)),
            Err(_) => return false,
        }
        // 通知前端即时补一条用户气泡
        let _ = self.event_tx.send(goal_continue_event(session_id, &message));
        true
    }

    /// 结束目标并持久化 + 广播（幂等）：保留目标文本与计数，只把状态置为终态，
    /// 前端继续展示“已完成 / 已结束”，但不再参与续跑。删除目标走
    /// [`astorm_common::session::Session::update_goal`] 的 `None` 分支。
    pub(crate) fn finish_goal_and_persist(
        &self,
        session_id: &str,
        expected_text: &str,
        status: GoalStatus,
    ) {
        let snapshot = {
            let mut rec = match self.record.lock() {
                Ok(r) => r,
                Err(_) => return,
            };
            let Some(goal) = rec.goal.as_mut() else {
                return;
            };
            // 旧目标的判定结果不得结束/覆盖期间新设置的目标
            if !goal.is_active() || goal.text != expected_text {
                return;
            }
            goal.status = status;
            rec.updated_at = unix_now();
            rec.clone()
        };
        if self.storage.upsert_session(&snapshot).is_ok() {
            let _ = self.event_tx.send(session_info_event(session_id, &snapshot));
        }
    }
}

/// 调一次单次 LLM 做目标判定（严格 JSON）。
async fn judge_goal(
    session: &Session,
    goal: &GoalState,
    last_final: &str,
) -> Result<(GoalDecision, TokenUsage), String> {
    let (provider, model_ref) = {
        let rec = session
            .record
            .lock()
            .map_err(|_| "会话记录锁中毒".to_string())?;
        (rec.provider.clone(), rec.model.clone().unwrap_or_default())
    };
    let client = session
        .llm
        .client_for(provider.as_deref())
        .or_else(|| session.llm.default_client())
        .ok_or_else(|| "未配置可用的模型供应商".to_string())?;
    let api_model = session
        .llm
        .resolve_model_name(provider.as_deref(), &model_ref);
    let cancel = session
        .cancel
        .read()
        .map_err(|_| "会话取消锁中毒".to_string())?
        .child_token();

    let messages = vec![
        ChatMessage::system(judge_system_prompt(&goal.lang)),
        ChatMessage::user(judge_user_prompt(&goal.lang, &goal.text, last_final)),
    ];
    let request = ChatRequest::new(messages).with_model(&api_model);
    let result = crate::util::llm_util::complete_json(&client, request, cancel).await?;
    let decision: GoalDecision = serde_json::from_value(result.value.clone())
        .map_err(|e| format!("目标判定解析失败: {e}，原始返回: {}", result.value))?;
    Ok((decision, result.usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::ToolCall;

    fn assistant(content: &str, tools: bool, summary: bool) -> ChatMessage {
        let mut m = ChatMessage::assistant(content);
        if tools {
            m.tool_calls = Some(vec![ToolCall::new("t1", "file", "{}")]);
        }
        if summary {
            m.metadata = Some(serde_json::json!({ "summary": true }));
        }
        m
    }

    #[test]
    fn last_final_skips_tool_calls_and_summary() {
        let history = vec![
            ChatMessage::user("问"),
            assistant("中间带工具", true, false),
            ChatMessage::tool("结果", "t1"),
            assistant("最终回复", false, false),
        ];
        assert_eq!(
            last_final_assistant(&history)
                .and_then(|m| m.content.clone())
                .as_deref(),
            Some("最终回复")
        );
        // 只有带工具/摘要的助手消息 → 无最终回复
        let only_tools = vec![
            ChatMessage::user("问"),
            assistant("摘要", false, true),
            assistant("带工具", true, false),
        ];
        assert!(last_final_assistant(&only_tools).is_none());
    }

    #[test]
    fn judge_prompt_language_follows_goal() {
        assert!(judge_system_prompt("zh-CN").contains("完成判定器"));
        assert!(judge_system_prompt("en-US").contains("completion judge"));
        assert!(judge_user_prompt("zh-CN", "目标X", "回复Y").contains("【会话目标】"));
        assert!(judge_user_prompt("en-US", "goal X", "reply Y").contains("GOAL:"));
    }
}
