//! 工具层：Tool trait、执行上下文与工具定义构建（业务无关）。
//!
//! 参考 astorm-desktop 的工具设计裁剪：
//! - [`Tool`]：所有工具必须实现的接口，`execute` 接收 [`ToolContext`]
//! - [`ToolContext`]：工具执行上下文（取消令牌 / 会话标识 / 可选事件通道）
//! - [`tool_defs`]：由工具列表构建 OpenAI 格式工具定义（空 → None，按名称排序）

use crate::types::LoopEvent;
use async_trait::async_trait;
use llm_provider::ChatMessage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// OpenAI 格式的工具定义（复用 llm-provider 类型，避免重复定义）。
pub mod provider {
    pub use llm_provider::{FunctionDef, ToolDef};
}

/// 由工具列表构建工具定义（空列表 → None，按名称排序）。
pub fn tool_defs(tools: &[Arc<dyn Tool>]) -> Option<Vec<provider::ToolDef>> {
    if tools.is_empty() {
        return None;
    }
    let mut defs: Vec<provider::ToolDef> = tools
        .iter()
        .map(|t| provider::ToolDef {
            r#type: "function".into(),
            function: provider::FunctionDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            },
        })
        .collect();
    defs.sort_by(|a, b| a.function.name.cmp(&b.function.name));
    Some(defs)
}

/// 工具管理器：名称 → 工具 的映射（引擎执行查找 + 定义构建）。
///
/// 与业务侧注册中心（如 astorm-core 的 `ToolManager`）职责不同：
/// 本类型只做循环内的名称映射与工具定义导出，注册 / 来源追踪由业务层负责。
#[derive(Default)]
pub struct ToolMap {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolMap {
    /// 创建空管理器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 由工具列表构建（同名后注册者覆盖）。
    pub fn from_tools(tools: Vec<Arc<dyn Tool>>) -> Self {
        let mut map = Self::new();
        for t in tools {
            map.register(t);
        }
        map
    }

    /// 注册 / 覆盖工具。
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// 全部工具名称（无顺序保证）。
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// 按名称获取工具。
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// 是否为空（无工具时 LLM 请求不带 tools）。
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 工具定义（空 → None，按名称排序）。
    pub fn tool_defs(&self) -> Option<Vec<provider::ToolDef>> {
        let list: Vec<Arc<dyn Tool>> = self.tools.values().cloned().collect();
        tool_defs(&list)
    }
}

/// 工具执行上下文（引擎每次调用构造后传给工具）。
pub struct ToolContext {
    /// 取消令牌（工具可自行检测取消并提前返回）
    pub cancel: CancellationToken,
    /// 会话标识（工具查询会话元数据 / 注入环境变量用）
    pub session_id: String,
    /// 本次工具调用 id（= assistant `tool_calls[].id`）：审批拦截 / 重放据此定位历史 tool 消息。
    pub tool_call_id: String,
    /// 事件通道发送端（None = 业务未启用工具事件）
    pub(crate) emitter: Option<mpsc::UnboundedSender<LoopEvent>>,
    /// 追加消息槽：工具可经 [`ToolContext::inject_message`] 向**当前循环**追加消息
    /// （如 `read_images` 追加图片 user 消息）。引擎在本轮 `tool_result` 之后取出并注入，
    /// 只存在于本轮请求载荷，不落盘、不转发事件。
    pub(crate) injections: Arc<Mutex<Vec<ChatMessage>>>,
}

impl ToolContext {
    /// 创建空上下文（无会话标识 / 调用 id / 事件通道，注入槽为空）。
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            session_id: String::new(),
            tool_call_id: String::new(),
            emitter: None,
            injections: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 设置会话标识。
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    /// 设置本次工具调用 id。
    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = tool_call_id.into();
        self
    }

    /// 挂接事件通道发送端。
    pub fn with_emitter(mut self, tx: mpsc::UnboundedSender<LoopEvent>) -> Self {
        self.emitter = Some(tx);
        self
    }

