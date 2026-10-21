//! 重开提问：把用户忽略（或被取代 / 被中断）的 `chat_with_user` 提问重新登记为待决中断。
//!
//! 用户忽略提问后本轮已停、台账里的待决项也已被取走（旧 `request_id` 失效），但那条 tool 消息上的
//! `metadata.interrupt.request.form` 仍在——前端的「继续填写表单」据此重新弹出**原表单**（空白，
//! 不保留上次填写）：后端换一个新 `request_id` 就地写回 `pending` 标记并重新登记台账，
//! 用户提交后回答仍写回**同一条** tool 结果（与首次回答完全同一条链路），随后循环续跑。
//!
//! 限制：只在会话空闲、且没有其他待决中断时允许重开——否则新等待态会与运行中的本轮抢占这条
//! tool 消息（本轮收尾会把它的结果改写成终态）。同轮被 `abandon_deferred_calls` 放弃的 deferred
//! 调用不会被重开复活，由模型按已获得的信息重新发起。

use serde_json::{json, Value};
use uuid::Uuid;

use astorm_common::session::interrupt::InterruptType;

use crate::agent::session::{unix_now, SessionImpl};
use crate::tool::interrupt::PendingInterrupt;

/// 可重开的终态：提问被忽略 / 被新消息取代 / 进程中断 / 落定失败。
/// `pending`（正在等用户填）与 `resolved`（已作答）不可重开。
const RESUMABLE_STATUS: [&str; 4] = ["ignored", "superseded", "interrupted", "failed"];

impl SessionImpl {
    /// 重开被忽略（或被取代 / 被中断）的提问：重新登记待决中断，前端表单卡随之重新弹出。
    ///
    /// 入参 `tool_call_id` 即前端工具卡上的 `call_…` id——原表单定义与回答落点都锚在它上面。
    pub fn reopen_form_interrupt(&self, session_id: &Uuid, tool_call_id: &str) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;
        let sid = session_id.to_string();
        // 已有待决中断（另一张审批卡 / 表单卡）→ 拒绝：两个等待态会互相覆盖
        if crate::app::App::try_global()
            .map(|app| app.interrupts.has_session_pending(&sid))
            .unwrap_or(false)
        {
            return Err("该会话已有待处理的中断，请先处理它".into());
        }
        // 运行中 → 拒绝：本轮收尾会就地改写这条 tool 消息的结果，重开的等待态会被冲掉
        if session.running.lock().map(|running| *running).unwrap_or(false) {
            return Err("会话正在运行，请等本轮结束后再继续填写".into());
        }
        let marker = session
            .interrupt_marker_of(tool_call_id)
            .ok_or_else(|| "找不到该提问的中断记录，无法继续填写".to_string())?;
        let request = rebuild_form_request(&marker, tool_call_id)?;
        let request_id = request
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        // 工具名从 assistant 消息的 tool_calls 取回（与冷启动恢复同口径）
        let tool_name = session
            .tool_call_of(tool_call_id)
            .map(|(name, _)| name)
            .unwrap_or_else(|| "chat_with_user".to_string());
        // 就地写回新信封 + pending 标记：冷启动恢复 / 历史回读都能重绘表单卡
        session.replace_tool_message(
            tool_call_id,
            &request.to_string(),
            Some(crate::agent::history::interrupt_marker(
                &request_id,
                InterruptType::Form.as_str(),
                "pending",
                request.clone(),
            )),
            Some(json!({ "status": "pending", "deferred": false })),
        );
        // 重新登记待决（顺带把会话状态置回 awaiting_form + 广播 session_info → 侧栏橙点）
        crate::tool::policy::register_pending_interrupt(
            &sid,
            PendingInterrupt {
                request_id,
                interrupt_type: InterruptType::Form,
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.clone(),
                command: None,
                request: request.clone(),
                requested_at: unix_now(),
            },
        );
        // 让前端把表单卡弹回输入框位置（与首次中断同一条事件，前端按 tool_call_id 关联工具卡）
        let _ = session
            .event_tx
            .send(react_loop::event::interrupt_with(&sid, &tool_name, Some(request)));
        Ok(())
    }
}

