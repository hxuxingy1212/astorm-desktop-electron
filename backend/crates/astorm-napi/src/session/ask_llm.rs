//! ask_llm：单次 LLM 补全（流式）。插件 SDK `llm.chat` 复用本应用 LLM 调用的入口。
//!
//! 返回即启动（补全事件经全局事件总线 `type:"llm"` + `call_id` 回传，见
//! astorm-core::app::ask_llm::AppState::spawn_llm_chat）。不创建 napi ReadableStream。

use astorm_core::app::AskLlmRequest as CoreAskLlmRequest;
use llm_provider::ChatMessage;
use napi_derive::napi;
use serde::Serialize;

/// 单次补全请求参数（napi 镜像）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskLlmRequest {
    /// 模型引用 id（可选，缺省用默认模型）
    pub model: Option<String>,
    /// 供应商名（可选，缺省用默认供应商）
    pub provider: Option<String>,
    /// 对话消息（role: system / user / assistant）
    pub messages: Vec<AskLlmMessage>,
    /// 思考强度（可选）：low / medium / high
    pub reasoning_effort: Option<String>,
}

/// 单条对话消息（napi 镜像）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
pub struct AskLlmMessage {
    /// 角色：system / user / assistant
    pub role: String,
    /// 文本内容
    pub content: String,
}

/// 启动一次单次补全：复用 LLM 注册表与供应商配置。
/// 返回后事件以 `{"type":"llm","call_id":callId,"kind":"chunk|reasoning|end|error",..}`
/// 出现在全局事件流（subscribe），前端按 call_id 路由（契约见 docs/plugin-system.md）。
#[napi]
pub fn ask_llm(request: AskLlmRequest, call_id: String) -> napi::Result<()> {
    crate::app()?
        .spawn_llm_chat(request.into(), call_id)
        .map_err(|e| napi::Error::from_reason(e))
}

impl From<AskLlmRequest> for CoreAskLlmRequest {
    fn from(r: AskLlmRequest) -> Self {
        Self {
            model: r.model.unwrap_or_default(),
            provider: r.provider,
            messages: r.messages.into_iter().map(Into::into).collect(),
            reasoning_effort: r.reasoning_effort,
        }
    }
}

impl From<AskLlmMessage> for ChatMessage {
    fn from(m: AskLlmMessage) -> Self {
        ChatMessage {
            role: m.role,
            content: Some(m.content),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            metadata: None,
        }
    }
}