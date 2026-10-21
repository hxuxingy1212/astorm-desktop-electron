# react-loop API 文档

业务无关的 ReAct 循环基础层：多轮 LLM 推理 + 工具执行引擎。

- 代码位置：`crates/react-loop`
- 依赖：`llm-provider`（消息 / 请求 / 客户端类型）、`tokio`、`tokio-util` 等
- 设计原则：**引擎无状态、与业务零耦合**——循环状态全部在 `LoopContext`，业务能力经 `ReactLoop` hook trait 注入

---

## 1. 概览

```
┌─────────────────────────────────────────────────────────┐
│                      业务层（调用方）                      │
│  实现 ReactLoop（hook）  实现 Tool（工具）  装配 LoopContext │
└──────────────────────────────┬──────────────────────────┘
                               │ 注入
┌──────────────────────────────▼──────────────────────────┐
│                    react-loop（基础层）                   │
│  ReactEngine::run(ctx) ──► 多轮循环：                     │
│  LLM 推理 → 保存回复 → 工具执行 → 下一轮                    │
│  事件（event 协议 / LoopEvent 通道）实时回调业务            │
└─────────────────────────────────────────────────────────┘
```

核心概念：

| 概念 | 说明 |
|---|---|
| `LoopContext` | 引擎与业务共享的循环状态（业务创建、引擎驱动、hook 读写） |
| `ReactLoop` | 业务 hook trait：生命周期感知点 + 公共方法，全部可写状态（`&mut LoopContext`） |
| `ReactEngine` | 循环引擎本体：LLM 客户端 + 业务 hook，`run()` 驱动循环 |
| `Tool` / `ToolMap` | 工具抽象与名称→工具映射（执行查找 + 定义导出） |
| `LoopEvent` | 工具 → 业务的**自定义事件**通道（mpsc，类型化 downcast） |
| `event` 模块 | 前端协议事件 JSON 构造（全部携带 `session_id`） |

---

## 2. 快速开始（案例取自 `tests/tests/e2e/react_loop.rs`）

### 2.1 声明依赖

```toml
[dependencies]
llm-provider = { path = "../llm-provider" }
react-loop = { path = "../react-loop" }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time"] }
tokio-util = "0.7"
```

### 2.2 定义工具（实现 `Tool`）

工具内部可通过 `ToolContext` 发送自定义事件（`ctx.emit`），接收侧按类型 downcast 消费：

```rust
use async_trait::async_trait;
use react_loop::{Tool, ToolContext};
use serde_json::Value;

/// 工具自定义事件载荷（接收侧按类型 downcast 消费）。
struct ToolProgress { tool: String, progress: u32 }

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "回显参数 text" }
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
        // 模拟耗时
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(format!("echo: {}", params["text"].as_str().unwrap_or("")))
    }
}
```

### 2.3 实现 `ReactLoop` hook（业务边界）

```rust
use llm_provider::{ChatMessage, ChatRequest};
use react_loop::{LlmResult, LoopContext, ReactLoop};
use serde_json::Value;
use std::io::Write;
use std::sync::{Arc, Mutex};

struct LoopHooks {
    events: Arc<Mutex<Vec<Value>>>,
    history: Arc<Mutex<Vec<ChatMessage>>>,
}

#[async_trait]
impl ReactLoop for LoopHooks {
    /// 循环前：把初始消息一并收集进 history（业务层通常自行持久化初始历史）
    async fn before_loop(&self, ctx: &mut LoopContext) -> Result<(), String> {
        let mut history = self.history.lock().unwrap();
        for msg in &ctx.chat_request.messages {
            history.push(msg.clone());
        }
        Ok(())
    }

    /// 消息落库（引擎产生 assistant / tool 消息时逐条调用）
    fn append_message(&self, _ctx: &LoopContext, message: ChatMessage) {
        self.history.lock().unwrap().push(message);
    }

    /// 循环后收尾
    async fn after_loop(&self, _ctx: &mut LoopContext) -> Result<(), String> { Ok(()) }

    /// 每轮启动 / 每轮收尾 / 每次 LLM 调用后（感知点，可按需扩展）
    fn loop_start(&self, _ctx: &mut LoopContext) {}
    fn loop_end(&self, _ctx: &mut LoopContext) {}
    async fn after_llm(&self, _ctx: &mut LoopContext, _result: &LlmResult) {}

    /// 事件路由：实时打印到控制台并收集
    fn emit_event(&self, _ctx: &LoopContext, event: Value) {
        println!("[event] {}", event);
        let _ = std::io::stdout().flush();
        self.events.lock().unwrap().push(event);
    }
}
```

