//! 「启动」（`start_session`）：把「仅创建」（`auto_run = false`）落下、尚未执行的指令跑起来。
//!
//! 覆盖：
//! - 仅创建 → 状态 `idle`、指令已落历史（末尾即 user）
//! - 启动 → 入队即 `running`，循环真的跑了（LLM 不可达 → `error` 收尾）
//! - **不变量**：启动**不追加新的 user 消息** —— 指令只该有一条（否则模型看到两遍）
//! - 守卫：无历史的会话（落盘记录冷启动重建）拒绝启动
//!
//! 运行：`cargo test -p astorm-tests --test session_start`
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

/// 播种一条**没有历史**的会话记录（不装载进内存注册表）：启动应被守卫拒绝。
fn seed_historyless_record(storage: &Arc<dyn Storage>) -> Uuid {
    let id = Uuid::new_v4();
    storage
        .upsert_session(&SessionRecord {
            id,
            pid: None,
            session_type: SessionType::Normal,
            scheduled_task_id: None,
            agent_type: None,
            title: Some("启动用例-无历史".into()),
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
            status: SessionStatus::Idle,
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

fn wait_until(what: &str, timeout_ms: u64, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("等待超时：{what}");
}

fn request(user_message: &str, auto_run: bool) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "test-model".into(),
        provider: Some("test".into()),
        pid: None,
        agent_type: None,
        session_type: None,
        scheduled_task_id: None,
        workspace: None,
        session_mode: Some(SessionMode::Execute),
        permission_mode: None,
        reasoning_effort: Some(ReasoningEffort::default()),
        user_message: user_message.into(),
        attachments: None,
        auto_run: Some(auto_run),
        mentions: None,
        elements: None,
        office_refs: None,
        quotes: None,
        goal: None,
        goal_lang: None,
    }
}

/// 注意：不能用 `#[tokio::test]`（`App::init` 自建内嵌 runtime，异步上下文内回收会 panic）。
#[test]
fn start_runs_parked_instruction_without_duplicating_it() {
    let data_dir =
        std::env::temp_dir().join(format!("astorm_session_start_{}", std::process::id()));
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

    // ---------- 1. 守卫：落盘记录冷启动重建后没有历史 → 拒绝启动 ----------
    let historyless = seed_historyless_record(&storage);
    let err = session
        .start_session(&historyless)
        .expect_err("无历史的会话不该被启动");
    assert!(
        err.contains("历史为空"),
        "错误应说明历史为空，实际: {err}"
    );
    // 被拒绝就应保持 idle，不能留下「假装在跑」的状态
    assert_eq!(
        status_of(&storage, historyless),
        Some(SessionStatus::Idle),
        "拒绝启动后状态应原样保持 idle"
    );

    // ---------- 2. 「仅创建」：状态 idle，但指令已写进历史（末尾即 user） ----------
    let sid = session
        .create_session(request("打个招呼", false))
        .expect("仅创建会话应成功");
    assert_eq!(
        status_of(&storage, sid),
        Some(SessionStatus::Idle),
        "仅创建（autoRun: false）应落 idle —— 看板据此归「待办」"
    );
    let parked = session.list_messages(&sid).expect("读取历史应成功");
    assert_eq!(
        parked.iter().filter(|m| m.role == "user").count(),
        1,
        "仅创建就该把指令写进历史（供「启动」消费）"
    );
    assert_eq!(
        parked.last().map(|m| m.role.as_str()),
        Some("user"),
        "仅创建后历史末尾应是那条指令"
    );

    // ---------- 3. 启动：入队即 running ----------
    session.start_session(&sid).expect("启动仅创建的会话应成功");
    assert_eq!(
        status_of(&storage, sid),
        Some(SessionStatus::Running),
        "启动应立刻落 running（卡片随事件移出「待办」）"
    );

    // 循环真的被驱动了：LLM 不可达 → error 收尾（与「只是改了状态」区分开）
    wait_until("启动后的循环以 error 收尾（LLM 不可达）", 15_000, || {
        status_of(&storage, sid) == Some(SessionStatus::Error)
    });

    // ---------- 4. 关键不变量：启动不追加新的 user 消息 ----------
    // 指令在「仅创建」时就已落历史；若这里变成 2 条，说明又把同一条指令发了一遍，
    // 模型会看到两遍同样的请求。
    let after = session.list_messages(&sid).expect("读取历史应成功");
    assert_eq!(
        after.iter().filter(|m| m.role == "user").count(),
        1,
        "启动不该追加 user 消息（指令已在历史里），实际历史: {:?}",
        after.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&data_dir);
}
