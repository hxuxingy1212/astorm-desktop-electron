//! 事件协议：类型常量 + 构造助手（与前端 JS 侧约定的唯一出口）。
//!
//! 只收录 react 循环层事件。**所有事件固定携带三个字段**：
//! `type`（事件类型）、`session_id`（定位到会话）、`timestamp`（unix 毫秒，当前时间）。
//! 业务层专属事件（如 agent 生命周期）由业务侧自行定义，不进入本模块。
//! 禁止在业务代码里手工 `json!` 拼事件——字段命名在此处统一定义，
//! JS 侧协议文档（`docs/开发规范.md` §8）与此一一对应。

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// 事件类型常量（与前端约定，不可随意改名）。
pub mod ty {
    pub const USER_MESSAGE: &str = "user_message";
    pub const TEXT: &str = "text";
    pub const REASONING: &str = "reasoning";
    /// 本轮流式输出作废重来（流中途传输中断 → provider 自动重开）
    pub const STREAM_RESET: &str = "stream_reset";
    pub const TOOL_CALL: &str = "tool_call";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const LOOP_LIFE: &str = "loop_life";
    pub const SESSION_STATS: &str = "session_stats";
    pub const INTERRUPT: &str = "interrupt";
    pub const DONE: &str = "done";
    pub const CANCELLED: &str = "cancelled";
    pub const ERROR: &str = "error";
}

/// 当前 unix 毫秒时间戳（事件时间，与前端 `Date.now()` 对齐）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 用户消息回显：`{ type, session_id, timestamp, message, round }`。
pub fn user_message(session_id: &str, message: &str, round: i32) -> Value {
    json!({
        "type": ty::USER_MESSAGE,
        "session_id": session_id,
        "timestamp": now_ms(),
        "message": message,
        "round": round,
    })
}

/// LLM 文本增量：`{ type, session_id, timestamp, round, content }`。
/// `round` 为当前循环轮次；前端据此把各轮的正文/思考/工具拆到独立消息。
pub fn text(session_id: &str, content: &str, round: u32) -> Value {
    json!({
        "type": ty::TEXT,
        "session_id": session_id,
        "timestamp": now_ms(),
        "round": round,
        "content": content,
    })
}

/// LLM 思考链增量：`{ type, session_id, timestamp, round, content }`。
pub fn reasoning(session_id: &str, content: &str, round: u32) -> Value {
    json!({
        "type": ty::REASONING,
        "session_id": session_id,
        "timestamp": now_ms(),
        "round": round,
        "content": content,
    })
}

/// 本轮流式输出作废重来：`{ type, session_id, timestamp, round }`。
///
/// 触发场景：流中途传输中断（连接被对端/中间设备 RST）后 provider 自动重开重发
/// （见 `llm_provider::LlmEvent::Restart`）。前端收到后必须丢弃本轮**此前**已渲染的
/// 正文/思考（否则两次尝试的内容会被拼在一起），后续增量照常累积到同一轮。
pub fn stream_reset(session_id: &str, round: u32) -> Value {
    json!({
        "type": ty::STREAM_RESET,
        "session_id": session_id,
        "timestamp": now_ms(),
        "round": round,
    })
}

/// LLM 工具调用：`{ type, session_id, timestamp, round, tool_calls: [{ id, name, params }] }`。
pub fn tool_call(session_id: &str, calls: Vec<Value>, round: u32) -> Value {
    json!({
        "type": ty::TOOL_CALL,
        "session_id": session_id,
        "timestamp": now_ms(),
        "round": round,
        "tool_calls": calls,
    })
}

/// 工具结果：`{ type, session_id, timestamp, round, id, name, result, duration_ms? }`。
pub fn tool_result(
    session_id: &str,
    id: &str,
    name: &str,
    result: &str,
    duration_ms: Option<u32>,
    round: u32,
) -> Value {
    tool_result_with_status(session_id, id, name, result, duration_ms, round, None)
}

/// 工具结果（带显式状态）：`{ ..., status? }`。
///
/// `status` = `"ok"` / `"error"` 时前端直接按此定状态；缺省（None）由前端按结果文风推断
/// （引擎路径保持 None）。审批重放等**业务侧补发**的结果用它表达「重放失败 / 用户拒绝」
/// 等非标准失败文风的失败态。
pub fn tool_result_with_status(
    session_id: &str,
    id: &str,
    name: &str,
    result: &str,
    duration_ms: Option<u32>,
    round: u32,
    status: Option<&str>,
) -> Value {
    tool_result_with_meta(session_id, id, name, result, duration_ms, round, status, None)
}

/// 工具结果（带显式状态 + 结构化元数据）：
/// `{ type, session_id, timestamp, round, id, name, result, duration_ms?, status?, <meta>… }`。
///
/// `meta` 为工具经 [`Tool::result_meta`](crate::Tool::result_meta) 返回的对象，引擎不解释语义：
/// 其顶层键被**平铺**进事件（如 `{"denial": {…}}` → `event["denial"]`），同时原样落进 tool 消息
/// 的 metadata，供历史回读恢复（如安全拦截的分类）。
///
/// 引擎自身也会写入一个协议键：`deferred: true`（伴随 `status: "error"`）表示该调用因**同一轮
/// 前序调用等待用户确认**而暂未执行，已落成配对占位；用户确认后由业务侧按序补执行。
pub fn tool_result_with_meta(
    session_id: &str,
    id: &str,
    name: &str,
    result: &str,
    duration_ms: Option<u32>,
    round: u32,
    status: Option<&str>,
    meta: Option<&Value>,
) -> Value {
    let mut event = json!({
        "type": ty::TOOL_RESULT,
        "session_id": session_id,
        "timestamp": now_ms(),
        "round": round,
        "id": id,
        "name": name,
        "result": result,
    });
    if let Some(ms) = duration_ms {
        event["duration_ms"] = json!(ms);
    }
    if let Some(status) = status {
        event["status"] = json!(status);
    }
    // 工具元数据平铺进事件顶层（保留 result/status/type 等既有键不被覆写）
    if let Some(Value::Object(map)) = meta {
        for (k, v) in map {
            if !event.as_object().map(|o| o.contains_key(k)).unwrap_or(false) {
                event[k] = v.clone();
            }
        }
    }
    event
}