### 2.4 装配引擎与上下文，运行循环

```rust
use llm_provider::ChatClient;
use react_loop::{LoopContext, LoopEvent, ReactEngine, ToolMap};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

let engine = ReactEngine::new(
    Arc::new(ChatClient::new(base_url, api_key)),   // LLM 客户端（llm-provider）
    hooks,                                           // 业务 hook（Arc<dyn ReactLoop>）
);

// 循环上下文
let mut ctx = LoopContext::new("session-id", CancellationToken::new());
ctx.chat_request = ChatRequest::new(vec![ChatMessage::user("请调用 echo 工具...")])
    .with_model(&model);                            // 模型名（必填）
ctx.tools = ToolMap::from_tools(vec![Arc::new(EchoTool)]);
// chat_request.tools 保持 None → 引擎每轮从 ctx.tools 实时导出定义

// 工具自定义事件通道：tx 放入 ctx.event_tx，rx 由旁路任务消费
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
    }
});

// 运行循环：事件经 hook 实时回调，结束后返回
engine.run(&mut ctx).await.unwrap();

// 运行摘要（用量 / 轮次）
println!("round={} total={}", ctx.round, ctx.total_usage.total);
```

### 2.5 输出示例（真实运行）

```text
[event] {"session_id":"e2e-session","status":"started","type":"loop_life"}
[event] {"content":"The user wants me to call the echo tool...","session_id":"e2e-session","type":"reasoning"}
[event] {"session_id":"e2e-session","tool_calls":[{"id":"call_00_xxx","name":"echo","params":{"text":"hello-from-react-loop"}}],"type":"tool_call"}
[tool-event] tool=echo progress=50
[event] {"duration_ms":501,"id":"call_00_xxx","name":"echo","result":"echo: hello-from-react-loop","session_id":"e2e-session","type":"tool_result"}
[event] {"content":"echo 工具的输出是：...","session_id":"e2e-session","type":"text"}
[event] {"session_id":"e2e-session","status":"finished","type":"loop_life"}
round=2 total=917
```

---

## 3. 核心 API

### 3.1 `LoopContext` —— 循环状态

```rust
pub struct LoopContext {
    pub session_id: String,                                  // 会话标识（事件 / 工具 / 存储路由）
    pub round: u32,                                          // 已执行轮数（引擎自增）
    pub cancel: CancellationToken,                           // 取消标识（业务注入，引擎检查）
    pub interrupt: bool,                                     // 打断标识（工具要求中断时引擎置位）
    pub total_usage: TokenUsage,                             // 累积 token 用量（含 DeepSeek cache_hit/miss）
    pub started_at: Option<Instant>,                         // 循环开始时间（引擎设置）
    pub ended_at: Option<Instant>,                           // 循环结束时间（引擎设置）
    pub metadata: HashMap<String, Value>,                    // 扩展元数据（业务自由读写）
    pub event_tx: Option<mpsc::UnboundedSender<LoopEvent>>,  // 工具自定义事件通道
    pub chat_request: ChatRequest,                           // 请求模板 + 会话对话唯一持有者
    pub tools: ToolMap,                                      // 工具映射（引擎执行查找）
    pub client: Option<Arc<ChatClient>>,                     // 本循环 LLM 客户端（多供应商路由；
                                                             // None 回落引擎构造时的客户端）
}
```

| 方法 | 签名 | 说明 |
|---|---|---|
| `new` | `(session_id, cancel) -> Self` | 创建上下文（需业务填充 messages / model / tools 等） |
| `is_cancelled` | `(&self) -> bool` | 是否已取消 |
| `is_interrupted` | `(&self) -> bool` | 是否被打断 |
| `get_meta` / `set_meta` | `(&self, key)` / `(&mut self, key, value)` | 读写扩展元数据 |

### 3.2 `ReactLoop` —— 业务 hook trait（全部 `&mut LoopContext`）

```rust
#[async_trait]
pub trait ReactLoop: Send + Sync {
    // ── 生命周期 hook（按执行阶段排序） ──────────────
    async fn before_loop(&self, ctx: &mut LoopContext) -> Result<(), String>; // 循环前（一次）
    fn loop_start(&self, ctx: &mut LoopContext);                              // 每轮启动
    async fn after_llm(&self, ctx: &mut LoopContext, result: &LlmResult);     // 每次 LLM 调用后
    fn loop_end(&self, ctx: &mut LoopContext);                                // 每轮收尾
    async fn after_loop(&self, ctx: &mut LoopContext) -> Result<(), String>;  // 循环后（一次）

    // ── 公共方法（循环任意阶段可调用） ────────────────
    fn append_message(&self, ctx: &LoopContext, message: ChatMessage);        // 消息落库
    fn emit_event(&self, ctx: &LoopContext, event: Value);                    // 事件路由
}
```

