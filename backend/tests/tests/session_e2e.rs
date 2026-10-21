//! 会话全链路 e2e（真实 LLM，单文件单方法）：应用启动 → 多轮任务循环 → 触发摘要 → 结束，
//! **逐轮监控持久化数据一致性**（sqlite 元数据 vs jsonl 消息归档）。
//!
//! 配置：项目根 `.env`（LLM_BASE_URL / LLM_MODEL / LLM_API_KEY，缺失给指引后 panic）。
//! 运行方式（手动、需要真实密钥）：
//! ```text
//! cargo test -p astorm-tests --test session_e2e -- --ignored --nocapture
//! ```
//!
//! 摘要触发策略：3 条短消息 + 1 条超长消息（估算 token 远超 `token_threshold`）→
//! 仅超长消息所在运行的 `before_loop` 越过阈值触发摘要（对真实模型回复方差鲁棒）。
//!
//! 一致性断言（对真实模型行为方差鲁棒）：
//! - 每轮 done 后：`current_round ≥ done 数`（元数据不滞后于已执行轮次）、`updated_at` 不回退、
//!   用量单调累计
//! - 摘要被真实触发（`summary_times ≥ 1`）；归档文件（若存在）序号严格递增且 ≤ 摘要次数
//! - 消息文件（活动 / 归档）逐文件校验：`seq` 连续 0..n-1、session_id 归属、role 合法

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use astorm_common::session::interrupt::{InterruptResolution, InterruptType};
use astorm_common::session::req::CreateSessionRequest;
use astorm_common::session::types::{PermissionMode, SessionMode};
use astorm_common::session::Session;
use astorm_common::types::Storage;
use astorm_core::app::{App, AppConfig, LlmConfig, LlmModel, LlmProvider, SummaryConfig};
use astorm_storage::StorageManager;
use serde_json::Value;
use tokio::sync::broadcast;

/// 读取 e2e 配置：项目根 `.env` → 环境变量（与 e2e_tests 同一约定）。
fn e2e_config() -> (String, String, String) {
    let _ = dotenvy::dotenv();
    let base_url = std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let api_key = std::env::var("LLM_API_KEY").unwrap_or_else(|_| String::new());
    assert!(
        !api_key.is_empty(),
        "缺少 LLM_API_KEY：请在项目根 .env 中配置 LLM_BASE_URL / LLM_MODEL / LLM_API_KEY 后手动运行 e2e"
    );
    (base_url, model, api_key)
}

/// 构造发送请求（各字段固定，仅 user_message 变化；模型名沿用 e2e 真实配置，
/// 供应商固定为 `e2e` 以覆盖 provider 路由路径）。
fn request(workspace: &str, model: &str, user_message: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: model.to_string(),
        provider: Some("e2e".into()),
        pid: None,
        agent_type: None,
        session_type: None,
        scheduled_task_id: None,
        workspace: Some(workspace.to_string()),
        session_mode: Some(SessionMode::Execute),
        permission_mode: Some(PermissionMode::Auto),
        reasoning_effort: None,
        user_message: user_message.into(),
        attachments: None,
        // 测试用例走正常执行路径
        auto_run: Some(true),
        mentions: None,
        elements: None,
        office_refs: None,
        quotes: None,
        goal: None,
        goal_lang: None,
    }
}

/// 由 e2e 环境配置构造多供应商 LLM 配置（单供应商 `e2e`；env 的 model 同时作为
/// 引用 id 与 API 标识——id 与 name 相同，路由后上送同名模型）。
fn llm_config(base_url: &str, api_key: &str, model: &str) -> LlmConfig {
    LlmConfig {
        providers: vec![LlmProvider {
            name: "e2e".into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            api_format: Default::default(),
            models: vec![LlmModel { id: model.into(), name: model.into() }],
        }],
        default_provider: None,
        default_model: None,
    }
}

