//! 中断运行时编排：中断信封 → 台账 → 用户决议 → **就地把决议写回 tool 消息** → 续跑。
//!
//! 统一流程（审批与提问同构）：
//! 1. 工具执行时返回中断信封（`interrupt_type = approval | form`）并登记台账（`tool::interrupt`）
//! 2. 引擎据 `Tool::interrupt_payload` 中断循环 → 发带 `payload` 的 `interrupt` 事件
//! 3. 前端弹审批卡 / 提问表单 → 用户决定 → 经 `submitInterrupt` 交回 [`InterruptResolution`]
//! 4. 本模块在**消息出队时**解析决议，把结果就地写入那条 tool 消息（决议即 tool 结果，
//!    不产生额外用户消息），随后由 `run` 继续下一轮 ReAct：
//!    - 审批同意 → 台账一次性放行 → **重放**被拦工具 → 写真实结果（`approved` / `failed`）
//!    - 审批拒绝 → 写拒绝说明（`denied`）
//!    - 表单回答 → 写「用户已回答」文本（`resolved`）
//!    - 表单忽略 → 写「用户忽略」文本（`ignored`）且**不续跑**（省 token）
//!
//! 崩溃恢复（[`recover_pending_interrupts`]，冷启动重建会话时调用）：
//! - `pending` → 从 `metadata.interrupt.request` 重建台账（前端可重新审批 / 重新提问）
//! - `replaying` → 保守判为**结果未知**并覆盖为 `interrupted`（绝不自动重跑非幂等操作）

use std::sync::Arc;

use astorm_common::session::interrupt::{InterruptResolution, InterruptType};
use llm_provider::ChatMessage;
use react_loop::ToolContext;
use serde_json::{json, Value};

use crate::agent::history::interrupt_marker;
use crate::agent::session::Session;
use crate::tool::audit::AuditApproval;
use crate::tool::denial::{DenialCategory, DenialSource};
use crate::tool::interrupt::PendingInterrupt;

/// 重放中进程退出后的占位结果（保守：不重跑，只报未知）。
const INTERRUPTED_RESULT: &str = "该操作已获用户批准并开始执行，但程序在返回结果前退出：**执行结果未知**，请勿假设成功。如需确认，请检查目标当前状态（读取文件 / 执行幂等命令）后再决定下一步。";

