//! 存储层集成测试（离线）：工具移植引入的新存储能力。
//!
//! - 规划持久化（`plans` 表：upsert / get / delete / `delete_session` 级联）
//! - 全文检索（消息追加即进 tantivy 索引 → `Storage::search` 命中）
//!
//! 工具执行层（`chat_with_user` / `task_list` 等）为 `pub(crate)`，
//! 无法从 tests 层直接驱动，其行为在 `astorm-core` 内 `#[cfg(test)]` 单测覆盖；
//! 此处经 public `Storage` 门面验证落盘与检索语义。

use std::sync::Arc;

use astorm_common::types::{MessageRecord, PlanRecord, PlanStep, Storage};
use astorm_storage::StorageManager;
use serde_json::json;

/// 打开临时目录组合存储（sqlite 元数据 + jsonl 消息 + tantivy 索引），返回存储与目录。
/// `tag` 用于区分并行测试（tantivy 目录锁按进程/目录互斥）。
fn temp_storage(tag: &str) -> (Arc<dyn Storage>, std::path::PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("astorm_tests_storage_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&dir).unwrap());
    (storage, dir)
}

fn sample_plan() -> PlanRecord {
    PlanRecord {
        task_id: "plan-1".into(),
        title: "发布准备".into(),
        description: "上线前任务".into(),
        nodes: vec![
            PlanStep {
                task_id: "n1".into(),
                task_name: "编译".into(),
                task_description: "cargo build".into(),
                status: "pending".into(),
                created_at: 1,
                updated_at: 1,
            },
            PlanStep {
                task_id: "n2".into(),
                task_name: "测试".into(),
                task_description: "cargo test".into(),
                status: "pending".into(),
                created_at: 1,
                updated_at: 1,
            },
        ],
        created_at: 1,
        updated_at: 1,
    }
}

/// 任务规划生命周期：upsert → get → 更新写回 → 会话隔离 → delete → 会话语义级联。
#[test]
fn plan_lifecycle_and_cascade_delete() {
    let (storage, dir) = temp_storage("plan");
    let sess = "s-1";

    // upsert → get
    storage.upsert_plan(sess, &sample_plan()).unwrap();
    let got = storage.get_plan(sess, "plan-1").unwrap().expect("规划应存在");
    assert_eq!(got.title, "发布准备");
    assert_eq!(got.nodes.len(), 2);
    assert!(got.nodes.iter().all(|n| n.status == "pending"));

    // 更新写回（task_list update_nodes 的落盘语义）
    let mut updated = got.clone();
    updated.nodes[0].status = "completed".into();
    updated.updated_at = 2;
    storage.upsert_plan(sess, &updated).unwrap();
    let got = storage.get_plan(sess, "plan-1").unwrap().unwrap();
    assert_eq!(got.nodes[0].status, "completed");
    assert_eq!(got.nodes[1].status, "pending");

    // 会话隔离：其他会话查不到
    assert!(storage.get_plan("s-other", "plan-1").unwrap().is_none());

    // delete
    storage.delete_plan(sess, "plan-1").unwrap();
    assert!(storage.get_plan(sess, "plan-1").unwrap().is_none());

    // delete_session 级联清理规划
    storage.upsert_plan(sess, &sample_plan()).unwrap();
    storage.delete_session(sess).unwrap();
    assert!(
        storage.get_plan(sess, "plan-1").unwrap().is_none(),
        "delete_session 应级联删除会话规划"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 全文检索链路：消息追加落盘即自动进 tantivy 索引，search 命中。
#[test]
fn search_indexes_messages_on_append() {
    let (storage, dir) = temp_storage("search");
    let sess = "s-2";
    let msgs = vec![
        MessageRecord {
            id: "m1".into(),
            session_id: sess.into(),
            role: "assistant".into(),
            content: "rust async runtime concurrency pattern".into(),
            tool_calls: json!(null),
            seq: 0,
            created_at: 1,
            reasoning_content: None,
            tool_call_id: None,
            metadata: None,
        },
        MessageRecord {
            id: "m2".into(),
            session_id: sess.into(),
            role: "assistant".into(),
            content: "python data science pandas".into(),
            tool_calls: json!(null),
            seq: 1,
            created_at: 2,
            reasoning_content: None,
            tool_call_id: None,
            metadata: None,
        },
    ];
    storage.append_messages(sess, &msgs).unwrap();

    let hits = storage.search("runtime", 5).unwrap();
    assert_eq!(hits.iter().filter(|h| h.message_id == "m1").count(), 1, "应命中 m1");
    assert!(!hits.iter().any(|h| h.message_id == "m2"), "不应命中 m2");

    // session 删除后索引随动清理
    storage.delete_session(sess).unwrap();
    let hits = storage.search("runtime", 5).unwrap();
    assert!(hits.is_empty(), "删除会话后索引应清空");

    let _ = std::fs::remove_dir_all(&dir);
}
/// 智能体卡片生命周期（`agents` 表：upsert → get → list 内置置前 → 覆盖更新 → delete）。
#[test]
fn agent_lifecycle_roundtrip() {
    let (storage, dir) = temp_storage("agents");

    let main = astorm_common::types::agent::builtin_main_agent(1);
    let custom = astorm_common::types::agent::AgentRecord {
        agent_id: "agent-1".into(),
        name: "研究员".into(),
        description: "调研".into(),
        guidance: "先检索再回答".into(),
        tool_ids: vec!["search".into()],
        skill_ids: vec!["writer".into()],
        mode: astorm_common::types::agent::AgentExecMode::ReadWrite,
        permission: astorm_common::types::agent::AgentPermissionMode::Manual,
        builtin: false,
        created_at: 2,
        updated_at: 2,
    };
    storage.upsert_agent(&main).unwrap();
    storage.upsert_agent(&custom).unwrap();

    let got = storage.get_agent("agent-1").unwrap().expect("智能体应存在");
    assert_eq!(got.name, "研究员");
    assert_eq!(got.tool_ids, vec!["search".to_string()]);
    // 字段完整性：指引 / 技能集 / 内置标记均随 JSON 往返
    assert_eq!(got.guidance, "先检索再回答");
    assert_eq!(got.skill_ids, vec!["writer".to_string()]);
    assert!(!got.builtin);

    // 列表：内置卡片置前（前端「默认主智能体」在首位）
    let list = storage.list_agents().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].agent_id, astorm_common::types::agent::BUILTIN_MAIN_AGENT_ID);
    assert!(list[0].is_builtin_main());

    // 覆盖更新（同 id upsert）：工具集整体替换
    let mut updated = got.clone();
    updated.tool_ids = vec!["search".into(), "read_file".into()];
    updated.updated_at = 3;
    storage.upsert_agent(&updated).unwrap();
    let got = storage.get_agent("agent-1").unwrap().unwrap();
    assert_eq!(got.tool_ids, vec!["search".to_string(), "read_file".to_string()]);
    assert_eq!(storage.list_agents().unwrap().len(), 2, "upsert 不应产生重复行");

    storage.delete_agent("agent-1").unwrap();
    assert!(storage.get_agent("agent-1").unwrap().is_none());
    assert_eq!(storage.list_agents().unwrap().len(), 1, "仅剩内置卡片");

    let _ = std::fs::remove_dir_all(&dir);
}