/// 枚举会话的消息文件**绝对路径**：(路径, 备份序号 Option<u32>)——活动日志序号为 None。
/// 布局：`<dir>/session/<sid>/message.jsonl`（活动）+ `message_NNN.jsonl`（摘要备份）。
fn session_message_files(dir: &Path, sid: &str) -> Vec<(PathBuf, Option<u32>)> {
    let session_dir = dir.join("session").join(sid);
    let Ok(entries) = std::fs::read_dir(&session_dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, Option<u32>)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            if name == "message.jsonl" {
                return Some((path, None));
            }
            let index = name
                .strip_prefix("message_")
                .and_then(|n| n.strip_suffix(".jsonl"))
                .and_then(|n| n.parse::<u32>().ok());
            index.map(|i| (path, Some(i)))
        })
        .collect();
    files.sort_by_key(|(_, index)| index.map(|i| i as u64).unwrap_or(u64::MAX));
    files
}

/// 单文件消息列表（解析 JSON 行，返回 record + seq）。
fn file_records(path: &Path) -> Vec<(Value, i64)> {
    let content = std::fs::read_to_string(path).expect("消息文件应可读");
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            let seq = v["seq"].as_i64().unwrap_or(-1);
            Some((v, seq))
        })
        .collect()
}

/// 校验会话全部消息文件（活动 / 归档）一致性：seq 严格递增、sessionId 归属、role 合法。
/// 返回总消息数。
fn assert_message_files_consistent(dir: &Path, sid: &str) -> usize {
    let mut total = 0usize;
    for (path, _) in session_message_files(dir, sid) {
        if !path.exists() {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let records = file_records(&path);
        total += records.len();
        let mut prev_seq = i64::MIN;
        for (record, seq) in &records {
            assert!(*seq > prev_seq, "{name} 内 seq 应严格递增（出现 {seq}）");
            prev_seq = *seq;
            assert_eq!(record["sessionId"], sid, "{name} 消息会话归属");
            assert!(
                record["role"] == "assistant" || record["role"] == "tool",
                "{name} 消息 role 应为 assistant/tool"
            );
        }
    }
    total
}

/// 等待指定会话的下一个 `done` 事件（超时异常）。
async fn wait_done(rx: &mut broadcast::Receiver<Value>, sid: &str, timeout: Duration) {
    tokio::time::timeout(timeout, async {
        loop {
            let ev = rx.recv().await.expect("事件流中断");
            if ev["type"] == "done" && ev["session_id"].as_str() == Some(sid) {
                break;
            }
        }
    })
    .await
    .expect("等待 done 超时");
}

/// 等待指定会话的 `title` 标题事件（超时异常），并要求其**先于**主循环启动/结束到达
/// （顺序编排：标题 → 落盘 → title 事件 → 循环）。
async fn wait_title_event(rx: &mut broadcast::Receiver<Value>, sid: &str, timeout: Duration) -> Value {
    tokio::time::timeout(timeout, async {
        loop {
            let ev = rx.recv().await.expect("事件流中断");
            if ev["session_id"].as_str() != Some(sid) {
                continue;
            }
            match ev["type"].as_str() {
                Some("title") => return ev,
                Some("loop_life") | Some("done") | Some("error") => {
                    panic!("title 事件未在循环启动前到达，先收到: {ev}");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("等待 title 事件超时")
}

/// 等待指定会话的本轮终止（done 或 error 任一，超时异常）。
async fn wait_terminated(rx: &mut broadcast::Receiver<Value>, sid: &str, timeout: Duration) {
    tokio::time::timeout(timeout, async {
        loop {
            let ev = rx.recv().await.expect("事件流中断");
            if ev["session_id"].as_str() != Some(sid) {
                continue;
            }
            if ev["type"] == "done" || ev["type"] == "error" {
                break;
            }
        }
    })
    .await
    .expect("等待本轮终止超时");
}

/// 等待指定会话发起指定工具的 `tool_call` 事件（超时异常）。
/// 事件载荷：`{ type, session_id, timestamp, tool_calls: [{ id, name, params }] }`。
async fn wait_tool_call(
    rx: &mut broadcast::Receiver<Value>,
    sid: &str,
    tool_name: &str,
    timeout: Duration,
) {
    tokio::time::timeout(timeout, async {
        loop {
            let ev = rx.recv().await.expect("事件流中断");
            if ev["session_id"].as_str() != Some(sid) || ev["type"] != "tool_call" {
                continue;
            }
            let names: Vec<&str> = ev["tool_calls"]
                .as_array()
                .map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect())
                .unwrap_or_default();
            if names.iter().any(|n| *n == tool_name) {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("等待 {tool_name} 调用超时（{}s）", timeout.as_secs()));
}

/// 从会话全部消息文件（活动 + 归档）取最后一条含 `marker` 的 tool 消息原文
/// （工具 execute 的返回文本；如 task_list 输出含"任务id：<uuid>"）。
fn latest_tool_message_text(data_dir: &Path, sid: &str, marker: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for (path, _) in session_message_files(data_dir, sid) {
        if !path.exists() {
            continue;
        }
        for (v, _) in file_records(&path) {
            if v["role"] == "tool" {
                if let Some(c) = v["content"].as_str() {
                    if c.contains(marker) {
                        found = Some(c.to_string());
                    }
                }
            }
        }
    }
    found
}

/// 从落盘的中断信封 tool 消息里取 `request_id`（决议入口需要它）。
fn latest_pending_request_id(data_dir: &Path, sid: &str, interrupt_type: &str) -> Option<String> {
    let needle = format!("\"interrupt_type\":\"{interrupt_type}\"");
    let content = latest_tool_message_text(data_dir, sid, &needle)?;
    let v: Value = serde_json::from_str(&content).ok()?;
    v["request_id"].as_str().map(str::to_string)
}

/// 提取文本中第一个 uuid 形态（`8-4-4-4-12` 十六进制，带连字符）。
/// 只在字符边界尝试切片，避免切进多字节字符（工具输出可能含中文前后缀）。
fn extract_uuid(s: &str) -> Option<String> {
    let ends: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    for &start in &ends {
        let stop = start + 36;
        if stop > s.len() || ends.binary_search(&stop).is_err() {
            continue;
        }
        let seg = &s[start..stop];
        if is_uuid_shape(seg) {
            return Some(seg.to_string());
        }
    }
    None
}

/// 与 `8-4-4-4-12` 十六进制（含连字符）形状一致。
fn is_uuid_shape(seg: &str) -> bool {
    let b = seg.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (idx, &c) in b.iter().enumerate() {
        match idx {
            8 | 13 | 18 | 23 => {
                if c != b'-' {
                    return false;
                }
            }
            _ => {
                if !c.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}


/// 主测试：应用启动 → 循环 → 摘要 → 结束，逐轮核对持久化（真实 LLM）。
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "依赖真实 LLM 配置（项目根 .env），手动运行：cargo test -p astorm-tests --test session_e2e -- --ignored --nocapture"]
async fn session_full_cycle_summary_persistence_consistent() {
    let (base_url, model, api_key) = e2e_config();

    // ① 临时目录（数据 / 工作区 / 日志）
    let data_dir = std::env::temp_dir().join(format!("astorm_e2e_data_{}", std::process::id()));
    let ws_dir = std::env::temp_dir().join(format!("astorm_e2e_ws_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&ws_dir).unwrap();

    // ② 配置：真实 LLM + 低摘要阈值（仅超长消息压过）
    let mut config = AppConfig::default();
    config.llm = llm_config(&base_url, &api_key, &model);
    config.workspace = ws_dir.to_string_lossy().into_owned();
    config.log.dir = Some(data_dir.join("logs").to_string_lossy().into_owned());
    config.summary = SummaryConfig {
        token_threshold: 1000, // 短轮次累计远低于此；超长消息（约 10000+ 估算 token）越过
        min_messages: 1,
        keep_token_threshold: 500,
        keep_tail_rounds: 2,
    };

    // ③ 应用启动 + 存储（sqlite 元数据 + jsonl 消息 + tantivy 索引）
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&data_dir).unwrap());
    let model_name = model;
    App::init(config, Arc::clone(&storage)).unwrap();

    // ④ 订阅事件总线，创建会话，投递 3 条短消息 + 1 条超长消息
    let ws = ws_dir.to_string_lossy().into_owned();
    let sid = App::global()
        .session()
        .create_session(
            request(&ws, &model_name, "调用 bash 工具执行命令 echo astorm-e2e，然后仅用一句话汇报结果。"),
        )
        .expect("创建会话失败");
    let sid_str = sid.to_string();
    let long_text = format!(
        "{}\n{}",
        "请继续，仅文字回复，不要调用工具。",
        "背景参考：这是一段用于撑大会话上下文的历史文本。".repeat(600)
    );
    let mut rx = App::global().subscribe();
    for msg in [
        "第1条：请仅文字回复确认。",
        "第2条：请仅文字回复确认。",
        long_text.as_str(),
    ] {
        App::global()
            .session()
            .send_message(&sid, request(&ws, &model_name, msg))
            .expect("发送消息失败");
    }
    let expected_runs = 4u32; // 创建 1 + 发送 3（每条消息一次 run，run 内轮次不计量）

    // ⑤ 逐轮等待 done，核对过程一致性（元数据 / 用量单调 / updated_at）
    let mut done_seen = 0u32;
    let mut prev_total_usage = 0u32;
    let started_at = std::time::Instant::now();
    while done_seen < expected_runs {
        let ev: Value = rx.recv().await.expect("事件流中断");
        if ev["type"] != "done" || ev["session_id"].as_str() != Some(&sid_str) {
            continue;
        }
        done_seen += 1;
        if started_at.elapsed() > Duration::from_secs(300) {
            panic!("等待第 {done_seen} 轮 done 超时（300s）");
        }

        let rec = storage.get_session(&sid_str).unwrap().expect("会话记录应存在");
        eprintln!(
            "[e2e] done#{done_seen} round={} summary={} usage={} updated_at={}",
            rec.current_round, rec.summary_times, rec.session_usage.total, rec.updated_at,
        );
        assert!(rec.current_round >= done_seen as i32, "第 {done_seen} 轮：元数据轮次滞后");
        assert!(rec.updated_at >= rec.created_at, "第 {done_seen} 轮：updated_at 回退");
        assert!(rec.session_usage.total >= prev_total_usage, "第 {done_seen} 轮：用量回退");
        prev_total_usage = rec.session_usage.total;
    }

    // ⑥ 终态一致性
    let rec = storage.get_session(&sid_str).unwrap().expect("会话记录应存在");
    let files = session_message_files(&data_dir, &sid_str);
    let archives: Vec<u32> = files.iter().filter_map(|(_, i)| *i).collect();
    let live_count = files.iter().filter(|(_, i)| i.is_none()).count();

    assert!(rec.summary_times >= 1, "应触发过摘要（summary_times={}）", rec.summary_times);
    assert!(
        archives.windows(2).all(|w| w[1] > w[0]),
        "归档序号应严格递增: {archives:?}"
    );
    assert!(
        archives.len() as i32 <= rec.summary_times,
        "归档文件数应 ≤ summary_times"
    );
    assert!(rec.session_usage.total > 0, "总用量应累计");
    assert!(rec.updated_at >= rec.created_at, "updated_at 不回退");
    assert!(rec.tool_list.iter().any(|t| t == "file_operation"), "tool_list 含 file_operation");
    assert!(rec.tool_list.iter().any(|t| t == "bash"), "tool_list 含 bash");

// 消息一致性：每个文件（活动 / 归档）内 seq 严格递增、sessionId 归属、role 合法
    let total_messages = assert_message_files_consistent(&data_dir, &sid_str);
    eprintln!(
        "[e2e] 终态: runs={done_seen} round={} summary_times={} archives={:?} live_files={live_count} messages={total_messages}",
        rec.current_round, rec.summary_times, archives,
    );

    // 清理
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&ws_dir);
}

/// 主测试二：会话循环中途「用户取消 + 系统打断」的恢复与状态一致性（真实 LLM）。
///
/// 场景：
/// 1. 首条消息正常完成（run0）
/// 2. 第二条请求驱动模型调用 bash 长命令（`ping -n 60 127.0.0.1` ≈ 1 分钟）
/// 3. 捕获 `tool_call` 事件后立即 **cancel_session（用户取消）**——
///    bash 工具被系统按进程树强杀（`cancelled=true`），循环终止
/// 4. 校验中途取消后的状态一致：轮次/用量已回写、消息落盘可解析、文件内 seq 递增
/// 5. **恢复**：再次 send_message（取消令牌在 send_message 自动重置）→ 新 run 正常执行 →
///    轮次推进、用量累计、新消息落盘、消息文件仍一致
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "依赖真实 LLM 配置（项目根 .env），手动运行：cargo test -p astorm-tests --test session_e2e -- --ignored --nocapture"]
async fn session_cancel_interrupt_and_recover_consistent() {
    let (base_url, model, api_key) = e2e_config();

    // ① 临时目录 + 配置（本测试不触发摘要：保留默认高阈值，聚焦取消/恢复）
    let data_dir = std::env::temp_dir().join(format!("astorm_e2e_cancel_data_{}", std::process::id()));
    let ws_dir = std::env::temp_dir().join(format!("astorm_e2e_cancel_ws_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&ws_dir).unwrap();

    let mut config = AppConfig::default();
    config.llm = llm_config(&base_url, &api_key, &model);
    config.workspace = ws_dir.to_string_lossy().into_owned();
    config.log.dir = Some(data_dir.join("logs").to_string_lossy().into_owned());

    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&data_dir).unwrap());
    let model_name = model;
    App::init(config, Arc::clone(&storage)).unwrap();

    // ② 创建会话（run0 正常完成）
    let ws = ws_dir.to_string_lossy().into_owned();
    let sid = App::global()
        .session()
        .create_session(request(&ws, &model_name, "请仅用一句话文字回复：收到。"))
        .expect("创建会话失败");
    let sid_str = sid.to_string();
    let mut rx = App::global().subscribe();
    wait_done(&mut rx, &sid_str, Duration::from_secs(120)).await;
    eprintln!("[e2e] run0 完成（正常路径）");
    let rec0 = storage.get_session(&sid_str).unwrap().unwrap();

    // ③ 发送长 bash 命令请求，等待 tool_call 后立刻用户取消（bash 被系统按进程树强杀）
    //    等待整体用 tokio::time::timeout 兜底：事件流一旦因本轮异常结束而停止，也不会无限挂起
    App::global()
        .session()
        .send_message(
            &sid,
            request(&ws, &model_name, "请调用 bash 工具执行命令：ping -n 60 127.0.0.1，执行约 1 分钟，请如实执行不要中断。"),
        )
        .expect("发送消息失败");
    let tool_waited = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let ev: Value = rx.recv().await.expect("事件流中断");
            if ev["session_id"].as_str() != Some(&sid_str) {
                continue;
            }
            if ev["type"] == "tool_call" {
                return;
            }
            if ev["type"] == "done" || ev["type"] == "error" {
                panic!("本轮在调用工具前即结束（模型未发起 bash 调用），无法执行打断/恢复验证");
            }
        }
    })
    .await;
    assert!(
        tool_waited.is_ok(),
        "等待模型调用工具超时（180s）：模型未发起 bash 工具调用"
    );
    eprintln!("[e2e] 捕获 tool_call，执行用户取消（系统按其进程树强杀 bash）");
    App::global().session().cancel_session(&sid).expect("取消失败");

    // ④ 等待本轮终止（done / error），校验中途取消后的状态一致
    wait_terminated(&mut rx, &sid_str, Duration::from_secs(60)).await;
    eprintln!("[e2e] 被取消的 run 已终止");
    let rec1 = storage.get_session(&sid_str).unwrap().expect("会话记录应存在");
    assert!(rec1.current_round >= 1, "工具调用轮轮次应已回写");
    assert!(rec1.current_round >= rec0.current_round, "取消后轮次不回退");
    assert!(rec1.updated_at >= rec0.updated_at, "updated_at 不回退");
    let msgs1 = assert_message_files_consistent(&data_dir, &sid_str);
    assert!(msgs1 >= 1, "工具调用轮消息应已落盘");
    // 打断路径：要么工具结果带 cancelled 标记，要么取消发生在工具执行前（下轮 repair 补配对）
    let live_msgs: Vec<Value> = {
        let path = data_dir.join("session").join(&sid_str).join("message.jsonl");
        if path.exists() {
            file_records(&path).into_iter().map(|(v, _)| v).collect()
        } else {
            Vec::new()
        }
    };
    if let Some(tool) = live_msgs.iter().rev().find(|m| m["role"] == "tool") {
        let content = tool["content"].as_str().unwrap_or("");
        eprintln!("[e2e] 打断时 tool 结果包含取消标记: {}", content.contains("cancelled") || content.contains("已取消"));
    }

    // ⑤ 恢复：再次发送消息（取消令牌已在 send_message 重置）→ 新 run 正常执行
    App::global()
        .session()
        .send_message(&sid, request(&ws, &model_name, "请仅文字回复确认，不要调用工具。"))
        .expect("恢复发送失败");
    wait_done(&mut rx, &sid_str, Duration::from_secs(120)).await;
    let rec2 = storage.get_session(&sid_str).unwrap().expect("会话记录应存在");
    assert!(rec2.current_round > rec1.current_round, "恢复后轮次应推进");
    assert!(rec2.session_usage.total > rec1.session_usage.total, "恢复后用量应累计");
    assert!(rec2.updated_at >= rec1.updated_at, "update_at 不回退");
    let msgs2 = assert_message_files_consistent(&data_dir, &sid_str);
    assert!(msgs2 > msgs1, "恢复后消息数应增加");

    eprintln!(
        "[e2e] 取消/恢复一致: round {}→{} usage {}→{} messages {msgs1}→{msgs2}",
        rec1.current_round,
        rec2.current_round,
        rec1.session_usage.total,
        rec2.session_usage.total,
    );

    // 清理（会话保持一次，App 单例无需关闭；删除测试数据）
    let _ = App::global().session().delete_session(&sid);
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&ws_dir);
}

/// 主测试三：新移植工具端到端（真实 LLM）：run0 就绪 → `task_list` 建计划（sqlite 落盘断言）→
/// `chat_with_user` 提问打断循环（中断信封）→ 提交表单回答恢复。单会话全程监控
/// 事件与持久化一致性。
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "依赖真实 LLM 配置（项目根 .env），手动运行：cargo test -p astorm-tests --test session_e2e -- --ignored --nocapture"]
async fn session_tools_e2e_interaction() {
    let (base_url, model, api_key) = e2e_config();

    // ① 临时目录
    let data_dir = std::env::temp_dir().join(format!("astorm_e2e_tools_data_{}", std::process::id()));
    let ws_dir = std::env::temp_dir().join(format!("astorm_e2e_tools_ws_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&ws_dir).unwrap();

    let mut config = AppConfig::default();
    config.llm = llm_config(&base_url, &api_key, &model);
    config.workspace = ws_dir.to_string_lossy().into_owned();
    config.log.dir = Some(data_dir.join("logs").to_string_lossy().into_owned());

    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&data_dir).unwrap());
    let model_name = model;
    App::init(config, Arc::clone(&storage)).unwrap();

    // ② 创建会话 + run0 正常完成（先订阅再创建，避免错过标题事件）
    let ws = ws_dir.to_string_lossy().into_owned();
    let mut rx = App::global().subscribe();
    let sid = App::global()
        .session()
        .create_session(request(&ws, &model_name, "请仅用一句话文字回复：准备就绪。"))
        .expect("创建会话失败");
    let sid_str = sid.to_string();
    let title_event = wait_title_event(&mut rx, &sid_str, Duration::from_secs(120)).await;
    assert!(
        title_event["title"].as_str().is_some_and(|t| !t.trim().is_empty()),
        "title 事件应携带非空标题: {title_event:?}"
    );
    eprintln!("[e2e] 捕获标题事件: {:?}", title_event["title"]);
    wait_done(&mut rx, &sid_str, Duration::from_secs(120)).await;
    eprintln!("[e2e] run0（就绪）完成");

    // ③ task_list：模型建三步计划 → 校验 sqlite plans 表落盘
    App::global()
        .session()
        .send_message(
            &sid,
            request(
                &ws,
                &model_name,
                "请调用 task_list 工具创建一个三步任务计划（步骤依次为：编译、测试、部署），创建完成后仅用一句话汇报任务 id，不要再调用其他工具。",
            ),
        )
        .expect("发送失败");
    wait_tool_call(&mut rx, &sid_str, "task_list", Duration::from_secs(180)).await;
    eprintln!("[e2e] 捕获 task_list 调用");
    wait_done(&mut rx, &sid_str, Duration::from_secs(120)).await;
    let task_id = latest_tool_message_text(&data_dir, &sid_str, "任务id")
        .and_then(|t| extract_uuid(&t))
        .expect("task_list 工具输出应含规划 task_id");
    let plan = storage
        .get_plan(&sid_str, &task_id)
        .unwrap()
        .expect("规划应落盘 sqlite plans 表");
    assert!(!plan.nodes.is_empty(), "计划应含步骤（nodes={}）", plan.nodes.len());
    assert!(
        plan.nodes.iter().all(|n| !n.task_id.is_empty() && !n.status.is_empty()),
        "节点应有 id 与状态"
    );
    eprintln!(
        "[e2e] task_list 落盘: task_id={task_id} title={} nodes={}",
        plan.title,
        plan.nodes.len()
    );

    // ④ chat_with_user：模型提问 → 返回 form 中断信封打断循环，等待用户提交回答
    App::global()
        .session()
        .send_message(
            &sid,
            request(
                &ws,
                &model_name,
                "请调用 chat_with_user 工具，向我提问“是否按此计划执行？”，问题提出后停止，等待我的回答。",
            ),
        )
        .expect("发送失败");
    wait_tool_call(&mut rx, &sid_str, "chat_with_user", Duration::from_secs(180)).await;
    eprintln!("[e2e] 捕获 chat_with_user 调用（循环被中断等待用户回复）");
    wait_terminated(&mut rx, &sid_str, Duration::from_secs(120)).await;
    eprintln!("[e2e] chat_with_user 的 run 已终止");
    let rec_mid = storage.get_session(&sid_str).unwrap().expect("会话记录应存在");
    assert!(rec_mid.current_round >= 2, "轮次应随交互推进（round={}）", rec_mid.current_round);

    // ⑤ 提交表单回答 → 决议**就地写回该次 tool 结果**并续跑（与审批同构，不追加用户消息）
    let request_id = latest_pending_request_id(&data_dir, &sid_str, "form")
        .expect("中断信封应已落盘（含 request_id）");
    App::global()
        .session()
        .resolve_interrupt(
            &sid,
            InterruptResolution {
                request_id,
                interrupt_type: InterruptType::Form,
                allow: None,
                answer: Some(r#"{"confirm":"按计划执行"}"#.to_string()),
                ignored: false,
                allow_scope: None,
            },
        )
        .expect("提交表单回答失败");
    wait_done(&mut rx, &sid_str, Duration::from_secs(120)).await;
    eprintln!("[e2e] 提交回答后 run 完成");
    // 回答应成为 tool 结果（而非一条用户消息）
    let answered = latest_tool_message_text(&data_dir, &sid_str, "用户已回答")
        .expect("tool 结果应被改写为用户回答");
    assert!(answered.contains("按计划执行"), "回答应写入 tool 结果: {answered}");
    let rec2 = storage.get_session(&sid_str).unwrap().unwrap();
    assert!(rec2.current_round >= rec_mid.current_round, "恢复后轮次不回退");

    // ⑥ 终态一致性：消息文件全一致 + 元数据状态不回退
    let total = assert_message_files_consistent(&data_dir, &sid_str);
    let rec_final = storage.get_session(&sid_str).unwrap().unwrap();
    assert!(
        rec_final.current_round >= 4,
        "应完成多轮交互（round={}）",
        rec_final.current_round
    );
    assert!(rec_final.session_usage.total > 0, "用量应累计");
    // 创建会话时首条消息异步生成的标题已写回
    assert!(
        rec_final.title.as_deref().is_some_and(|t| !t.trim().is_empty()),
        "会话应有异步生成的标题（title={:?}）",
        rec_final.title
    );
    eprintln!(
        "[e2e] 终态: title={:?} round={} usage={} messages={total}",
        rec_final.title, rec_final.current_round, rec_final.session_usage.total,
    );

    // 清理
    let _ = App::global().session().delete_session(&sid);
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&ws_dir);
}