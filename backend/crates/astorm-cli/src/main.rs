//! astorm CLI：astorm-server 的薄客户端（P0 命令集）。
//!
//! 命令：newtask / tasks / show / stop / pending / approve / deny / status。
//! 全局：`--server`（env `ASTORM_SERVER`，缺省 http://127.0.0.1:8787）、
//! `--token`（env `ASTORM_TOKEN`）、`--json`。

mod client;

use std::io::Write;
use std::time::Duration;

use clap::Parser as _;
use serde_json::{json, Value};

use crate::client::Client;

/// 待审批扫描时 SSE 读超时：静默该时长即认为回放取完（环形缓冲已尽）。
const PENDING_SCAN_TIMEOUT: Duration = Duration::from_secs(2);

/// astorm 无头服务客户端。
#[derive(clap::Parser)]
#[command(name = "astorm", version, about = "astorm 无头服务客户端（连接 astorm-server）")]
struct Cli {
    /// 服务端地址（env ASTORM_SERVER；缺省 http://127.0.0.1:8787）
    #[arg(long, global = true)]
    server: Option<String>,
    /// 访问令牌（env ASTORM_TOKEN）
    #[arg(long, global = true)]
    token: Option<String>,
    /// 输出原始 JSON（脚本消费）
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// 创建并执行任务（缺省跟随输出直到完成；--detach 只创建）
    Newtask {
        /// 任务内容
        message: String,
        /// 工作区目录（缺省用服务端默认）
        #[arg(short, long)]
        workspace: Option<String>,
        /// 模型引用 id（缺省用服务端默认模型）
        #[arg(long)]
        model: Option<String>,
        /// 供应商名
        #[arg(long)]
        provider: Option<String>,
        /// 模式：plan / execute / auto
        #[arg(long)]
        mode: Option<String>,
        /// 自动授权（permission=auto，工具不再逐次审批）
        #[arg(long)]
        auto: bool,
        /// 只创建不跟随（后台跑，用 `astorm show <id> --follow` 观战）
        #[arg(long)]
        detach: bool,
    },
    /// 列出任务
    Tasks,
    /// 查看任务（历史消息；--follow 跟随实时输出）
    Show {
        /// 任务 id（会话 id）
        id: String,
        /// 跟随实时输出直到 done / cancelled / error
        #[arg(long)]
        follow: bool,
    },
    /// 停止任务（取消当前循环）
    Stop { id: String },
    /// 列出等待审批的任务
    Pending,
    /// 同意审批（对指定任务的当前待决中断）
    Approve {
        /// 任务 id
        session: String,
        /// 授权范围：once（缺省）/ session（本会话内同目标不再询问）
        #[arg(long, default_value = "once")]
        scope: String,
    },
    /// 拒绝审批
    Deny { session: String },
    /// 服务端状态
    Status,
}