    /// 推送自定义事件到业务接收方（无通道时静默忽略）。
    pub fn emit<T: std::any::Any + Send + Sync>(&self, data: T) {
        if let Some(tx) = &self.emitter {
            let _ = tx.send(LoopEvent::custom(data));
        }
    }

    /// 向当前循环追加一条消息（如 `read_images` 追加带图片的 user 消息）。
    ///
    /// 引擎在**本轮最后一个 `tool_result` 之后**取出并追加到请求消息（多模态消息内容
    /// 经 [`ChatMessage::content_json`](llm_provider::ChatMessage::content_json) 上送）。
    /// 该消息仅存在于本轮循环的请求载荷：不落库、不进前端、不参与会话历史。
    pub fn inject_message(&self, message: ChatMessage) {
        if let Ok(mut slot) = self.injections.lock() {
            slot.push(message);
        }
    }

    /// 取出并清空本次工具调用注入的全部消息（引擎 / 审批重放消费）。
    pub fn take_injections(&self) -> Vec<ChatMessage> {
        self.injections
            .lock()
            .map(|mut slot| std::mem::take(&mut *slot))
            .unwrap_or_default()
    }
}

/// Tool trait —— 所有工具必须实现此接口。
///
/// `execute` 接收 [`ToolContext`]：取消令牌 / 会话标识 / 调用 id / 事件通道均由引擎注入。
/// 中断（打断循环等待用户）只有一条通道：`interrupt_payload` —— 由工具在**执行后**
/// 依据结果决定（如安全拦截待审批、向用户提问），载荷由引擎原样透传给前端。
/// 另有 `result_meta` 通道：结果的结构化元数据（如拦截分类），不打断循环。
/// 需要「随工具结果显示额外消息」（如 `read_images` 把图片作为 user 消息交给模型）时，
/// 在 `execute` 内经 [`ToolContext::inject_message`] 注入——引擎在本轮 `tool_result`
/// 之后追加到请求载荷，且仅本轮可见。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称（LLM 调用时的 `function.name`）。
    fn name(&self) -> &str;
    /// 工具描述（提供给 LLM 理解用途）。
    fn description(&self) -> &str;
    /// 参数 JSON Schema（提供给 LLM 生成合法参数）。
    fn parameters_schema(&self) -> serde_json::Value;
    /// 执行结果要求中断时返回**载荷**（引擎据此发带 `payload` 的 interrupt 事件并中止循环）。
    /// 默认 None（普通工具不打断）；`result` 为 `execute` 返回的结果原文。
    /// 不打断的情况（如 Auto 模式下不提问）直接让 `execute` 返回不带中断标记的文本即可。
    fn interrupt_payload(&self, _result: &str) -> Option<serde_json::Value> { None }
    /// 结果元数据（引擎原样并入 tool_result 事件与 tool 消息 metadata，不解释其语义）。
    ///
    /// `ok` = 执行是否成功；`result` 为**工具自产文本**（成功 = `execute` 结果原文，
    /// 失败 = `Err` 原文，均未经引擎包装），故工具可直接识别自己产出的结构化载荷。
    /// 用于给「失败但结果只是文本」的场景补结构化信息（如安全拦截的分类：
    /// `{"status":"error","denial":{"category":"file_write",…}}`）。`status` 可覆盖默认状态。
    /// 落进消息 metadata 后重启读历史仍可恢复。默认 None。
    fn result_meta(&self, _ok: bool, _result: &str) -> Option<serde_json::Value> { None }
    /// 执行工具并返回（结果文本, 结构化元数据）。
    ///
    /// 默认委托 [`Tool::execute`] 且不产生元数据；需要把**执行时才知道**的结构化信息
    /// （如文件 diff：只有拿到 before/after 才能算）传给前端的工具覆盖它。元数据与
    /// `result_meta` 同机制并入 `tool_result` 事件与消息 metadata（落库、冷启动可恢复）。
    async fn execute_with_meta(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<(String, Option<serde_json::Value>), String> {
        self.execute(params, ctx).await.map(|text| (text, None))
    }
    /// 执行工具：返回结果文本（写入历史并回传 LLM）。
    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<String, String>;
}
