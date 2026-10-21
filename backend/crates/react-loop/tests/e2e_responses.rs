//! e2e: /responses(OpenAI Responses API)端点经 reqwest 客户端的行为矩阵。
//!
//! set LLM_BASE_URL=... && set LLM_MODEL=deepseek-v4-flash && set LLM_API_KEY=... &&\
//! cargo test -p react-loop --test e2e_responses -- --ignored --nocapture
//! 对照: undici(node)同 body 可 200 且 output_text.delta 与 reasoning_text.delta 双双完整(实测)。

use serde_json::json;

const USER_MSG: &str = "请用 Markdown 格式回答（标题、加粗、列表、代码块）：帮我总结 Vue3 与 React 的核心差异。";

async fn probe(label: &str, client: &reqwest::Client, base: &str, key: &str, model: &str) {
    let body = json!({
        "model": model,
        "stream": true,
        "input": [{ "role": "user", "content": USER_MSG }],
        "tools": [
            { "type": "function", "name": "bash", "description": "执行 shell 命令", "parameters": json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}) },
            { "type": "function", "name": "file_operation", "description": "文件操作", "parameters": json!({"type":"object","properties":{"operation":{"type":"string"},"path":{"type":"string"}},"required":["operation","path"]}) }
        ]
    });
    let resp = match client
        .post(format!("{base}/responses"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            println!("[{label}] req err: {e}");
            return;
        }
    };
    let status = resp.status();
    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            println!("[{label}] read err {e}");
            return;
        }
    };
    let output = text.matches("response.output_text.delta").count();
    let reasoning = text.matches("response.reasoning_text.delta").count();
    let completed = text.contains("response.completed");
    println!(
        "[{label}] http={status} output_text.delta={output} reasoning_text.delta={reasoning} completed={completed} bytes={} body_preview={}",
        text.len(),
        if status.is_success() { "-" } else { &text[..text.len().min(240)] }
    );
}

#[tokio::test]
#[ignore]
async fn responses_endpoint_matrix() {
    let key = std::env::var("LLM_API_KEY").unwrap_or_default();
    if key.is_empty() {
        eprintln!("SKIP: LLM_API_KEY 未设置");
        return;
    }
    let base = std::env::var("LLM_BASE_URL").unwrap_or_default();
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());
    println!("=== base={base} model={model} ===");

    let default_c = reqwest::Client::new();
    let h1_c = match reqwest::Client::builder().http1_only().build() {
        Ok(c) => c,
        Err(e) => {
            println!("h1 build err {e}");
            return;
        }
    };

    probe("reqwest-default", &default_c, &base, &key, &model).await;
    probe("reqwest-h1", &h1_c, &base, &key, &model).await;
    // 再各跑一次确认稳定
    probe("reqwest-default-2", &default_c, &base, &key, &model).await;
    probe("reqwest-h1-2", &h1_c, &base, &key, &model).await;

    // 工具调用回归：要求执行 shell 命令 → 必须产出 ToolCalls 事件(验证 function_call 事件映射)
    println!("=== tool-call 回归(经 chat_stream,应收到 ToolCalls) ===");
    let tool_body = json!({
        "model": model,
        "stream": true,
        "input": [{ "role": "user", "content": "请调用 bash 工具运行命令 `echo hello-from-responses`,然后汇报输出。" }],
        "tools": [
            { "type": "function", "name": "bash", "description": "在会话工作区执行 shell 命令。", "parameters": json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}) }
        ]
    });
    for attempt in 0..2 {
        let resp = match default_c
            .post(format!("{base}/responses"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {key}"))
            .json(&tool_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                println!("[toolcall{attempt}] req err {e}");
                continue;
            }
        };
        let status = resp.status();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                println!("[toolcall{attempt}] read err {e}");
                continue;
            }
        };
        let has_call_added = text.contains("\"type\":\"function_call\"");
        let has_args_delta = text.contains("response.function_call_arguments.delta");
        let has_done = text.contains("response.output_item.done");
        let has_completed = text.contains("response.completed");
        let bash_args = ["echo hello-from-responses", "hello-from-responses", "echo"]
            .iter()
            .any(|k| text.contains(k));
        println!(
            "[toolcall{attempt}] http={status} function_call_item={has_call_added} args_delta={has_args_delta} item_done={has_done} completed={has_completed} bash_args_present={bash_args} bytes={}",
            text.len()
        );
    }
}