/// 冷启动恢复：扫描历史的 tool 消息 `metadata.interrupt`，重建待决台账。幂等。
///
/// 在 `SessionImpl::get_or_rehydrate` 重建会话后调用（仅冷启动路径）。
pub(crate) fn recover_pending_interrupts(session: &Session) {
    let Ok(sid) = session.record.lock().map(|r| r.id.to_string()) else {
        return;
    };
    // 需就地覆盖的「重放中」条目：(tool_call_id, 标记)
    let mut interrupted: Vec<(String, Value)> = Vec::new();
    // 待重建的待决项：((tool_call_id, request, requested_at), 类型字符串)
    let mut pendings: Vec<(String, Value, i64, String)> = Vec::new();
    {
        let Ok(history) = session.history.lock() else {
            return;
        };
        for msg in history.iter() {
            if msg.role != "tool" {
                continue;
            }
            let Some(marker) = msg.metadata.as_ref().and_then(|m| m.get("interrupt")) else {
                continue;
            };
            let status = marker.get("status").and_then(|v| v.as_str()).unwrap_or_default();
            let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
            if tool_call_id.is_empty() {
                continue;
            }
            let request = marker.get("request").cloned().unwrap_or(Value::Null);
            let request_id = marker
                .get("request_id")
                .and_then(|v| v.as_str())
                .or_else(|| request.get("request_id").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let interrupt_type = marker
                .get("interrupt_type")
                .and_then(|v| v.as_str())
                .or_else(|| request.get("interrupt_type").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            match status {
                "pending" => pendings.push((
                    tool_call_id,
                    request,
                    marker.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0),
                    interrupt_type,
                )),
                "replaying" => {
                    // 进程在重放中退出：结果未知，保守不重跑（保持 tool_call 成对）
                    interrupted.push((
                        tool_call_id,
                        interrupt_marker(&request_id, &interrupt_type, "interrupted", request),
                    ));
                }
                _ => {}
            }
        }
    }
    for (tool_call_id, request, requested_at, interrupt_type) in pendings {
        let request_id = request
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        // 工具名从 assistant tool_calls 取回（审批重放时按名查找工具）
        let tool_name = session
            .tool_call_of(&tool_call_id)
            .map(|(name, _)| name)
            .unwrap_or_else(|| {
                if interrupt_type == InterruptType::Form.as_str() {
                    "chat_with_user".to_string()
                } else {
                    String::new()
                }
            });
        crate::tool::policy::register_pending_interrupt(
            &sid,
            PendingInterrupt {
                request_id,
                interrupt_type: parse_type(&interrupt_type),
                tool_call_id,
                tool_name,
                command: PendingInterrupt::command_of(&request),
                request,
                requested_at,
            },
        );
    }
    for (tool_call_id, marker) in interrupted {
        session.replace_tool_message(
            &tool_call_id,
            INTERRUPTED_RESULT,
            Some(marker),
            // 终态：清掉 deferred 占位标记
            Some(json!({ "status": "error", "deferred": false })),
        );
    }
}

/// 普通用户消息到达：该会话全部待决中断标记 `superseded` 并失效（从台账移除）。
///
/// 避免模型看到悬空的中断信封（待审批命令 / 待回答问题）而反复重试同一操作。
pub(crate) fn supersede_pending(session: &Session, sid: &str) {
    let Some(app) = crate::app::App::try_global() else {
        return;
    };
    for item in app.interrupts.take_session_pending(sid) {
        let content = match item.interrupt_type {
            InterruptType::Approval => format!(
                "该操作需要用户审批（{}），但用户未审批并发起了新的请求：本次审批请求已失效，操作未执行。",
                item.command.as_deref().unwrap_or("-")
            ),
            InterruptType::Form => {
                "用户未回答该提问，转而提出了新的要求：本次提问已被取代，其内容作废。".to_string()
            }
            InterruptType::MailCompose => {
                "用户未确认该邮件发送，转而提出了新的要求：本次邮件未发送，已作废。".to_string()
            }
        };
        let marker = marker_for(&item, "superseded");
        session.replace_tool_message(
            &item.tool_call_id,
            &content,
            Some(marker),
            Some(json!({ "status": "error", "deferred": false })),
        );
        emit_tool_result(session, sid, &item, &content, "error");
        // 前置中断被取代 → 同轮 deferred 调用不会续跑，改写为确定的未执行
        abandon_deferred_calls(session, sid, &item.tool_call_id);
    }
}

/// 决议入口（消息出队时调用）：把用户决议就地写回被中断的 tool 消息。
///
/// 返回 `(是否继续 LLM 轮次, 重放工具注入的追加消息)`：表单「忽略」不续跑（省 token），
/// 其余均续跑（ReAct 继续）。注入消息（如 `read_images` 重放后产生的图片 user 消息）
/// 由调用方追加到本轮请求载荷——紧跟被重放的 tool 消息之后，仅本轮可见。
pub(crate) async fn resume_interrupt(
    session: &Arc<Session>,
    sid: &str,
    pending: &PendingInterrupt,
    resolution: &InterruptResolution,
) -> (bool, Vec<ChatMessage>) {
    match pending.interrupt_type {
        InterruptType::Approval => {
            let allow = resolution.allow.unwrap_or(false);
            let command = pending.command.clone().unwrap_or_default();
            // 「当前会话允许」：先把目标记入会话内存白名单（重启丢失），
            // 本会话内同目标后续不再弹卡（静态预检 + 沙箱策略双重放行）。
            let session_scope = allow
                && resolution.allow_scope.as_deref() == Some("session")
                && crate::tool::session_allow::record_from_pending(sid, pending) > 0;
            audit_decision(sid, &pending.request, allow, session_scope);
            if allow {
                if let Some(app) = crate::app::App::try_global() {
                    // 一次性放行额度：重放时工具内 consume_allowed_rerun 命中 → 无防护执行
                    app.interrupts.allow_oneshot(sid, &command);
                }
                let (still_blocked, mut injected) = replay_tool(session, sid, pending).await;
                // 审批通过 → 续跑同轮被推迟的调用（引擎中断时标记 deferred；
                // 此前因前序调用待审批而未执行，详见 run_deferred_calls）
                let mut blocked = still_blocked;
                if !still_blocked {
                    let (extra, reblocked) =
                        run_deferred_calls(session, sid, &pending.tool_call_id).await;
                    injected.extend(extra);
                    blocked = reblocked;
                }
                // 续跑中又有调用需要审批（二次拦截）：不发起 LLM 轮次，等用户继续决定
                (!blocked, injected)
            } else {
                if let Some(app) = crate::app::App::try_global() {
                    app.interrupts.deny_command(sid, &command);
                }
                let content = format!(
                    "用户拒绝执行该操作：{command}。请勿重复执行相同操作，改换其他思路完成目标。"
                );
                session.replace_tool_message(
                    &pending.tool_call_id,
                    &content,
                    Some(marker_for(pending, "denied")),
                    Some(json!({
                        "status": "error",
                        // 终态：清掉 deferred 占位标记（避免被当成待续跑调用）
                        "deferred": false,
                        // 用户拒绝：保留原拦截分类（前端工具卡显示分类徽标）
                        "denial": denial_of_pending(pending),
                    })),
                );
                emit_tool_result(session, sid, pending, &content, "error");
                // 前置操作被拒 → 同轮 deferred 调用不再续跑，改写为确定的未执行
                abandon_deferred_calls(session, sid, &pending.tool_call_id);
                (true, Vec::new())
            }
        }
        InterruptType::MailCompose => {
            if resolution.ignored {
                // 用户取消发送：写清事实后**续跑**（让模型知道被取消，可换方案）。
                let content =
                    "用户取消了本次邮件发送（未发送任何邮件）。如需继续，请调整内容或换思路后重新发起。"
                        .to_string();
                session.replace_tool_message(
                    &pending.tool_call_id,
                    &content,
                    Some(marker_for(pending, "denied")),
                    Some(json!({ "status": "error", "deferred": false })),
                );
                emit_tool_result(session, sid, pending, &content, "error");
                crate::tool::policy::audit(
                    sid,
                    crate::tool::policy::audit_kind_of(crate::tool::denial::DenialCategory::Mail),
                    "用户取消了邮件发送",
                    crate::tool::audit::AuditApproval::Deny,
                    pending.command.as_deref(),
                    Some(crate::tool::denial::DenialCategory::Mail),
                    Some(crate::tool::denial::DenialSource::StaticPrecheck),
                );
                let (injected, reblocked) =
                    run_deferred_calls(session, sid, &pending.tool_call_id).await;
                (!reblocked, injected)
            } else {
                // 用户确认发送：从**原 tool 调用参数**取回邮件内容（信封刻意不含正文，
                // 避免重复进历史；与审批重放同一取参路径）后真正发信。
                let params = session
                    .tool_call_of(&pending.tool_call_id)
                    .map(|(_, p)| p)
                    .unwrap_or(Value::Null);
                let mail = crate::app::App::try_global().map(|app| app.mail());
                let outcome = match mail {
                    Some(mail) => crate::tool::mail::send_confirmed(&mail, &params).await,
                    None => Err("App 未初始化，无法发送邮件".to_string()),
                };
                let (content, ok) = match outcome {
                    Ok(msg) => (msg, true),
                    Err(e) => (format!("邮件发送失败：{e}"), false),
                };
                session.replace_tool_message(
                    &pending.tool_call_id,
                    &content,
                    Some(marker_for(pending, if ok { "resolved" } else { "failed" })),
                    Some(if ok {
                        json!({ "status": "ok" })
                    } else {
                        json!({ "status": "error", "deferred": false })
                    }),
                );
                emit_tool_result(session, sid, pending, &content, if ok { "ok" } else { "error" });
                crate::tool::policy::audit(
                    sid,
                    crate::tool::policy::audit_kind_of(crate::tool::denial::DenialCategory::Mail),
                    &content,
                    crate::tool::audit::AuditApproval::Agree,
                    pending.command.as_deref(),
                    Some(crate::tool::denial::DenialCategory::Mail),
                    Some(crate::tool::denial::DenialSource::StaticPrecheck),
                );
                // 发送完成 → 续跑同轮 deferred 调用。
                let (injected, reblocked) =
                    run_deferred_calls(session, sid, &pending.tool_call_id).await;
                (!reblocked, injected)
            }
        }
        InterruptType::Form => {
            if resolution.ignored {
                // 用户放弃回答：写清事实后**不续跑**，会话回到空闲（后续用户消息可正常继续）
                let content =
                    "用户忽略了本次提问，未提供任何信息。请勿重复提问：若缺少必要条件可中止任务，否则按已有信息继续。"
                        .to_string();
                session.replace_tool_message(
                    &pending.tool_call_id,
                    &content,
                    Some(marker_for(pending, "ignored")),
                    Some(json!({ "status": "error", "deferred": false })),
                );
                emit_tool_result(session, sid, pending, &content, "error");
                // 提问被忽略 → 同轮 deferred 调用不再续跑，改写为确定的未执行
                abandon_deferred_calls(session, sid, &pending.tool_call_id);
                (false, Vec::new())
            } else {
                let content = format_form_answer(pending, resolution.answer.as_deref().unwrap_or(""));
                session.replace_tool_message(
                    &pending.tool_call_id,
                    &content,
                    Some(marker_for(pending, "resolved")),
                    Some(json!({ "status": "ok" })),
                );
                emit_tool_result(session, sid, pending, &content, "ok");
                // 提问已回答 → 同轮 deferred 调用续跑（提问本身不再是阻碍）
                let (injected, reblocked) =
                    run_deferred_calls(session, sid, &pending.tool_call_id).await;
                // 续跑中又有调用需要审批：不发起 LLM 轮次，等用户继续决定
                (!reblocked, injected)
            }
        }
    }
}

/// 重放被拦工具：`replaying` 先落盘 → 原参数执行 → 结果就地替换。
///
/// 参数无需额外持久化：来自 assistant 消息 `tool_calls`（[`Session::tool_call_of`]）。
/// 返回 `(是否再次中断, 工具注入的追加消息)`——注入消息由调用方并入本轮载荷；
/// 再次中断（一次性额度未命中 / 二次拦截）时已登记台账并发出 `interrupt` 事件。
async fn replay_tool(
    session: &Session,
    sid: &str,
    pending: &PendingInterrupt,
) -> (bool, Vec<ChatMessage>) {
    // 先落 replaying（进程若在重放中退出 → 冷启动保守判为结果未知，不重跑）
    let replaying = format!(
        "用户已同意放行，正在重新执行：{}",
        pending.command.as_deref().unwrap_or("-")
    );
    session.replace_tool_message(
        &pending.tool_call_id,
        &replaying,
        Some(marker_for(pending, "replaying")),
        // 重放进行中：工具卡保持 pending（前端按 status=pending 转圈）
        Some(json!({ "status": "pending" })),
    );

    let Some((tool_name, params)) = session.tool_call_of(&pending.tool_call_id) else {
        let content = "无法重放该操作：未能从历史中取回工具调用参数。请重新发起该操作。".to_string();
        session.replace_tool_message(
            &pending.tool_call_id,
            &content,
            Some(marker_for(pending, "failed")),
            Some(json!({ "status": "error" })),
        );
        emit_tool_result(session, sid, pending, &content, "error");
        return (false, Vec::new());
    };
    execute_and_record(session, sid, &pending.tool_call_id, &tool_name, params, Some(pending)).await
}

/// 执行一次工具调用并把结果**就地写回**其 tool 消息（内存历史 + jsonl + 前端事件）。
///
/// 审批重放（被拦调用）与「同轮 deferred 调用」共用：两者都已有一条 tool 消息占位，
/// 执行后就地替换即可（不新增配对消息）。`pending` 为 `Some` 时按中断终态写标记
/// （`approved` / `failed`）；为 `None`（deferred 续跑）时无中断标记，并置 `deferred: false`
/// 以清掉占位标记（避免后续被重复续跑）。
///
/// 返回 `(是否再次中断, 工具注入的追加消息)`：再次中断时已登记台账并发出 `interrupt`
/// 事件，调用方应停止续跑、等待用户决定。
async fn execute_and_record(
    session: &Session,
    sid: &str,
    tool_call_id: &str,
    tool_name: &str,
    params: Value,
    pending: Option<&PendingInterrupt>,
) -> (bool, Vec<ChatMessage>) {
    let Some(tool) =
        crate::app::App::try_global().and_then(|app| app.tools.tool_map().get(tool_name))
    else {
        let content = if pending.is_some() {
            format!("无法重放该操作：工具 `{tool_name}` 当前不可用。")
        } else {
            format!("无法执行该操作：工具 `{tool_name}` 当前不可用。")
        };
        write_and_emit(
            session,
            sid,
            tool_call_id,
            tool_name,
            &content,
            "error",
            json!({ "status": "error", "deferred": false }),
            pending,
        );
        return (false, Vec::new());
    };

    let cancel = session.cancel.read().map(|c| c.clone()).unwrap_or_default();
    let tool_ctx = ToolContext::new(cancel)
        .with_session_id(sid)
        .with_tool_call_id(tool_call_id);
    let (content, status, exec_meta) = match tool.execute_with_meta(params, &tool_ctx).await {
        Ok((out, em)) => match crate::tool::policy::interrupt_request_from_result(&out) {
            // 再次被拦（一次性额度未命中 / 二次拦截）：交回中断态并重新弹卡
            Some(request) => {
                let re_marker = interrupt_marker(
                    request.get("request_id").and_then(|v| v.as_str()).unwrap_or_default(),
                    request
                        .get("interrupt_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("approval"),
                    "pending",
                    request.clone(),
                );
                session.replace_tool_message(
                    tool_call_id,
                    &out,
                    Some(re_marker),
                    Some(json!({ "status": "pending" })),
                );
                emit_tool_result_raw(session, sid, tool_call_id, tool_name, &out, "error", None);
                let _ = session.event_tx.send(react_loop::event::interrupt_with(
                    sid,
                    tool_name,
                    Some(request),
                ));
                return (true, Vec::new());
            }
            None => (out, "approved", em),
        },
        Err(e) => (
            if pending.is_some() {
                format!("工具 `{tool_name}` 重放失败：{e}")
            } else {
                format!("工具 `{tool_name}` 执行失败：{e}")
            },
            "failed",
            None,
        ),
    };
    // 工具注入的追加消息（如 read_images 的图片 user 消息）：紧跟 tool 消息之后上送
    let injected = tool_ctx.take_injections();
    let ui_status = if status == "approved" { "ok" } else { "error" };
    // 结果元数据：状态 +（失败时的）分类——失败原因由工具自报拒绝信封时解析
    let mut meta = json!({ "status": ui_status, "deferred": false });
    // 执行时产生的元数据（如文件 diff）：补充未占用键（不覆盖状态 / 分类）
    if let Some(em) = exec_meta {
        if let (Some(dst), Value::Object(em_map)) = (meta.as_object_mut(), em) {
            for (k, v) in em_map {
                dst.entry(k).or_insert(v);
            }
        }
    }
    if ui_status == "error" {
        if let Some(d) = crate::tool::denial::denial_meta_from_error(&content)
            .or_else(|| tool.result_meta(false, &content))
        {
            if let (Some(src), Some(dst)) = (d.get("denial"), meta.as_object_mut()) {
                dst.insert("denial".into(), src.clone());
            }
        }
    }
    write_and_emit(session, sid, tool_call_id, tool_name, &content, ui_status, meta, pending);
    (false, injected)
}

/// 就地写回终态结果并转发前端事件（`pending` 有值时附中断终态标记）。
fn write_and_emit(
    session: &Session,
    sid: &str,
    tool_call_id: &str,
    tool_name: &str,
    content: &str,
    ui_status: &str,
    meta: Value,
    pending: Option<&PendingInterrupt>,
) {
    let marker = pending.map(|p| {
        marker_for(p, if ui_status == "ok" { "approved" } else { "failed" })
    });
    session.replace_tool_message(tool_call_id, content, marker, Some(meta.clone()));
    emit_tool_result_raw(session, sid, tool_call_id, tool_name, content, ui_status, Some(&meta));
}

/// 审批 / 提问确认后，按原顺序补执行同轮被**推迟**（`deferred`）的调用。
///
/// 引擎在中断时为同轮剩余调用落成 `deferred` 占位（不执行）；用户确认后在此续跑：
/// 逐个执行并就地写回结果。任一调用再次返回中断信封即停止（已发新的 `interrupt` 事件并
/// 登记台账），其余保持 deferred，等用户下次确认后继续。
///
/// 返回 `(工具注入的追加消息, 是否因再次待审批而中止)`——后者为真时调用方**不应**再发起
/// LLM 轮次（等用户决定，与引擎中断同语义）。
async fn run_deferred_calls(
    session: &Session,
    sid: &str,
    after_tool_call_id: &str,
) -> (Vec<ChatMessage>, bool) {
    let calls = session.deferred_calls_after(after_tool_call_id);
    let mut injected = Vec::new();
    for (tool_call_id, tool_name, params) in calls {
        let (interrupted, extra) =
            execute_and_record(session, sid, &tool_call_id, &tool_name, params, None).await;
        injected.extend(extra);
        if interrupted {
            return (injected, true);
        }
    }
    (injected, false)
}

/// 前置确认被放弃（用户忽略提问 / 直接改发新消息）：把同轮 deferred 占位改写成明确的未执行，
/// 并清掉 `deferred` 标记（不再续跑），避免历史里留下「确认后将自动继续执行」误导模型。
fn abandon_deferred_calls(session: &Session, sid: &str, after_tool_call_id: &str) {
    for (tool_call_id, tool_name, _params) in session.deferred_calls_after(after_tool_call_id) {
        let content = "该工具调用未执行：其前置操作未获用户确认。如需，请重新发起。";
        let meta = json!({ "status": "error", "deferred": false });
        session.replace_tool_message(&tool_call_id, content, None, Some(meta.clone()));
        emit_tool_result_raw(session, sid, &tool_call_id, &tool_name, content, "error", Some(&meta));
    }
}

/// 待决中断对应的拒绝分类（取信封 `approval.category`，旧记录回退 `kind`）。
///
/// 用户拒绝 / 取代等决议把结果写回 tool 消息时携带它，前端工具卡即可显示分类徽标
/// （否则重读历史只剩一句拒绝文案）。
fn denial_of_pending(pending: &PendingInterrupt) -> Value {
    let approval = pending.request.get("approval").cloned().unwrap_or(Value::Null);
    let category = approval
        .get("category")
        .and_then(|v| v.as_str())
        .or_else(|| approval.get("kind").and_then(|v| v.as_str()));
    json!({
        "category": category.unwrap_or("danger_command"),
        // 用户拒绝：分类来自已登记的拦截（静态预检/sandbox 事件），可信度沿用原登记
        "source": approval.get("source").and_then(|v| v.as_str()).unwrap_or("static_precheck"),
        "detail": pending
            .request
            .pointer("/approval/reason")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        "targets": approval.get("targets").cloned().unwrap_or_else(|| json!([])),
    })
}

/// 把表单回答格式化为**模型可读文本**（按表单字段顺序输出，未在表单声明的键兜底列出）。
///
/// 回答非对象（纯文本 / 数组 / 无法解析）时原样转述，保证用户输入不丢失。
fn format_form_answer(pending: &PendingInterrupt, raw: &str) -> String {
    let parsed: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);
    if !parsed.is_object() {
        let text = match &parsed {
            Value::String(s) => s.clone(),
            Value::Null => raw.trim().to_string(),
            other => render_value(other),
        };
        return if text.is_empty() {
            "用户已回答：（空）".to_string()
        } else {
            format!("用户已回答：{text}")
        };
    }
    let mut lines = vec!["用户已回答：".to_string()];
    let mut consumed: Vec<String> = Vec::new();
    if let Some(fields) = pending.request.pointer("/form/fields").and_then(|f| f.as_array()) {
        for field in fields {
            let name = field.get("name").and_then(|n| n.as_str()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            if let Some(v) = parsed.get(name) {
                lines.push(format!("- {name}: {}", render_value(v)));
                consumed.push(name.to_string());
            }
        }
    }
    if let Some(map) = parsed.as_object() {
        for (k, v) in map {
            if !consumed.iter().any(|c| c == k) {
                lines.push(format!("- {k}: {}", render_value(v)));
            }
        }
    }
    if lines.len() == 1 {
        lines.push("（用户未填写任何字段）".to_string());
    }
    lines.join("\n")
}

/// 标量 / 数组的展示渲染（多选逗号连接）。
fn render_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(render_value).collect::<Vec<_>>().join("，"),
        Value::Bool(b) => b.to_string(),
        Value::Null => "（空）".to_string(),
        other => other.to_string(),
    }
}

