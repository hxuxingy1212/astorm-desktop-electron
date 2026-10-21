//! 记忆相关的集成测试共用脚手架。
//!
//! 为什么需要它：`App::init` 是**进程级单例**（`OnceLock`），因此「真装配」的集成测试必须
//! 每个测试目标各占一个进程 —— 于是每个 `tests/*.rs` 都要一份「临时目录 + 存储 + 会话/概念
//! 种子 + 配置」的脚手架。放在 `tests/common/mod.rs`（子目录不会被当成独立测试目标）共用。

#![allow(dead_code)] // 各测试文件只用其中一部分

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use astorm_common::types::record::SessionType;
use astorm_common::types::{SessionRecord, Storage};
use astorm_core::app::{App, AppConfig, AppState};
use astorm_storage::StorageManager;
use tokio::sync::broadcast;

/// 普通会话（应触发自动编译）
pub const NORMAL_SID: &str = "11111111-2222-3333-4444-555555555555";
/// 子会话（委派执行创建，**不该**进记忆）
pub const CHILD_SID: &str = "99999999-8888-7777-6666-555555555555";

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 临时根目录（下辖应用根 `app/` 与用户目录 `user/`），Drop 时清理。
pub struct Fixture {
    pub root: PathBuf,
    /// 应用根：`ASTORM_DATA_DIR` 指向它，会话目录因此落在 `<app_root>/data/session`
    /// （`AppDir` 会在 `user_data_dir()` 之后再拼一层 `data/`，不能把会话直接种在环境变量那一层）
    pub app_root: PathBuf,
    pub data_dir: PathBuf,
    pub user_dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    /// 建临时目录并设置 `ASTORM_DATA_DIR`。
    ///
    /// 必须在调用 `App::init`（进而 `user_data_dir()`）**之前**：应用根目录只解析一次环境变量。
    pub fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "astorm_mem_it_{tag}_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let app_root = root.join("app");
        let data_dir = app_root.join("data");
        let user_dir = root.join("user");
        std::fs::create_dir_all(&user_dir).unwrap();
        // SAFETY: 每个测试目标是独立进程，且这是进程内唯一改动环境变量的地方
        // （`App::init` 之后不再改动），不存在与其它线程并发读写环境变量的风险。
        unsafe {
            std::env::set_var("ASTORM_DATA_DIR", &app_root);
        }
        Self {
            root,
            app_root,
            data_dir,
            user_dir,
        }
    }

    /// 种两条真实形态的会话（system → user → assistant）。
    pub fn seed_sessions(&self) -> &Self {
        self.write_session(NORMAL_SID, "认证方案选择 JWT 而非 session，理由是支持多端无状态鉴权。");
        self.write_session(CHILD_SID, "这是子会话的内部委派轨迹，不该进长期记忆。");
        self
    }

    fn write_session(&self, sid: &str, text: &str) {
        let msgs = serde_json::json!([
            {"id": format!("{sid}:0"), "sessionId": sid, "role": "system", "content": "你是 astorm……", "toolCalls": null, "seq": 0, "createdAt": 1760000000},
            {"id": format!("{sid}:1"), "sessionId": sid, "role": "user", "content": text, "toolCalls": null, "seq": 1, "createdAt": 1760000100},
            {"id": format!("{sid}:2"), "sessionId": sid, "role": "assistant", "content": "赞成，JWT 更合适。", "toolCalls": null, "seq": 2, "createdAt": 1760000200}
        ]);
        let jsonl = msgs
            .as_array()
            .unwrap()
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let dir = self.data_dir.join("session").join(sid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("message.jsonl"), jsonl).unwrap();
    }

    /// 种一个概念文件（与 `ConceptDocument::to_markdown` 落盘形态一致）。
    pub fn seed_concept(
        &self,
        dir: &str,
        slug: &str,
        type_name: &str,
        title: &str,
        description: &str,
        body: &str,
    ) {
        // 必须走官方入口建 Bundle（写归属标记）：否则后面调 reindex 会被归属守卫正确拒绝
        astorm_memory::paths::BundlePaths::new(self.bundle())
            .ensure_dirs(&[dir.to_string()])
            .expect("初始化测试 Bundle 失败");
        let path = self
            .bundle()
            .join("wiki")
            .join(dir)
            .join(format!("{slug}.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "---\ntype: {type_name}\ntitle: {title}\ndescription: {description}\n\
             tags:\n- 测试\nobserved_at: 2026-09-22\n---\n\n{body}\n"
        );
        std::fs::write(path, content).unwrap();
    }

    /// 应用配置：假供应商（本机不可达端口，保证零真实额度）+ 记忆开关组合。
    pub fn config_full(
        &self,
        enabled: bool,
        auto_compile: bool,
        inject_memories: bool,
        injected_concepts: usize,
    ) -> AppConfig {
        AppConfig::from_json(
            &serde_json::json!({
                "log": { "level": "info" },
                "workspace": self.user_dir.to_string_lossy(),
                "llm": {
                    "providers": [{
                        "name": "smoke",
                        "baseUrl": "http://127.0.0.1:9/v1",
                        "apiKey": "unused",
                        "models": [{ "id": "fake", "name": "fake" }]
                    }],
                    "defaultProvider": "smoke"
                },
                "memory": {
                    "enabled": enabled,
                    "autoCompile": auto_compile,
                    "provider": "smoke",
                    "model": "fake",
                    "injectMemories": inject_memories,
                    "injectedConcepts": injected_concepts
                }
            })
            .to_string(),
        )
        .expect("解析测试配置失败")
    }

    /// Bundle 根（`<用户目录>/memory`）。
    pub fn bundle(&self) -> PathBuf {
        self.user_dir.join("memory")
    }

    /// 已落盘的 raw 会话文件主干列表（`<日期>-<会话前8位>`）。
    pub fn raw_stems(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.bundle().join("raw/sessions")) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .collect();
        out.sort();
        out
    }
}

