//! 定时任务单测：sqlite `scheduled_tasks` CRUD 全流程 + cron 表达式归一化（本地离线；
//! 不触发 cron 执行 / LLM）。
//!
//! `start` / 热注册依赖全局 `App` 与运行中的 runtime，不在单测覆盖；此处仅验证 Scheduler 的
//! 存储读写、启停状态簿记，以及 job 注册本身的解析/时区行为（后者不依赖 App）。

use std::path::PathBuf;
use std::sync::Arc;

use astorm_common::schedule::ScheduledTaskRequest;
use astorm_common::session::types::{PermissionMode, SessionMode};
use astorm_common::types::{ScheduledTask, Storage};
use astorm_storage::StorageManager;
use chrono::Timelike;
use tokio_cron_scheduler::JobScheduler;

use crate::scheduler::{Scheduler, normalize_cron};

/// 各测试独立临时目录（同一进程内并行跑，共用目录会撞 tantivy 索引锁）。
fn tmp_dir() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("astorm_scheduler_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn sample_task(task_id: &str) -> ScheduledTask {
    ScheduledTask {
        task_id: task_id.into(),
        name: "每日巡检".into(),
        description: String::new(),
        cron_expr: "0 9 * * *".into(),
        mode: crate::scheduler::MODE_CONTINUOUS_SESSION.into(),
        instruction: "巡检系统状态".into(),
        workspace_path: String::new(),
        session_mode: None,
        permission_mode: None,
        agent_type: None,
        session_id: String::new(),
        enabled: 0,
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn scheduled_task_crud() {
    let base = tmp_dir();
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&base).unwrap());
    let sched = Scheduler::new(storage, base.join("scheduler"));

    // 初始为空
    assert!(sched.list_tasks().await.unwrap().is_empty());

    // add：空工作区 → 默认调度工作区落盘（基座 = 用户目录 schedule/，即 <base>/<task_id>）
    let ws_base = base.join("scheduler");
    let id = sched.add_task(&sample_task("t1")).await.unwrap();
    assert_eq!(id, "t1");
    let got = sched.get_task_by_id("t1").await.unwrap();
    assert!(!got.workspace_path.is_empty(), "默认工作区应被填充");
    assert_eq!(
        PathBuf::from(&got.workspace_path),
        ws_base.join("t1"),
        "默认工作区应为 <基座>/<task_id>"
    );
    assert!(PathBuf::from(&got.workspace_path).is_dir(), "默认工作区应已创建");
    assert_eq!(got.enabled, 0);

    // list
    let list = sched.list_tasks().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "每日巡检");

    // update：改字段 + 启用
    let mut t = sample_task("t1");
    t.name = "每日巡检v2".into();
    t.enabled = 1;
    sched.update_task(&t).await.unwrap();
    let got = sched.get_task_by_id("t1").await.unwrap();
    assert_eq!(got.name, "每日巡检v2");
    assert_eq!(got.enabled, 1);

    // 会话目录被清空后再保存 → 重新落回默认调度工作区（与新增同一规则）
    let mut cleared = sample_task("t1");
    cleared.workspace_path = "   ".into();
    sched.update_task(&cleared).await.unwrap();
    assert_eq!(
        PathBuf::from(&sched.get_task_by_id("t1").await.unwrap().workspace_path),
        ws_base.join("t1"),
        "清空会话目录应回落到默认调度工作区"
    );

    // 显式指定目录：原样保留，不被默认值覆盖
    let mut custom = sample_task("t1");
    let custom_dir = base.join("custom-ws");
    custom.workspace_path = custom_dir.to_string_lossy().into_owned();
    sched.update_task(&custom).await.unwrap();
    assert_eq!(
        PathBuf::from(&sched.get_task_by_id("t1").await.unwrap().workspace_path),
        custom_dir,
        "显式会话目录应原样保留"
    );

    // enable / disable
    sched.enable_task("t1", false).await.unwrap();
    assert_eq!(sched.get_task_by_id("t1").await.unwrap().enabled, 0);
    sched.enable_task("t1", true).await.unwrap();
    assert_eq!(sched.get_task_by_id("t1").await.unwrap().enabled, 1);

    // 启动（无 App 依赖的注册会被跳过）→ stop 幂等
    sched.stop().await;

    // remove
    sched.remove_task("t1").await.unwrap();
    assert!(sched.list_tasks().await.unwrap().is_empty());
    assert!(sched.get_task_by_id("t1").await.is_err(), "删除后应不可查");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn 五段表达式补秒为六段() {
    // 前端四种频率全部产出 5 段；调度器（croner with_seconds_required）只认 6 段
    assert_eq!(normalize_cron("0 9 * * *").unwrap(), "0 0 9 * * *");
    assert_eq!(normalize_cron("30 18 * * 1,3,5").unwrap(), "0 30 18 * * 1,3,5");
    assert_eq!(normalize_cron("  0   9  *  *  *  ").unwrap(), "0 0 9 * * *");
}

#[test]
fn 六段表达式原样保留() {
    assert_eq!(normalize_cron("0 0 9 * * *").unwrap(), "0 0 9 * * *");
    assert_eq!(normalize_cron("*/30 * * * * *").unwrap(), "*/30 * * * * *");
}