/// 构造该待决项的中断标记（终态 / 中间态）。
fn marker_for(pending: &PendingInterrupt, status: &str) -> Value {
    interrupt_marker(
        &pending.request_id,
        pending.interrupt_type.as_str(),
        status,
        pending.request.clone(),
    )
}

fn parse_type(s: &str) -> InterruptType {
    match s {
        "form" => InterruptType::Form,
        "mail_compose" => InterruptType::MailCompose,
        _ => InterruptType::Approval,
    }
}

/// 审批决议审计（agree / deny；明细取信封中的分类、来源与 command）。
fn audit_decision(sid: &str, request: &Value, allow: bool, session_scope: bool) {
    let approval = request.get("approval").cloned().unwrap_or(Value::Null);
    // 新记录带 category（7 值）；旧记录只有 kind（file_read/file_write/network）
    let category_str = approval
        .get("category")
        .and_then(|v| v.as_str())
        .or_else(|| approval.get("kind").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let command = approval.get("command").and_then(|v| v.as_str()).unwrap_or_default();
    let verb = if allow { "同意" } else { "拒绝" };
    // 「当前会话允许」额外留痕：明细标注范围，便于回溯为何后续不再弹卡
    let scope_note = if session_scope { "｜会话白名单（本会话内同目标不再弹卡）" } else { "" };
    let category = parse_category(category_str);
    let source = approval.get("source").and_then(|v| v.as_str()).and_then(parse_source);
    crate::tool::policy::audit(
        sid,
        crate::tool::policy::audit_kind_of(category),
        &format!("{verb} 执行：{command}（{category_str}{scope_note}）"),
        if allow { AuditApproval::Agree } else { AuditApproval::Deny },
        Some(command),
        Some(category),
        source,
    );
}

/// 分类字符串 → 枚举（兼容旧 `kind` 值；未知值按危险命令/策略硬拒归类）。
fn parse_category(s: &str) -> DenialCategory {
    match s {
        "file_read" => DenialCategory::FileRead,
        "file_write" => DenialCategory::FileWrite,
        "network" => DenialCategory::Network,
        "resource" => DenialCategory::Resource,
        "browser" => DenialCategory::Browser,
        _ => DenialCategory::DangerCommand,
    }
}

/// 分类来源字符串 → 枚举（信封 `approval.source`；未知 / 缺失 → None，审计字段缺省不写）。
fn parse_source(s: &str) -> Option<DenialSource> {
    match s {
        "static_precheck" => Some(DenialSource::StaticPrecheck),
        "sandbox_preflight" => Some(DenialSource::SandboxPreflight),
        "sandbox_runtime" => Some(DenialSource::SandboxRuntime),
        "stderr_attribution" => Some(DenialSource::StderrAttribution),
        "exit_code" => Some(DenialSource::ExitCode),
        _ => None,
    }
}

/// 把工具结果推给前端（决议改写不经引擎，事件需在此补发；前端按 tool_call id 就地更新工具卡）。
/// `status` = `"ok"` / `"error"` 显式给出工具卡状态（拒绝 / 忽略的文风不统一，不靠文本推断）；
/// 结果本身是拒绝信封时，同时带上分类元数据（与引擎路径同构）。
fn emit_tool_result(
    session: &Session,
    sid: &str,
    pending: &PendingInterrupt,
    content: &str,
    status: &str,
) {
    let meta = (status == "error")
        .then(|| crate::tool::denial::denial_meta_from_error(content))
        .flatten();
    emit_tool_result_raw(
        session,
        sid,
        &pending.tool_call_id,
        &pending.tool_name,
        content,
        status,
        meta.as_ref(),
    );
}

/// [`emit_tool_result`] 的底层形态：调用方直接给出 tool_call_id / tool_name 与元数据，
/// 供 deferred 续跑（无 `PendingInterrupt` 上下文）复用。
fn emit_tool_result_raw(
    session: &Session,
    sid: &str,
    tool_call_id: &str,
    tool_name: &str,
    content: &str,
    status: &str,
    meta: Option<&Value>,
) {
    let round = session
        .record
        .lock()
        .map(|r| r.current_round.max(0) as u32)
        .unwrap_or(0);
    let _ = session.event_tx.send(react_loop::event::tool_result_with_meta(
        sid,
        tool_call_id,
        tool_name,
        content,
        None,
        round,
        Some(status),
        meta,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn form_pending(fields: Value) -> PendingInterrupt {
        let request = json!({
            "interrupt": true, "interrupt_type": "form", "request_id": "r1",
            "tool_call_id": "call_1",
            "form": { "message": "去哪？", "tip": null, "fields": fields },
        });
        PendingInterrupt {
            request_id: "r1".into(),
            interrupt_type: InterruptType::Form,
            tool_call_id: "call_1".into(),
            tool_name: "chat_with_user".into(),
            command: None,
            request,
            requested_at: 1,
        }
    }

    #[test]
    fn format_form_answer_follows_field_order_and_keeps_extras() {
        let pending = form_pending(json!([
            { "name": "name", "field_type": "text" },
            { "name": "tags", "field_type": "multiple_choice" },
        ]));
        // 键序与表单一致；未声明的键兜底补列
        let text = format_form_answer(&pending, r#"{"tags":["a","b"],"extra":"x","name":"张三"}"#);
        assert_eq!(text, "用户已回答：\n- name: 张三\n- tags: a，b\n- extra: x");
    }

    #[test]
    fn format_form_answer_handles_empty_and_non_object() {
        let pending = form_pending(json!([{ "name": "name", "field_type": "text" }]));
        assert!(format_form_answer(&pending, "{}").contains("用户未填写任何字段"));
        assert!(format_form_answer(&pending, "就是随便写的").contains("就是随便写的"));
    }

    #[test]
    fn render_value_joins_arrays_and_keeps_booleans() {
        assert_eq!(render_value(&json!(true)), "true");
        assert_eq!(render_value(&json!(["x", "y"])), "x，y");
        assert_eq!(render_value(&json!(null)), "（空）");
    }
}