fn main() {
    let cli = Cli::parse();
    let base = cli
        .server
        .clone()
        .or_else(|| std::env::var("ASTORM_SERVER").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8787".to_string());
    let token = cli
        .token
        .clone()
        .or_else(|| std::env::var("ASTORM_TOKEN").ok());
    let client = Client::new(base, token);
    let code = match run(&client, &cli) {
        Ok(()) => 0,
        Err(e) => {
            // 跟随模式的终态错误已就地打印（空串哨兵），这里只补带前缀的意外错误
            if !e.is_empty() {
                eprintln!("astorm: {e}");
            }
            1
        }
    };
    std::process::exit(code);
}

fn run(client: &Client, cli: &Cli) -> Result<(), String> {
    match &cli.cmd {
        Cmd::Status => {
            let data = client.get("/api/v1/status")?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&data).unwrap());
                return Ok(());
            }
            println!(
                "astorm-server {}（{}-{}）",
                data["version"].as_str().unwrap_or("?"),
                data["platform"].as_str().unwrap_or("?"),
                data["arch"].as_str().unwrap_or("?"),
            );
            println!(
                "数据目录 {} · 工作空间 {}",
                data["dataDir"].as_str().unwrap_or("?"),
                data["workspace"].as_str().unwrap_or("?"),
            );
            if let Some(providers) = data["llm"]["providers"].as_array() {
                for p in providers {
                    let models: Vec<&str> = p["models"]
                        .as_array()
                        .map(|ms| ms.iter().filter_map(|m| m["id"].as_str()).collect())
                        .unwrap_or_default();
                    println!(
                        "供应商 {}（{}）模型: {}",
                        p["name"].as_str().unwrap_or("?"),
                        p["baseUrl"].as_str().unwrap_or("?"),
                        models.join(", ")
                    );
                }
            }
            Ok(())
        }
        Cmd::Newtask {
            message,
            workspace,
            model,
            provider,
            mode,
            auto,
            detach,
        } => {
            let body = json!({
                "model": model.clone().unwrap_or_default(),
                "provider": provider.clone(),
                "userMessage": message,
                "workspace": workspace.clone(),
                "sessionMode": mode.clone(),
                "permissionMode": if *auto { Some(json!("auto")) } else { None },
                "autoRun": true,
            });
            let data = client.post("/api/v1/sessions", &body)?;
            let id = data["id"].as_str().ok_or("响应缺少 id")?.to_string();
            if cli.json {
                println!("{{\"id\":\"{id}\"}}");
                return Ok(());
            }
            if *detach {
                println!("任务已创建：{id}");
                println!("观战：astorm show {id} --follow");
                return Ok(());
            }
            println!("任务已创建：{id}（Ctrl-C 停止跟随；astorm stop {id} 取消任务）");
            follow_session(client, &id, cli)
        }
        Cmd::Tasks => {
            let data = client.get("/api/v1/sessions")?;
            let sessions = data.as_array().cloned().unwrap_or_default();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&sessions).unwrap());
                return Ok(());
            }
            if sessions.is_empty() {
                println!("（无任务）");
                return Ok(());
            }
            println!("{:<38}  {:<18} {}", "ID", "状态", "标题");
            for s in &sessions {
                println!(
                    "{:<38}  {:<18} {}",
                    s["id"].as_str().unwrap_or("?"),
                    s["status"].as_str().unwrap_or("?"),
                    s["title"].as_str().unwrap_or("（无标题）"),
                );
            }
            Ok(())
        }
        Cmd::Show { id, follow } => {
            let messages = client.get(&format!("/api/v1/sessions/{id}/messages"))?;
            let list = messages.as_array().cloned().unwrap_or_default();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&messages).unwrap());
                return Ok(());
            }
            for m in &list {
                print_message(m);
            }
            if *follow {
                // 从当前消息末尾之后开始跟随（环形缓冲会回放不到的历史就靠 messages 了）
                follow_session(client, id, cli)
            } else {
                Ok(())
            }
        }
        Cmd::Stop { id } => {
            client.post_empty(&format!("/api/v1/sessions/{id}/cancel"))?;
            if cli.json {
                println!("{{\"cancelled\":true}}");
            } else {
                println!("已发送取消：{id}");
            }
            Ok(())
        }
        Cmd::Pending => {
            let data = client.get("/api/v1/sessions")?;
            let sessions = data.as_array().cloned().unwrap_or_default();
            let waiting: Vec<&Value> = sessions
                .iter()
                .filter(|s| s["status"].as_str() == Some("awaiting_approval"))
                .collect();
            if cli.json {
                let mut out = Vec::new();
                for s in &waiting {
                    let sid = s["id"].as_str().unwrap_or_default();
                    out.push(json!({ "session": sid, "interrupt": find_pending(client, sid)? }));
                }
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
                return Ok(());
            }
            if waiting.is_empty() {
                println!("（无待审批任务）");
                return Ok(());
            }
            for s in &waiting {
                let sid = s["id"].as_str().unwrap_or_default();
                println!("任务 {}（{}）", sid, s["title"].as_str().unwrap_or("（无标题）"));
                match find_pending(client, sid)? {
                    Some(p) => print_pending(&p),
                    None => println!("  （未在事件缓冲中找到待决中断，可能刚重启：查看会话轨迹确认）"),
                }
            }
            println!("处理：astorm approve <任务id> / astorm deny <任务id>");
            Ok(())
        }
        Cmd::Approve { session, scope } => {
            resolve(client, session, true, Some(scope.clone()), cli)
        }
        Cmd::Deny { session } => resolve(client, session, false, None, cli),
    }
}

/// 提交审批决议：先从事件缓冲找到待决中断的 request_id。
fn resolve(
    client: &Client,
    session: &str,
    allow: bool,
    scope: Option<String>,
    cli: &Cli,
) -> Result<(), String> {
    let pending = find_pending(client, session)?
        .ok_or_else(|| format!("任务 {session} 当前没有待决中断（astorm pending 查看）"))?;
    let request_id = pending["request_id"]
        .as_str()
        .ok_or("中断载荷缺少 request_id")?
        .to_string();
    let body = json!({
        "requestId": request_id,
        "interruptType": "approval",
        "allow": allow,
        "allowScope": scope,
    });
    client.post(&format!("/api/v1/sessions/{session}/interrupts/resolve"), &body)?;
    if cli.json {
        println!("{{\"resolved\":true,\"allow\":{allow}}}");
    } else {
        println!("已提交决议：{}（{session}）", if allow { "同意" } else { "拒绝" });
    }
    Ok(())
}