时序：

```text
loop_life("started")（引擎自推）
before_loop → [ loop_start → LLM → after_llm → 工具 → loop_end ]×N → after_loop
loop_life("finished")（引擎自推，仅成功路径）
```

### 3.3 `ReactEngine` —— 引擎本体

```rust
pub fn new(client: Arc<ChatClient>, hooks: Arc<dyn ReactLoop>) -> Self;      // 创建（无状态，可并发）
pub async fn run(&self, ctx: &mut LoopContext) -> Result<(), String>;        // 运行循环
```

- 引擎无内部状态：同一实例可对多个 `LoopContext` 并发 `run`
- 出错时立即返回 `Err`，跳过 `loop_end` / `after_loop`，由调用方处理

### 3.4 `Tool` / `ToolContext` / `ToolMap`

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn interrupt_payload(&self, _result: &str) -> Option<Value> { None }  // 结果要求中断时的载荷
    fn result_meta(&self, _ok: bool, _result: &str) -> Option<Value> { None }  // 结果元数据（不中断）
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String>;
}

pub struct ToolContext {
    pub cancel: CancellationToken,          // 取消令牌（工具可自检取消）
    pub session_id: String,                 // 会话标识
    pub tool_call_id: String,               // 本次 tool_call id（中断决议/重放定位历史 tool 消息）
    // emitter（私有）：事件通道发送端
}
// ToolContext: new(cancel) / with_session_id / with_tool_call_id / with_emitter / emit<T: Any>(data)

pub struct ToolMap { /* 名称 → Arc<dyn Tool> */ }
// new / from_tools(Vec) / register / get(name) / tool_names / is_empty / tool_defs
```

- 引擎按名实时查找执行；`tool_defs()` 每轮导出定义（未预置时）——**循环内经 hook 注册的工具下一轮对 LLM 生效**（支持动态注册）
- 未知工具：写错误消息进历史 + 发 `tool_result`（无时长），继续执行后续调用

**中断通道**：工具在**执行后**用 `interrupt_payload(result)` 声明「结果要求用户介入」
（安全拦截待审批 / 向用户提问）。命中即「结果先成对落库 → 发 `interrupt` 事件（带 `payload`）→
中止本轮循环」，故不会产生孤立的 assistant `tool_calls`。

这是 react-loop 的**唯一**中断机制：引擎只透传载荷、不解释语义；业务侧据此让前端弹审批卡
或提问表单，用户决定后经专用接口把决议写回那条 tool 消息并续跑（详见
`docs/backend-contract.md` §4.2）。不需要打断的场景（如 Auto 模式下不提问）直接让 `execute`
返回不带中断标记的文本即可。

**结果元数据通道**：`result_meta(ok, result)` 让工具为**不打断**的结果补结构化信息
（`result` 为工具自产文本：成功 = 结果原文，失败 = `Err` 原文，均未经引擎包装）。
引擎不解释语义，只做两件事：把返回对象的顶层键**平铺**进 `tool_result` 事件
（保留 `type` / `result` / `id` / `name` / `session_id` / `round` 等既有键不被覆写），
并写入 tool 消息 `metadata`（重启读历史可恢复）。可覆盖 `status`，用于「返回文本但语义是失败」
的场景（如安全拦截：`{"status":"error","denial":{…}}`）；引擎路径缺省按 `ok` 给出
`status`（`ok` / `error`），前端无需再按文本推断状态。

## 4. 约定与扩展点

1. **消息元数据契约**（随消息序列化持久化、不上送 LLM）：
   - assistant 消息：`started_at` / `ended_at`（unix 毫秒）、`thinking_ms`（思考耗时，从首个 reasoning 分片到首个 content 分片）
   - 工具结果消息：`started_at` / `ended_at`（unix 毫秒）
2. **时间戳统一 unix 毫秒**（与前端 `Date.now()` 对齐）
3. **引擎无状态**：`ReactEngine` 可安全并发；状态只在 `LoopContext`
4. **动态工具注册**：`ctx.tools.register()`（per-round hook 内）+ 不预置 `chat_request.tools` → 下一轮对 LLM 可见
5. **取消 / 中断**：`ctx.cancel` 取消令牌；`interrupt_payload` 命中即中断循环并触发
   `interrupt_with` 事件（带载荷，前端据 `payload.interrupt_type` 渲染审批卡或提问表单）
