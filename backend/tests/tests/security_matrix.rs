//! 执行模式（Plan / Execute / Auto）× 审批策略（Manual / Auto）安全矩阵全流程集成测试。
//!
//! **真实执行**（无 LLM 依赖、无 mock）：直接驱动 `file_operation` / `bash` 工具与
//! `send_message` 审批标记链路，覆盖：
//! - 会话空间安全矩阵：空间内外读写的放行 / 审批 / 硬拒（六种模式组合）
//! - 中断全链路（审批与提问同构）：拦截 → 中断信封纯 JSON（`interrupt_type` / `request_id` /
//!   `tool_call_id`）→ 冷启动由历史 `metadata.interrupt` 恢复台账 → `resolve_interrupt` 决议 →
//!   同意后**后端自动重放**并就地替换历史 tool 消息 / 拒绝写拒绝结果 / 取代 / 重放中崩溃保守报未知
//! - 提问链路：回答即 tool 结果（不追加用户消息）/ 忽略落 ignored 终态且不续跑 / 被普通消息取代
//! - 失效 / 类型串用的决议被拒绝（前端可见错误，不再靠文本标记与审计兜底）
//! - Auto 两遍执行触发条件（非零退出但无防护拦截 → 不二跑）
//! - 运行时切换模式 / 审批策略（send_message 回写 record → 工具策略即时生效）
//! - 会话设置异步改写（`update_settings` **不发消息** → 活记录立即生效，缺省字段保持原值）
//! - 审计日志字段（approval=None 时 approved_at 为空）
//!
//! 运行：`cargo test -p astorm-tests --test security_matrix`
//! （独立测试二进制进程独占 App 单例；bash 用例真实 spawn 进程）

use std::path::Path;
use std::sync::Arc;

use astorm_common::session::interrupt::{InterruptResolution, InterruptType};
use astorm_common::session::req::CreateSessionRequest;
use astorm_common::session::SessionSettings;
use astorm_common::session::types::{PermissionMode, ReasoningEffort, SessionMode};
use astorm_common::session::Session as SessionFacade;
use astorm_common::types::record::{MessageRecord, SessionRecord, SessionType};
use astorm_common::types::Storage;
use astorm_core::app::{App, AppConfig, LlmConfig, LlmProvider};
use astorm_core::tool::file::FileOperationTool;
use astorm_core::tool::shell::ShellTool;
use astorm_storage::StorageManager;
use react_loop::{Tool, ToolContext};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn ctx(sid: &str) -> ToolContext {
    ToolContext::new(CancellationToken::new()).with_session_id(sid)
}

/// 建会话记录（指定模式 × 审批策略 + 工作区）并装载进内存注册表（策略层从内存快照读取）。
fn setup_session(
    storage: &Arc<dyn Storage>,
    mode: SessionMode,
    perm: PermissionMode,
    ws: &Path,
) -> String {
    let id = Uuid::new_v4();
    let record = SessionRecord {
        id,
        pid: None,
        session_type: SessionType::Normal,
        scheduled_task_id: None,
        agent_type: None,
        title: None,
        title_zh: None,
        title_en: None,
        model: None,
        provider: None,
        workspace: Some(ws.to_string_lossy().into_owned()),
        session_mode: mode,
        reasoning_effort: ReasoningEffort::default(),
        permission_mode: perm,
        skill_list: vec![],
        tool_list: vec![],
        loaded_tool_list: vec![],
        mcp_list: vec![],
        session_usage: Default::default(),
        contributed_to_parent: Default::default(),
        subagent_usage: Default::default(),
        context_tokens: 0,
        summary_times: 0,
        question_times: 0,
        current_round: 0,
        status: Default::default(),
        status_updated_at: 0,
        goal: None,
        created_at: 1,
        updated_at: 1,
    };
    storage.upsert_session(&record).unwrap();
    App::global()
        .session()
        .get_or_rehydrate(&id)
        .expect("会话应装载进内存注册表");
    id.to_string()
}

