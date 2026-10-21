//! 单次 LLM 补全（插件系统复用本应用 LLM 调用）。
//!
//! 不创建会话、不进会话协议流：以 `call_id` 标识把补全事件发布到**全局事件总线**
//! （`subscribe()` 那条流），前端按 `type:"llm"` + `call_id` 路由。
//! 与总线方案统一后，不再为单次调用创建/销毁 napi ReadableStream，
//! 规避 napi-rs 流对象在 tokio worker 线程 drop 时触发的 GC 竞态。

use futures::StreamExt;
use llm_provider::{ChatMessage, ChatRequest, LlmEvent, LlmStream};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// 单次补全请求（core 业务类型；napi 层镜像后转换）。
#[derive(Debug, Clone)]
pub struct AskLlmRequest {
    /// 模型引用 id（空 = 默认模型）
    pub model: String,
    /// 供应商名（None = 默认供应商）
    pub provider: Option<String>,
    /// 对话消息（role: system / user / assistant）
    pub messages: Vec<ChatMessage>,
    /// 思考强度（可选）：low / medium / high
    pub reasoning_effort: Option<String>,
}

impl crate::app::AppState {
    /// 单次 LLM 补全：复用 LLM 注册表/供应商配置，以 `call_id` 在全局事件总线发布。
    ///
    /// 事件契约（经 `subscribe()` 全局流回传，前端按 `type:"llm"` + `call_id` 路由）：
    /// - `{"type":"llm","call_id":..,"kind":"chunk","delta":..}` — 正文增量
    /// - `{"type":"llm","call_id":..,"kind":"reasoning","delta":..}` — 思考链增量
    /// - `{"type":"llm","call_id":..,"kind":"end","usage":..|null,"finishReason":..}` — 正常结束
    /// - `{"type":"llm","call_id":..,"kind":"error","message":..}` — 错误（随后补一条 end）
    /// 总线无订阅者时发送失败即提前停止（无人收听不再空转）。
    pub fn spawn_llm_chat(&self, request: AskLlmRequest, call_id: String) -> Result<(), String> {
        let llm = self.llm.snapshot();
        if llm.is_empty() {
            return Err("未配置 LLM 供应商：请在「设置 → 模型」中添加供应商与模型".into());
        }
        let client = llm
            .client_for(request.provider.as_deref())
            .ok_or_else(|| "LLM 供应商不存在：请检查 provider 参数或默认供应商配置".to_string())?;
        let model = if request.model.is_empty() {
            llm.default_model().ok_or_else(|| "未指定模型且无默认模型".to_string())?
        } else {
            llm.resolve_model_name(request.provider.as_deref(), &request.model)
        };

        let mut chat_request = ChatRequest::new(request.messages).with_model(model.clone());
        if let Some(effort) = &request.reasoning_effort {
            chat_request = chat_request.with_reasoning_effort(effort.clone());
        }

        let tx = self.event_tx.clone();
        let handle = self.runtime.handle().clone();
        handle.spawn(async move {
            let stream = match client
                .chat_stream(&model, chat_request, CancellationToken::new().child_token())
                .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    let _ = tx.send(error_event(&call_id, format!("LLM 请求失败: {e}")));
                    apply_end(&tx, &call_id);
                    return;
                }
            };
            forward_llm_events(stream, tx, &call_id).await;
        });

        Ok(())
    }
}

/// 消费上游 LLM 事件流并逐条转换为 `type:"llm"` 总线事件发布。
async fn forward_llm_events(mut stream: LlmStream, tx: broadcast::Sender<Value>, call_id: &str) {
    while let Some(item) = stream.next().await {
        let next = match item {
            Ok(LlmEvent::Chunk(c)) => json!({ "kind": "chunk", "delta": c.content }),
            Ok(LlmEvent::ReasoningChunk(c)) => json!({ "kind": "reasoning", "delta": c.content }),
            Ok(LlmEvent::End(e)) => json!({
                "kind": "end",
                "usage": e.usage,
                "finishReason": e.finish_reason
            }),
            Ok(LlmEvent::Start) | Ok(LlmEvent::ToolCalls(_)) => continue,
            // 流中传输中断后自动重开：通知订阅方作废**已收到的**增量（忽略该 kind 的
            // 老订阅方行为与今天一致：两段文本会被拼接）。
            Ok(LlmEvent::Restart) => json!({ "kind": "reset" }),
            Ok(LlmEvent::Error(msg)) => json!({ "kind": "error", "message": msg }),
            Err(e) => json!({ "kind": "error", "message": format!("LLM 流错误: {e}") }),
        };
        let mut payload = next;
        let value = payload.as_object_mut().map(|m| {
            m.insert("type".into(), Value::String("llm".into()));
            m.insert("call_id".into(), Value::String(call_id.into()));
        });
        debug_assert!(value.is_some(), "llm 事件必然是对象");
        let is_end = payload["kind"] == "end";
        if tx.send(payload).is_err() {
            return; // 总线无订阅者 → 停止
        }
        if is_end {
            return;
        }
    }
    // 上游流耗尽但无 End：兜底补一条 end，保持协议完整。
    apply_end(&tx, call_id);
}

fn error_event(call_id: &str, message: String) -> Value {
    json!({ "type": "llm", "call_id": call_id, "kind": "error", "message": message })
}

fn apply_end(tx: &broadcast::Sender<Value>, call_id: &str) {
    let _ = tx.send(json!({
        "type": "llm",
        "call_id": call_id,
        "kind": "end",
        "usage": serde_json::Value::Null,
        "finishReason": "stream_closed"
    }));
}