/// 从事件缓冲里找该会话最新的待决审批中断（SSE 读到静默即止）。
fn find_pending(client: &Client, session: &str) -> Result<Option<Value>, String> {
    let mut found: Option<Value> = None;
    client.sse_until_idle(
        &format!("/api/v1/sessions/{session}/events"),
        PENDING_SCAN_TIMEOUT,
        |_seq, event, data| {
            if event == "interrupt" && data.get("payload").is_some() {
                let p = &data["payload"];
                // 只认还没决议的审批类中断（approval）；表单类待决暂不走 CLI
                if p["interrupt_type"].as_str() == Some("approval") {
                    found = Some(p.clone());
                }
            }
            true
        },
    )?;
    Ok(found)
}

fn print_pending(p: &Value) {
    println!(
        "  类型 {} · request_id {}",
        p["interrupt_type"].as_str().unwrap_or("?"),
        p["request_id"].as_str().unwrap_or("?"),
    );
    if let Some(ap) = p.get("approval") {
        println!("  分类 {} · 目标 {}", ap["category"].as_str().unwrap_or("?"), ap["targets"]);
        if let Some(cmd) = ap["command"].as_str() {
            println!("  命令: {cmd}");
        }
        if let Some(reason) = ap["reason"].as_str() {
            println!("  原因: {reason}");
        }
    }
    if let Some(msg) = p.get("message").and_then(|m| m.as_str()) {
        println!("  {msg}");
    }
}

/// 跟随会话事件流：正文增量直写 stdout；done/cancelled/error 收尾。
fn follow_session(client: &Client, session: &str, cli: &Cli) -> Result<(), String> {
    let mut code: Option<i32> = None;
    client.sse(
        &format!("/api/v1/sessions/{session}/events"),
        |_seq, event, data| {
            match event {
                "text" => {
                    print!("{}", data["content"].as_str().unwrap_or(""));
                    let _ = std::io::stdout().flush();
                }
                "reasoning" => {
                    // 思考链默认不进终端正文（--json 场景在原始流里自然可见）
                }
                "tool_call" => {
                    if let Some(calls) = data["tool_calls"].as_array() {
                        for c in calls {
                            println!(
                                "\n[工具] {}",
                                c["name"].as_str().unwrap_or("?")
                            );
                        }
                    }
                }
                "interrupt" => {
                    println!(
                        "\n[等待审批] tool={} · astorm pending 查看详情，astorm approve {session} 同意",
                        data["tool_name"].as_str().unwrap_or("?")
                    );
                }
                "done" => {
                    println!("\n[完成] {session}");
                    code = Some(0);
                    return false;
                }
                "cancelled" => {
                    println!("\n[已取消] {session}");
                    code = Some(0);
                    return false;
                }
                "error" => {
                    println!(
                        "\n[错误] {}",
                        data["message"].as_str().unwrap_or("未知错误")
                    );
                    code = Some(1);
                    return false;
                }
                _ => {}
            }
            true
        },
    )?;
    if cli.json {
        // --json + 跟随的组合少见：跟随输出已是流式文本，这里仅补终态
        if let Some(c) = code {
            println!("{{\"exitCode\":{c}}}");
        }
    }
    match code {
        Some(c) if c != 0 => Err(String::new()), // 已打印错误；以非零码退出但不再打一行前缀
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

/// 渲染一条历史消息（精简文本视图）。
fn print_message(m: &Value) {
    let role = m["role"].as_str().unwrap_or("?");
    match role {
        "user" => println!("──────── 用户 ────────\n{}", m["content"].as_str().unwrap_or("")),
        "assistant" => {
            println!("──────── 助手 ────────");
            if let Some(r) = m["reasoningContent"].as_str() {
                println!("（思考）{r}");
            }
            println!("{}", m["content"].as_str().unwrap_or(""));
            if let Some(calls) = m["toolCalls"].as_array() {
                for c in calls {
                    println!("[调用工具] {}", c["name"].as_str().unwrap_or("?"));
                }
            }
        }
        "tool" => {
            // 工具结果默认折叠为一行摘要（完整内容走 --json）
            let content = m["content"].as_str().unwrap_or("");
            let brief: String = content.chars().take(120).collect();
            println!("[工具结果] {brief}{}", if content.chars().count() > 120 { "…" } else { "" });
        }
        _ => {}
    }
}
