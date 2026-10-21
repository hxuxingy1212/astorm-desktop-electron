//! 循环引擎：多轮 LLM 推理 + 工具执行（业务无关）。
//!
//! - [`LoopContext`]：引擎与业务共享的循环状态（业务创建，引擎驱动）
//! - [`LlmResult`]：单轮 LLM 调用累积结果
//! - [`ReactEngine`]：循环引擎本体（LLM 客户端 + 业务 hook），经
//!   [`ReactLoop`](crate::ReactLoop) hook 与业务交互
//!
//! # 消息元数据契约（随消息序列化持久化、不上送 LLM）
//!
//! - assistant 消息：`started_at` / `ended_at`（unix 毫秒）、`thinking_ms`（思考耗时，毫秒）
//! - 工具结果消息：`started_at` / `ended_at`（unix 毫秒）
//!
//! 由引擎在构建消息时写入，前端据此渲染对话的时间信息。

use crate::event;
use crate::loop_guard::{LoopGuard, GuardAction};
use crate::react_loop::ReactLoop;
use crate::tool::{ToolContext, ToolMap};
use crate::types::LoopEvent;
use futures::StreamExt;
use llm_provider::{ChatClient, ChatMessage, ChatRequest, LlmEvent, TokenUsage, ToolCall};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 最大推理轮次上限（超过后强制结束）。
const MAX_ROUNDS: u32 = 100;
/// LLM 流式无数据超时（秒）：流在此时长内无任何分片则中断本轮，避免会话永久卡在流式中。
const LLM_STREAM_IDLE_TIMEOUT_SECS: u64 = 120;

/// 当前 unix 毫秒时间戳（消息元数据计时用，与前端 `Date.now()` 对齐）。
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 循环上下文：业务创建、引擎驱动、hook 读写。
pub struct LoopContext {
    /// 会话标识（工具执行 / 业务存储路由）
    pub session_id: String,
    /// 已执行累计轮数（引擎自增，跨用户消息累计；会话信息展示）
    pub round: u32,
    /// 本条用户消息内的轮数（引擎自增；每条消息重新计时，用于 [`MAX_ROUNDS`] 上限判定）
    pub turn_round: u32,
    /// 上一轮 LLM 请求的真实 prompt token 数（当前上下文占用；provider 未返回用量时为 0）
    pub last_prompt_tokens: u32,
    /// 取消标识（业务注入，引擎检查）
    pub cancel: CancellationToken,
    /// 打断标识（工具要求中断时由引擎置位）
    pub interrupt: bool,
    /// 累积 token 用量（引擎逐轮累计）
    pub total_usage: TokenUsage,
    /// 循环开始时间（引擎在 run 入口设置）
    pub started_at: Option<Instant>,
    /// 循环结束时间（引擎在 run 出口设置）
    pub ended_at: Option<Instant>,
    /// 扩展元数据（业务自由读写：提示词模板、持久化游标等）
    pub metadata: HashMap<String, Value>,
    /// 工具事件通道发送端（业务创建 channel 后放入，自留 receiver 消费）
    pub event_tx: Option<mpsc::UnboundedSender<LoopEvent>>,
    /// 请求模板 + 会话对话的唯一持有者：`messages` 存历史对话（业务装入初始历史，
    /// 引擎追加 assistant / tool 消息）；`model` / `tools` / `reasoning_effort` 等为
    /// 请求配置。每轮构建请求时只克隆配置 + 过滤后的消息（剥离 metadata、丢弃空消息）
    pub chat_request: ChatRequest,
    /// 工具管理器（名称 → 工具，引擎执行查找）
    pub tools: ToolMap,
    /// 本次循环的 LLM 客户端（业务注入：按请求路由到的供应商客户端；
    /// `None` 时引擎回落构造时注入的客户端）
    pub client: Option<Arc<ChatClient>>,
}

impl LoopContext {
    /// 创建循环上下文。
    ///
    /// 创建后业务需填充：
    /// - `chat_request`：初始历史（`messages`）+ 模型名（`model`），可选工具定义 / 思考强度
    /// - `tools`：本次循环可用的工具映射（未预置工具定义时引擎每轮实时导出）
    /// - 可选：`event_tx`（工具自定义事件通道）、`metadata`（扩展元数据）、
    ///   `client`（本循环 LLM 客户端，缺省回落引擎构造时的客户端）
    pub fn new(session_id: impl Into<String>, cancel: CancellationToken) -> Self {
        Self {
            session_id: session_id.into(),
            round: 0,
            turn_round: 0,
            last_prompt_tokens: 0,
            cancel,
            interrupt: false,
            total_usage: TokenUsage::default(),
            started_at: None,
            ended_at: None,
            metadata: HashMap::new(),
            event_tx: None,
            chat_request: ChatRequest::new(Vec::new()),
            tools: ToolMap::default(),
            client: None,
        }
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// 是否被打断。
    pub fn is_interrupted(&self) -> bool {
        self.interrupt
    }

    /// 读取扩展元数据。
    pub fn get_meta(&self, key: &str) -> Option<&Value> {
        self.metadata.get(key)
    }

    /// 写入扩展元数据。
    pub fn set_meta(&mut self, key: impl Into<String>, value: Value) {
        self.metadata.insert(key.into(), value);
    }
}

/// 单轮 LLM 调用累积结果（流式分片合并后的产物）。
pub struct LlmResult {
    /// LLM 回复的文本内容
    pub content: String,
    /// LLM 的思考链内容（streaming reasoning）
    pub reasoning_content: String,
    /// LLM 请求的工具调用列表
    pub tool_calls: Vec<ToolCall>,
    /// 本轮 token 用量（完整保留 provider 返回字段，含 DeepSeek 等扩展用量：
    /// cache_hit / cache_miss）
    pub usage: TokenUsage,
    /// 本轮 LLM 调用开始时间（unix 毫秒）
    pub started_at: u64,
    /// 本轮 LLM 调用结束时间（unix 毫秒，流结束 / End 事件时）
    pub ended_at: u64,
    /// 思考阶段耗时（毫秒；从首个 reasoning 分片到首个 content 分片，
    /// 纯思考无内容时按流结束计，无思考则为 0）
    pub thinking_ms: u64,
}

impl LlmResult {
    /// 是否有文本内容输出。
    pub fn has_content(&self) -> bool {
        !self.content.is_empty()
    }