/// 循环生命周期：`{ type, session_id, timestamp, status }`（status: started / finished）。
pub fn loop_life(session_id: &str, status: &str) -> Value {
    json!({
        "type": ty::LOOP_LIFE,
        "session_id": session_id,
        "timestamp": now_ms(),
        "status": status,
    })
}

/// 会话用量统计：`{ type, session_id, timestamp, prompt_tokens, completion_tokens, total_tokens }`。
pub fn session_stats(session_id: &str, prompt: u32, completion: u32, total: u32) -> Value {
    json!({
        "type": ty::SESSION_STATS,
        "session_id": session_id,
        "timestamp": now_ms(),
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": total,
    })
}

/// 中断通知：`{ type, session_id, timestamp, tool_name }`。
pub fn interrupt(session_id: &str, tool_name: &str) -> Value {
    interrupt_with(session_id, tool_name, None)
}

/// 中断通知（可带载荷）：`{ type, session_id, timestamp, tool_name, payload? }`。
///
/// `payload` 为工具经 [`Tool::interrupt_payload`](crate::Tool::interrupt_payload) 返回的载荷
/// （安全拦截的审批请求 / 向用户提问的表单），前端据此弹审批卡或提问表单；
/// 无载荷时与 [`interrupt`] 等价。
pub fn interrupt_with(session_id: &str, tool_name: &str, payload: Option<Value>) -> Value {
    let mut event = json!({
        "type": ty::INTERRUPT,
        "session_id": session_id,
        "timestamp": now_ms(),
        "tool_name": tool_name,
    });
    if let Some(payload) = payload {
        event["payload"] = payload;
    }
    event
}

/// 循环结束：`{ type, session_id, timestamp }`。
pub fn done(session_id: &str) -> Value {
    json!({
        "type": ty::DONE,
        "session_id": session_id,
        "timestamp": now_ms(),
    })
}

/// 循环被用户取消（暂停）：`{ type, session_id, timestamp }`。
/// 区别于 `done` 正常完成——前端据此收尾但不弹「任务完成」通知、不派发排队消息。
pub fn cancelled(session_id: &str) -> Value {
    json!({
        "type": ty::CANCELLED,
        "session_id": session_id,
        "timestamp": now_ms(),
    })
}

/// 错误通知：`{ type, session_id, timestamp, message }`。
pub fn error(session_id: &str, message: &str) -> Value {
    json!({
        "type": ty::ERROR,
        "session_id": session_id,
        "timestamp": now_ms(),
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_without_payload_equals_plain_interrupt() {
        let plain = interrupt("s1", "chat_with_user");
        assert_eq!(plain["type"], json!(ty::INTERRUPT));
        assert_eq!(plain["tool_name"], json!("chat_with_user"));
        assert!(plain.get("payload").is_none(), "无载荷不应带 payload 字段");
        // 固定字段契约：type / session_id / timestamp
        assert_eq!(plain["session_id"], json!("s1"));
        assert!(plain["timestamp"].is_number());
    }

    #[test]
    fn interrupt_with_payload_carries_interrupt_request() {
        let payload = json!({
            "interrupt": true, "interrupt_type": "approval", "request_id": "req-1",
        });
        let ev = interrupt_with("s1", "bash", Some(payload.clone()));
        assert_eq!(ev["type"], json!(ty::INTERRUPT));
        assert_eq!(ev["payload"], payload);
    }

    #[test]
    fn tool_result_status_is_optional() {
        let plain = tool_result("s1", "call_1", "bash", "ok", Some(12), 2);
        assert_eq!(plain["duration_ms"], json!(12));
        assert!(plain.get("status").is_none());
        let errored = tool_result_with_status("s1", "call_1", "bash", "拒绝", None, 2, Some("error"));
        assert_eq!(errored["status"], json!("error"));
        assert!(errored.get("duration_ms").is_none());
    }

    #[test]
    fn tool_result_meta_is_flattened_without_overwriting_reserved_keys() {
        let meta = json!({
            "denial": { "category": "file_write", "source": "static_precheck" },
            // 保留键不得被元数据覆写
            "result": "meta-should-not-win",
            "type": "evil",
        });
        let ev = tool_result_with_meta("s1", "call_1", "bash", "已拒绝", None, 1, Some("error"), Some(&meta));
        assert_eq!(ev["denial"]["category"], json!("file_write"));
        assert_eq!(ev["status"], json!("error"));
        assert_eq!(ev["result"], json!("已拒绝"), "保留键不可被 meta 覆写");
        assert_eq!(ev["type"], json!(ty::TOOL_RESULT));
        // 无 meta 时与带 status 的旧行为一致
        let plain = tool_result_with_meta("s1", "call_1", "bash", "ok", None, 1, None, None);
        assert!(plain.get("denial").is_none());
        assert!(plain.get("status").is_none());
    }
}
