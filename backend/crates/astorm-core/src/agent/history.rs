//! 历史一致性与消息落盘：tool / tool_result 成对修复 + `ChatMessage → MessageRecord` 转换
//! + 审批重放的就地替换（追加式日志上的「更新」语义）。
//!
//! 会话消息历史由 `hook::append_message` 与 [`Session::repair_tool_calls`] 写入：
//! 两者都经 [`message_to_record`] 统一转换为持久化记录（jsonl 追加）。

use std::collections::HashSet;

use astorm_common::types::record::MessageRecord;
use llm_provider::ChatMessage;
use react_loop::LoopContext;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::session::{unix_now, Session};

impl Session {
    /// 会话是否已被删除（注册表已移除、存储行已删）。
    ///
    /// 删除后运行中的循环仍可能收尾：终态落盘、标题生成、设置同步等写回点都要先问一句，
    /// 否则会把这行记录 upsert 回存储 —— 表现为「删掉的会话重启后又出现在列表里」。
    pub(crate) fn is_deleted(&self) -> bool {
        self.deleted.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 修复历史中孤立的 assistant tool_calls：无对应 tool 响应的调用在**该 assistant 消息之后**
    /// 紧邻插入占位 tool 消息（LLM API 要求 tool_call 与 tool 响应配对，且须紧跟其后——
    /// 追加到末尾会被后续 user 消息隔开，仍返回 400）。
    ///
    /// 触发场景：打断（立即发送）发生在工具执行中途，assistant(tool_calls) 已写入历史但
    /// 工具未返回。占位消息**只注入本次请求载荷**（不落盘）：
    /// - 位置语义正确：紧跟 assistant，天然成对
    /// - 重启安全：历史保持原始形态，每次 before_loop 重新按位置修复，无需维护 seq 单调性
    ///   （历史 seq 即位置，中插占位并落盘会导致重启排序错位）
    ///
    /// 注：安全拦截审批**不再**依赖此修复——拦截时 tool 消息照常成对落库（见 `agent::approval`）。
    pub(crate) fn repair_tool_calls(&self, ctx: &mut LoopContext) {
        let tool_result_ids: HashSet<String> = ctx
            .chat_request
            .messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.clone())
            .collect();

        // 收集带未配对 tool_calls 的 assistant 消息下标与其缺失调用 [(tool_call_id, tool_name)]
        let mut repairs: Vec<(usize, Vec<(String, String)>)> = Vec::new();
        for (i, m) in ctx.chat_request.messages.iter().enumerate() {
            if m.role != "assistant" {
                continue;
            }
            let Some(calls) = m.tool_calls.clone() else { continue };
            let calls: Vec<(String, String)> = calls
                .into_iter()
                .map(|tc| (tc.id, tc.function.name))
                .collect();
            if calls.is_empty() {
                continue;
            }
            let missing: Vec<(String, String)> = calls
                .into_iter()
                .filter(|(id, _)| !tool_result_ids.contains(id))
                .collect();
            if !missing.is_empty() {
                repairs.push((i, missing));
            }
        }
        if repairs.is_empty() {
            return;
        }
        let repair_count: usize = repairs.iter().map(|(_, m)| m.len()).sum();

        // 从后往前插入,保持下标稳定:每条占位 tool 紧跟其 assistant 消息之后
        for (assistant_idx, missing) in repairs.into_iter().rev() {
            let mut insert_at = assistant_idx + 1;
            for (tool_call_id, tool_name) in missing {
                let msg = ChatMessage::tool(
                    format!("该工具调用未执行（{tool_name}），无对应结果"),
                    tool_call_id,
                );
                ctx.chat_request.messages.insert(insert_at, msg);
                insert_at += 1;
            }
        }
        tracing::warn!(
            "[session {}] 修复孤立 tool_call: {} 条（紧跟 assistant 插入占位 tool 响应成对）",
            ctx.session_id,
            repair_count,
        );
    }

