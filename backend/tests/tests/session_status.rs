//! 会话运行状态（`SessionRecord.status`）：侧栏状态图标的唯一真相源。
//!
//! 覆盖：
//! - 创建会话即 `running` 并落盘（侧栏立即转圈）
//! - 终态（LLM 不可达 → `error`）落盘（重启后红点仍在）
//! - 冷启动修正：落盘 `running` 但进程内并无该会话在跑 → 列表返回 `interrupted`（灰点「上次被中断」）
//! - 非 `running` 的状态原样返回（`done` / `awaiting_form` 等图标可恢复）
//!
//! 运行：`cargo test -p astorm-tests --test session_status`
//! （无 LLM 依赖：供应商地址指向必然连不通的本机端口，循环快速以 error 收尾）

use std::sync::Arc;
use std::time::{Duration, Instant};

use astorm_common::session::req::CreateSessionRequest;
use astorm_common::session::types::{ReasoningEffort, SessionMode};
use astorm_common::session::Session as SessionFacade;
use astorm_common::types::record::{SessionRecord, SessionStatus, SessionType};
use astorm_common::types::Storage;
use astorm_core::app::{App, AppConfig, LlmConfig, LlmProvider};
use astorm_storage::StorageManager;
use uuid::Uuid;

/// 播种一条会话记录（不装载进内存注册表 → 模拟「上次运行留下的落盘记录」）。
fn seed_record(storage: &Arc<dyn Storage>, status: SessionStatus) -> Uuid {
    let id = Uuid::new_v4();
    storage
        .upsert_session(&SessionRecord {
            id,
            pid: None,
            session_type: SessionType::Normal,
            scheduled_task_id: None,
            agent_type: None,
            title: Some(format!("状态用例-{status:?}")),
            title_zh: None,
            title_en: None,
            model: None,
            provider: None,
            workspace: None,
            session_mode: SessionMode::Execute,
            reasoning_effort: ReasoningEffort::default(),
            permission_mode: Default::default(),
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
            created_at: 1,
            updated_at: 1,
            status,
            status_updated_at: 1,
            goal: None,
        })
        .expect("播种会话记录应成功");
    id
}

fn status_of(storage: &Arc<dyn Storage>, id: Uuid) -> Option<SessionStatus> {
    storage
        .list_sessions()
        .ok()?
        .into_iter()
        .find(|r| r.id == id)
        .map(|r| r.status)
}

fn wait_until(
    what: &str,
    timeout_ms: u64,
    mut cur: impl FnMut() -> Option<SessionStatus>,
    expect: SessionStatus,
) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if cur() == Some(expect) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "等待超时：{what}（当前 status={:?}，期望 {:?}）",
        cur(),
        expect
    );
}

/// 注意：不能用 `#[tokio::test]`（`App::init` 自建内嵌 runtime，异步上下文内回收会 panic）。
#[test]
fn session_status_persists_and_recovers_on_cold_start() {
    let data_dir = std::env::temp_dir().join(format!("astorm_session_status_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    // SAFETY: 测试启动阶段单线程执行，无并发读该环境变量
    unsafe {
        std::env::set_var("ASTORM_DATA_DIR", &data_dir);
    }
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&data_dir).unwrap());

    let mut config = AppConfig::default();
    config.workspace = data_dir.join("root-ws").to_string_lossy().into_owned();
    config.log.dir = Some(data_dir.join("logs").to_string_lossy().into_owned());
    // 供应商地址指向本机必然不可达端口：循环快速以 error 收尾，不产生真实网络请求
    config.llm = LlmConfig {
        providers: vec![LlmProvider {
            name: "test".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: "unused".into(),
            api_format: Default::default(),
            models: vec![],
        }],
        default_provider: None,
        default_model: None,
    };
    App::init(config, Arc::clone(&storage)).unwrap();
    let session = App::global().session();

    // ---------- 1. 冷启动修正：落盘 running 但进程内无该会话 → interrupted ----------
    let stale = seed_record(&storage, SessionStatus::Running);
    let listed = session.list_sessions().expect("读取会话列表应成功");
    let stale_row = listed.iter().find(|r| r.id == stale).expect("会话应在列表中");
    assert_eq!(
        stale_row.status,
        SessionStatus::Interrupted,
        "落盘 running 但进程内无该会话 → 应报告为 interrupted（上次被中断）"
    );
    // 只修正返回值，不回写磁盘
    assert_eq!(
        status_of(&storage, stale),
        Some(SessionStatus::Running),
        "list_sessions 不应回写磁盘（无写副作用）"
    );

    // ---------- 2. 非 running 状态原样返回（done / awaiting_form 图标可恢复） ----------
    let done = seed_record(&storage, SessionStatus::Done);
    let form = seed_record(&storage, SessionStatus::AwaitingForm);
    let listed = session.list_sessions().unwrap();
    assert_eq!(listed.iter().find(|r| r.id == done).unwrap().status, SessionStatus::Done);
    assert_eq!(
        listed.iter().find(|r| r.id == form).unwrap().status,
        SessionStatus::AwaitingForm
    );

    // ---------- 3. 创建会话 → running 落盘；循环失败 → error 落盘 ----------
    let sid = session
        .create_session(CreateSessionRequest {
            model: "test-model".into(),
            provider: Some("test".into()),
            pid: None,
            agent_type: None,
            session_type: None,
            scheduled_task_id: None,
            workspace: Some(data_dir.join("ws").to_string_lossy().into_owned()),
            session_mode: Some(SessionMode::Execute),
            permission_mode: None,
            reasoning_effort: Some(ReasoningEffort::default()),
            user_message: "你好".into(),
            attachments: None,
            // 测试用例走正常执行路径
            auto_run: Some(true),
            mentions: None,
            elements: None,
            office_refs: None,
            quotes: None,
            goal: None,
            goal_lang: None,
        })
        .expect("创建会话应成功");

    assert_eq!(
        status_of(&storage, sid),
        Some(SessionStatus::Running),
        "创建会话（首请求入队并 spawn）应落盘 running"
    );
    // 进程内该会话在跑 → 列表如实返回 running（不被冷启动修正吞掉）
    let listed = session.list_sessions().unwrap();
    assert_eq!(
        listed.iter().find(|r| r.id == sid).unwrap().status,
        SessionStatus::Running,
        "进程内运行中的会话，列表应返回 running"
    );

    // LLM 不可达 → 循环以 error 收尾，终态落盘
    wait_until(
        "会话状态变为 error（LLM 不可达）",
        45_000,
        || status_of(&storage, sid),
        SessionStatus::Error,
    );
    let record = session
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|r| r.id == sid)
        .unwrap();
    assert_eq!(record.status, SessionStatus::Error);
    assert!(record.status_updated_at > 0, "状态变更时间应被记录");

    // ---------- 4. 再次冷启动视角：error 原样保留（红点可恢复） ----------
    let listed = session.list_sessions().unwrap();
    assert_eq!(listed.iter().find(|r| r.id == sid).unwrap().status, SessionStatus::Error);

    let _ = std::fs::remove_dir_all(&data_dir);
}