    /// 是否发起了工具调用。
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// 循环引擎：LLM 客户端 + 业务 hook（模型名经 [`LoopContext::chat_request`] 注入）。
pub struct ReactEngine {
    client: Arc<ChatClient>,
    hooks: Arc<dyn ReactLoop>,
}

impl ReactEngine {
    /// 创建引擎（LLM 客户端与业务 hook 必填）。
    pub fn new(client: Arc<ChatClient>, hooks: Arc<dyn ReactLoop>) -> Self {
        Self { client, hooks }
    }

    /// 运行一次 ReAct 循环（三阶段：before → loop → after）。
    ///
    /// hook 时序（`before_loop` / `after_loop` 整个循环各一次，
    /// `loop_stat` / `after_llm` / `loop_end` 每轮一次）：
    ///
    /// ```text
    /// loop_life("started")（循环入口，一次）
    /// before_loop（循环前：准备 / 注入 system prompt）
    ///   └─► [ loop_start ─► LLM 推理 ─► 保存回复 ─► after_llm ─► 工具执行 ─► loop_end ]
    ///        └──────────────── 有工具调用且未中断 / 取消时，进入下一轮 ────────────────┘
    /// loop_life("finished")（循环结束，一次）→ after_loop（持久化用量 / 清理）
    /// ```
    ///
    /// - 循环状态经 `ctx`，业务交互经构造时注入的 [`ReactLoop`] hook
    /// - 工具经 `ctx.tools`（执行查找）与 `ctx.chat_request`（请求工具定义）注入，
    ///   由调用方按业务规则预配置（如按会话模式过滤）
    /// - 出错时立即返回 `Err`：本轮 `loop_end` 与收尾 `after_loop` 均不执行，
    ///   由调用方决定错误处理与部分状态持久化
    pub async fn run(&self, ctx: &mut LoopContext) -> Result<(), String> {
        ctx.started_at = Some(Instant::now());
        // 只观察本次运行的新回复，不扫描会被摘要重写的历史，也不依赖轮次。
        let mut loop_guard = LoopGuard::default();
        let mut loop_warning = None;

        // 循环生命周期：开始（整个循环仅一次，即使 before_loop 失败也已宣告开始）
        self.hooks.emit_event(ctx, event::loop_life(&ctx.session_id, "started"));

        // ── Phase 1: Pre-loop（整个循环前，仅一次）──────────────────
        // before_loop：业务准备（注入 system prompt、初始化元数据等）
        self.hooks.before_loop(ctx).await?;

        // ── Phase 2: ReAct loop — 多轮推理 + 工具执行 ────────────────
        // 每轮时序：loop_start → LLM 推理 → 保存回复 → after_llm → 工具执行 → loop_end
        // 退出条件：达到 MAX_ROUNDS / 会话取消 / LLM 无工具调用（正常结束）/ 工具中断
        // 上限按 **本条消息** 的轮数（turn_round）判定：ctx.round 是跨消息累计值（会话信息
        // 展示 + 事件 round），不参与上限，否则整条会话累计满 100 轮后新消息将永不推理。
        while ctx.turn_round < MAX_ROUNDS {
            // 会话取消 → 终止循环（部分状态保留在 ctx，由调用方处理）
            if ctx.cancel.is_cancelled() {
                break;
            }
            ctx.turn_round += 1;
            ctx.round += 1;

            // loop 内每轮启动阶段（业务：提示注入后、LLM 前做剪支 / 摘要等准备）
            self.hooks.loop_start(ctx).await;

            // Step 1: LLM 推理 — 流式接收文本、思考链、工具调用、用量
            let result = self.execute_llm_round_with_warning(ctx, loop_warning.take()).await?;
            // 记录本轮请求的真实 prompt token（当前上下文占用；业务据此做摘要阈值判定）
            track_context_tokens(&mut ctx.last_prompt_tokens, &result.usage);

            // Step 2: 保存 LLM 回复到会话消息（含工具调用 / 思考链）+ 累计 token 用量
            if let Some(msg) = build_assistant_message(&result) {
                ctx.chat_request.messages.push(msg.clone());
                self.hooks.append_message(ctx, msg);
            }
            ctx.total_usage.accumulate(&result.usage);

            // Step 3: 每次 LLM 调用后（业务：每轮感知点）
            self.hooks.after_llm(ctx, &result).await;

            // Step 4: 无工具调用 → 结束循环（本轮即最终轮，不再触发 loop_end）
            if !result.has_tool_calls() {
                break;
            }

            match loop_guard.observe(&result.tool_calls) {
                GuardAction::Continue => {}
                GuardAction::Warn => {
                    loop_warning = Some(ChatMessage::user(
                        "[工具循环警告] 最近的工具调用中，相同工具和参数已重复至少 3 次。请检查已有结果，停止无进展的重试，改用其他方法；无法推进时向用户说明原因。",
                    ));
                }
                GuardAction::Stop => {
                    let reason = "工具循环已停止：最近 12 次调用中，相同工具和参数重复至少 6 次。请调整方法后重试。";
                    // assistant 已落盘；为整个批次补齐 tool 响应，禁止执行本批次副作用。
                    for tc in &result.tool_calls {
                        let msg = ChatMessage::tool(reason, tc.id.clone());
                        ctx.chat_request.messages.push(msg.clone());
                        self.hooks.append_message(ctx, msg);
                        self.hooks.emit_event(ctx, event::tool_result(
                            &ctx.session_id, &tc.id, &tc.function.name, reason, None, ctx.round,
                        ));
                    }
                    return Err(reason.into());
                }
            }

            // Step 5: 有工具调用 → 顺序执行（结果写回历史 + 持久化 + 转发事件）
            let interrupted = self.execute_tool_calls(ctx, &result.tool_calls).await?;
            if interrupted || ctx.cancel.is_cancelled() {
                break;
            }

            // loop 内每轮收尾阶段：仅在本轮正常完成（工具执行完毕且未被中断 / 取消）、
            // 即将进入下一轮推理时触发
            self.hooks.loop_end(ctx);
        }

        // ── Phase 3: Finalize（循环结束后，仅一次）───────────────────
        ctx.ended_at = Some(Instant::now());

        // 循环生命周期：结束（整个循环仅一次，仅成功路径触发）
        self.hooks.emit_event(ctx, event::loop_life(&ctx.session_id, "finished"));

        // after_loop：业务收尾（持久化用量、清理等）
        self.hooks.after_loop(ctx).await?;
        Ok(())
    }