    /// 就地替换某条 tool 消息（审批重放：`pending → replaying → approved/failed`，
    /// 以及拒绝 / 取代 / 重启后未知态）。
    ///
    /// 内存按 `tool_call_id` 定位后按下标替换（保持位置，tool_call 仍紧邻其 assistant 消息）；
    /// 落盘追加**同 id、同 seq** 的新记录——追加式日志上的「更新」语义：
    /// [`crate::agent::history::message_to_record`] 对 tool 消息用确定性 id，
    /// `load_messages` 按 id 去重（后写覆盖先写），回读即得最新版本。
    ///
    /// `marker` 为**中断标记本体**（见 [`interrupt_marker`]），写入时自动包成
    /// `metadata.interrupt`（与回读侧 `metadata.interrupt.status` 对称）；`None` 表示清除标记。
    /// `meta` 为结果元数据（`{"status": "ok|error", "denial": {…}}`，与引擎路径同构）：
    /// 顶层键并入 metadata（保留既有 `started_at`/`ended_at` 等），供前端回读状态与拒绝分类。
    /// 返回是否命中（未命中：历史中无该 tool 消息）。
    pub(crate) fn replace_tool_message(
        &self,
        tool_call_id: &str,
        content: &str,
        marker: Option<Value>,
        meta: Option<Value>,
    ) -> bool {
        if self.deleted.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let Ok(sid) = self.record.lock().map(|r| r.id.to_string()) else {
            return false;
        };
        let mut history = match self.history.lock() {
            Ok(history) => history,
            Err(_) => return false,
        };
        let Some(idx) = history
            .iter()
            .position(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tool_call_id))
        else {
            return false;
        };
        let mut msg = history[idx].clone();
        msg.content = Some(content.to_string());
        // 元数据**合并**（不整体替换）：保留引擎写入的耗时 / 状态，再覆盖中断标记与本次元数据
        let mut md = msg
            .metadata
            .take()
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| json!({}));
        if let Some(obj) = md.as_object_mut() {
            // 中断标记统一挂在 metadata.interrupt 下（与 attach_interrupt_marker / 前端回读同构）
            match marker {
                Some(m) => {
                    obj.insert("interrupt".into(), m);
                }
                None => {
                    obj.remove("interrupt");
                }
            }
            if let Some(Value::Object(map)) = meta {
                for (k, v) in map {
                    obj.insert(k, v);
                }
            }
        }
        msg.metadata = if md.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            None
        } else {
            Some(md)
        };
        history[idx] = msg.clone();
        let fallback_seq = idx as i64;
        drop(history);

        // seq 取已持久化记录的原值（历史下标可能因 system 提示词注入等与 seq 不完全一致）；
        // 未命中（异常）时退回下标。
        let id = tool_message_id(&sid, tool_call_id);
        let seq = self
            .storage
            .load_messages(&sid)
            .ok()
            .and_then(|recs| recs.iter().rev().find(|r| r.id == id).map(|r| r.seq))
            .unwrap_or(fallback_seq);
        let record = message_to_record(&msg, &sid, seq);
        if let Err(e) = self
            .storage
            .append_messages(&sid, std::slice::from_ref(&record))
        {
            tracing::error!("[session {sid}] 工具消息替换落盘失败: {e}");
            return false;
        }
        true
    }

    /// 从历史中取回某次工具调用的 `(tool_name, params)`。
    ///
    /// 审批重放用：重放所需参数**无需额外持久化**——它已在 assistant 消息的 `tool_calls`
    /// 里随消息流落盘，此处按 `tool_call_id` 原样取回。
    pub(crate) fn tool_call_of(&self, tool_call_id: &str) -> Option<(String, Value)> {
        let history = self.history.lock().ok()?;
        for msg in history.iter() {
            let Some(calls) = &msg.tool_calls else { continue };
            for tc in calls {
                if tc.id == tool_call_id {
                    let params = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or_else(|_| Value::Object(Default::default()));
                    return Some((tc.function.name.clone(), params));
                }
            }
        }
        None
    }

    /// 取某条 tool 消息上落盘的**中断标记**（`metadata.interrupt`）；消息不存在 / 无标记 → `None`。
    ///
    /// 中断的终态同样保留 `metadata.interrupt.request`（中断态持久化的唯一载体），
    /// 故「继续填写被忽略的提问」（见 `agent::reopen`）能据此原样重建原表单定义。
    pub(crate) fn interrupt_marker_of(&self, tool_call_id: &str) -> Option<Value> {
        let history = self.history.lock().ok()?;
        marker_of_in(&history, tool_call_id)
    }

    /// 同轮中排在 `tool_call_id` 之后、且标记为 `deferred` 的调用（因前序调用等待用户确认而暂未执行）。
    ///
    /// 返回 `(tool_call_id, tool_name, params)` 列表，供审批 / 提问确认后按原顺序补执行。
    /// 引擎在中断时为剩余调用写入 `metadata.deferred = true` 的配对占位（见 react-loop 引擎）。
    ///
    /// **注意**：只取快照后即释放 `history` 锁——调用方随后要 `await` 执行工具，持锁跨 await 会死锁。
    pub(crate) fn deferred_calls_after(&self, tool_call_id: &str) -> Vec<(String, String, Value)> {
        let Ok(history) = self.history.lock() else {
            return Vec::new();
        };
        deferred_calls_in(&history, tool_call_id)
    }
}