/// 装配：存储（sqlite 落在临时应用根）+ `App::init`（进程内仅此一次）。
pub struct Boot {
    pub app: &'static Arc<AppState>,
    pub storage: Arc<StorageManager>,
}

pub fn boot(f: &Fixture, cfg: AppConfig) -> Boot {
    let manager = Arc::new(StorageManager::open_default(&f.data_dir).expect("打开临时存储失败"));
    let storage: Arc<dyn Storage> = manager.clone();
    App::init(cfg, storage).expect("App::init 失败");
    Boot {
        app: App::global(),
        storage: manager,
    }
}

/// 轮询等待条件成立（管道跑在应用自己的 runtime 上，测试侧只能轮询）。
pub fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    cond()
}

/// 取出自上次以来累积的 `type == "memory"` 事件。
pub fn take_memory_events(
    rx: &mut broadcast::Receiver<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if ev.get("type").and_then(|t| t.as_str()) == Some("memory") {
            out.push(ev);
        }
    }
    out
}

/// 最小可用会话记录（测试只关心 `session_type`）。
pub fn record(sid: &str, kind: SessionType) -> SessionRecord {
    SessionRecord {
        id: uuid::Uuid::parse_str(sid).unwrap(),
        pid: None,
        session_type: kind,
        scheduled_task_id: None,
        agent_type: None,
        title: Some("测试会话".into()),
        title_zh: None,
        title_en: None,
        model: None,
        provider: None,
        workspace: None,
        session_mode: Default::default(),
        reasoning_effort: Default::default(),
        permission_mode: Default::default(),
        skill_list: vec![],
        tool_list: vec![],
        mcp_list: vec![],
        loaded_tool_list: vec![],
        session_usage: Default::default(),
        contributed_to_parent: Default::default(),
        subagent_usage: Default::default(),
        context_tokens: 0,
        summary_times: 0,
        question_times: 0,
        current_round: 0,
        created_at: 0,
        updated_at: 0,
        status: Default::default(),
        status_updated_at: 0,
        goal: None,
    }
}