    /// 单轮 LLM 调用：克隆请求模板（刷新消息）→ 流式接收 → 累积结果并转发事件。
    async fn execute_llm_round_with_warning(
        &self, ctx: &mut LoopContext, warning: Option<ChatMessage>,
    ) -> Result<LlmResult, String> {
        // 会话对话唯一持有在 chat_request.messages；每轮构建请求时克隆消息并剥离
        // metadata（改动只发生在克隆体上，不污染状态）
        let llm_messages: Vec<ChatMessage> = ctx
            .chat_request
            .messages
            .iter()
            // tool 消息永不过滤（content 空也要上送）：它是 assistant(tool_calls) 的配对
            // 载体，少一条就是「tool_calls 后必须紧跟全部 tool 响应」400（如 file_operation
            // 空列表冷启动回灌的空结果消息）
            .filter(|m| m.role == "tool" || m.content.is_some() || m.content_parts.is_some() || m.tool_calls.is_some())
            .cloned()
            .map(|mut m| {
                // 元数据参与序列化但不上送 LLM,构造请求前剥离
                m.metadata = None;
// 兜底清洗:assistant tool_call 的 arguments 须是**合法 JSON 对象**,
                        // 非法时回退空对象 {},避免上游 400(历史回灌的旧数据也可能脏)
                        if let Some(tcs) = &mut m.tool_calls {
                            for tc in tcs {
                                if serde_json::from_str::<Value>(&tc.function.arguments)
                                    .map(|v| !v.is_object())
                                    .unwrap_or(true)
                                {
                                    tc.function.arguments = "{}".into();
                                }
                            }
                        }
                m
            })
            .collect();
        // 协议兜底：丢弃无前置 assistant(tool_calls) 的孤立 tool 消息（压缩 / 修复后可能出现），
        // 否则上游 400: Messages with role 'tool' must be a response to a preceding message with 'tool_calls'
        let mut llm_messages = drop_orphan_tool_messages(llm_messages);
        // 在 loop_start 的剪支/摘要之后、完整 tool 响应区块之后注入。
        // 仅进入本次请求，不污染持久化历史；有工具调用的轮次同样会送达。
        if let Some(warning) = warning {
            llm_messages.push(warning);
        }

        let mut request = ctx.chat_request.clone_with_messages(llm_messages);
        // 业务未显式配置工具定义时，回退用工具管理器导出
        if request.tools.is_none() {
            request.tools = ctx.tools.tool_defs();
        }
        // 模型名来自请求模板（chat_request.model）
        if request.model.is_empty() {
            return Err("ChatRequest 缺少模型名（model 为空）".into());
        }
        let model = request.model.clone();

        // 本循环客户端由 ctx 路由（业务按供应商注入）；未注入时回落引擎构造时的客户端
        let client = ctx.client.clone().unwrap_or_else(|| self.client.clone());
        let mut stream = client
            .chat_stream(&model, request, ctx.cancel.child_token())
            .await
            .map_err(|e| format!("LLM 请求失败: {}", e))?;

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut full_content = String::new();
        let mut full_reasoning = String::new();
        let mut usage = TokenUsage::default();
        // 计时：started_at 为调用起点；thinking 从首个 reasoning 分片起算，
        // 到首个 content 分片（或流结束）结算
        let started_at = unix_ms();
        let mut ended_at = 0u64;
        let mut thinking_ms = 0u64;
        let mut thinking_started: Option<Instant> = None;

        // 流式消费带无数据超时：provider 首字节后若停滞（网络挂死等），超时后报错中断，
        // 由 run 侧转 error 事件，避免会话永久卡在流式中（前端无法恢复）
        loop {
            let item = tokio::time::timeout(
                std::time::Duration::from_secs(LLM_STREAM_IDLE_TIMEOUT_SECS),
                stream.next(),
            )
            .await
            .map_err(|_| format!("LLM 流无数据超时（{LLM_STREAM_IDLE_TIMEOUT_SECS}s），已中断"))?;
            let Some(item) = item else { break };
            let event = match item {
                Ok(event) => event,
                Err(e) => {
                    // 取消导致的流中断(provider 投递 Cancelled / 取消引发的连接关闭):
                    // 按流提前结束处理,保留半截内容,交由 after_loop 以 cancelled 事件
                    // 收尾,不当错误上报
                    if ctx.cancel.is_cancelled() {
                        break;
                    }
                    return Err(format!("LLM stream 错误: {}", e));
                }
            };
            match event {
                LlmEvent::Start => {}
                // 流中传输中断后 provider 自动重开（连接被 RST 等）：本轮此前累积的
                // 正文/思考/工具调用全部作废，并通知前端丢弃已渲染的半截内容重画。
                // 请求 messages 未变（本轮尚未 append 回复），轮次计数与计时不重来。
                LlmEvent::Restart => {
                    full_content.clear();
                    full_reasoning.clear();
                    tool_calls.clear();
                    usage = TokenUsage::default();
                    thinking_started = None;
                    thinking_ms = 0;
                    self.hooks
                        .emit_event(ctx, event::stream_reset(&ctx.session_id, ctx.round));
                }
                LlmEvent::Chunk(c) => {
                    full_content.push_str(&c.content);
                    // 首个 content 分片到达 → 思考阶段结束
                    if let Some(s) = thinking_started.take() {
                        thinking_ms = s.elapsed().as_millis() as u64;
                    }
                    self.hooks
                        .emit_event(ctx, event::text(&ctx.session_id, &c.content, ctx.round));
                }
                LlmEvent::ReasoningChunk(c) => {
                    full_reasoning.push_str(&c.content);
                    if thinking_started.is_none() {
                        thinking_started = Some(Instant::now());
                    }
                    self.hooks
                        .emit_event(ctx, event::reasoning(&ctx.session_id, &c.content, ctx.round));
                }
                LlmEvent::ToolCalls(items) => {
                    // 收入时即清洗:arguments 必须是合法 JSON 对象,
                    // 非法(或非对象)回退 {},保证写入历史 / 后续请求与执行参数解析都稳定
                    for item in &items {
                        let mut tc = item.clone();
                        if serde_json::from_str::<Value>(&tc.function.arguments)
                            .map(|v| !v.is_object())
                            .unwrap_or(true)
                        {
                            tc.function.arguments = "{}".into();
                        }
                        tool_calls.push(tc);
                    }
                    // 前端期望的字段路径：name / params（从 function.arguments 解析）
                    let calls: Vec<Value> = tool_calls
                        .iter()
                        .map(|t| {
                            let params: Value =
                                serde_json::from_str(&t.function.arguments).unwrap_or(json!({}));
                            json!({ "id": t.id, "name": t.function.name, "params": params })
                        })
                        .collect();
                    self.hooks
                        .emit_event(ctx, event::tool_call(&ctx.session_id, calls, ctx.round));
                }
LlmEvent::End(e) => {
                    // 完整保留 provider 返回的用量（含 DeepSeek 等厂商扩展字段）
                    if let Some(u) = e.usage {
                        usage = u;
                    }
                    ended_at = unix_ms();
                    // 纯思考无内容：按流结束结算思考阶段
                    if let Some(s) = thinking_started.take() {
                        thinking_ms = s.elapsed().as_millis() as u64;
                    }
                    break;
                }
                LlmEvent::Error(msg) => {
                    return Err(format!("LLM 服务错误: {}", msg));
                }
            }
        }

        Ok(LlmResult {
            content: full_content,
            reasoning_content: full_reasoning,
            tool_calls,
            usage,
            started_at,
            ended_at,
            thinking_ms,
        })
    }