#[test]
fn 非法表达式报错且不落盘() {
    for bad in ["", "   ", "0 9 * *", "0 0 9 * * * *", "0 99 * * *", "not a cron"] {
        assert!(normalize_cron(bad).is_err(), "应拒绝非法表达式: {bad:?}");
    }
}

#[tokio::test]
async fn 非法表达式在落盘前被拒() {
    let base = tmp_dir();
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&base).unwrap());
    let sched = Scheduler::new(storage, base.join("scheduler"));

    let mut bad = sample_task("bad");
    bad.cron_expr = "0 9 * *".into();
    assert!(sched.add_task(&bad).await.is_err(), "非法表达式不应落盘");
    assert!(sched.list_tasks().await.unwrap().is_empty());

    // 合法 5 段 → 落库**保持用户原样的 5 段**（前端频率标签按 5 段解析；6 段只用于注册 job）
    let id = sched.add_task(&sample_task("ok")).await.unwrap();
    assert_eq!(id, "ok");
    assert_eq!(sched.get_task_by_id("ok").await.unwrap().cron_expr, "0 9 * * *");

    let _ = std::fs::remove_dir_all(&base);
}

/// 会话参数：新增时落库；更新时**缺省不覆盖**（`Option` 增量语义），显式给出才改写。
#[tokio::test]
async fn 任务会话参数落库与增量合并() {
    let base = tmp_dir();
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&base).unwrap());
    let sched = Scheduler::new(storage, base.join("scheduler"));

    let mut req = ScheduledTaskRequest {
        id: Some("t1".into()),
        name: "巡检".into(),
        description: "描述".into(),
        cron_expr: "0 9 * * *".into(),
        mode: "new_session".into(),
        instruction: "跑一遍".into(),
        workspace_path: base.join("ws").to_string_lossy().into_owned(),
        session_mode: Some(SessionMode::Plan),
        permission_mode: Some(PermissionMode::Auto),
        agent_type: Some("builtin-explore".into()),
    };
    assert_eq!(sched.add_from_request(&req).await.unwrap(), "t1");

    let got = sched.get_task_by_id("t1").await.unwrap();
    assert_eq!(got.session_mode, Some(SessionMode::Plan));
    assert_eq!(got.permission_mode, Some(PermissionMode::Auto));
    assert_eq!(got.agent_type.as_deref(), Some("builtin-explore"));

    // 更新只给文本字段 → 会话参数保持（旧前端 / 工具不携带时不应被抹掉）
    req.session_mode = None;
    req.permission_mode = None;
    req.agent_type = None;
    req.name = "巡检v2".into();
    sched.update_from_request(&req).await.unwrap();

    let got = sched.get_task_by_id("t1").await.unwrap();
    assert_eq!(got.name, "巡检v2");
    assert_eq!(got.session_mode, Some(SessionMode::Plan), "缺省不应抹掉已配置的执行模式");
    assert_eq!(got.agent_type.as_deref(), Some("builtin-explore"), "缺省不应抹掉智能体");

    // 显式给出 → 只改该字段
    req.session_mode = Some(SessionMode::Auto);
    req.agent_type = Some("builtin-main".into());
    sched.update_from_request(&req).await.unwrap();

    let got = sched.get_task_by_id("t1").await.unwrap();
    assert_eq!(got.session_mode, Some(SessionMode::Auto));
    assert_eq!(got.agent_type.as_deref(), Some("builtin-main"));
    assert_eq!(
        got.permission_mode,
        Some(PermissionMode::Auto),
        "未给出的字段保持原值"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// 任务未显式指定执行模式时兜底为**执行**：`create_session` 对 `None` 取
/// `SessionMode::default()` = Plan，会让定时任务落在只读模式下（写文件被工具层硬拒）；
/// 页面与工具建任务的默认都是「执行」，这里保证任何来源建的任务语义一致。
#[test]
fn 未指定执行模式时兜底为执行() {
    let mut task = sample_task("t1");
    task.session_mode = None;
    assert_eq!(
        super::effective_session_mode(&task),
        Some(SessionMode::Execute),
        "未指定 → 执行（而非默认的 Plan）"
    );

    task.session_mode = Some(SessionMode::Plan);
    assert_eq!(
        super::effective_session_mode(&task),
        Some(SessionMode::Plan),
        "显式指定应原样保留（只读任务仍然只读）"
    );

    task.session_mode = Some(SessionMode::Auto);
    assert_eq!(super::effective_session_mode(&task), Some(SessionMode::Auto));
}

/// 回归：5 段表达式必须能注册成 cron job，且按**本地时间**触发（而非 UTC）。
///
/// 修复前 `Job::new_async`（TZ=Utc + 秒必填）会让 5 段直接解析失败 → 任务静默永不触发；
/// 时区取 Utc 则会让「每天 09:00」在 UTC+8 机器上实际 17:00 才跑。
#[tokio::test]
async fn 五段表达式按本地时间注册() {
    let mut job_sched = JobScheduler::new().await.unwrap();
    let task = sample_task("t1");
    let uuid = super::register_job(&task, &job_sched)
        .await
        .expect("5 段表达式应可注册为 cron job");

    let next = job_sched
        .next_tick_for_job(uuid)
        .await
        .unwrap()
        .expect("已注册的 cron job 应有下次触发时间");
    let local = next.with_timezone(&chrono::Local);
    assert_eq!(
        (local.hour(), local.minute()),
        (9, 0),
        "下次触发应为本地 09:00（实际 {local}）"
    );
}
