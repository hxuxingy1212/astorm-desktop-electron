//! 工具集单测：本地离线组 + 真实网络组（`#[ignore]`）。
//!
//! - 离线：`chat_with_user`（默认 Plan 路径）、`task_list`（sqlite 全流程 + 错误路径）、
//!   `history_search`（消息落盘自动索引 → 命中）、`webfetch`（未注入内置渲染器报错）、
//!   `search` 路由器（未知子工具 / history 转发；子工具实现位于 `tool::search/` 目录）、
//!   `skills`（内建 skill-creator load / search + register 注册与校验错误路径）、
//!   `schedule_task`（Plan 守卫 / add 必填 mode / 增量 update / run / 名称与描述搜索 / progress 摘要）
//! - 网络（`#[ignore]`）：`github_trending` / `arxiv_search` / `websearch` 直连真实端点

use std::sync::Arc;

use astorm_common::session::types::{PermissionMode, ReasoningEffort, SessionMode};
use astorm_common::types::record::{SessionRecord, SessionStatus, SessionType};
use astorm_common::types::{MessageRecord, Storage};
use astorm_storage::StorageManager;
use react_loop::Tool;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::app::{App, AppConfig, LlmConfig, LlmProvider};
use crate::tool::ToolContext;

fn space_workspace() -> std::path::PathBuf {
    if cfg!(windows) {
        std::path::PathBuf::from(r"C:\Users\Me\My Workspace")
    } else {
        std::path::PathBuf::from("/Users/Me/My Workspace")
    }
}