/// 构造 send_message 请求（mode/permission 缺省 = 不回写会话属性）。
fn msg(text: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "test-model".into(),
        provider: None,
        pid: None,
        agent_type: None,
        session_type: None,
        scheduled_task_id: None,
        workspace: None,
        session_mode: None,
        permission_mode: None,
        reasoning_effort: None,
        user_message: text.into(),
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

/// 解析工具返回的**中断信封**（审批 / 提问同构），并断言类型。
fn interrupt_payload(out: &str, expect_type: &str) -> Value {
    let v: Value = serde_json::from_str(out.trim()).expect("中断信封应为纯 JSON");
    assert_eq!(v["interrupt"], json!(true), "结果应为中断信封: {out}");
    assert_eq!(v["interrupt_type"], json!(expect_type), "中断类型: {out}");
    assert!(v["request_id"].as_str().unwrap_or("").len() >= 32, "应带 request_id: {out}");
    v
}

/// 审批信封中的展示字段快捷断言用（`approval.command` 等）。
fn approval_of(v: &Value) -> &Value {
    v.get("approval").expect("审批信封应带 approval 段")
}

/// 会话内 tool 消息的确定性 id（与后端 `agent::history::tool_message_id` 同构）。
fn tool_msg_id(sid: &str, tcid: &str) -> String {
    format!("{sid}:tool:{tcid}")
}

/// 轮询等待条件成立（异步重放没有同步信号，只能轮询）。
fn wait_until(what: &str, timeout_ms: u64, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("等待超时（{timeout_ms}ms）: {what}");
}

/// 播种「assistant 工具调用 + 中断信封 tool 结果（`metadata.interrupt`）」并**冷启动装载**会话。
///
/// 真实链路里：重放参数来源是 assistant 消息的 `tool_calls`（已随消息流落盘），
/// 中断态载体是 tool 消息的 `metadata.interrupt`（含完整信封）——故测试直接落这两条记录，
/// 再经 `get_or_rehydrate`（冷启动路径）重建台账，完整覆盖「重启后仍可审批 / 仍可回答」的契约。
///
/// `status` 为中断标记状态（`pending` 走待决议链路；`replaying` 模拟重放中进程被杀）。
/// 返回 `(tool_call_id, request_id)`。
#[allow(clippy::too_many_arguments)]
fn seed_interrupt(
    storage: &Arc<dyn Storage>,
    sid: &str,
    mode: SessionMode,
    perm: PermissionMode,
    ws: &Path,
    tool_name: &str,
    params: &Value,
    request: &Value,
    status: &str,
) -> (String, String) {
    let id = Uuid::parse_str(sid).unwrap();
    storage
        .upsert_session(&SessionRecord {
            id,
            pid: None,
            session_type: SessionType::Normal,
            scheduled_task_id: None,
            agent_type: None,
            title: None,
            title_zh: None,
            title_en: None,
            model: None,
            provider: None,
            workspace: Some(ws.to_string_lossy().into_owned()),
            session_mode: mode,
            reasoning_effort: ReasoningEffort::default(),
            permission_mode: perm,
            skill_list: vec![],
            tool_list: vec![],
            loaded_tool_list: vec![],
            mcp_list: vec![],
            session_usage: Default::default(),
            contributed_to_parent: Default::default(),
            subagent_usage: Default::default(),
            context_tokens: 0,
            summary_times: 0,
            question_times: 0,
            current_round: 0,
            status: Default::default(),
            status_updated_at: 0,
            goal: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    let tcid = format!("call_{}", sid.replace('-', "")[..12].to_string());
    let request_id = request["request_id"].as_str().unwrap().to_string();
    // 信封里的 tool_call_id 与 assistant tool_calls 保持一致（决议据此定位那条 tool 消息）
    let mut request = request.clone();
    request["tool_call_id"] = json!(tcid);
    let messages = vec![
        MessageRecord {
            id: Uuid::new_v4().to_string(),
            session_id: sid.into(),
            role: "assistant".into(),
            content: String::new(),
            tool_calls: json!([{
                "id": tcid,
                "type": "function",
                "function": { "name": tool_name, "arguments": serde_json::to_string(params).unwrap() },
            }]),
            seq: 1,
            created_at: 1,
            reasoning_content: None,
            tool_call_id: None,
            metadata: None,
        },
        MessageRecord {
            id: tool_msg_id(sid, &tcid),
            session_id: sid.into(),
            role: "tool".into(),
            // 非待决议态（重放中等）：内容已不在中断门，标记才是恢复依据
            content: if status == "pending" {
                request.to_string()
            } else {
                "正在重新执行".to_string()
            },
            tool_calls: Value::Null,
            seq: 2,
            created_at: 1,
            reasoning_content: None,
            tool_call_id: Some(tcid.clone()),
            metadata: Some(json!({ "interrupt": {
                "request_id": request_id,
                "interrupt_type": request["interrupt_type"],
                "status": status,
                "request": request,
                "updated_at": 1,
            } })),
        },
    ];
    storage.append_messages(sid, &messages).unwrap();
    App::global()
        .session()
        .get_or_rehydrate(&id)
        .expect("会话应装载进内存注册表");
    (tcid, request_id)
}

/// 播种一条**待审批**中断（被拦 bash 命令）。
#[allow(clippy::too_many_arguments)]
fn seed_pending_approval(
    storage: &Arc<dyn Storage>,
    sid: &str,
    mode: SessionMode,
    perm: PermissionMode,
    ws: &Path,
    command: &str,
    status: &str,
) -> (String, String) {
    seed_pending_approval_targets(storage, sid, mode, perm, ws, command, &[], status)
}

/// 同上，但显式给出审批 `targets`（会话白名单按 target 归纳目录 / 主机）。
#[allow(clippy::too_many_arguments)]
fn seed_pending_approval_targets(
    storage: &Arc<dyn Storage>,
    sid: &str,
    mode: SessionMode,
    perm: PermissionMode,
    ws: &Path,
    command: &str,
    targets: &[String],
    status: &str,
) -> (String, String) {
    let request_id = Uuid::new_v4().to_string();
    let request = json!({
        "interrupt": true,
        "interrupt_type": "approval",
        "request_id": request_id,
        "message": "待审批",
        "approval": {
            "category": "file_write",
            "source": "static_precheck",
            "command": command,
            "targets": targets,
            "reason": "测试拦截"
        },
    });
    seed_interrupt(
        storage,
        sid,
        mode,
        perm,
        ws,
        "bash",
        &json!({ "command": command }),
        &request,
        status,
    )
}

/// 播种一条**待回答提问**中断（chat_with_user）。
fn seed_pending_form(
    storage: &Arc<dyn Storage>,
    sid: &str,
    mode: SessionMode,
    perm: PermissionMode,
    ws: &Path,
    fields: Value,
    status: &str,
) -> (String, String) {
    let request_id = Uuid::new_v4().to_string();
    let request = json!({
        "interrupt": true,
        "interrupt_type": "form",
        "request_id": request_id,
        "message": "已向用户提问",
        "form": { "message": "请提供部署信息", "tip": "请提供部署信息", "fields": fields },
    });
    seed_interrupt(
        storage,
        sid,
        mode,
        perm,
        ws,
        "chat_with_user",
        &json!({ "tip": "请提供部署信息", "fields": fields }),
        &request,
        status,
    )
}

/// 提交审批决议（同意 / 拒绝）。
fn submit_approval(sid: &str, request_id: &str, allow: bool) -> Result<(), String> {
    submit_approval_scoped(sid, request_id, allow, None)
}

/// 同上，可带同意范围（`Some("session")` = 记入会话内存白名单）。
fn submit_approval_scoped(
    sid: &str,
    request_id: &str,
    allow: bool,
    scope: Option<&str>,
) -> Result<(), String> {
    App::global().session().resolve_interrupt(
        &Uuid::parse_str(sid).unwrap(),
        InterruptResolution {
            request_id: request_id.into(),
            interrupt_type: InterruptType::Approval,
            allow: Some(allow),
            answer: None,
            ignored: false,
            allow_scope: scope.map(str::to_string),
        },
    )
}

/// 提交表单回答（或忽略）。
fn submit_form(sid: &str, request_id: &str, answer: Option<&str>) -> Result<(), String> {
    App::global().session().resolve_interrupt(
        &Uuid::parse_str(sid).unwrap(),
        InterruptResolution {
            request_id: request_id.into(),
            interrupt_type: InterruptType::Form,
            allow: None,
            answer: answer.map(str::to_string),
            ignored: answer.is_none(),
            allow_scope: None,
        },
    )
}

/// 读回某会话某 tool_call 的持久化记录（决议就地改写的断言）。
fn tool_record(storage: &Arc<dyn Storage>, sid: &str, tcid: &str) -> MessageRecord {
    storage
        .load_messages(sid)
        .expect("读消息历史")
        .into_iter()
        .filter(|r| r.id == tool_msg_id(sid, tcid))
        .next_back()
        .unwrap_or_else(|| panic!("应存在 tool 记录 {tcid}"))
}

/// tool 记录上的中断标记状态。
fn tool_record_status(storage: &Arc<dyn Storage>, sid: &str, tcid: &str) -> String {
    tool_record(storage, sid, tcid)
        .metadata
        .and_then(|m| m.get("interrupt").cloned())
        .and_then(|a| a.get("status").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// 会话持久化历史里的 user 消息条数（断言「决议不追加用户消息」）。
fn user_record_count(storage: &Arc<dyn Storage>, sid: &str) -> usize {
    storage
        .load_messages(sid)
        .expect("读消息历史")
        .iter()
        .filter(|r| r.role == "user")
        .count()
}

fn audit_lines(logs_dir: &Path, sid: &str) -> Vec<Value> {
    let path = logs_dir.join("audit").join(format!("{sid}.jsonl"));
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// 会话审计中 approval=xxx 的记录数。
fn audit_count(logs_dir: &Path, sid: &str, approval: &str) -> usize {
    audit_lines(logs_dir, sid)
        .iter()
        .filter(|r| r["approval"].as_str() == Some(approval))
        .count()
}

/// 注意：不能用 `#[tokio::test]`——`App::init` 自建内嵌 tokio runtime，
/// 在 runtime 上下文内构造/回收会触发 tokio 1.53 的
/// 「Cannot drop a runtime in a context where blocking is not allowed」。
/// 改用普通 `#[test]` + 显式 runtime 驱动 async 段。
///
/// POSIX 专用：用例里的 bash 命令依赖 `cat`/`sh`/`head`/`T=…`/`/etc` 等
/// Unix 工具与语法（上游在 Docker/Linux e2e 中运行）。Windows cmd 没有
/// 这些命令，整树断言无法成立，故仅在 Unix 上编译运行。
#[cfg(unix)]
#[test]
fn security_mode_permission_matrix_full_flow() {
    // 注意：同步准备段（App::init 等）必须在 block_on 之外——其内部临时构建的
    // tokio runtime 在 async 上下文内回收会触发 tokio 1.53 的 drop panic
    let data_dir = std::env::temp_dir().join(format!("astorm_sec_matrix_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    // 应用数据目录重定向到测试临时目录（不与运行中的应用抢 skill_index / sqlite 锁）
    // SAFETY: 测试启动阶段、单线程执行，无并发读该环境变量
    unsafe {
        std::env::set_var("ASTORM_DATA_DIR", &data_dir);
    }
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&data_dir).unwrap());

    let mut config = AppConfig::default();
    config.workspace = data_dir.join("root-ws").to_string_lossy().into_owned();
    config.log.dir = Some(data_dir.join("logs").to_string_lossy().into_owned());
    // 工具测试不发起 LLM 调用，供应商地址任意即可（providers 非空以满足装配）
    config.llm = LlmConfig {
        providers: vec![LlmProvider {
            name: "test".into(),
            base_url: "https://example.invalid".into(),
            api_key: "unused".into(),
            api_format: Default::default(),
            models: vec![],
        }],
        default_provider: None,
        default_model: None,
    };
    let base_config = config.clone();
    App::init(config, Arc::clone(&storage)).unwrap();

    let ws = data_dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("inner.txt"), "inner-content-line").unwrap();
    let outside = data_dir.join("outside.txt");
    std::fs::write(&outside, "outside-content-line").unwrap();
    let outside_str = outside.to_string_lossy().to_string();
    let ws_str = ws.to_string_lossy().to_string();
    let logs_dir = data_dir.join("logs");

    let file = FileOperationTool;
    let bash = ShellTool;

    // 六种模式组合会话（各自独立工作区外的审批目标文件）
    let plan_m = setup_session(&storage, SessionMode::Plan, PermissionMode::Manual, &ws);
    let plan_a = setup_session(&storage, SessionMode::Plan, PermissionMode::Auto, &ws);
    let exec_m = setup_session(&storage, SessionMode::Execute, PermissionMode::Manual, &ws);
    let exec_a = setup_session(&storage, SessionMode::Execute, PermissionMode::Auto, &ws);
    let auto_m = setup_session(&storage, SessionMode::Auto, PermissionMode::Manual, &ws);
    let _auto_a = setup_session(&storage, SessionMode::Auto, PermissionMode::Auto, &ws);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
    // ================= §1 file_operation 矩阵 =================

    // --- Plan × Manual：空间内读放行 / 空间外读审批 / 写一律硬拒（无审批） ---
    let out = file
        .execute(
            json!({"operation": "read", "path": ws.join("inner.txt")}),
            &ctx(&plan_m),
        )
        .await
        .expect("空间内读应放行");
    assert!(out.contains("inner-content-line"), "空间内读内容: {out}");

    let out = file
        .execute(json!({"operation": "read", "path": &outside_str}), &ctx(&plan_m))
        .await
        .expect("空间外读返回审批请求而非错误");
    let payload = interrupt_payload(&out, "approval");
    assert_eq!(approval_of(&payload)["category"], json!("file_read"));
    // 引擎识别：file_operation 的审批结果同样触发中断载荷
    assert!(file.interrupt_payload(&out).is_some(), "文件审批结果应触发中断载荷");

    let err = file
        .execute(
            json!({"operation": "write", "path": ws.join("pm.txt"), "content": "x"}),
            &ctx(&plan_m),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "空间内写应硬拒: {err}");

    let err = file
        .execute(
            json!({"operation": "write", "path": &outside_str, "content": "x"}),
            &ctx(&plan_m),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "空间外写应硬拒: {err}");
    assert!(!err.contains("interrupt"), "Plan 写不提供审批: {err}");

    // --- Plan × Manual：非法 operation（模型把工具名当操作值）→ 报可纠正错误，
    // 既不能被误标为 Plan 写禁令（此前 `!read_only` fail-closed 分支命中），也不得落盘 ---
    let junk_target = ws.join("inner.txt");
    let err = file
        .execute(
            json!({"operation": "file_operation", "path": junk_target.display().to_string()}),
            &ctx(&plan_m),
        )
        .await
        .unwrap_err();
    assert!(err.contains("未知文件操作"), "非法 operation 应报未知操作: {err}");
    for op in ["read", "write", "append", "edit", "list", "grep", "glob"] {
        assert!(err.contains(op), "报错应列出合法值 {op}: {err}");
    }
    assert!(!err.contains("Plan"), "参数错误不得被误标为 Plan 写禁令: {err}");
    assert!(!err.contains("interrupt"), "非法 operation 不提供审批: {err}");
    assert_eq!(
        std::fs::read_to_string(&junk_target).unwrap(),
        "inner-content-line",
        "非法 operation 不得改动文件"
    );

    // --- Plan × Auto：Plan 只读是**硬规则**，permission=auto 只免弹审批卡、不放宽写禁令 ---
    let err = file
        .execute(
            json!({"operation": "write", "path": ws.join("pa-inner.txt"), "content": "x"}),
            &ctx(&plan_a),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "Plan×Auto 空间内写也应硬拒: {err}");
    assert!(!err.contains("interrupt"), "Plan 写不提供审批: {err}");
    assert!(!ws.join("pa-inner.txt").exists(), "Plan×Auto 不得真的写盘");

    let pa_outside = data_dir.join("pa-outside.txt");
    let err = file
        .execute(
            json!({"operation": "write", "path": pa_outside.display().to_string(), "content": "x"}),
            &ctx(&plan_a),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "Plan×Auto 空间外写应硬拒: {err}");
    assert!(!pa_outside.exists(), "Plan×Auto 不得写出会话空间");

    // Plan×Auto 空间外**读**：读不属于写禁令 → 自动免卡放行（无 interrupt 载荷）
    let out = file
        .execute(json!({"operation": "read", "path": &outside_str}), &ctx(&plan_a))
        .await
        .expect("Plan×Auto 空间外读应免卡放行");
    assert!(out.contains("outside-content-line"), "应读到空间外内容: {out}");
    assert!(
        file.interrupt_payload(&out).is_none(),
        "自动审批下空间外读不应弹审批卡: {out}"
    );

    // --- Execute × Manual：空间内读写放行 / 空间外读与写均审批 ---
    file.execute(
        json!({"operation": "write", "path": ws.join("em-inner.txt"), "content": "x"}),
        &ctx(&exec_m),
    )
    .await
    .expect("空间内写应放行");

    let out = file
        .execute(json!({"operation": "read", "path": &outside_str}), &ctx(&exec_m))
        .await
        .expect("空间外读返回审批引导");
    assert_eq!(approval_of(&interrupt_payload(&out, "approval"))["category"], json!("file_read"));

    let out = file
        .execute(
            json!({"operation": "write", "path": &outside_str, "content": "x"}),
            &ctx(&exec_m),
        )
        .await
        .expect("空间外写返回审批引导");
    assert_eq!(approval_of(&interrupt_payload(&out, "approval"))["category"], json!("file_write"));

    // --- Execute × Auto / Auto × Manual：全放行 ---
    let out_file = data_dir.join("outside-by-exec-auto.txt");
    file.execute(
        json!({"operation": "write", "path": out_file.display().to_string(), "content": "auto"}),
        &ctx(&exec_a),
    )
    .await
    .expect("自动审批下空间外写应直接放行");
    assert_eq!(std::fs::read_to_string(&out_file).unwrap(), "auto");

    file.execute(
        json!({"operation": "read", "path": &outside_str}),
        &ctx(&auto_m),
    )
    .await
    .expect("Auto 模式空间外读应放行");

    // ================= §2 bash 矩阵（真实 spawn） =================

    // --- Plan × Manual：可执行；空间内读写放行；空间外读审批 / 写硬拒；禁 cd ---
    let out = bash
        .execute(json!({"command": "echo bash-plan-ok"}), &ctx(&plan_m))
        .await
        .expect("Plan 下 bash 应可执行");
    assert!(out.contains("bash-plan-ok"), "输出: {out}");

    let out = bash
        .execute(
            json!({"command": format!("cat {}/inner.txt", ws_str)}),
            &ctx(&plan_m),
        )
        .await
        .expect("空间内读应放行");
    assert!(out.contains("inner-content-line"), "空间内 cat: {out}");

    let out = bash
        .execute(json!({"command": "cat /etc/hosts"}), &ctx(&plan_m))
        .await
        .expect("空间外读返回审批引导");
    let payload = interrupt_payload(&out, "approval");
    assert_eq!(
        approval_of(&payload)["category"],
        json!("file_read"),
        "bash 空间外读应审批: {out}"
    );

    let err = bash
        .execute(
            json!({"command": format!("echo plan-write > {}", outside_str)}),
            &ctx(&plan_m),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "bash 空间外写应硬拒: {err}");
    // 硬拒绝带结构化分类（工具结果元数据 → tool_result 的 denial / 消息 metadata.denial）
    let meta = bash
        .result_meta(false, &err)
        .expect("Plan 硬拒应携带拒绝信封");
    assert_eq!(meta["status"], json!("error"));
    assert_eq!(meta["denial"]["category"], json!("file_write"));
    assert_eq!(meta["denial"]["source"], json!("static_precheck"));
    assert!(
        !std::path::Path::new(&outside_str).join("plan-write").exists(),
        "外部目录不应被写入"
    );

    let err = bash
        .execute(json!({"command": "cd /tmp"}), &ctx(&plan_m))
        .await
        .unwrap_err();
    assert!(err.contains("cd"), "cd 应被禁止: {err}");
    assert_eq!(
        bash.result_meta(false, &err).expect("禁 cd 应携带拒绝信封")["denial"]["category"],
        json!("cwd_change"),
        "禁 cd 是工具约定（硬规则），单独成类而非 danger_command: {err}"
    );

    // Plan 只读：**会话空间内**写同样硬拒（与 file_operation 的 Plan 语义对齐）
    let plan_inner = ws.join("pm-bash-inner.txt");
    let err = bash
        .execute(
            json!({"command": format!("echo pm-inner > {}", plan_inner.display())}),
            &ctx(&plan_m),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "Plan 下空间内写应硬拒: {err}");
    assert!(!err.contains("interrupt"), "Plan 写不提供审批: {err}");
    assert!(!plan_inner.exists(), "Plan 下空间内写不得落盘");

    // 零副作用设备不受影响：`2>/dev/null` 是丢弃输出的惯用法，不视为写目标
    bash.execute(json!({"command": "ls /etc 2>/dev/null"}), &ctx(&plan_m))
        .await
        .expect("Plan 下 `2>/dev/null` 不应被当成写而硬拒");

    // --- Plan × Auto：permission=auto 不得成为 Plan 只读的逃生口 ---
    // 回归：曾因 auto 分支按 static_outside_write 命中做**无防护二跑**，直接写出会话空间。
    let pa_bash_outside = data_dir.join("pa-bash-outside.txt");
    let err = bash
        .execute(
            json!({"command": format!("echo pa-escape > {}", pa_bash_outside.display())}),
            &ctx(&plan_a),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "Plan×Auto 空间外写应硬拒: {err}");
    assert_eq!(
        bash.result_meta(false, &err).expect("Plan×Auto 硬拒应携带拒绝信封")["denial"]["category"],
        json!("file_write"),
        "Plan×Auto 写应按 file_write 分类: {err}"
    );
    assert!(
        !pa_bash_outside.exists(),
        "Plan×Auto 绝不能无防护二跑写出文件（逃生口回归）"
    );

    // Plan×Auto 会话空间内写同样硬拒
    let pa_bash_inner = ws.join("pa-bash-inner.txt");
    let err = bash
        .execute(
            json!({"command": format!("echo pa-inner > {}", pa_bash_inner.display())}),
            &ctx(&plan_a),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "Plan×Auto 空间内写应硬拒: {err}");
    assert!(!pa_bash_inner.exists(), "Plan×Auto 空间内写不得落盘");

    // Plan×Auto 空间外读仍可审批语义：读不属写禁令，自动审批下免卡执行
    let out = bash
        .execute(json!({"command": "cat /etc/hosts"}), &ctx(&plan_a))
        .await
        .expect("Plan×Auto 空间外读应免卡执行");
    assert!(!out.is_empty(), "空间外读应有输出: {out}");

    // --- Execute × Manual：空间外读写审批 / 危险命令硬拒 ---
    let out = bash
        .execute(json!({"command": "cat /etc/hosts"}), &ctx(&exec_m))
        .await
        .expect("Execute 空间外读返回审批引导");
    assert_eq!(approval_of(&interrupt_payload(&out, "approval"))["category"], json!("file_read"));

    // 危险命令：预检静态拦截（进程不会启动），硬拒并记审计
    let err = bash
        .execute(json!({"command": "shutdown -h now"}), &ctx(&exec_m))
        .await
        .unwrap_err();
    assert!(
        err.contains("危险命令") || err.contains("拦截"),
        "危险命令应硬拒: {err}"
    );
    assert_eq!(
        bash.result_meta(false, &err).expect("危险命令应携带拒绝信封")["denial"]["category"],
        json!("danger_command")
    );

    // 空间内写放行
    let out = bash
        .execute(
            json!({"command": format!("echo em-write > {}/em-bash.txt", ws_str)}),
            &ctx(&exec_m),
        )
        .await
        .expect("Execute 空间内 bash 写应放行");
    assert!(out.contains("em-write"));

    // --- Execute × Auto：两遍执行（空间外写在防护拦截时自动放行重跑） ---
    let out_file = data_dir.join("outside-by-exec-auto-bash.txt");
    let out = bash
        .execute(
            json!({"command": format!("echo tw != {}", out_file.display())}),
            &ctx(&exec_a),
        )
        .await;
    drop(out);
    let out = bash
        .execute(
            json!({"command": format!("echo tw > {}", out_file.display())}),
            &ctx(&exec_a),
        )
        .await
        .expect("Auto 审批下空间外写应经两遍执行后成功");
    let _ = out;
    assert!(
        out_file.exists(),
        "两遍执行后外部文件应存在（一遍直写或二跑写入）"
    );

    // ================= §3 审批：拦截 → 冷启动恢复 → 自动重放 =================

    // --- 工具拦截返回**纯 JSON 中断信封**（approval 段 + tool_call_id，供前端关联工具卡） ---
    let probe_sid = setup_session(&storage, SessionMode::Execute, PermissionMode::Manual, &ws);
    let f_probe = data_dir.join("outside-probe.txt");
    let cmd_probe = format!("echo probe > {}", f_probe.display());
    let out = bash
        .execute(json!({"command": &cmd_probe}), &ctx(&probe_sid))
        .await
        .expect("空间外写返回审批请求");
    let req_probe = interrupt_payload(&out, "approval");
    assert_eq!(approval_of(&req_probe)["category"], json!("file_write"));
    assert_eq!(approval_of(&req_probe)["command"], json!(cmd_probe));
    assert!(req_probe["request_id"].as_str().unwrap().len() >= 32);
    assert!(!f_probe.exists(), "被拦命令不应执行");
    // 引擎侧识别：审批结果即中断载荷；普通结果不触发中断
    assert!(bash.interrupt_payload(&out).is_some(), "审批结果应触发中断载荷");
    assert!(bash.interrupt_payload("普通结果文本").is_none(), "普通结果不触发中断");

    // --- 同意 → 冷启动恢复台账 → 后端自动重放（就地替换历史 tool 消息） ---
    let replay_sid = Uuid::new_v4().to_string();
    let f_replay = data_dir.join("outside-replay.txt");
    let cmd_replay = format!("echo sec-replay > {}", f_replay.display());
    let (tcid, req_id) = seed_pending_approval(
        &storage,
        &replay_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        &cmd_replay,
        "pending",
    );
    assert_eq!(
        tool_record_status(&storage, &replay_sid, &tcid),
        "pending",
        "播种后应为待审批"
    );
    submit_approval(&replay_sid, &req_id, true).expect("同意决议应入队");
    wait_until("自动重放写入外部文件", 10_000, || f_replay.exists());
    assert_eq!(
        std::fs::read_to_string(&f_replay).unwrap().trim(),
        "sec-replay",
        "同意后外部文件应真实写入"
    );
    wait_until("tool 消息就地更新为 approved", 10_000, || {
        tool_record_status(&storage, &replay_sid, &tcid) == "approved"
    });
    assert!(
        tool_record(&storage, &replay_sid, &tcid).content.contains("sec-replay"),
        "tool 记录应就地替换为真实执行结果"
    );
    assert!(audit_count(&logs_dir, &replay_sid, "agree") >= 1, "同意应记审计");

    // --- 一次性额度：重放消费后，同命令再次执行重新生成审批 ---
    let out = bash
        .execute(json!({"command": &cmd_replay}), &ctx(&replay_sid))
        .await
        .expect("额度消费后同命令应再次审批");
    let again = interrupt_payload(&out, "approval");
    assert_ne!(again["request_id"], json!(req_id), "应是新的审批请求");

    // --- 「当前会话允许」（allow_scope=session）：同目标不再弹卡，其它目录 / 会话不受影响 ---
    let sess_sid = Uuid::new_v4().to_string();
    let f_sess = data_dir.join("outside-session-1.txt");
    let cmd_sess = format!("echo sec-session > {}", f_sess.display());
    let (_tcid_sess, req_sess) = seed_pending_approval_targets(
        &storage,
        &sess_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        &cmd_sess,
        &[f_sess.to_string_lossy().to_string()],
        "pending",
    );
    submit_approval_scoped(&sess_sid, &req_sess, true, Some("session"))
        .expect("会话级同意决议应入队");
    wait_until("会话级同意后重放写入", 10_000, || f_sess.exists());
    assert!(audit_lines(&logs_dir, &sess_sid)
        .iter()
        .any(|l| l["detail"].as_str().unwrap_or("").contains("会话白名单")), "会话级同意应留痕");

    // 同目录另一文件：命中会话白名单 → 不再弹卡，沙箱内直接执行
    let f_sess2 = data_dir.join("outside-session-2.txt");
    let cmd_sess2 = format!("echo sec-session-2 > {}", f_sess2.display());
    let out = bash
        .execute(json!({"command": &cmd_sess2}), &ctx(&sess_sid))
        .await
        .expect("命中会话白名单应直接执行");
    assert!(
        bash.interrupt_payload(&out).is_none(),
        "会话白名单命中不应再弹审批: {out}"
    );
    assert!(f_sess2.exists(), "命令应真实执行并写入");

    // 别的目录仍弹卡（授权只覆盖目标所在目录**及其子树**，不波及目录之外的路径）
    let other_dir = std::env::temp_dir().join(format!("astorm_sec_other_{}", std::process::id()));
    std::fs::create_dir_all(&other_dir).unwrap();
    let f_other = other_dir.join("outside-session-other.txt");
    let cmd_other = format!("echo other > {}", f_other.display());
    let out = bash
        .execute(json!({"command": &cmd_other}), &ctx(&sess_sid))
        .await
        .expect("未命中目标应仍返回审批");
    let _ = interrupt_payload(&out, "approval");
    assert!(!f_other.exists(), "未命中白名单的命令不应执行");

    // 其它会话不受影响（会话隔离；换一个未被创建过的新目标）
    let f_sess3 = data_dir.join("outside-session-3.txt");
    let cmd_sess3 = format!("echo sec-session-3 > {}", f_sess3.display());
    let out = bash
        .execute(json!({"command": &cmd_sess3}), &ctx(&exec_m))
        .await
        .expect("其它会话应仍返回审批");
    let _ = interrupt_payload(&out, "approval");
    assert!(!f_sess3.exists(), "其它会话不应共享白名单");

    // --- 拒绝 → 就地写拒绝结果，不执行 ---
    let deny_sid = Uuid::new_v4().to_string();
    let f_deny = data_dir.join("outside-deny.txt");
    let cmd_deny = format!("echo sec-deny > {}", f_deny.display());
    let (tcid_deny, req_deny) = seed_pending_approval(
        &storage,
        &deny_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        &cmd_deny,
        "pending",
    );
    submit_approval(&deny_sid, &req_deny, false).expect("拒绝决议应入队");
    wait_until("tool 消息标记 denied", 10_000, || {
        tool_record_status(&storage, &deny_sid, &tcid_deny) == "denied"
    });
    assert!(!f_deny.exists(), "被拒命令不应产生写入");
    assert!(
        tool_record(&storage, &deny_sid, &tcid_deny).content.contains("拒绝"),
        "tool 记录应写拒绝说明"
    );
    assert!(audit_count(&logs_dir, &deny_sid, "deny") >= 1, "拒绝应记审计");

    // --- 普通消息取代未决 pending：标记 superseded 且不执行 ---
    let sup_sid = Uuid::new_v4().to_string();
    let f_sup = data_dir.join("outside-superseded.txt");
    let cmd_sup = format!("echo sec-sup > {}", f_sup.display());
    let (tcid_sup, _req_sup) = seed_pending_approval(
        &storage,
        &sup_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        &cmd_sup,
        "pending",
    );
    App::global()
        .session()
        .send_message(&Uuid::parse_str(&sup_sid).unwrap(), msg("换个思路，不要写工作区外"))
        .expect("普通消息应入队");
    wait_until("tool 消息标记 superseded", 10_000, || {
        tool_record_status(&storage, &sup_sid, &tcid_sup) == "superseded"
    });
    assert!(!f_sup.exists(), "被取代的审批请求不应执行");

    // --- 重放中进程退出（status=replaying）→ 冷启动保守报「结果未知」，绝不重跑 ---
    let crash_sid = Uuid::new_v4().to_string();
    let f_crash = data_dir.join("outside-crash.txt");
    let cmd_crash = format!("echo sec-crash > {}", f_crash.display());
    // 播种即处于「已同意（落 replaying）→ 进程被杀」的冷启动场景：恢复必须保守判未知、不重跑
    let (tcid_crash, _req_crash) = seed_pending_approval(
        &storage,
        &crash_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        &cmd_crash,
        "replaying",
    );
    assert_eq!(
        tool_record_status(&storage, &crash_sid, &tcid_crash),
        "interrupted",
        "冷启动应把 replaying 保守转为「结果未知」"
    );
    assert!(
        tool_record(&storage, &crash_sid, &tcid_crash).content.contains("执行结果未知"),
        "应就地写明结果未知"
    );
    assert!(!f_crash.exists(), "重放中崩溃不得自动重跑非幂等操作");

    // --- 提问（chat_with_user）：用户回答**即 tool 结果**（与审批同构，不追加用户消息） ---
    let form_sid = Uuid::new_v4().to_string();
    let (tcid_form, req_form) = seed_pending_form(
        &storage,
        &form_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        json!([
            { "name": "env", "field_type": "text" },
            { "name": "confirm", "field_type": "boolean" },
        ]),
        "pending",
    );
    assert_eq!(tool_record_status(&storage, &form_sid, &tcid_form), "pending");
    // 冷启动恢复已把该提问登记进台账（否则下面提交会报「不存在或已失效」）
    submit_form(&form_sid, &req_form, Some(r#"{"confirm":true,"env":"prod"}"#))
        .expect("表单回答应入队");
    wait_until("tool 消息标记 resolved", 10_000, || {
        tool_record_status(&storage, &form_sid, &tcid_form) == "resolved"
    });
    let record = tool_record(&storage, &form_sid, &tcid_form);
    assert!(record.content.contains("用户已回答"), "tool 结果应为用户回答: {}", record.content);
    // 按**表单字段声明顺序**输出（而非 JSON 键序），布尔值保留
    assert!(
        record.content.contains("- env: prod") && record.content.contains("- confirm: true"),
        "回答应逐字段列出: {}",
        record.content
    );
    assert_eq!(user_record_count(&storage, &form_sid), 0, "决议不应追加用户消息");

    // --- 提问忽略：落 ignored 终态、不续跑（省 token），同样不追加用户消息 ---
    let ignore_sid = Uuid::new_v4().to_string();
    let (tcid_ignore, req_ignore) = seed_pending_form(
        &storage,
        &ignore_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        json!([{ "name": "env", "field_type": "text" }]),
        "pending",
    );
    submit_form(&ignore_sid, &req_ignore, None).expect("忽略决议应入队");
    wait_until("tool 消息标记 ignored", 10_000, || {
        tool_record_status(&storage, &ignore_sid, &tcid_ignore) == "ignored"
    });
    assert!(
        tool_record(&storage, &ignore_sid, &tcid_ignore).content.contains("忽略"),
        "忽略应写明事实"
    );
    assert_eq!(user_record_count(&storage, &ignore_sid), 0, "忽略同样不追加用户消息");

    // --- 提问被普通消息取代：待决提问置 superseded（回答入口随之失效） ---
    let sup_form_sid = Uuid::new_v4().to_string();
    let (tcid_sup_form, req_sup_form) = seed_pending_form(
        &storage,
        &sup_form_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        json!([{ "name": "env", "field_type": "text" }]),
        "pending",
    );
    App::global()
        .session()
        .send_message(&Uuid::parse_str(&sup_form_sid).unwrap(), msg("先不回答，改成别的需求"))
        .expect("普通消息应入队");
    wait_until("提问标记 superseded", 10_000, || {
        tool_record_status(&storage, &sup_form_sid, &tcid_sup_form) == "superseded"
    });
    let stale = submit_form(&sup_form_sid, &req_sup_form, Some(r#"{"env":"dev"}"#))
        .expect_err("被取代的提问不应再受理回答");
    assert!(stale.contains("不存在或已失效"), "失效回答错误文案: {stale}");

    // --- 失效 / 不存在的决议：入队即被拒绝（前端提示「已失效」），不写审计 ---
    let err = submit_approval(&exec_m, "00000000-0000-0000-0000-000000000000", true)
        .expect_err("不存在的 request_id 应被拒绝");
    assert!(err.contains("不存在或已失效"), "失效决议错误文案: {err}");
    // 类型串用同样拒绝（审批决议指向表单待决项）
    let mismatch_sid = Uuid::new_v4().to_string();
    let (_tcid_x, form_req_x) = seed_pending_form(
        &storage,
        &mismatch_sid,
        SessionMode::Execute,
        PermissionMode::Manual,
        &ws,
        json!([{ "name": "name", "field_type": "text" }]),
        "pending",
    );
    let mismatch = App::global().session().resolve_interrupt(
        &Uuid::parse_str(&mismatch_sid).unwrap(),
        InterruptResolution {
            request_id: form_req_x,
            interrupt_type: InterruptType::Approval,
            allow: Some(true),
            answer: None,
            ignored: false,
            allow_scope: None,
        },
    )
    .expect_err("类型串用应被拒绝");
    assert!(mismatch.contains("中断类型不符"), "类型不符错误文案: {mismatch}");

    // ================= §5b 运行期文件拒绝：分类驱动（不猜方向） =================
    // 卡片只有两个合法来源：① 静态预检的确定分类（分析得出空间外读/写）；
    // ② 沙箱的结构化分类（Event::Denied/Evicted 或 report.denials 的 Category）。
    // 内核文件拒绝（越出沙箱白名单）只把 EPERM 写进进程输出、**不产生分类** →
    // 按普通命令失败交回 LLM：不弹卡、不硬拒、不重跑。

    // (a) EPERM 特征文本但静态零文件引用（甚至根本不是文件操作）→ 普通失败
    let noisy = "echo \"Operation not permitted\" >&2; exit 1";
    for (label, sid) in [("execute", &exec_m), ("plan", &plan_m)] {
        let out = bash
            .execute(json!({"command": noisy}), &ctx(sid))
            .await
            .unwrap_or_else(|e| panic!("{label}: EPERM 文本应作普通失败返回，而非拒绝信封: {e}"));
        assert!(
            bash.interrupt_payload(&out).is_none(),
            "{label}: 无结构化分类的拒绝不应弹卡: {out}"
        );
        assert!(
            bash.result_meta(false, &out).is_none(),
            "{label}: 不应携带拒绝信封: {out}"
        );
        let v: Value = serde_json::from_str(&out).expect("应为普通 JSON 结果");
        assert_eq!(v["returncode"], json!(1), "{label}: 原样返回失败结果: {out}");
    }

    // (b) 静态零引用 + 真被内核拦住：python chr(47) 拼路径写 home
    // 强制层探针：受保护写 home（Seatbelt 生效应 EPERM 拒绝；降级则直接成功）
    let python_ok = std::process::Command::new("python3")
        .arg("-c")
        .arg("print(1)")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let home = std::env::var("HOME").unwrap_or_default();
    if python_ok && !home.is_empty() {
        let probe = astorm_sandbox::runner::run_sync(astorm_sandbox::policy::SandboxInput {
            command: format!("echo probe > {}", Path::new(&home).join("astorm_sec_probe.txt").display()),
            shell: Default::default(),
            timeout: std::time::Duration::from_secs(30),
            mode: astorm_sandbox::policy::ExecMode::Sync,
            protection: astorm_sandbox::policy::ProtectionConfig {
                enabled: true,
                file: astorm_sandbox::policy::FilePolicyCfg {
                    read_white: vec![format!("{ws_str}/**")],
                    write_white: vec![format!("{ws_str}/**")],
                    ..Default::default()
                },
                net: astorm_sandbox::policy::NetPolicyCfg {
                    use_whitelist: true,
                    ..Default::default()
                },
                danger: astorm_sandbox::policy::DangerPolicyCfg {
                    use_builtin: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            strict: false,
            output_encoding: astorm_sandbox::policy::OutputEncoding::Auto,
            cwd: Some(ws.clone()),
            // 本次探针刻意写**真实 HOME**：不注入持久 HOME（否则 HOME 变成沙箱可写目录，
            // 内核拒绝场景就探不出来了）。workspace 只影响 exec 放行，与本探针无关。
            workspace: None,
            session_home: None,
            cancel: None,
        });
        let seatbelt_enforced =
            !probe.ok && probe.output.to_lowercase().contains("not permitted");
        let _ = std::fs::remove_file(Path::new(&home).join("astorm_sec_probe.txt"));
        eprintln!(
            "[security_matrix] 强制层探针: backend={} ok={} output={:?}",
            probe.backend, probe.ok, probe.output
        );
        if !seatbelt_enforced {
            eprintln!("[security_matrix] 沙箱强制层未生效（降级环境），跳过内核拒绝场景");
        }
        // chr(47) 拼出绝对路径，静态分析零文件引用
        let home_parts: Vec<String> = home
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| format!("chr(47)+'{s}'"))
            .collect();
        let py_target = format!("chr(47)+{}", home_parts.join("+"));
        let py_cmd = format!(
            "python3 -c \"open({py_target}+chr(47)+'astorm_sec_matrix_z.txt','w').write('py-written')\""
        );
        let written = Path::new(&home).join("astorm_sec_matrix_z.txt");
        let _ = std::fs::remove_file(&written);

        // Execute × Manual：无结构化分类 → 普通失败（不弹卡、不硬拒），文件未被写入
        let out = bash
            .execute(json!({"command": py_cmd}), &ctx(&exec_m))
            .await
            .expect("内核拒绝应按普通失败返回");
        assert!(
            bash.interrupt_payload(&out).is_none(),
            "内核拒绝无分类，不应弹卡: {out}"
        );
        assert!(
            bash.result_meta(false, &out).is_none(),
            "内核拒绝无分类，不应携带拒绝信封: {out}"
        );
        let v: Value = serde_json::from_str(&out).expect("应为普通 JSON 结果");
        assert_ne!(v["returncode"], json!(0), "内核拒绝应非零退出: {out}");
        assert!(!written.exists(), "沙箱拦住后文件不得写入");

        // Plan × Manual：同一命令同样按普通失败返回（不再硬拒成「禁止写文件」）
        let out = bash
            .execute(json!({"command": py_cmd}), &ctx(&plan_m))
            .await
            .expect("Plan 下内核拒绝也按普通失败返回");
        assert!(
            bash.result_meta(false, &out).is_none(),
            "Plan 下无分类不应硬拒: {out}"
        );
        assert!(!written.exists(), "Plan 下被拦命令不得写入");

        // 同意 → 后端自动重放（无防护）→ DAC 允许 → 文件真实写入（重放链路仍可用）
        let py_sid = Uuid::new_v4().to_string();
        let (_tcid_py, req_py) = seed_pending_approval(
            &storage,
            &py_sid,
            SessionMode::Execute,
            PermissionMode::Manual,
            &ws,
            &py_cmd,
            "pending",
        );
        submit_approval(&py_sid, &req_py, true).expect("同意决议应入队");
        wait_until("审批放行后自动重放写入", 15_000, || written.exists());
        assert_eq!(
            std::fs::read_to_string(&written).unwrap().trim(),
            "py-written",
            "放行后外部文件应真实写入"
        );

        // Execute × Auto：无分类的内核 EPERM 不触发二跑 → 普通失败、文件未写入
        let outside_auto = Path::new(&home).join("astorm_sec_matrix_auto.txt");
        let _ = std::fs::remove_file(&outside_auto);
        let py_auto = format!(
            "python3 -c \"open({py_target}+chr(47)+'astorm_sec_matrix_auto.txt','w').write('auto')\""
        );
        let before_auto = audit_count(&logs_dir, &exec_a, "auto");
        let out = bash
            .execute(json!({"command": py_auto}), &ctx(&exec_a))
            .await
            .expect("Auto 下内核拒绝应按普通失败返回");
        assert!(
            bash.interrupt_payload(&out).is_none() && bash.result_meta(false, &out).is_none(),
            "Auto 下无分类拒绝应作普通失败返回（不弹卡、不硬拒）: {out}"
        );
        assert!(
            !outside_auto.exists(),
            "Auto 不得凭无分类的 EPERM 无防护重跑（文件不应写入）"
        );
        assert_eq!(
            audit_count(&logs_dir, &exec_a, "auto"),
            before_auto,
            "无分类信号不应产生自动二跑审计"
        );
    } else {
        eprintln!("[security_matrix] 环境无 python3/HOME，跳过内核拒绝场景");
    }

    // (c) 沙箱**结构化分类** → 卡片（Manual）/ 硬拒（Plan），且 Auto 不重跑
    // 文件黑名单是沙箱预检能给出 Category::FileWrite 的确定分类；黑名单优先于白名单，
    // 故可对「工作区内」路径制造分类拒绝（静态预检不参与，纯靠沙箱分类）。
    let black_target = ws.join("blacklisted.txt");
    let black_target_str = black_target.to_string_lossy().to_string();
    let mut black_cfg = base_config.clone();
    black_cfg.security.file.write_black = vec![black_target_str.clone()];
    App::update_config(black_cfg).expect("热更文件写黑名单应成功");

    let black_cmd = format!("echo x > {black_target_str}");
    // Execute × Manual：按沙箱分类弹卡（今天这里曾是静默 returncode=-1）
    let out = bash
        .execute(json!({"command": &black_cmd}), &ctx(&exec_m))
        .await
        .expect("黑名单命中应返回审批请求");
    let payload = interrupt_payload(&out, "approval");
    assert_eq!(
        approval_of(&payload)["category"],
        json!("file_write"),
        "应按沙箱分类报 file_write: {out}"
    );
    assert_eq!(
        approval_of(&payload)["source"],
        json!("sandbox_preflight"),
        "来源应为沙箱预检分类: {out}"
    );
    assert!(!black_target.exists(), "被拦命令不应执行");
    // Plan × Manual：写 → 硬拒（Plan 只读，不给一次性审批）
    let err = bash
        .execute(json!({"command": &black_cmd}), &ctx(&plan_m))
        .await
        .unwrap_err();
    assert!(
        err.contains("Plan") && err.contains("切换"),
        "Plan 下分类为写应硬拒并提示切换模式: {err}"
    );
    assert!(!black_target.exists(), "Plan 下被拦命令不应执行");
    // Execute × Auto：硬规则不重跑（黑名单不得被无防护重跑绕过）
    let _auto_black = bash
        .execute(json!({"command": &black_cmd}), &ctx(&exec_a))
        .await;
    assert!(
        !black_target.exists(),
        "Auto 不得对结构化文件分类无防护重跑（黑名单被绕过）"
    );

    // (d) Auto + 内置危险命令：硬拦截不得被二跑绕过
    let danger_ws = data_dir.join("danger-ws");
    std::fs::create_dir_all(&danger_ws).unwrap();
    let keep = danger_ws.join("keep.txt");
    std::fs::write(&keep, "keep").unwrap();
    let danger_sid = setup_session(&storage, SessionMode::Auto, PermissionMode::Manual, &danger_ws);
    let err = bash
        .execute(json!({"command": "rm -rf *"}), &ctx(&danger_sid))
        .await
        .unwrap_err();
    assert!(
        err.contains("危险命令"),
        "Auto 下内置危险命令应硬拒: {err}"
    );
    assert!(keep.exists(), "Auto 不得二跑执行危险命令（工作区文件应仍在）");

    // 恢复默认安全配置，避免影响后续断言
    App::update_config(base_config.clone()).expect("恢复默认安全配置应成功");

    // ================= §4 运行时切换模式 / 审批策略 =================
    // exec-manual 会话发送携带 mode=Auto 的消息 → record 回写 → 工具策略即时生效
    let mut switch = msg("切换到自动模式");
    switch.session_mode = Some(SessionMode::Auto);
    App::global()
        .session()
        .send_message(&Uuid::parse_str(&exec_m).unwrap(), switch)
        .expect("send_message 应成功");

    let updated = storage
        .get_session(&exec_m)
        .expect("会话应已落盘")
        .expect("会话记录应存在");
    assert_eq!(updated.session_mode, SessionMode::Auto, "模式应回写落盘");

    // 行为级证明：此前需审批的空间外写，切换后直接执行
    let f_switched = data_dir.join("outside-switched.txt");
    bash.execute(
        json!({"command": format!("echo switched > {}", f_switched.display())}),
        &ctx(&exec_m),
    )
    .await
    .expect("切换 Auto 后空间外写应直接放行");
    assert!(f_switched.exists(), "切换后外部文件应写入");

    // ================= §4b 会话设置异步改写（不发消息） =================
    // 设置是**会话的持久化属性**：update_settings 直接写内存活记录 → 不必发消息，
    // 工具策略（每个工具调用都读活记录）立即是新值；缺省字段保持原值，不回落默认。
    let sid_settings = setup_session(&storage, SessionMode::Execute, PermissionMode::Manual, &ws);
    App::global()
        .session()
        .update_settings(
            &Uuid::parse_str(&sid_settings).unwrap(),
            SessionSettings {
                session_mode: Some(SessionMode::Auto),
                ..Default::default()
            },
        )
        .expect("update_settings 应成功（不发消息）");

    let rec = storage
        .get_session(&sid_settings)
        .expect("会话应可读")
        .expect("会话记录应存在");
    assert_eq!(rec.session_mode, SessionMode::Auto, "模式应落盘");
    assert_eq!(
        rec.permission_mode,
        PermissionMode::Manual,
        "未给出的字段应保持原值（缺省 ≠ 回落默认）"
    );

    // 行为级证明：一次消息都没发，空间外写已直接放行（与 §4 的 send_message 路径同口径）
    let f_settings = data_dir.join("outside-settings.txt");
    bash.execute(
        json!({"command": format!("echo settings > {}", f_settings.display())}),
        &ctx(&sid_settings),
    )
    .await
    .expect("update_settings 切 Auto 后空间外写应直接放行");
    assert!(f_settings.exists(), "设置改写后外部文件应写入");

    // 只带 permission 的一次改写不得动 mode（逐字段独立性）；同值再写幂等不报错
    App::global()
        .session()
        .update_settings(
            &Uuid::parse_str(&sid_settings).unwrap(),
            SessionSettings {
                permission_mode: Some(PermissionMode::Auto),
                ..Default::default()
            },
        )
        .expect("仅改权限应成功");
    App::global()
        .session()
        .update_settings(
            &Uuid::parse_str(&sid_settings).unwrap(),
            SessionSettings {
                permission_mode: Some(PermissionMode::Auto),
                ..Default::default()
            },
        )
        .expect("同值改写应幂等成功");
    let rec = storage.get_session(&sid_settings).unwrap().unwrap();
    assert_eq!(rec.permission_mode, PermissionMode::Auto, "权限应落盘");
    assert_eq!(rec.session_mode, SessionMode::Auto, "只改权限不得动模式");

    // 空设置 = 无操作（不报错）
    App::global()
        .session()
        .update_settings(&Uuid::parse_str(&sid_settings).unwrap(), SessionSettings::default())
        .expect("空设置应是无操作");

    // ================= §4c 内核层拒绝的补位（会话 2026_09_14_00_22_47 复盘） =================
    // 出厂默认是「网络白名单开启 + 名单为空」→ profile 里 `(deny network-outbound)`：连接在
    // connect/bind 阶段就被 EPERM，**结构化网络事件永远不会产生**（lsof 监视看不到连接），
    // 于是 Auto 没信号可重跑、Manual 没依据弹卡。下面的补位各自可测。
    App::update_config(base_config.clone()).expect("恢复默认安全配置（白名单开启 + 空名单）");
    let kd_manual = setup_session(&storage, SessionMode::Execute, PermissionMode::Manual, &ws);
    let kd_auto = setup_session(&storage, SessionMode::Execute, PermissionMode::Auto, &ws);

    // (a) 零副作用设备：静态豁免（analyzer::is_null_device）与内核放行必须一致，
    //     否则 `2>/dev/null` 这条 shell 惯用法会以裸 EPERM 失败
    let out = bash
        .execute(
            json!({"command": "echo probe 2>/dev/null; echo REDIR_OK"}),
            &ctx(&kd_manual),
        )
        .await
        .expect("零副作用重定向不应让工具失败");
    assert!(out.contains("REDIR_OK"), "命令应正常执行: {out}");
    assert!(
        !out.contains("Operation not permitted"),
        "`2>/dev/null` 被内核拒绝（写放行缺 /dev/null）: {out}"
    );

    // (b) Auto + 联网意图：内核整片拒绝出站时按「静态检测联网意图」放行（关沙箱重跑），
    //     不再静默失败（127.0.0.1:9 必被拒连，但命令本身跑得起来 = 已放行）
    let before_auto = audit_count(&logs_dir, &kd_auto, "auto");
    bash.execute(
        json!({"command": "curl -s --max-time 2 http://127.0.0.1:9/ || true"}),
        &ctx(&kd_auto),
    )
    .await
    .expect("网络命令应被自动放行（连接失败属命令自身结果）");
    let lines = audit_lines(&logs_dir, &kd_auto);
    assert!(
        lines.iter().any(|r| r["approval"] == json!("auto")
            && r["detail"].as_str().unwrap_or("").contains("静态检测联网意图")),
        "Auto 应记录「静态检测联网意图」放行: {lines:?}"
    );
    assert!(
        audit_count(&logs_dir, &kd_auto, "auto") > before_auto,
        "Auto 放行应写审计"
    );

    // (c) Manual + 联网意图：返回审批信封（而非静默失败或裸报错）
    let out = bash
        .execute(
            json!({"command": "curl -s --max-time 2 http://127.0.0.1:9/"}),
            &ctx(&kd_manual),
        )
        .await
        .expect("应返回审批信封而非错误");
    let env = interrupt_payload(&out, "approval");
    assert_eq!(env["approval"]["category"], json!("network"), "{out}");
    assert_eq!(env["approval"]["source"], json!("static_precheck"), "{out}");
    assert!(
        env["approval"]["reason"].as_str().unwrap_or("").contains("联网"),
        "审批理由应说明联网需求: {out}"
    );

    // (d) 负例：纯本地命令不得被当成联网（旧主机判定把 `git status` 的 status 当主机名，
    //     白名单模式下弹莫名其妙的审批卡 / Auto 关沙箱重跑）
    for cmd in ["git status", "git log --oneline -3", "npm run --silent nothing"] {
        let out = bash
            .execute(json!({"command": cmd}), &ctx(&kd_manual))
            .await
            .expect("本地命令应正常执行");
        assert!(
            !out.contains("\"interrupt\":true"),
            "本地命令 {cmd} 不应触发审批卡: {out}"
        );
        assert!(
            !out.contains("[沙箱提示]"),
            "本地命令 {cmd} 不应带内核拒绝诊断: {out}"
        );
    }

    // (e) 内核拒绝可诊断：静态分析看不见的写目标（变量形式）被内核拦下 →
    //     结果里必须有诊断（否则模型把「沙箱拦了」误当成「命令写错了」，反复试错）
    let denied_path = data_dir.join("kernel-denied.txt");
    let out = bash
        .execute(
            json!({"command": format!("T={}; echo x > \"$T\"", denied_path.display())}),
            &ctx(&kd_manual),
        )
        .await
        .expect("工具本身应返回结果");
    assert!(out.contains("Operation not permitted"), "写目标应被内核拒绝: {out}");
    assert!(out.contains("[沙箱提示]"), "内核拒绝应带诊断: {out}");
    // 出厂默认「白名单开启 + 名单为空」→ 内核确实拒绝所有出站，文案必须如实这么说
    assert!(
        out.contains("拒绝所有出站连接"),
        "白名单开启且空名单时应说明网络被内核拒绝: {out}"
    );
    assert!(
        !denied_path.exists(),
        "被内核拒绝的写不应落盘: {}",
        denied_path.display()
    );

    // (f) 系统配置区（TLS / DNS）必须可读：即使出站已放行，缺了它们网络工具照样用不了——
    //     curl 启动即 fopen('/private/etc/ssl/openssl.cnf')，读不到就以裸 EPERM 失败，
    //     现场看起来就是「网络被沙箱禁了」（会话 2026_09_14_00_22_47 的原样错误）
    let out = bash
        .execute(
            json!({"command":
                "head -c 1 /private/etc/ssl/openssl.cnf >/dev/null; echo M_tls=$?; \
                 head -c 1 /etc/ssl/cert.pem >/dev/null; echo M_ca=$?; \
                 head -1 /etc/resolv.conf >/dev/null; echo M_dns=$?; echo M_end"}),
            &ctx(&kd_auto),
        )
        .await
        .expect("读系统配置区不应让工具失败");
    for ok in ["M_tls=0", "M_ca=0", "M_dns=0", "M_end"] {
        assert!(out.contains(ok), "系统配置区读取失败（{ok} 缺失）: {out}");
    }
    assert!(
        !out.contains("Operation not permitted"),
        "系统配置区（TLS 配置 / 证书 / DNS）被内核拒绝 → 出站放行了也用不了: {out}"
    );

    // (g) 诊断文案必须按**实际**策略生成：关掉联网白名单后内核是 (allow network-outbound)，
    //     此时再写「网络默认拒绝所有出站」会让模型直接放弃联网方案（现场就被这句带偏了）
    let mut net_off = base_config.clone();
    net_off.security.net.use_whitelist = false;
    App::update_config(net_off).expect("热更关闭网络白名单应成功");
    let out = bash
        .execute(
            json!({"command": format!("T={}; echo x > \"$T\"", denied_path.display())}),
            &ctx(&kd_auto),
        )
        .await
        .expect("工具本身应返回结果");
    assert!(out.contains("[沙箱提示]"), "内核拒绝应带诊断: {out}");
    assert!(
        !out.contains("拒绝所有出站连接"),
        "白名单关闭时不得断言网络被禁（内核已放行出站）: {out}"
    );
    assert!(
        out.contains("未在内核层限制出站"),
        "白名单关闭时应明说网络没被沙箱限制: {out}"
    );
    App::update_config(base_config.clone()).expect("恢复默认安全配置（白名单开启）");

    // ================= §5 Auto 两遍执行触发条件 =================
    // 非零退出但无防护拦截（grep 无匹配）：不应触发「审计 + 二跑」
    let before = audit_count(&logs_dir, &exec_a, "auto");
    let out = bash
        .execute(
            json!({"command": format!("grep -c zzzz-no-match {}/inner.txt", ws_str)}),
            &ctx(&exec_a),
        )
        .await
        .expect("非零退出属正常执行结果");
    assert!(!out.is_empty());
    let after = audit_count(&logs_dir, &exec_a, "auto");
    assert_eq!(before, after, "无拦截的非零退出不应触发自动二跑审计");

    // ================= §6 审计日志字段 =================
    let lines = audit_lines(&logs_dir, &plan_m);
    assert!(
        lines
            .iter()
            .any(|r| r["type"] == json!("fileread")
                && r["approval"] == json!("none")
                && r["approved_at"].is_null()),
        "拦截记录应 approval=none 且 approved_at 为空: {:?}",
        lines
    );
    let lines = audit_lines(&logs_dir, &replay_sid);
    assert!(
        lines.iter().any(|r| r["approval"] == json!("agree")
            && r["approved_at"].is_null() == false),
        "同意记录应有审批时间: {:?}",
        lines
    );
    let lines = audit_lines(&logs_dir, &deny_sid);
    assert!(
        lines.iter().any(|r| r["approval"] == json!("deny")),
        "拒绝动作应记审计"
    );
    // 审计记录带安全拦截分类（读取拒 / 写拒绝 / 网络 各一条），供审计页按分类筛选
    assert!(
        lines
            .iter()
            .any(|r| r["approval"] == json!("deny") && r["category"] == json!("file_write")),
        "拒绝决议应带 category: {:?}",
        lines
    );
    assert!(
        audit_lines(&logs_dir, &plan_m)
            .iter()
            .any(|r| r["type"] == json!("fileread") && r["category"] == json!("file_read")),
        "空间外读拦截应带 category=file_read"
    );
    assert!(
        audit_lines(&logs_dir, &exec_m)
            .iter()
            .any(|r| r["category"] == json!("danger_command")),
        "危险命令硬拒应带 category=danger_command"
    );
    assert!(
        audit_lines(&logs_dir, &exec_a)
            .iter()
            .any(|r| r["approval"] == json!("auto") && r["category"] == json!("file_write")),
        "自动放行（空间外写）应带 category=file_write"
    );
    // 分类来源一并留痕（前端审计页据此标注「确定 / 疑似」，不再前端伪造）
    assert!(
        audit_lines(&logs_dir, &exec_m)
            .iter()
            .any(|r| r["category"] == json!("danger_command") && r["source"].is_string()),
        "硬拒记录应带 source（分类来源 / 可信度）：{:?}",
        audit_lines(&logs_dir, &exec_m)
    );
    assert!(
        audit_lines(&logs_dir, &plan_m)
            .iter()
            .any(|r| r["category"] == json!("file_read")
                && r["source"] == json!("static_precheck")),
        "空间外读拦截应带 source=static_precheck"
    );
    // 失效决议在入队时即被拒绝（不写审计）——已在上方断言

    // ================= §7 安全配置热更 =================
    // update_config 修改 security 后，工具下一次执行即时读取（不重启后端）：
    // 此前 plan_m 会话读空间外文件需审批 → 追加读白名单后直接放行
    let mut hot = base_config.clone();
    // glob 精确匹配文件本身（无通配符 = 字面匹配）
    hot.security.file.read_white = vec![outside_str.clone()];
    App::update_config(hot).expect("热更应成功");
    let out = file
        .execute(json!({"operation": "read", "path": &outside_str}), &ctx(&plan_m))
        .await
        .expect("热更白名单后空间外读应直接放行");
    assert!(out.contains("outside-content-line"), "热更后读取内容: {out}");

    // 热更回退：恢复默认（无白名单）→ 又回到审批门
    let revert = base_config;
    App::update_config(revert).expect("热更回退应成功");
    let out = file
        .execute(json!({"operation": "read", "path": &outside_str}), &ctx(&plan_m))
        .await
        .expect("回退后重新返回审批引导");
    assert!(out.contains("\"interrupt\":true"), "热更回退后应恢复审批: {out}");

    // ================= §8 系统技能目录：只读 (可读/执行，禁止修改) =================
    // skill_dir = {ASTORM_DATA_DIR}/skills（AppDir::new().skills_dir）
    let skills_dir = data_dir.join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();
    let skill_script = skills_dir.join("my_skill.sh");
    let skill_script_str = skill_script.to_string_lossy().to_string();
    std::fs::write(&skill_script, "#!/bin/sh\necho skill-ran\n").unwrap();

    // (a) 读技能脚本：Plan 模式空间外读无需审批（skill 目录只读白名单放行）
    let out = bash
        .execute(json!({"command": format!("cat {}", skill_script_str)}), &ctx(&plan_m))
        .await
        .expect("bash 读技能脚本应放行");
    assert!(out.contains("skill-ran"), "读技能脚本内容: {out}");

    // (b) 执行技能脚本：同样放行
    let out = bash
        .execute(
            json!({"command": format!("sh {}", skill_script_str)}),
            &ctx(&exec_m),
        )
        .await
        .expect("bash 执行技能脚本应放行");
    assert!(out.contains("skill-ran"), "执行技能脚本输出: {out}");

    // (c) 写技能目录：所有模式一律硬拒（Plan/Execute/Auto 均无审批、无重跑）
    for (label, sid) in [
        ("plan", &plan_m),
        ("execute", &exec_m),
        ("auto", &auto_m),
    ] {
        let target = skills_dir.join(format!("should_not_exist_{label}.txt"));
        let cmd = format!("echo hack > {}", target.display());
        let err = bash
            .execute(json!({"command": cmd}), &ctx(sid))
            .await
            .expect_err("写技能目录应被硬拒");
        assert!(
            err.contains("技能目录") || err.contains("只读"),
            "[{label}] 写技能目录错误文案: {err}"
        );
        assert!(
            !target.exists(),
            "[{label}] 技能目录不应被写入: {}",
            target.display()
        );
    }

    // ================= §9 工具链动态注入：rustup 真实工具链受保护可执行 =================
    // resolve_tool_dirs 把 ~/.rustup/toolchains/*/bin 注入 exec 白名单后，
    // rustc（~/.cargo/bin shim 背后的真实工具链）受保护运行应直接成功，
    // 而非 EPERM → stderr 归因 → 审批/自动二跑（注入失效时的旧路径）。
    if astorm_sandbox::toolchain::rustup_installed() {
        let out = bash
            .execute(json!({"command": "rustc --version"}), &ctx(&exec_m))
            .await
            .expect("rustc 受保护执行应成功");
        assert!(out.contains("rustc"), "rustc 版本输出: {out}");
        assert!(
            !out.contains("\"interrupt\":true"),
            "工具链 exec 已动态放行，不应转审批: {out}"
        );
    } else {
        eprintln!("[security_matrix] 本机非 rustup 安装（cargo 不在 ~/.cargo/bin），跳过工具链动态注入场景");
    }

    // ================= 清理 =================
    App::global().shutdown();
    let _ = std::fs::remove_dir_all(&data_dir);
    });
}