/// [`Session::interrupt_marker_of`] 的纯函数形态（便于单测）：按 `tool_call_id` 取中断标记。
fn marker_of_in(history: &[ChatMessage], tool_call_id: &str) -> Option<Value> {
    history
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some(tool_call_id))
        .and_then(|m| m.metadata.as_ref())
        .and_then(|md| md.get("interrupt"))
        .cloned()
}

/// [`Session::deferred_calls_after`] 的纯函数形态（便于单测）：从历史消息切片中取出
/// 排在 `tool_call_id` 之后、且配对 tool 消息标记 `deferred == true` 的调用。
fn deferred_calls_in(history: &[ChatMessage], tool_call_id: &str) -> Vec<(String, String, Value)> {
    // 仍处于 deferred 的 tool_call_id 集合（已执行者终态写回时把标记置 false）
    let deferred_ids: HashSet<String> = history
        .iter()
        .filter(|m| m.role == "tool")
        .filter(|m| {
            m.metadata
                .as_ref()
                .and_then(|md| md.get("deferred"))
                .and_then(|v| v.as_bool())
                == Some(true)
        })
        .filter_map(|m| m.tool_call_id.clone())
        .collect();
    if deferred_ids.is_empty() {
        return Vec::new();
    }
    for msg in history.iter() {
        let Some(calls) = &msg.tool_calls else { continue };
        let Some(pos) = calls.iter().position(|tc| tc.id == tool_call_id) else {
            continue;
        };
        return calls[pos + 1..]
            .iter()
            .filter(|tc| deferred_ids.contains(&tc.id))
            .map(|tc| {
                let params = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or_else(|_| Value::Object(Default::default()));
                (tc.id.clone(), tc.function.name.clone(), params)
            })
            .collect();
    }
    Vec::new()
}

/// tool 消息的**确定性 id**：`<session_id>:tool:<tool_call_id>`。
/// 同一工具调用无论写入多少次都映射到同一记录 id（支撑就地替换 / 回读去重）。
pub(crate) fn tool_message_id(session_id: &str, tool_call_id: &str) -> String {
    format!("{session_id}:tool:{tool_call_id}")
}

/// 中断状态标记（写入 tool 消息 `metadata.interrupt`，持久化中断态的唯一载体）。
///
/// `status` 状态机（按类型）：
/// - approval：`pending`（待用户决定）→ `replaying`（同意后重放中）→ `approved` / `failed`；
///   `pending` → `denied` / `superseded`；`replaying` → `interrupted`（重放中进程退出，结果未知）
/// - form：`pending` → `resolved`（用户已回答）；`pending` → `ignored` / `superseded`
///
/// `request` 保存**完整中断信封**：冷启动据此重建台账（审批重放 / 前端重绘提问表单），
/// 是「重启后待审批、待回答问题不丢」的依据。
pub(crate) fn interrupt_marker(
    request_id: &str,
    interrupt_type: &str,
    status: &str,
    request: Value,
) -> Value {
    json!({
        "request_id": request_id,
        "interrupt_type": interrupt_type,
        "status": status,
        "request": request,
        "updated_at": unix_now(),
    })
}