#[test]
fn shell_workspace_expansion_quotes_spaces() {
    let ws = space_workspace();
    let out = crate::tool::expand_session_vars_shell(
        "cd $SESSION_WORKSPACE && echo ok",
        "sess-1",
        &ws,
    );
    #[cfg(windows)]
    assert_eq!(out, r#"cd "C:\Users\Me\My Workspace" && echo ok"#);
    #[cfg(not(windows))]
    assert_eq!(out, "cd '/Users/Me/My Workspace' && echo ok");
}

#[test]
fn shell_workspace_expansion_respects_existing_quotes() {
    let ws = space_workspace();
    let out = crate::tool::expand_session_vars_shell(
        r#"cd "$SESSION_WORKSPACE/sub" && echo ok"#,
        "sess-1",
        &ws,
    );
    #[cfg(windows)]
    assert_eq!(out, r#"cd "C:\Users\Me\My Workspace/sub" && echo ok"#);
    #[cfg(not(windows))]
    assert_eq!(out, r#"cd "/Users/Me/My Workspace/sub" && echo ok"#);
}

#[cfg(not(windows))]
#[test]
fn shell_workspace_expansion_respects_single_quotes() {
    let ws = space_workspace();
    let out = crate::tool::expand_session_vars_shell(
        "cd '$SESSION_WORKSPACE'",
        "sess-1",
        &ws,
    );
    assert_eq!(out, "cd '/Users/Me/My Workspace'");
}

#[test]
fn shell_workspace_expansion_braced_and_session_id() {
    let ws = space_workspace();
    let out = crate::tool::expand_session_vars_shell(
        "echo ${SESSION_WORKSPACE} $SESSION_ID",
        "sess-42",
        &ws,
    );
    #[cfg(windows)]
    assert_eq!(out, r#"echo "C:\Users\Me\My Workspace" sess-42"#);
    #[cfg(not(windows))]
    assert_eq!(out, "echo '/Users/Me/My Workspace' sess-42");
}

#[test]
fn path_expansion_stays_unquoted() {
    let ws = space_workspace();
    let out = crate::tool::expand_session_vars("$SESSION_WORKSPACE/out.txt", "s", &ws);
    assert_eq!(out, format!("{}/out.txt", ws.to_string_lossy()));
}

#[test]
fn expanded_shell_command_handles_space_path() {
    let dir = std::env::temp_dir().join(format!("astorm ws {}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let expanded = crate::tool::expand_session_vars_shell(
        "cd $SESSION_WORKSPACE && echo spelled-ok",
        "sess-space",
        &dir,
    );
    #[cfg(windows)]
    let output = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .args(["/D", "/C"])
            .raw_arg(&expanded)
            .output()
    };
    #[cfg(not(windows))]
    let output = std::process::Command::new("sh")
        .args(["-c", &expanded])
        .output();
    let output = output.expect("shell should run");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("spelled-ok"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 构造最小会话上下文（无事件通道；供工具 execute 直接调用）。
fn test_ctx(session_id: &str) -> ToolContext {
    ToolContext::new(CancellationToken::new()).with_session_id(session_id)
}

/// `chat_with_user`：默认路径（无会话记录 → 默认 Plan，不拒绝）返回**中断信封**（form）；
/// Auto 拒绝路径依赖真实会话记录，留待 e2e（真实 LLM）覆盖。
#[tokio::test]
async fn chat_user_default_path_returns_form_interrupt_envelope() {
    let ctx = test_ctx("sess-chat");
    let tool = crate::tool::chat::UserChatTool;

    let out = tool
        .execute(
            json!({
                "description": "请确认计划",
                "fields": [{
                    "name": "confirm",
                    "field_type": "boolean",
                    "description": "是否确认",
                    "required": true,
                    "true_label": "确认",
                    "false_label": "再想想"
                }]
            }),
            &ctx,
        )
        .await
        .expect("默认路径应成功");
    // 与安全审批同构：同一信封形状，仅 interrupt_type / 专属载荷不同
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["interrupt"], true);
    assert_eq!(v["interrupt_type"], "form");
    assert!(v["request_id"].as_str().unwrap().len() >= 32);
    assert_eq!(v["form"]["message"], "请确认计划");
    assert_eq!(v["form"]["fields"][0]["name"], "confirm");
    assert_eq!(v["form"]["fields"][0]["field_type"], "boolean");
    // 前端标题取 description、开关取正反文案 → 三个字段都要原样透传到信封
    assert_eq!(v["form"]["fields"][0]["description"], "是否确认");
    assert_eq!(v["form"]["fields"][0]["required"], true);
    assert_eq!(v["form"]["fields"][0]["true_label"], "确认");
    assert_eq!(v["form"]["fields"][0]["false_label"], "再想想");
    assert_eq!(v["form"]["tip"], "请确认计划");
    // 引擎据此打断（单通道：interrupt_payload 按结果识别）
    assert!(tool.interrupt_payload(&out).is_some(), "提问信封应触发中断载荷");

    // 无参数调用退化为纯文本提问（fields 为空）
    let out2 = tool.execute(json!({"description": null}), &ctx).await.unwrap();
    let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
    assert_eq!(v2["interrupt_type"], "form");
    assert_eq!(v2["form"]["fields"].as_array().map(|a| a.len()), Some(0));
    // 普通结果文本不触发中断
    assert!(tool.interrupt_payload("普通文本").is_none());
}

/// 存储依赖类工具全流程（App 单例进程内仅初始化一次，故集中在一个测试）：
/// task_list 生命周期（sqlite）→ history_search 命中（tantivy）→ webfetch 未配置报错 → search 路由器。
/// 同上：`App::init` 内嵌 runtime 不能在 `#[tokio::test]` 上下文回收，用 `#[test]` + block_on。
#[test]
fn storage_tools_lifecycle_and_history_search() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
    let data_dir = std::env::temp_dir().join(format!("astorm_tool_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    // 应用数据目录重定向到测试临时目录（不与运行中的应用抢 skill_index / sqlite 锁）
    // SAFETY: 测试启动阶段、单线程执行，无并发读该环境变量
    unsafe {
        std::env::set_var("ASTORM_DATA_DIR", &data_dir);
    }
    let storage: Arc<dyn Storage> = Arc::new(StorageManager::open_default(&data_dir).unwrap());

    let mut config = AppConfig::default();
    config.workspace = data_dir.join("ws").to_string_lossy().into_owned();
    config.log.dir = Some(data_dir.join("logs").to_string_lossy().into_owned());
    // 会话白名单断言需要 workspace 路径（config 在 App::init 处被移动）
    let ws_root = config.workspace.clone();
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
    App::init(config, Arc::clone(&storage)).unwrap();

    let sess = "sess-1";
    let ctx = test_ctx(sess);

    // ---------- task_list：create / update_nodes / delete ----------
    let task_tool = crate::tool::task_list::TaskListTool;
    let out = task_tool
        .execute(
            json!({
                "action": "create",
                "title": "发布准备",
                "description": "上线前任务",
                "steps": [
                    { "task_name": "编译", "task_description": "cargo build" },
                    { "task_name": "测试", "task_description": "cargo test" },
                    { "task_name": "部署" }
                ]
            }),
            &ctx,
        )
        .await
        .expect("create 应成功");
    // "任务id：<uuid>，已创建规划（3 个步骤）"
    let task_id = out
        .split('：')
        .nth(1)
        .and_then(|s| s.split('，').next())
        .expect("输出应含任务 id")
        .trim()
        .to_string();
    let plan = storage.get_plan(sess, &task_id).unwrap().expect("规划应持久化");
    assert_eq!(plan.title, "发布准备");
    assert_eq!(plan.nodes.len(), 3);
    assert!(plan.nodes.iter().all(|n| n.status == "pending"), "新建节点应为 pending");
    assert_eq!(plan.nodes[0].task_name, "编译");
    assert_eq!(plan.nodes[0].task_description, "cargo build");

    // update_nodes：步骤 1、3 → completed
    let out = task_tool
        .execute(
            json!({
                "action": "update_nodes",
                "task_id": task_id,
                "step_numbers": [1, 3],
                "status": "completed"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.contains("已更新 2 个步骤状态为 completed"), "输出: {out}");
    let plan = storage.get_plan(sess, &task_id).unwrap().unwrap();
    assert_eq!(plan.nodes[0].status, "completed");
    assert_eq!(plan.nodes[1].status, "pending");
    assert_eq!(plan.nodes[2].status, "completed");

    // 错误路径：越界步骤 / 非法状态 / 未知计划
    let err = task_tool
        .execute(
            json!({"action": "update_nodes", "task_id": task_id, "step_numbers": [9], "status": "completed"}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("步骤 9 不存在"), "越界步骤应报错: {err}");
    let err = task_tool
        .execute(
            json!({"action": "update_nodes", "task_id": task_id, "step_numbers": [1], "status": "bogus"}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("status 必须是"), "非法状态应报错: {err}");
    let err = task_tool
        .execute(
            json!({"action": "update_nodes", "task_id": "not-exist", "step_numbers": [1], "status": "completed"}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("未找到 task_id"), "未知计划应报错: {err}");

    // delete → 规划不存在
    task_tool.execute(json!({"action": "delete", "task_id": task_id}), &ctx).await.unwrap();
    assert!(storage.get_plan(sess, &task_id).unwrap().is_none(), "删除后规划应不存在");

    // ---------- history_search：消息落盘自动索引 → 全文命中（本地 tantivy） ----------
    let msgs = vec![
        MessageRecord {
            id: "m1".into(),
            session_id: sess.into(),
            role: "assistant".into(),
            content: "rust async runtime concurrency pattern".into(),
            tool_calls: json!(null),
            seq: 0,
            created_at: 1,
            metadata: None,
            reasoning_content: None,
            tool_call_id: None,
        },
        MessageRecord {
            id: "m2".into(),
            session_id: sess.into(),
            role: "assistant".into(),
            content: "python data science pandas".into(),
            tool_calls: json!(null),
            seq: 1,
            created_at: 2,
            metadata: None,
            reasoning_content: None,
            tool_call_id: None,
        },
    ];
    storage.append_messages(sess, &msgs).unwrap();

    let hist = crate::tool::search::history_search::HistorySearchTool;
    let out = hist.execute(json!({"query": "runtime", "limit": 5}), &ctx).await.unwrap();
    assert!(out.contains("m1"), "应命中 m1: {out}");
    assert!(!out.contains("m2"), "不应命中 m2: {out}");
    let out = hist.execute(json!({"query": "not-in-any-message", "limit": 5}), &ctx).await.unwrap();
    assert!(out.contains("未找到"), "无命中应给出明确文案: {out}");

    // ---------- webfetch：未注入内置渲染器时报指引错误 ----------
    let err = crate::tool::search::webfetch::WebFetchTool
        .execute(json!({"url": "https://example.com"}), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("未配置"), "未配置应报错: {err}");

    // ---------- search 路由器：未知 action / history 转发 ----------
    let search_tool = crate::tool::search::SearchTool;
    let err = search_tool
        .execute(json!({"query": "x", "action": "unknown"}), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("未知 action"), "未知 action 应报错: {err}");
    let out = search_tool
        .execute(json!({"query": "pandas", "action": "history", "limit": 5}), &ctx)
        .await
        .unwrap();
    assert!(out.contains("m2"), "history 转发应命中 m2: {out}");

    // ---------- skills 工具：App 初始化已装内建 skill-creator，load / search ----------
    let skills_tool = crate::tool::skills::SkillsTool;
    let out = skills_tool
        .execute(json!({"action": "load", "skills": ["skill-creator"]}), &ctx)
        .await
        .expect("skills load 应成功");
    assert!(out.starts_with("## skill-creator"), "载入内建技能: {out}");
    let out = skills_tool
        .execute(json!({"action": "search", "query": "技能", "size": 5}), &ctx)
        .await
        .expect("skills search 应成功");
    assert!(out.contains("skill-creator"), "搜索应命中: {out}");

    // ---------- skills 工具 register：Execute 会话注册 / 校验错误路径 / Plan 拦截 ----------
    // register 为写操作：先建 Execute 模式会话（无记录会话默认 Plan，会被拦截）
    let reg_id = uuid::Uuid::new_v4();
    storage
        .upsert_session(&SessionRecord {
            id: reg_id,
            pid: None,
            session_type: SessionType::Normal,
            scheduled_task_id: None,
            agent_type: None,
            title: None,
            title_zh: None,
            title_en: None,
            model: None,
            provider: None,
            workspace: None,
            session_mode: SessionMode::Execute,
            reasoning_effort: ReasoningEffort::default(),
            permission_mode: PermissionMode::default(),
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
            status: Default::default(),
            status_updated_at: 0,
            goal: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    App::global()
        .session()
        .get_or_rehydrate(&reg_id)
        .expect("会话应装载进内存注册表");
    let reg_ctx = test_ctx(&reg_id.to_string());

    // 合法技能目录 → 注册成功 + load / search 命中
    let skill_src = data_dir.join("my-reg-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(
        skill_src.join("SKILL.md"),
        "---\nname: my-reg-skill\ndescription: 注册测试技能\n---\n\n注册测试正文指令",
    )
    .unwrap();
    let out = skills_tool
        .execute(
            json!({"action": "register", "path": skill_src.to_string_lossy()}),
            &reg_ctx,
        )
        .await
        .expect("register 应成功");
    assert!(out.contains("已注册 1 个技能"), "输出: {out}");
    assert!(out.contains("my-reg-skill"), "输出: {out}");
    let out = skills_tool
        .execute(json!({"action": "load", "skills": ["my-reg-skill"]}), &ctx)
        .await
        .unwrap();
    assert!(out.contains("注册测试正文指令"), "注册后应可 load: {out}");

    // frontmatter 损坏的技能 → 逐条反馈失败原因（不中断其他导入）
    let broken_src = data_dir.join("broken-skill");
    std::fs::create_dir_all(&broken_src).unwrap();
    std::fs::write(broken_src.join("SKILL.md"), "# 没有 frontmatter").unwrap();
    let out = skills_tool
        .execute(
            json!({"action": "register", "path": broken_src.to_string_lossy()}),
            &reg_ctx,
        )
        .await
        .unwrap();
    assert!(out.contains("导入失败"), "损坏技能应反馈失败原因: {out}");
    assert!(out.contains("frontmatter"), "失败原因应含解析错误: {out}");

    // 路径不存在 / 目录下无 SKILL.md → 明确报错并给出结构指引
    let err = skills_tool
        .execute(json!({"action": "register", "path": "/nonexistent/skill-dir"}), &reg_ctx)
        .await
        .unwrap_err();
    assert!(err.contains("路径不存在"), "应报路径不存在: {err}");
    let empty_dir = data_dir.join("empty-dir");
    std::fs::create_dir_all(&empty_dir).unwrap();
    let err = skills_tool
        .execute(json!({"action": "register", "path": empty_dir.to_string_lossy()}), &reg_ctx)
        .await
        .unwrap_err();
    assert!(err.contains("未发现技能"), "应报结构错误: {err}");

    // Plan 模式（无记录会话默认 Plan）→ 写操作被拦
    let plan_ctx = test_ctx("sess-plan-no-record");
    let err = skills_tool
        .execute(
            json!({"action": "register", "path": skill_src.to_string_lossy()}),
            &plan_ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("Plan"), "Plan 模式应拦截 register: {err}");

    // 清理测试注册的技能（App 用的是平台真实技能目录，避免污染）
    let _ = App::global().skill.remove("my-reg-skill");

    // ---------- schedule_task 工具：与页面同口径（Plan 守卫 / add 必填 mode / 增量 update / run） ----------
    let sched_tool = crate::tool::schedule_task::ScheduleTool;

    // list 只读：任意模式可用（此时还没有任务）
    let out = sched_tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
    assert!(out.contains("暂无定时任务"), "空列表应可读: {out}");

    // 写操作在 Plan（无记录会话默认 Plan）下全被拦
    for action in ["add", "update", "run", "remove"] {
        let err = sched_tool
            .execute(
                json!({"action": action, "id": "x", "name": "x", "cron_expr": "0 9 * * *", "mode": "new_session"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("Plan"), "{action} 在 Plan 下应被拦: {err}");
    }

    // add：mode 必填（工具不设默认，避免与页面默认「每次新建会话」分叉）
    let err = sched_tool
        .execute(json!({"action": "add", "name": "缺运行方式", "cron_expr": "0 9 * * *"}), &reg_ctx)
        .await
        .unwrap_err();
    assert!(err.contains("mode"), "add 缺 mode 应报错: {err}");
    let err = sched_tool
        .execute(
            json!({"action": "add", "name": "坏运行方式", "cron_expr": "0 9 * * *", "mode": "everyday"}),
            &reg_ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("mode 取值无效"), "非法 mode: {err}");
    // 枚举参数（会话执行模式）由 serde 契约把关
    let err = sched_tool
        .execute(
            json!({"action": "add", "name": "坏执行模式", "cron_expr": "0 9 * * *", "mode": "new_session", "session_mode": "write"}),
            &reg_ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("参数解析失败"), "非法枚举值应在解析期报错: {err}");
    // 非法 cron：落盘前就报错
    let err = sched_tool
        .execute(
            json!({"action": "add", "name": "坏表达式", "cron_expr": "9 * *", "mode": "new_session"}),
            &reg_ctx,
        )
        .await
        .unwrap_err();
    assert!(!err.is_empty(), "非法 cron 应报错: {err}");
    // 智能体不存在：错误里列出可用项（id + 名称）
    let err = sched_tool
        .execute(
            json!({"action": "add", "name": "坏智能体", "cron_expr": "0 9 * * *", "mode": "new_session", "agent_type": "nope"}),
            &reg_ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("未找到智能体") && err.contains("builtin-main"),
        "应报未找到并列出可用智能体: {err}"
    );

    // add 成功：智能体按**名称**识别 → 归一化成 id；未给工作区 → 落默认调度目录并建目录
    let out = sched_tool
        .execute(
            json!({
                "action": "add",
                "name": "每日构建巡检",
                "description": "跑一遍类型检查与单测",
                "cron_expr": "0 9 * * *",
                "mode": "new_session",
                "instruction": "跑 pnpm typecheck 与 cargo test",
                "agent_type": "General"
            }),
            &reg_ctx,
        )
        .await
        .expect("add 应成功");
    assert!(out.contains("默认停用"), "应提示默认停用: {out}");
    let task_id = out
        .split("ID: ")
        .nth(1)
        .and_then(|s| s.split('；').next())
        .expect("应给出任务 id")
        .to_string();
    let added = App::global().scheduler.get_task_by_id(&task_id).await.unwrap();
    assert_eq!(added.name, "每日构建巡检");
    assert_eq!(added.description, "跑一遍类型检查与单测");
    assert_eq!(added.mode, "new_session");
    assert_eq!(added.agent_type.as_deref(), Some("builtin-general"), "名称应归一化成 id");
    assert_eq!(added.session_mode, None, "未指定执行模式 → 任务不落该字段（触发时兜底执行）");
    assert_eq!(added.permission_mode, None);
    assert_eq!(added.enabled, 0, "新建默认停用");
    assert_eq!(
        std::path::Path::new(&added.workspace_path),
        std::path::Path::new(&format!("{ws_root}/schedule/{task_id}")),
        "未给工作区应落默认调度目录"
    );
    assert!(
        std::path::Path::new(&added.workspace_path).is_dir(),
        "默认工作区应已创建"
    );

    // search：与页面同口径 —— 名称或**描述**命中即可；cron 不再参与匹配
    let out = sched_tool
        .execute(json!({"action": "search", "query": "类型检查"}), &ctx)
        .await
        .unwrap();
    assert!(out.contains("每日构建巡检"), "描述命中应可搜到: {out}");
    let out = sched_tool
        .execute(json!({"action": "search", "query": "0 9 * * *"}), &ctx)
        .await
        .unwrap();
    assert!(out.contains("未找到匹配"), "cron 不再参与匹配: {out}");

    // list / search 输出含运行方式 / 执行模式 / 审批 / 工作区 / id（模型能自查）
    let out = sched_tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
    assert!(
        out.contains("每次新建会话") && out.contains("手动授权"),
        "list 应展示运行方式与执行/审批模式: {out}"
    );
    assert!(
        out.contains(&added.workspace_path) && out.contains(&task_id),
        "list 应含工作区与 id: {out}"
    );
    let out = sched_tool
        .execute(json!({"action": "search", "query": "构建"}), &ctx)
        .await
        .unwrap();
    assert!(out.contains("每日构建巡检") && out.contains(&task_id), "search 命中: {out}");

    // update：只给 cron → 其它字段不被清空（不沿用页面 API 的整包覆盖）
    let out = sched_tool
        .execute(json!({"action": "update", "id": task_id, "cron_expr": "30 18 * * 1-3"}), &reg_ctx)
        .await
        .unwrap();
    assert!(out.contains("cron →"), "应报告改动项: {out}");
    let updated = App::global().scheduler.get_task_by_id(&task_id).await.unwrap();
    assert_eq!(updated.cron_expr, "30 18 * * 1-3");
    assert_eq!(updated.name, "每日构建巡检", "只改 cron 不应清空 name");
    assert_eq!(updated.description, "跑一遍类型检查与单测", "只改 cron 不应清空 description");
    assert_eq!(
        updated.instruction, "跑 pnpm typecheck 与 cargo test",
        "只改 cron 不应清空 instruction"
    );
    assert_eq!(updated.agent_type.as_deref(), Some("builtin-general"), "未给的字段保持原值");

    // update：一个字段都没给 → 提示且不动数据
    let out = sched_tool
        .execute(json!({"action": "update", "id": task_id}), &reg_ctx)
        .await
        .unwrap();
    assert!(out.contains("未提供要更新的字段"), "应提示未给字段: {out}");

    // 不存在的 id：update / run 都给出可读错误
    let err = sched_tool
        .execute(json!({"action": "update", "id": "no-such-task", "name": "x"}), &reg_ctx)
        .await
        .unwrap_err();
    assert!(err.contains("任务不存在"), "update 不存在 id: {err}");
    let err = sched_tool
        .execute(json!({"action": "run", "id": "no-such-task"}), &reg_ctx)
        .await
        .unwrap_err();
    assert!(err.contains("任务不存在"), "run 不存在 id: {err}");

    // ---------- progress：最新执行的后 5 条消息 → 删减后回传 ----------
    // 没有执行记录 → 明确提示（Plan 下也可用：progress 是只读）
    let out = sched_tool
        .execute(
            json!({"action": "add", "name": "还没跑过的任务", "cron_expr": "0 9 * * *", "mode": "new_session"}),
            &reg_ctx,
        )
        .await
        .unwrap();
    let fresh_id = out
        .split("ID: ")
        .nth(1)
        .and_then(|s| s.split('；').next())
        .unwrap()
        .to_string();
    let out = sched_tool
        .execute(json!({"action": "progress", "id": fresh_id}), &ctx)
        .await
        .expect("progress 只读，Plan 下也应可用");
    assert!(out.contains("还没有执行记录"), "无执行记录应明确提示: {out}");
    sched_tool
        .execute(json!({"action": "remove", "id": fresh_id}), &reg_ctx)
        .await
        .unwrap();

    // 造一轮执行记录：system（应被跳过）+ 用户 + 带工具调用的助手 + 工具结果 + 收尾正文
    let prog_sid = uuid::Uuid::new_v4().to_string();
    storage
        .upsert_session(&SessionRecord {
            id: prog_sid.parse().unwrap(),
            pid: None,
            session_type: SessionType::Scheduled,
            scheduled_task_id: Some(task_id.clone()),
            agent_type: Some("builtin-general".into()),
            title: None,
            title_zh: None,
            title_en: None,
            model: Some("fake-model".into()),
            provider: None,
            workspace: None,
            session_mode: SessionMode::Execute,
            reasoning_effort: ReasoningEffort::default(),
            permission_mode: PermissionMode::default(),
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
            current_round: 2,
            status: SessionStatus::Done,
            status_updated_at: 0,
            goal: None,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_300,
        })
        .unwrap();

    let msg = |id: &str, role: &str, content: &str, seq: i64| MessageRecord {
        id: id.into(),
        session_id: prog_sid.clone(),
        role: role.into(),
        content: content.into(),
        tool_calls: serde_json::Value::Null,
        seq,
        created_at: 1_700_000_000 + seq,
        reasoning_content: None,
        tool_call_id: None,
        metadata: None,
    };
    let mut planned = msg("m2", "assistant", "", 2);
    planned.reasoning_content = Some("先看仓库状态，再跑检查".into());
    planned.tool_calls = json!([{
        "id": "call_1",
        "type": "function",
        "function": { "name": "bash", "arguments": "{\"command\":\"pnpm typecheck\"}" }
    }]);
    let mut tool_out = msg("m3", "tool", "TYPECHECK_DUMP 一大堆输出", 3);
    tool_out.tool_call_id = Some("call_1".into());
    tool_out.metadata = Some(json!({
        "status": "ok",
        "started_at": 1_700_000_010_000i64,
        "ended_at": 1_700_000_012_500i64
    }));
    let mut summary = msg("m4", "assistant", "TYPECHECK_PASSED 汇总：typecheck 通过，单测 2 个失败", 4);
    summary.reasoning_content = Some("汇总失败项".into());

    storage
        .append_messages(
            &prog_sid,
            &[
                msg("m1", "system", "SYSTEM_PROMPT_SHOULD_BE_SKIPPED", 1),
                msg("m1u", "user", "跑一遍检查", 1),
                planned,
                tool_out,
                summary,
            ],
        )
        .unwrap();

    let out = sched_tool
        .execute(json!({"action": "progress", "id": task_id}), &reg_ctx)
        .await
        .unwrap();
    // 头部：会话 / 状态 / 轮次 / 条数（system 不计入）
    assert!(out.contains(&prog_sid), "应给出会话 id: {out}");
    assert!(out.contains("已完成") && out.contains("轮次: 2"), "应给出状态与轮次: {out}");
    assert!(out.contains("共 4 条消息"), "system 消息不应计入: {out}");
    // 用户消息保留
    assert!(out.contains("跑一遍检查"), "用户消息应保留: {out}");
    // 助手：think 与正文完整保留
    assert!(out.contains("先看仓库状态，再跑检查"), "工具调用前的 think 应完整保留: {out}");
    assert!(out.contains("汇总失败项"), "收尾助手 think 应保留: {out}");
    assert!(
        out.contains("TYPECHECK_PASSED 汇总：typecheck 通过，单测 2 个失败"),
        "无工具调用的助手正文应完整保留: {out}"
    );
    // 工具调用：只说明调了哪个工具，参数不给
    assert!(out.contains("调用工具：bash（参数略）"), "应说明调用的工具: {out}");
    assert!(!out.contains("pnpm typecheck"), "工具参数不应出现: {out}");
    // 工具结果：压成「执行 xx 工具，执行时间 xx」一句
    assert!(out.contains("执行 bash 工具，执行时间"), "工具结果应压成一句: {out}");
    assert!(out.contains("耗时 2.5s") && out.contains("结果：成功"), "应带耗时与结果: {out}");
    assert!(!out.contains("TYPECHECK_DUMP"), "工具结果正文不应出现: {out}");
    assert!(!out.contains("SYSTEM_PROMPT_SHOULD_BE_SKIPPED"), "system 消息应跳过: {out}");

    // enable / disable / remove
    let out = sched_tool
        .execute(json!({"action": "enable", "id": task_id}), &reg_ctx)
        .await
        .unwrap();
    assert!(out.contains("已启用"), "{out}");
    assert_eq!(
        App::global().scheduler.get_task_by_id(&task_id).await.unwrap().enabled,
        1
    );
    let out = sched_tool
        .execute(json!({"action": "disable", "id": task_id}), &reg_ctx)
        .await
        .unwrap();
    assert!(out.contains("已停用"), "{out}");
    assert_eq!(
        App::global().scheduler.get_task_by_id(&task_id).await.unwrap().enabled,
        0
    );
    let out = sched_tool
        .execute(json!({"action": "remove", "id": task_id}), &reg_ctx)
        .await
        .unwrap();
    assert!(out.contains("已删除"), "{out}");
    assert!(
        App::global().scheduler.get_task_by_id(&task_id).await.is_err(),
        "删除后应不可查"
    );

    // run：本测试的 LLM 配置没有模型表 → 触发时给出可读的「未配置默认模型」错误（而非静默失败）
    let out = sched_tool
        .execute(
            json!({"action": "add", "name": "立即跑一次", "cron_expr": "0 9 * * *", "mode": "new_session", "agent_type": "builtin-main"}),
            &reg_ctx,
        )
        .await
        .unwrap();
    let run_id = out
        .split("ID: ")
        .nth(1)
        .and_then(|s| s.split('；').next())
        .unwrap()
        .to_string();
    let err = sched_tool
        .execute(json!({"action": "run", "id": run_id}), &reg_ctx)
        .await
        .unwrap_err();
    assert!(err.contains("未配置默认模型"), "run 应给出可读原因: {err}");
    sched_tool
        .execute(json!({"action": "remove", "id": run_id}), &reg_ctx)
        .await
        .unwrap();

    // ---------- 会话级内存白名单（审批卡「当前会话允许」） ----------
    use crate::tool::interrupt::PendingInterrupt;
    use astorm_common::session::interrupt::InterruptType;

    /// 构造审批待决信封（只带断言所需字段）。
    fn pending_for(category: &str, targets: Vec<String>, command: &str) -> PendingInterrupt {
        PendingInterrupt {
            request_id: format!("req-{category}"),
            interrupt_type: InterruptType::Approval,
            tool_call_id: "call-allow".into(),
            tool_name: "bash".into(),
            command: Some(command.to_string()),
            requested_at: 0,
            request: json!({
                "approval": { "category": category, "targets": targets, "command": command }
            }),
        }
    }

    // ① 文件写：记入目标父目录 → 同目录及子目录放行（写）；读不沾光；会话间隔离
    let allow_root = data_dir.join("allow-out");
    std::fs::create_dir_all(&allow_root).unwrap();
    let target = allow_root.join("a.txt");
    let file_pending = pending_for("file_write", vec![target.display().to_string()], "echo hi");
    assert_eq!(
        crate::tool::session_allow::record_from_pending(sess, &file_pending),
        1,
        "文件写目标应记入会话白名单"
    );
    let sub = allow_root.join("sub/b.txt");
    assert!(
        crate::tool::policy::path_allowed_by_session(sess, &sub, true),
        "同目录子路径应放行"
    );
    assert!(
        !crate::tool::policy::path_allowed_by_session(sess, &sub, false),
        "只授权了写，读不应放行"
    );
    assert!(
        !crate::tool::policy::path_allowed_by_session("other-sess", &sub, true),
        "会话之间应隔离"
    );
    // 并入 bash 沙箱策略：命中目录的命令仍在沙箱内跑（不再需要无防护重跑）
    let ws_path = std::path::Path::new(&ws_root);
    let prot = crate::tool::policy::bash_protection(ws_path, sess, true);
    let want = crate::tool::policy::normalize(&allow_root);
    assert!(
        prot.file.write_white.iter().any(|g| g == &format!("{want}/**")),
        "会话白名单目录应并入写白名单: {:?}",
        prot.file.write_white
    );
    // Plan 只读模式不得并入会话写目录（否则会话授权会放宽只读契约）
    let prot_plan = crate::tool::policy::bash_protection(ws_path, sess, false);
    assert!(
        !prot_plan
            .file
            .write_white
            .iter()
            .any(|g| g == &format!("{want}/**")),
        "Plan 模式不应并入会话写目录: {:?}",
        prot_plan.file.write_white
    );

    // ② 网络（能识别主机）：记主机并并入出/入站白名单（端口语义：带端口需同端口）
    let net_pending = pending_for(
        "network",
        vec!["api.example.com:443".into()],
        "curl https://api.example.com",
    );
    assert_eq!(crate::tool::session_allow::record_from_pending(sess, &net_pending), 1);
    assert!(
        crate::app::App::global()
            .session_allow()
            .host_allowed(sess, "api.example.com:443"),
        "同主机同端口应放行"
    );
    let prot = crate::tool::policy::bash_protection(ws_path, sess, true);
    for rule in ["domain:api.example.com:443"] {
        assert!(
            prot.net.out_white.iter().any(|r| r == rule),
            "会话主机应并入出站白名单: {:?}",
            prot.net.out_white
        );
        assert!(
            prot.net.in_white.iter().any(|r| r == rule),
            "会话主机应并入进站白名单: {:?}",
            prot.net.in_white
        );
    }

    // ③ 网络（拿不到主机）：退化为按命令放行
    let cmd = "curl https://unknown-host";
    let net_cmd_pending = pending_for("network", vec!["被默认拒绝".into()], cmd);
    assert_eq!(crate::tool::session_allow::record_from_pending(sess, &net_cmd_pending), 1);
    assert!(
        crate::app::App::global().session_allow().command_allowed(sess, cmd),
        "主机识别失败时应按整条命令放行"
    );

    // ④ 危险命令不可被会话白名单覆盖（不记录任何条目）
    assert_eq!(
        crate::tool::session_allow::record_from_pending(
            sess,
            &pending_for("danger_command", vec!["rm -rf /".into()], "rm -rf /")
        ),
        0,
        "危险命令不纳入会话白名单"
    );

    // 清理：显式 shutdown flush 日志后删除测试数据
    App::global().shutdown();
    let _ = std::fs::remove_dir_all(&data_dir);
    });
}

/// 真实网络组：直连 GitHub Trending / arXiv API / DuckDuckGo（手动运行，需网络）。
#[tokio::test]
#[ignore = "直连真实网络端点，手动运行：cargo test -p astorm-core -- --ignored network_tools"]
async fn network_tools_reach_real_endpoints() {
    let ctx = test_ctx("sess-net");

    let out = crate::tool::search::github_trending::GithubTrendingTool
        .execute(json!({"since": "daily"}), &ctx)
        .await
        .expect("github_trending 应成功");
    assert!(!out.trim().is_empty(), "github_trending 输出不应为空");

    let out = crate::tool::search::arxiv_search::ArxivSearchTool
        .execute(json!({"query": "rust language", "max_results": 3}), &ctx)
        .await
        .expect("arxiv_search 应成功");
    assert!(out.contains("- **"), "应返回论文条目: {out}");

    let out = crate::tool::search::websearch::WebSearchTool
        .execute(json!({"query": "tokio async rust", "max_results": 3}), &ctx)
        .await
        .expect("websearch 应成功");
    // 命中返回结果列表，被限流则返回提示文案——两者均含"搜索"
    assert!(out.contains("搜索"), "websearch 应返回结果或限流提示: {out}");
}

/// 智能体可见工具集：按名称白名单过滤（空 = 无工具；未知名称忽略）。
#[test]
fn tool_map_for_filters_by_agent_tool_list() {
    let registry = crate::tool::ToolRegistry::builtin();
    let all = registry.names();
    assert!(all.len() > 2, "内置工具集不应为空: {all:?}");

    // 子集：仅保留白名单内的工具
    let subset = vec![all[0].clone(), all[2].clone()];
    let map = registry.tool_map_for(&subset);
    let names = map.tool_names();
    assert_eq!(names.len(), 2, "应只剩白名单内的工具: {names:?}");
    for name in subset {
        assert!(map.get(&name).is_some(), "白名单工具应可用: {name}");
    }

    // 空白名单 = 智能体无工具（LLM 请求不带 tools）
    assert!(registry.tool_map_for(&[]).is_empty(), "空工具集应为空");

    // 卡片里残留的已移除工具名：静默忽略，不影响其余工具
    let stale = vec![all[0].clone(), "已移除的工具".to_string()];
    assert_eq!(registry.tool_map_for(&stale).tool_names().len(), 1);
}

/// 根会话归一化（子会话 → 父会话）：tabs / browser 共享主会话状态的基础。
#[test]
fn root_of_normalizes_child_to_parent() {
    let parent = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let mut record = SessionRecord {
        id: child,
        pid: Some(parent),
        session_type: SessionType::Child,
        scheduled_task_id: None,
        agent_type: None,
        title: None,
        title_zh: None,
        title_en: None,
        model: None,
        provider: None,
        workspace: None,
        session_mode: SessionMode::Execute,
        reasoning_effort: ReasoningEffort::default(),
        permission_mode: PermissionMode::default(),
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
    };

    // 子会话 → 父会话（一层即可：子会话不能再开子会话）
    assert_eq!(crate::tool::root_of(Some(&record), &child.to_string()), parent.to_string());

    // 顶层会话 / 记录缺失（未落库、App 未初始化）→ 原样返回
    record.pid = None;
    assert_eq!(crate::tool::root_of(Some(&record), &child.to_string()), child.to_string());
    assert_eq!(crate::tool::root_of(None, "sess-x"), "sess-x");
}