    /// 顺序执行本轮所有工具调用。
    ///
    /// 每个调用：按名查找（`ctx.tools` 映射）→ 构造 [`ToolContext`] → 执行 →
    /// 结果写回历史并转发事件；结果要求中断（`interrupt_payload`，如安全拦截待审批、
    /// 向用户提问）时置位中断并立即中止后续工具。
    /// 返回 `true` 表示遇到中断工具，调用方应终止循环。
    ///
    /// 工具经 [`ToolContext::inject_message`] 注入的消息（如 `read_images` 的图片 user
    /// 消息）随后追加到请求载荷：本轮最后一个调用 → 紧跟其 `tool_result`；仍有后续调用
    /// → 暂存到本轮最后一个 `tool_result` 之后（插在多个 tool 结果之间会破坏
    /// `assistant(tool_calls) → tool…` 区块的连续性，上游会 400）。注入消息仅本轮可见。
    async fn execute_tool_calls(
        &self,
        ctx: &mut LoopContext,
        tool_calls: &[ToolCall],
    ) -> Result<bool, String> {
        let last_index = tool_calls.len().saturating_sub(1);
        let mut pending: Vec<ChatMessage> = Vec::new();
        for (index, tc) in tool_calls.iter().enumerate() {
            if ctx.cancel.is_cancelled() {
                break;
            }
            let tool_name = tc.function.name.clone();
            let tool_id = tc.id.clone();

            // 未知工具：写错误消息进历史并转发事件（无时长），继续执行下一个调用
            let Some(tool) = ctx.tools.get(&tool_name) else {
                let err = format!("未知工具: {}", tool_name);
                let msg = ChatMessage::tool(err.clone(), tool_id.clone());
                ctx.chat_request.messages.push(msg.clone());
                self.hooks.append_message(ctx, msg);
                self.hooks
                    .emit_event(
                    ctx,
                    event::tool_result(&ctx.session_id, &tool_id, &tool_name, &err, None, ctx.round),
                );
                // 未知工具也要收尾：最后一个调用时把之前暂存的注入消息追加掉，避免丢失
                append_injected(ctx, &mut pending, Vec::new(), index, last_index);
                continue;
            };

            // 参数解析失败不作为**整轮错误**抛出（否则会中断循环并弹到前端 UI）：
            // 写成配对 tool 消息 + status=error 的 tool_result，错误原文（含原始参数）回给
            // 模型，由它自行纠正后重试，后续工具调用照常执行。
            let params: Value = match serde_json::from_str(&tc.function.arguments) {
                Ok(v) => v,
                Err(e) => {
                    let err = format!(
                        "工具 '{}' 参数解析失败: {}（原始参数: {}）",
                        tool_name,
                        e,
                        tc.function.arguments.chars().take(200).collect::<String>(),
                    );
                    let msg = ChatMessage::tool(err.clone(), tool_id.clone());
                    ctx.chat_request.messages.push(msg.clone());
                    self.hooks.append_message(ctx, msg);
                    self.hooks.emit_event(
                        ctx,
                        event::tool_result_with_meta(
                            &ctx.session_id,
                            &tool_id,
                            &tool_name,
                            &err,
                            None,
                            ctx.round,
                            Some("error"),
                            None,
                        ),
                    );
                    // 与未知工具同构：最后一个调用时把之前暂存的注入消息追加掉，避免丢失
                    append_injected(ctx, &mut pending, Vec::new(), index, last_index);
                    continue;
                }
            };

            let tool_ctx = ToolContext {
                cancel: ctx.cancel.clone(),
                session_id: ctx.session_id.clone(),
                tool_call_id: tool_id.clone(),
                emitter: ctx.event_tx.clone(),
                // 每次调用独立注入槽；本轮结束前由下方 drain（审批重放路径同理）
                injections: Arc::new(std::sync::Mutex::new(Vec::new())),
            };
            let started_unix = unix_ms();
            let started_at = Instant::now();
            // 记录是否执行成功:失败时不应置位中断——否则等待用户类工具(如 chat_with_user)
            // 一旦报错(如 Auto 模式禁止提问)会中断整个循环导致流程中止,应把错误交回模型自行恢复。
            // `meta_src` = 工具自产文本（成功=结果原文；失败=Err 原文，未经引擎包装），
            // 供 result_meta 识别工具自己的结构化载荷（如安全拦截信封）。
            let (result, ok, meta_src, exec_meta) = match tool.execute_with_meta(params, &tool_ctx).await {
                Ok((s, m)) => (s.clone(), true, s, m),
                Err(e) => (format!("工具 '{}' 执行失败: {}", tool_name, e), false, e, None),
            };
            let duration_ms = started_at.elapsed().as_millis() as u32;
            let ended_unix = unix_ms();
            // 工具提供的结构化结果元数据（引擎不解释语义）：平铺进 tool_result 事件，
            // 并落进消息 metadata（重启读历史可恢复）；可覆盖 status（如「返回文本但语义是失败」）
            // - `result_meta`：从结果文本识别（如安全拦截分类），优先；
            // - `execute_with_meta`：执行时产生的元数据（如文件 diff），补充未占用键。
            let mut meta = tool.result_meta(ok, &meta_src);
            if let Some(em) = exec_meta {
                let slot = meta.get_or_insert_with(|| json!({}));
                if let (Some(obj), Value::Object(em_map)) = (slot.as_object_mut(), em) {
                    for (k, v) in em_map {
                        obj.entry(k).or_insert(v);
                    }
                }
            }
            let status = meta
                .as_ref()
                .and_then(|m| m.get("status"))
                .and_then(|s| s.as_str())
                .unwrap_or(if ok { "ok" } else { "error" });

            // 工具结果写入会话消息（tool → tool_call_id 配对），持久化并转发事件；
            // 元数据携带开始 / 结束时间（unix 毫秒）与状态 / 工具元数据，供前端渲染
            let mut metadata = json!({
                "started_at": started_unix,
                "ended_at": ended_unix,
                "status": status,
            });
            if let (Some(Value::Object(map)), Some(md)) = (meta.as_ref(), metadata.as_object_mut()) {
                for (k, v) in map {
                    md.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            let msg = ChatMessage::tool(result.clone(), tool_id.clone()).with_metadata(metadata);
            ctx.chat_request.messages.push(msg.clone());
            self.hooks.append_message(ctx, msg);
            self.hooks.emit_event(
                ctx,
                event::tool_result_with_meta(
                    &ctx.session_id,
                    &tool_id,
                    &tool_name,
                    &result,
                    Some(duration_ms),
                    ctx.round,
                    Some(status),
                    meta.as_ref(),
                ),
            );

            // 工具注入的追加消息（如图片 user 消息）：最后一个调用 → 紧跟本 tool_result
            // 追加；仍有后续调用 → 暂存，等最后一个 tool_result 之后统一追加
            append_injected(ctx, &mut pending, tool_ctx.take_injections(), index, last_index);

            if ok {
                // 结果要求中断（安全拦截待审批 / 向用户提问）：结果照常成对落库/转发，再带载荷中断循环。
                // 载荷由工具提供（引擎不解释其语义），前端据 `payload` 弹审批卡或提问表单。
                if let Some(payload) = tool.interrupt_payload(&result) {
                    ctx.interrupt = true;
                    // 中断后本轮不再续跑，暂存的注入消息无处可用（避免残留到后续轮次）
                    pending.clear();
                    // 同轮**剩余调用**本轮不再执行（前序调用待用户确认），但不能让它们悬空：
                    // 逐条落成配对的 tool 消息（落库 + 转发事件），否则前端工具卡永远停在
                    // 「正在执行工具」，历史修复只能注入一句无原因的「未执行」。
                    // 标记 `deferred=true`：用户同意后由 astorm-core 的决议续跑路径按序补执行。
                    for tc in tool_calls.iter().skip(index + 1) {
                        let content = format!(
                            "该工具调用本轮暂未执行：同一轮的前一个调用（{tool_name}）正在等待用户确认；确认通过后将自动继续执行，若确认未通过则需时请重新发起。"
                        );
                        let meta = json!({ "status": "error", "deferred": true });
                        let msg = ChatMessage::tool(content.clone(), tc.id.clone())
                            .with_metadata(meta.clone());
                        ctx.chat_request.messages.push(msg.clone());
                        self.hooks.append_message(ctx, msg);
                        self.hooks.emit_event(
                            ctx,
                            event::tool_result_with_meta(
                                &ctx.session_id,
                                &tc.id,
                                &tc.function.name,
                                &content,
                                None,
                                ctx.round,
                                Some("error"),
                                Some(&meta),
                            ),
                        );
                    }
                    self.hooks.emit_event(
                        ctx,
                        event::interrupt_with(&ctx.session_id, &tool_name, Some(payload)),
                    );
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// 本轮工具注入消息的落点：把暂存 + 本次注入按序追加到请求载荷。
///
/// 仅追加到 `ctx.chat_request.messages`（本轮循环可见）：不落盘、不进会话历史、不发事件。
fn append_injected(
    ctx: &mut LoopContext,
    pending: &mut Vec<ChatMessage>,
    injected: Vec<ChatMessage>,
    index: usize,
    last_index: usize,
) {
    for mut msg in drain_injected(pending, injected, index, last_index) {
        // 标记「仅本轮请求可见」：业务侧摘要重写请求载荷时据此保留（不落会话历史）
        mark_ephemeral(&mut msg);
        ctx.chat_request.messages.push(msg);
    }
}

/// 标记工具注入消息（`metadata.ephemeral = true`）：只进本轮请求载荷，不进会话历史；
/// 摘要重写请求载荷时按此标记保留，避免丢失 read_images 注入的图片。
fn mark_ephemeral(msg: &mut ChatMessage) {
    let meta = msg.metadata.get_or_insert_with(|| json!({}));
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("ephemeral".into(), Value::Bool(true));
    }
}

/// 丢弃无前置 `assistant(tool_calls)` 的孤立 `tool` 消息（协议兜底，纯逻辑）。
///
/// LLM API 要求每条 `tool` 消息都必须是某个**前置** assistant `tool_calls` 的响应；
/// 历史压缩 / 请求载荷重写后若切分点落在「assistant(tool_calls) → tool」之间，就会出现
/// 孤立 tool，上送即 400。引擎在构造请求前统一剔除，保证载荷始终合法。
fn drop_orphan_tool_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut pending: HashSet<String> = HashSet::new();
    let mut out: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        if msg.role == "assistant" {
            if let Some(calls) = &msg.tool_calls {
                for c in calls {
                    pending.insert(c.id.clone());
                }
            }
            out.push(msg);
        } else if msg.role == "tool" {
            let id = msg.tool_call_id.clone().unwrap_or_default();
            // 有对应前置 tool_calls 才保留（tool_call id 唯一，命中即消费）
            if !id.is_empty() && pending.remove(&id) {
                out.push(msg);
            }
        } else {
            out.push(msg);
        }
    }
    out
}

/// 注入消息的顺序决策（纯逻辑，便于单测）：
/// `index` 已是本轮最后一个工具调用 → 返回「暂存消息 + 本次注入」（追加到该 tool_result 之后）；
/// 否则把本次注入并入暂存并返回空（等最后一个 tool_result 之后统一追加，避免破坏
/// `assistant(tool_calls) → tool…` 区块连续性导致上游 400）。
fn drain_injected(
    pending: &mut Vec<ChatMessage>,
    injected: Vec<ChatMessage>,
    index: usize,
    last_index: usize,
) -> Vec<ChatMessage> {
    if index >= last_index {
        pending.drain(..).chain(injected).collect()
    } else {
        pending.extend(injected);
        Vec::new()
    }
}

/// 更新「当前上下文占用」（上一轮请求的真实 prompt token）。
///
/// 仅在**本轮确实取到用量**时覆盖：provider 的用量随流正常结束的 `End` 一次投递，
/// 流异常中断（未收到 `End`，如只吐了思考链就断流）/ 供应商不返回 usage 时本轮用量为 0，
/// 此时必须保留上一次的有效值——否则会话信息卡的「上下文容量」被清零（且摘要阈值
/// 判定会退回字符估算）。
fn track_context_tokens(last: &mut u32, usage: &TokenUsage) {
    if usage.prompt > 0 {
        *last = usage.prompt;
    }
}

/// 构建 assistant 消息（含工具调用 / 思考链）。
/// 思考链独立挂 `reasoning_content`，不并入 content；正文是什么就是什么，
/// 不做「思考落正文」替换（思考只在 think 块展示，正文为空则无正文）。
/// 完全无内容无工具调用无思考时返回 None。
fn build_assistant_message(result: &LlmResult) -> Option<ChatMessage> {
    let has_reasoning = !result.reasoning_content.is_empty();
    if !result.has_tool_calls() && !result.has_content() && !has_reasoning {
        return None;
    }
    // 正文原样保留（不做思考落正文替换）；思考链独立挂 reasoning_content
    let content = result.content.clone();
    let mut msg = ChatMessage::assistant(content);
    if result.has_tool_calls() {
        msg = msg.with_tool_calls(result.tool_calls.clone());
    }
    if has_reasoning {
        msg = msg.with_reasoning(result.reasoning_content.clone());
    }
    Some(msg.with_metadata(json!({
        "started_at": result.started_at,
        "ended_at": result.ended_at,
        "thinking_ms": result.thinking_ms,
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        drain_injected, drop_orphan_tool_messages, track_context_tokens, LlmResult, LoopContext,
        ReactEngine,
    };
    use crate::react_loop::ReactLoop;
    use crate::tool::{Tool, ToolContext, ToolMap};
    use async_trait::async_trait;
    use llm_provider::{ChatClient, ChatMessage, TokenUsage, ToolCall};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    /// 新建循环上下文：本轮轮数与真实上下文 token 均从 0 起（上限按本轮计时）。
    #[test]
    fn loop_context_starts_with_fresh_turn_round() {
        let ctx = LoopContext::new("s1", CancellationToken::new());
        assert_eq!(ctx.turn_round, 0, "本轮轮数应每次循环重新计时");
        assert_eq!(ctx.round, 0);
        assert_eq!(ctx.last_prompt_tokens, 0);
    }

    /// 回归：流异常中断（未收到 `End`，如只吐思考链就断流）时本轮用量为 0，
    /// 不得把已知的上下文占用清零——否则会话信息卡「上下文容量」显示 0，
    /// 摘要阈值判定也会退回字符估算。
    #[test]
    fn missing_usage_keeps_previous_context_tokens() {
        let mut last = 12_345;
        track_context_tokens(&mut last, &TokenUsage::default());
        assert_eq!(last, 12_345, "本轮未上报用量 → 保留上一次有效值");
        track_context_tokens(&mut last, &TokenUsage::new(900, 10));
        assert_eq!(last, 900, "本轮上报用量 → 覆盖为新值");
        track_context_tokens(&mut last, &TokenUsage::new(0, 0));
        assert_eq!(last, 900);
    }

    /// 孤立 tool 消息（无前置 assistant tool_calls）被丢弃；成对的保留。
    #[test]
    fn drops_orphan_tool_messages() {
        let assistant = ChatMessage::assistant("")
            .with_tool_calls(vec![ToolCall::new("c1", "t", "{}")]);
        let paired = ChatMessage::tool("r1", "c1");
        let orphan = ChatMessage::tool("r2", "cX");

        let out = drop_orphan_tool_messages(vec![
            orphan.clone(),
            assistant,
            paired,
            orphan,
        ]);
        assert_eq!(out.len(), 2, "孤立 tool 全部丢弃，保留 assistant + 配对 tool");
        assert_eq!(out[0].role, "assistant");
        assert_eq!(out[1].tool_call_id.as_deref(), Some("c1"));
    }

    /// 单工具轮：注入消息紧跟该 tool_result（立即返回）。
    #[test]
    fn single_tool_round_flushes_immediately() {
        let mut pending = Vec::new();
        let out = drain_injected(&mut pending, vec![user("img1")], 0, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.as_deref(), Some("img1"));
        assert!(pending.is_empty());
    }

    /// 多工具轮：非最后一个调用的注入先暂存（不插入 tool 结果之间），
    /// 到最后一个 tool_result 之后按顺序一并追加。
    #[test]
    fn multi_tool_round_defers_to_last_result() {
        let mut pending = Vec::new();
        let first = drain_injected(&mut pending, vec![user("img1")], 0, 2);
        assert!(first.is_empty(), "非最后一个工具调用不应立即追加");
        assert_eq!(pending.len(), 1);

        let second = drain_injected(&mut pending, vec![user("img2")], 1, 2);
        assert!(second.is_empty());
        assert_eq!(pending.len(), 2);

        let last = drain_injected(&mut pending, vec![user("img3")], 2, 2);
        let texts: Vec<&str> = last.iter().filter_map(|m| m.content.as_deref()).collect();
        assert_eq!(texts, vec!["img1", "img2", "img3"], "按调用顺序追加");
        assert!(pending.is_empty());
    }

    /// 无注入时保持空（不产生额外消息）。
    #[test]
    fn no_injection_produces_nothing() {
        let mut pending = Vec::new();
        assert!(drain_injected(&mut pending, Vec::new(), 0, 0).is_empty());
        assert!(drain_injected(&mut pending, Vec::new(), 0, 3).is_empty());
        assert!(pending.is_empty());
    }

    // ---------- 中断时同轮剩余调用：落成 deferred 配对占位（不悬空） ----------

    #[derive(Default)]
    struct FakeHooks {
        messages: Mutex<Vec<ChatMessage>>,
        events: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl ReactLoop for FakeHooks {
        async fn before_loop(&self, _ctx: &mut LoopContext) -> Result<(), String> {
            Ok(())
        }
        async fn loop_start(&self, _ctx: &mut LoopContext) {}
        async fn after_llm(&self, _ctx: &mut LoopContext, _result: &LlmResult) {}
        fn loop_end(&self, _ctx: &mut LoopContext) {}
        async fn after_loop(&self, _ctx: &mut LoopContext) -> Result<(), String> {
            Ok(())
        }
        fn append_message(&self, _ctx: &LoopContext, message: ChatMessage) {
            self.messages.lock().unwrap().push(message);
        }
        fn emit_event(&self, _ctx: &LoopContext, event: Value) {
            self.events.lock().unwrap().push(event);
        }
    }

    /// 假工具：`interrupt` 为真时返回中断信封（触发审批中断）。
    struct FakeTool {
        name: String,
        interrupt: bool,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn parameters_schema(&self) -> Value {
            json!({ "type": "object" })
        }
        fn interrupt_payload(&self, result: &str) -> Option<Value> {
            let v: Value = serde_json::from_str(result).ok()?;
            (v.get("interrupt").and_then(|b| b.as_bool()) == Some(true)).then_some(v)
        }
        async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<String, String> {
            if self.interrupt {
                Ok(json!({
                    "interrupt": true, "interrupt_type": "approval", "request_id": "r1"
                })
                .to_string())
            } else {
                Ok("{\"ran\":true}".to_string())
            }
        }
    }

    /// 首个调用中断时，同轮第二个调用必须落成配对 tool 消息（metadata.deferred=true）
    /// 并发 `tool_result` 事件——否则前端工具卡永远停在「正在执行工具」，
    /// 历史修复只能注入一句无原因的「未执行」。
    #[tokio::test]
    async fn interrupt_defers_remaining_tool_calls() {
        let hooks = Arc::new(FakeHooks::default());
        let engine = ReactEngine::new(Arc::new(ChatClient::new("", "")), hooks.clone());
        let mut ctx = LoopContext::new("s1", CancellationToken::new());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(FakeTool { name: "t_int".into(), interrupt: true }),
            Arc::new(FakeTool { name: "t_ok".into(), interrupt: false }),
        ];
        ctx.tools = ToolMap::from_tools(tools);

        let calls = vec![
            ToolCall::new("c0", "t_int", "{}"),
            ToolCall::new("c1", "t_ok", "{}"),
        ];
        let interrupted = engine.execute_tool_calls(&mut ctx, &calls).await.unwrap();
        assert!(interrupted, "首个调用返回中断信封 → 本轮中断");

        // 两个调用都成对落库（c0 = 中断信封，c1 = deferred 占位）
        let tool_msgs: Vec<&ChatMessage> = ctx
            .chat_request
            .messages
            .iter()
            .filter(|m| m.role == "tool")
            .collect();
        assert_eq!(tool_msgs.len(), 2, "同轮剩余调用必须落成配对 tool 消息");
        let c1 = tool_msgs
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("c1"))
            .expect("c1 的 deferred 占位");
        assert_eq!(
            c1.metadata
                .as_ref()
                .and_then(|m| m.get("deferred"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(c1.content.as_deref().unwrap_or("").contains("暂未执行"));

        let events = hooks.events.lock().unwrap();
        let c1_ev = events
            .iter()
            .find(|e| {
                e.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    && e.get("id").and_then(|i| i.as_str()) == Some("c1")
            })
            .expect("c1 必须收到 tool_result 事件（前端卡片不再悬空）");
        assert_eq!(c1_ev.get("status").and_then(|s| s.as_str()), Some("error"));
        assert_eq!(c1_ev.get("deferred").and_then(|d| d.as_bool()), Some(true));
        assert!(
            events
                .iter()
                .any(|e| e.get("type").and_then(|t| t.as_str()) == Some("interrupt")),
            "中断事件仍照发"
        );

        // 剩余调用本轮不执行
        assert!(
            ctx.chat_request
                .messages
                .iter()
                .all(|m| m.content.as_deref() != Some("{\"ran\":true}")),
            "等待审批期间不得执行后续调用"
        );
    }

    /// 覆盖 `execute_with_meta` 的工具：元数据必须平铺进 tool_result 事件与消息 metadata。
    struct MetaTool;

    #[async_trait]
    impl Tool for MetaTool {
        fn name(&self) -> &str {
            "meta"
        }
        fn description(&self) -> &str {
            "meta"
        }
        fn parameters_schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<String, String> {
            Ok("ok".to_string())
        }
        async fn execute_with_meta(
            &self,
            _params: Value,
            _ctx: &ToolContext,
        ) -> Result<(String, Option<Value>), String> {
            Ok((
                "ok".to_string(),
                Some(json!({ "file_diff": { "path": "a.rs", "additions": 1, "deletions": 2 } })),
            ))
        }
    }

    #[tokio::test]
    async fn execute_with_meta_flows_into_event_and_message() {
        let hooks = Arc::new(FakeHooks::default());
        let engine = ReactEngine::new(Arc::new(ChatClient::new("", "")), hooks.clone());
        let mut ctx = LoopContext::new("s1", CancellationToken::new());
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MetaTool)];
        ctx.tools = ToolMap::from_tools(tools);

        let calls = vec![ToolCall::new("c0", "meta", "{}")];
        engine.execute_tool_calls(&mut ctx, &calls).await.unwrap();

        // 事件平铺
        let events = hooks.events.lock().unwrap();
        let ev = events
            .iter()
            .find(|e| {
                e.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    && e.get("id").and_then(|i| i.as_str()) == Some("c0")
            })
            .expect("tool_result 事件");
        assert_eq!(
            ev.pointer("/file_diff/additions").and_then(|v| v.as_i64()),
            Some(1)
        );
        drop(events);

        // 消息 metadata 落库（冷启动可恢复）
        let msg = ctx
            .chat_request
            .messages
            .iter()
            .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("c0"))
            .expect("tool 消息");
        assert_eq!(
            msg.metadata
                .as_ref()
                .and_then(|m| m.pointer("/file_diff/deletions"))
                .and_then(|v| v.as_i64()),
            Some(2)
        );
    }
}
