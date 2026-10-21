//! LLM 请求与消息类型。
//!
//! 单一消息类型通吃「循环状态」与「provider 请求」两个场景，
//! 避免在状态侧和 wire 侧各维护一套结构。

use serde::{Deserialize, Serialize};

/// 一条对话消息。
///
/// `content_parts` 用于多模态（VLM）场景：字段本身不单独序列化，而是由
/// [`ChatMessage::content_json`] 与 `content` 文本合并为 wire 格式的内容值
/// （见本类型的 [`Serialize`] 实现）——故 `content_parts` 非空时 `content`
/// 在请求体里是 block 数组，否则是纯字符串。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ChatMessage {
    /// 角色：system / user / assistant / tool
    pub role: String,
    /// 纯文本内容（VLM 消息可为 None，改用 content_parts）
    #[serde(default)]
    pub content: Option<String>,
    /// 多模态内容片段（仅内存持有；wire 格式经 `content_json` 合并上送）
    #[serde(skip)]
    pub content_parts: Option<Vec<ContentPart>>,
    /// assistant 消息携带的工具调用
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// tool 消息关联的 tool_call id
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// 思考链内容（streaming reasoning）
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// 业务元数据（随消息序列化持久化；发送给 LLM 前由引擎剥离，不参与请求）
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// wire 格式序列化：字段与旧 derive 逐字一致，唯一差别是 `content` 经
/// [`ChatMessage::content_json`] 计算——有 `content_parts` 时输出 block 数组
/// （文本 + 图片），否则输出纯字符串；`content_parts` 字段本身不上线。
/// 这样任何 `serde_json::to_value(&ChatRequest)` 的调用点都自动拿到多模态请求体。
impl Serialize for ChatMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let content = self.content_json();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("role", &self.role)?;
        if let Some(content) = &content {
            map.serialize_entry("content", content)?;
        }
        if let Some(tool_calls) = &self.tool_calls {
            map.serialize_entry("tool_calls", tool_calls)?;
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            map.serialize_entry("tool_call_id", tool_call_id)?;
        }
        if let Some(reasoning_content) = &self.reasoning_content {
            map.serialize_entry("reasoning_content", reasoning_content)?;
        }
        if let Some(metadata) = &self.metadata {
            map.serialize_entry("metadata", metadata)?;
        }
        map.end()
    }
}

impl ChatMessage {
    /// 构造 system 消息。
    pub fn system(content: impl Into<String>) -> Self {
        Self::with_role("system", content)
    }

    /// 构造 user 消息。
    pub fn user(content: impl Into<String>) -> Self {
        Self::with_role("user", content)
    }

    /// 构造 assistant 消息。
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::with_role("assistant", content)
    }

    /// 构造 tool 消息（关联某次工具调用）。
    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
            metadata: None,
        }
    }

    /// 构造多模态 user 消息（文本 + 图片等片段）。
    pub fn multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            role: "user".into(),
            content: None,
            content_parts: Some(parts),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    fn with_role(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Some(content.into()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    /// 追加工具调用（assistant 消息）。
    pub fn with_tool_calls(mut self, calls: Vec<ToolCall>) -> Self {
        self.tool_calls = Some(calls);
        self
    }

    /// 追加思考链内容。
    pub fn with_reasoning(mut self, content: impl Into<String>) -> Self {
        self.reasoning_content = Some(content.into());
        self
    }

    /// 设置业务元数据（随消息序列化，不发送给 LLM）。
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// 合并 `content` / `content_parts` 为 wire 格式的内容值：
    /// 有 `content_parts` 时返回片段数组（`content` 文本作为**首个 text 片段**并入，
    /// 保证如环境信息追加、注入消息说明等文本不因优先 parts 而丢失），
    /// 否则返回纯文本字符串。
    pub fn content_json(&self) -> Option<serde_json::Value> {
        if let Some(parts) = &self.content_parts
            && !parts.is_empty()
        {
            let mut all: Vec<ContentPart> = Vec::with_capacity(parts.len() + 1);
            if let Some(text) = self.content.as_ref().filter(|t| !t.is_empty()) {
                all.push(ContentPart::Text { text: text.clone() });
            }
            all.extend(parts.iter().cloned());
            return serde_json::to_value(&all).ok();
        }
        self.content.clone().map(serde_json::Value::String)
    }
}

/// 多模态内容片段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlContent },
}

/// 图片引用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageUrlContent {
    /// 图片地址：http(s) 外链、`data:image/...;base64,` 内联，或 `file-api-*`（Files API）。
    pub url: String,
    /// 处理精度：low / high / original / auto（缺省由供应商决定）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ImageUrlContent {
    /// 快捷构造（无 detail）。
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            detail: None,
        }
    }

    /// 指定处理精度（low / high / original / auto）。
    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }
}

/// 模型发起的工具调用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: FunctionCall,
}

impl ToolCall {
    /// 快捷构造（type 固定为 "function"）。
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            r#type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// 函数调用详情。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// 原始 JSON 字符串，由循环侧解析为参数
    pub arguments: String,
}

/// 工具定义（发给 provider 的 JSON Schema 描述）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: FunctionDef,
}

impl ToolDef {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self {
            r#type: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// 函数定义体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    /// JSON Schema 格式的参数定义
    pub parameters: serde_json::Value,
}

/// 一次完整的 LLM 请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    /// 思考强度（OpenAI reasoning_effort：low / medium / high / max，部分模型支持）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
}

impl ChatRequest {
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            model: String::new(),
            messages,
            tools: None,
            reasoning_effort: None,
            response_format: None,
        }
    }

    /// 指定本次请求的模型名（`chat_stream` 调用参数会覆盖此字段）。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// 设置思考强度（OpenAI `reasoning_effort`：low / medium / high / max）。
    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    pub fn with_response_format(mut self, response_format: serde_json::Value) -> Self {
        self.response_format = Some(response_format);
        self
    }

    /// 克隆请求配置（model / tools / reasoning_effort / response_format），
    /// 并替换为新消息——避免克隆含大体积历史消息的 `messages`。
    pub fn clone_with_messages(&self, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: self.model.clone(),
            messages,
            tools: self.tools.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            response_format: self.response_format.clone(),
        }
    }
}
