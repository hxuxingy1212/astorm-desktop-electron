//! e2e：react_loop 基础层的完整 ReAct 循环链路（真实 LLM，手动运行）。
//!
//! **只测试 `react_loop` 包本身，不依赖任何业务层代码**：
//! - 直接装配 [`react_loop::ReactEngine`] / [`LoopContext`] / [`ToolMap`]，
//!   hook 用测试替身（事件实时打印到控制台 + 消息内存收集）
//! - 事件**实时打印**（text / reasoning / tool_call / tool_result / loop_life / done）
//! - 工具内部发送的**自定义事件**（`LoopEvent` 通道）实时打印
//! - 循环结束后，hook 收集的消息按 **JSONL** 打印（每条一行，含计时元数据）
//!
//! 配置：项目根 `.env`（LLM_BASE_URL / LLM_MODEL / LLM_API_KEY）。
//! 运行方式（手动、需要真实密钥，`--nocapture` 使实时打印可见）：
//! ```text
//! cargo test -p astorm-tests --test e2e_tests -- --ignored --nocapture
//! ```

use async_trait::async_trait;
use llm_provider::{ChatClient, ChatMessage, ChatRequest};
use react_loop::{LlmResult, LoopContext, LoopEvent, ReactEngine, ReactLoop, Tool, ToolContext, ToolMap};
use serde_json::Value;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// hook 测试替身：事件**实时**打印到控制台并收集进内存；
/// `append_message` 把消息收集进内存 Vec（模拟持久化存储）。
struct LoopHooks {
    events: Arc<Mutex<Vec<Value>>>,
    history: Arc<Mutex<Vec<ChatMessage>>>,
}

#[async_trait]
impl ReactLoop for LoopHooks {
    /// 循环前：把初始消息（测试装入 chat_request.messages）一并收集进 history，
    /// 使 JSONL 输出呈现完整对话（业务层通常自行持久化初始历史）。
    async fn before_loop(&self, ctx: &mut LoopContext) -> Result<(), String> {
        let mut history = self.history.lock().unwrap();
        for msg in &ctx.chat_request.messages {
            history.push(msg.clone());
        }
        Ok(())
    }

    /// 消息落库：收集进内存。
    fn append_message(&self, _ctx: &LoopContext, message: ChatMessage) {
        self.history.lock().unwrap().push(message);
    }

    async fn after_loop(&self, _ctx: &mut LoopContext) -> Result<(), String> {
        Ok(())
    }

    async fn loop_start(&self, _ctx: &mut LoopContext) {}

    fn loop_end(&self, _ctx: &mut LoopContext) {}

    async fn after_llm(&self, _ctx: &mut LoopContext, _result: &LlmResult) {}

    fn emit_event(&self, _ctx: &LoopContext, event: Value) {
        println!("[event] {}", event);
        let _ = std::io::stdout().flush();
        self.events.lock().unwrap().push(event);
    }
}

/// 工具自定义事件载荷（测试工具经事件通道推送，接收侧按类型 downcast 消费）。
#[derive(Debug)]
struct ToolProgress {
    tool: String,
    progress: u32,
}

/// 最小测试工具：回显参数（触发工具调用链路）。
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "回显参数 text"
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        })
    }
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        // 工具内部发送自定义事件（经 ToolContext 事件通道 → LoopEvent::custom）
        ctx.emit(ToolProgress { tool: "echo".into(), progress: 50 });
        // 模拟耗时，便于观察工具执行窗口（started_at / ended_at / duration_ms）
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(format!("echo: {}", params["text"].as_str().unwrap_or("")))
    }
}

/// 读取 e2e 配置：项目根 `.env` → 环境变量（缺失时给出指引后 panic）。
fn e2e_config() -> (String, String, String) {
    let _ = dotenvy::dotenv(); // 读项目根 .env（best-effort）
    let base_url = std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let api_key = std::env::var("LLM_API_KEY").unwrap_or_else(|_| String::new());
    assert!(
        !api_key.is_empty(),
        "缺少 LLM_API_KEY：请在项目根 .env 中配置 LLM_BASE_URL / LLM_MODEL / LLM_API_KEY 后手动运行 e2e"
    );
    (base_url, model, api_key)
}

/// 真实 LLM 的完整 ReAct 循环链路（仅 react_loop 包，手动运行）。
#[tokio::test]
#[ignore = "依赖真实 LLM 配置（项目根 .env），手动运行：cargo test -p astorm-tests --test e2e_tests -- --ignored --nocapture"]
async fn react_loop_full_flow() {
    let (base_url, model, api_key) = e2e_config();

    // ① 引擎：LLM 客户端 + hook 测试替身
    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let history: Arc<Mutex<Vec<ChatMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let hooks: Arc<dyn ReactLoop> = Arc::new(LoopHooks {
        events: events.clone(),
        history: history.clone(),
    });
    let engine = ReactEngine::new(Arc::new(ChatClient::new(base_url, api_key)), hooks);

    // ② 循环上下文：初始用户消息 + 请求模板（模型名）+ 工具映射
    let message =
        "请调用 echo 工具，参数 text 设为 hello-from-react-loop，然后告诉我输出。".to_string();
    let mut ctx = LoopContext::new("e2e-session", CancellationToken::new());
    ctx.chat_request = ChatRequest::new(vec![ChatMessage::user(message)]).with_model(&model);
    ctx.tools = ToolMap::from_tools(vec![Arc::new(EchoTool)]);
    // chat_request.tools 保持 None → 引擎每轮从 ctx.tools 实时导出定义

    // ③ 工具事件通道：tx 放入 ctx.event_tx（引擎挂到 ToolContext），
    // rx 由旁路任务消费——工具发送的自定义事件实时打印
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<LoopEvent>();
    ctx.event_tx = Some(event_tx);
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            match ev {
                LoopEvent::Custom(any) => match any.downcast_ref::<ToolProgress>() {
                    Some(p) => println!("[tool-event] tool={} progress={}", p.tool, p.progress),
                    None => println!("[tool-event] custom(未知类型)"),
                },
                LoopEvent::Json(v) => println!("[tool-event] {}", v),
            }
            let _ = std::io::stdout().flush();
        }
    });

    // ④ 运行循环：事件经 hook 实时打印，结束后返回（180s 兜底超时）
    println!("[run] 开始执行 ReAct 循环（模型: {}）", model);
    let run = tokio::time::timeout(Duration::from_secs(180), engine.run(&mut ctx));
    run.await.expect("ReAct 循环超时（180s），检查 .env 配置与网络").unwrap();

    // ⑤ 运行摘要
    println!(
        "[summary] round={} prompt={} completion={} total={}",
        ctx.round,
        ctx.total_usage.prompt,
        ctx.total_usage.completion,
        ctx.total_usage.total,
    );

    // ⑥ 按 JSONL 打印内存持久化的消息（每条一行，含计时元数据）
    println!("[history-jsonl]");
    let history_guard = history.lock().unwrap();
    for msg in history_guard.iter() {
        println!("{}", serde_json::to_string(msg).unwrap_or_else(|_| "{}".into()));
    }
    let _ = std::io::stdout().flush();

    // ⑦ 轻量断言：历史含 assistant 回复 + 收到 loop_life started / finished
    assert!(
        history_guard.iter().any(|m| m.role == "assistant"),
        "消息中应包含 assistant 回复"
    );
    let all_events = events.lock().unwrap();
    assert!(
        all_events.iter().any(|e| e["type"] == "loop_life" && e["status"] == "started")
            && all_events.iter().any(|e| e["type"] == "loop_life" && e["status"] == "finished"),
        "应收到 loop_life started / finished 事件"
    );
}