/// 由落盘的中断标记重建「重新填写」用的中断信封（纯函数，便于单测）。
///
/// 校验：必须是提问（form）、状态可重开、且原表单确实有字段（纯 `tip` 文本提问没有可填内容）。
/// 新信封沿用原 `form` 定义，只换 `request_id` 并保持同一 `tool_call_id`。
fn rebuild_form_request(marker: &Value, tool_call_id: &str) -> Result<Value, String> {
    let interrupt_type = marker
        .get("interrupt_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if interrupt_type != InterruptType::Form.as_str() {
        return Err("只有提问表单可以继续填写".into());
    }
    let status = marker.get("status").and_then(|v| v.as_str()).unwrap_or_default();
    if !RESUMABLE_STATUS.contains(&status) {
        return Err(format!("该提问当前状态为 {status}，无法继续填写"));
    }
    let mut request = marker.get("request").cloned().unwrap_or(Value::Null);
    if !request.is_object() {
        return Err("该提问的中断记录不完整，无法继续填写".into());
    }
    let has_fields = request
        .pointer("/form/fields")
        .and_then(|fields| fields.as_array())
        .map(|fields| !fields.is_empty())
        .unwrap_or(false);
    if !has_fields {
        return Err("该提问没有可填写的字段".into());
    }
    request["interrupt_type"] = json!(InterruptType::Form.as_str());
    request["request_id"] = json!(Uuid::new_v4().to_string());
    request["tool_call_id"] = json!(tool_call_id);
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘的中断标记（`metadata.interrupt` 形态）。
    fn marker(status: &str, fields: Value) -> Value {
        json!({
            "interrupt_type": "form",
            "status": status,
            "updated_at": 1,
            "request": {
                "interrupt": true,
                "interrupt_type": "form",
                "request_id": "old",
                "tool_call_id": "call_1",
                "message": "已向用户提问，等待用户提交回答（回答将作为本工具结果返回）。",
                "form": { "message": "去哪？", "tip": null, "fields": fields },
            },
        })
    }

    #[test]
    fn rebuild_keeps_form_and_swaps_identity() {
        let m = marker("ignored", json!([{ "name": "arrange", "field_type": "single_choice" }]));
        let req = rebuild_form_request(&m, "call_1").expect("被忽略的提问应可重开");
        // 同一 tool_call_id：回答仍写回原工具卡那条 tool 消息
        assert_eq!(req["tool_call_id"], "call_1");
        assert_eq!(req["interrupt_type"], "form");
        // 换新 request_id：旧的已在决议入队时被台账取走，复用会直接报「已失效」
        assert_ne!(req["request_id"], "old");
        assert_eq!(req["form"]["fields"][0]["name"], "arrange");
        assert_eq!(req["interrupt"], true);
    }

    #[test]
    fn rebuild_accepts_superseded_and_rejects_pending_or_resolved() {
        let fields = json!([{ "name": "a", "field_type": "text" }]);
        assert!(rebuild_form_request(&marker("superseded", fields.clone()), "c").is_ok());
        for status in ["pending", "resolved", "approved", ""] {
            assert!(
                rebuild_form_request(&marker(status, fields.clone()), "c").is_err(),
                "{status} 不应可重开"
            );
        }
    }

    #[test]
    fn rebuild_rejects_approval_and_fieldless_form() {
        let approval = json!({
            "interrupt_type": "approval",
            "status": "denied",
            "request": { "interrupt_type": "approval" },
        });
        assert!(rebuild_form_request(&approval, "c").is_err(), "审批不可重开");
        // 纯 tip 文本提问（无字段）没有可填内容
        assert!(rebuild_form_request(&marker("ignored", json!([])), "c").is_err());
        assert!(rebuild_form_request(&marker("ignored", Value::Null), "c").is_err());
    }

    #[test]
    fn rebuild_rejects_broken_marker() {
        let broken = json!({ "interrupt_type": "form", "status": "ignored", "request": "不是对象" });
        assert!(rebuild_form_request(&broken, "c").is_err());
    }
}