/// tool 消息内容若为**中断请求**（纯 JSON 信封）→ 附 `metadata.interrupt.status = pending` 标记。
///
/// 这是**中断态持久化的唯一载体**（不新增会话记录字段）：冷启动时 `agent::interrupt`
/// 扫描历史中的该标记，重建台账（待审批 / 待回答问题）并让前端重绘卡片。
pub(crate) fn attach_interrupt_marker(mut message: ChatMessage) -> ChatMessage {
    if message.role != "tool" {
        return message;
    }
    let Some(content) = message.content.as_deref() else {
        return message;
    };
    let Some(request) = crate::tool::policy::interrupt_request_from_result(content) else {
        return message;
    };
    let request_id = request
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let interrupt_type = request
        .get("interrupt_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let marker = interrupt_marker(&request_id, &interrupt_type, "pending", request);
    let mut md = message.metadata.take().unwrap_or_else(|| json!({}));
    if let Value::Object(map) = &mut md {
        map.insert("interrupt".into(), marker);
    }
    message.metadata = Some(md);
    message
}

/// `ChatMessage` → 消息持久化记录（追加式 jsonl 落盘用；`seq` 为会话内递增序号）。
/// 思考链独立存 `reasoning_content`、工具结果经 `tool_call_id` 关联、耗时等进 metadata——
/// 历史回读时前端据此重建 think 块与工具卡片（参考 astorm-desktop 的持久化形态）。
/// tool 消息用确定性 id（见 [`tool_message_id`]），使同一次调用的更新可在追加式日志上去重。
pub(crate) fn message_to_record(message: &ChatMessage, session_id: &str, seq: i64) -> MessageRecord {
    let id = match message.tool_call_id.as_deref() {
        Some(tcid) if !tcid.is_empty() => tool_message_id(session_id, tcid),
        _ => Uuid::new_v4().to_string(),
    };
    MessageRecord {
        id,
        session_id: session_id.to_string(),
        role: message.role.clone(),
        content: message.content.clone().unwrap_or_default(),
        tool_calls: serde_json::to_value(message.tool_calls.clone()).unwrap_or(Value::Null),
        seq,
        created_at: unix_now(),
        reasoning_content: message.reasoning_content.clone(),
        tool_call_id: message.tool_call_id.clone(),
        metadata: message.metadata.clone(),
    }
}

/// 持久化记录 → `ChatMessage`（冷启动重建会话历史时用；与 `message_to_record` 互逆）。
/// 空工具调用的字段按惰性规则还原（无 tool_calls）；**tool 消息内容永不剥离**（见函数体内注释）。
/// user 消息带 `metadata.attachments` 时按附件路径**重新读盘**重建 `content_parts`，
/// 使重启后模型仍能看到用户发的图片（仅图片有此片段；非图片附件是原文路径引用，
/// 内容入口是落库文本里的「【附件】…」段。文件缺失/超限项跳过，保持纯文本）。
pub(crate) fn record_to_message(record: &MessageRecord) -> Option<ChatMessage> {
    let tool_calls = match &record.tool_calls {
        Value::Null => None,
        v => serde_json::from_value(v.clone()).ok(),
    };
    let content_parts = if record.role == "user" {
        record
            .metadata
            .as_ref()
            .and_then(crate::agent::attachment::rebuild_parts)
    } else {
        None
    };
    // user 消息：把 metadata 里的「引用说明」（@文件/$智能体/元素围栏，不渲染）拼回正文，
    // 使冷启动重建后的历史与当轮喂模型一致（它本就是注入喂模型副本的，不进 UI 回显）。
    let content = if record.role == "user" {
        let note = crate::agent::attachment::refs_note_of(record.metadata.as_ref());
        if note.is_empty() {
            record.content.clone()
        } else if record.content.is_empty() {
            note
        } else {
            format!("{}\n\n{}", record.content, note)
        }
    } else {
        record.content.clone()
    };
    // **tool 消息的 content 永不剥离**（含空字符串）：tool 结果是 assistant(tool_calls)
    // 的配对载体，cold-start 若把空结果还原成 content=None，请求装配过滤器会把这条
    // tool 消息整条丢掉 → 其 tool_calls 悬空，上游 400（「tool_calls 后必须紧跟全部
    // tool 响应」）。空内容也要保持 Some("") 以维持消息在请求中的位置。
    let content = Some(content);
    Some(ChatMessage {
        role: record.role.clone(),
        content,
        content_parts,
        tool_calls,
        tool_call_id: record.tool_call_id.clone(),
        reasoning_content: record.reasoning_content.clone(),
        metadata: record.metadata.clone(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 只有「同轮、排在被中断调用之后、且仍标记 deferred」的调用才返回续跑。
    #[test]
    fn deferred_calls_in_returns_only_pending_siblings_after() {
        let assistant = ChatMessage::assistant(String::new()).with_tool_calls(vec![
            llm_provider::ToolCall::new("c0", "bash", r#"{"command":"a"}"#),
            llm_provider::ToolCall::new("c1", "bash", r#"{"command":"b"}"#),
            llm_provider::ToolCall::new("c2", "file_operation", r#"{"operation":"read","path":"x.docx"}"#),
        ]);
        let deferred = |id: &str| {
            ChatMessage::tool("暂未执行", id)
                .with_metadata(json!({ "status": "error", "deferred": true }))
        };
        let history = vec![
            assistant,
            // c1 已执行（终态把 deferred 置 false）→ 不再续跑
            ChatMessage::tool("执行结果", "c1").with_metadata(json!({ "status": "error", "deferred": false })),
            deferred("c2"),
        ];

        let calls = deferred_calls_in(&history, "c0");
        assert_eq!(calls.len(), 1, "只返回仍 deferred 的 c2");
        assert_eq!(calls[0].0, "c2");
        assert_eq!(calls[0].1, "file_operation");
        assert_eq!(calls[0].2, json!({ "operation": "read", "path": "x.docx" }));

        // c2 之后没有更多调用 → 空
        assert!(deferred_calls_in(&history, "c2").is_empty());
        // 未知 id → 空
        assert!(deferred_calls_in(&history, "nope").is_empty());
    }

    /// user 消息 metadata.refsNote（引用说明）在冷启动重建时拼回正文，与当轮喂模型一致；
    /// 无说明的 user 消息保持原文（回显不变）。
    #[test]
    fn record_to_message_appends_refs_note_for_user() {
        let mut rec = message_to_record(&ChatMessage::user("src/a.ts"), "s1", 1);
        crate::agent::attachment::set_refs_note(&mut rec.metadata, "【引用指令】\n- 读取 src/a.ts");
        let msg = record_to_message(&rec).expect("应可还原");
        assert_eq!(
            msg.content.as_deref(),
            Some("src/a.ts\n\n【引用指令】\n- 读取 src/a.ts")
        );

        // 无说明 → 原文不变
        let plain = message_to_record(&ChatMessage::user("你好"), "s1", 2);
        assert_eq!(record_to_message(&plain).unwrap().content.as_deref(), Some("你好"));
    }

    #[test]
    fn tool_message_id_is_deterministic_per_call() {
        assert_eq!(tool_message_id("s1", "call_1"), "s1:tool:call_1");
        // 同一次调用重复落盘映射到同一记录 id（就地更新的前提）
        assert_eq!(tool_message_id("s1", "call_1"), tool_message_id("s1", "call_1"));
        assert_ne!(tool_message_id("s1", "call_1"), tool_message_id("s1", "call_2"));
    }

    /// 空 tool 结果的 content 冷启动不剥离（保持 Some("")）：
    /// 剥成 None 会被请求装配过滤器整条丢弃 → 其 assistant(tool_calls) 悬空 → 上游 400。
    #[test]
    fn record_to_message_never_strips_empty_tool_content() {
        let rec = message_to_record(&ChatMessage::tool("", "call_x"), "s1", 5);
        assert_eq!(rec.content, "");
        let msg = record_to_message(&rec).expect("应可还原");
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_x"));
        assert!(
            msg.content.is_some(),
            "tool 消息 content 必须保持占位（配对载体），不得还原成 None"
        );
        // 带空 content 的 tool 消息必须能通过请求装配过滤器（role=tool 恒保留）
        let passes_filter =
            msg.content.is_some() || msg.content_parts.is_some() || msg.tool_calls.is_some();
        assert!(passes_filter);
    }

    #[test]
    fn message_to_record_uses_tool_call_id_for_tool_messages() {
        let msg = ChatMessage::tool("结果", "call_9");
        let rec = message_to_record(&msg, "s1", 3);
        assert_eq!(rec.id, "s1:tool:call_9");
        assert_eq!(rec.tool_call_id.as_deref(), Some("call_9"));
        // 非 tool 消息仍用随机 id
        let user = message_to_record(&ChatMessage::user("你好"), "s1", 4);
        assert!(user.id.len() >= 32 && user.id != "s1:tool:call_9");
    }

    #[test]
    fn attach_interrupt_marker_marks_pending_only_for_interrupt_results() {
        let payload = json!({
            "interrupt": true, "interrupt_type": "approval", "request_id": "req-1",
            "tool_call_id": "call_1", "message": "待审批",
            "approval": { "kind": "file_write", "command": "echo x > /tmp/a", "reason": "空间外写" },
        })
        .to_string();
        let marked = attach_interrupt_marker(ChatMessage::tool(payload, "call_1"));
        let it = &marked.metadata.as_ref().unwrap()["interrupt"];
        assert_eq!(it["status"], json!("pending"));
        assert_eq!(it["request_id"], json!("req-1"));
        assert_eq!(it["interrupt_type"], json!("approval"));
        assert_eq!(it["request"]["approval"]["kind"], json!("file_write"));

        // 提问信封同样登记为待决中断
        let form = json!({
            "interrupt": true, "interrupt_type": "form", "request_id": "req-2",
            "tool_call_id": "call_2", "form": { "message": "去哪？", "fields": [] },
        })
        .to_string();
        let marked = attach_interrupt_marker(ChatMessage::tool(form, "call_2"));
        assert_eq!(marked.metadata.as_ref().unwrap()["interrupt"]["interrupt_type"], json!("form"));

        // 普通工具结果 / 非 tool 消息不加标记
        let plain = attach_interrupt_marker(ChatMessage::tool("普通结果", "call_3"));
        assert!(plain.metadata.is_none());
        assert!(attach_interrupt_marker(ChatMessage::user("{\"interrupt\":true}"))
            .metadata
            .is_none());
    }
}
