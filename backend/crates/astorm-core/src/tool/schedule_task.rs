//! `schedule_task` 工具：管理定时任务（Cron）。
//!
//! 与「定时任务」页面**同一套语义与参数**（都走 [`Scheduler::add_from_request`] /
//! [`Scheduler::update_from_request`]）：
//! - `list` / `search` / `progress`：只读，任意模式可用；
//! - `add` / `update` / `remove` / `enable` / `disable` / `run`：写操作，Plan 模式下拒绝。
//!
//! 缺省值（与页面一致）：会话执行模式 = 执行、审批 = 手动授权、智能体 = 内置主智能体、
//! 会话目录 = `<用户目录>/schedule/<task_id>`（自动建目录）；模型 / 思考强度**不是任务参数**，
//! 触发时一律用设置里的默认模型。`add` 不设默认运行方式（必须显式给 `mode`），
//! 且新建任务**默认停用**（enabled=0），需再 `enable` 或用 `run` 立即跑一次。
//!
//! `progress` 看的是「这条任务跑得怎么样」：取该任务最新会话的最后 [`PROGRESS_TAIL`] 条消息，
//! 工具结果压成一句（工具名 + 执行时间）、助手保留 think 与正文、工具调用只留名称。

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use astorm_common::schedule::ScheduledTaskRequest;
use astorm_common::session::types::{PermissionMode, SessionMode};
use astorm_common::types::record::MessageRecord;
use astorm_common::types::ScheduledTask;
use chrono::{Local, TimeZone};

use crate::app::App;
use crate::scheduler::{Scheduler, MODE_CONTINUOUS_SESSION, MODE_NEW_SESSION};
use crate::tool::{session_mode, ToolContext};

/// `progress` 只回传最新会话的**尾部**这么多条消息（够看"跑到哪一步"，又不至于把工具结果全灌进上下文）。
const PROGRESS_TAIL: usize = 5;

/// `schedule_task` 工具。
pub struct ScheduleTool;

#[derive(Deserialize)]
struct ScheduleArgs {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cron_expr: Option<String>,
    /// 运行方式：new_session / continuous_session（add 必填）
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    instruction: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
    /// 会话执行模式：plan / execute / auto
    #[serde(default)]
    session_mode: Option<SessionMode>,
    /// 会话审批模式：manual / auto
    #[serde(default)]
    permission_mode: Option<PermissionMode>,
    /// 执行智能体：id 或名称（名称会归一化成 id 落库）
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    query: Option<String>,
}

#[async_trait]
impl crate::tool::Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule_task"
    }

    fn description(&self) -> &str {
        "管理定时任务（Cron）：list 查看 / search 搜索（按名称与描述）/ progress 查看最新执行进展 / add 新建 / update 修改 / remove 删除 / enable 启用 / disable 停用 / run 立即执行一次。新建默认停用，需 enable 后才会按时触发；模型与思考强度不在任务里配置（触发时用设置里的默认模型）"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "search", "progress", "add", "update", "remove", "enable", "disable", "run"],
                    "description": "操作类型：list 查看 / search 按名称与描述搜索 / progress 查看最新执行进展 / add 新建 / update 修改 / remove 删除 / enable 启用 / disable 停用 / run 立即执行一次"
                },
                "id": { "type": "string", "description": "任务 id（progress / update / remove / enable / disable / run 使用；list 结果里带）" },
                "name": { "type": "string", "description": "任务名称（add 必填；update 可选）" },
                "description": { "type": "string", "description": "任务描述（add 新建时设置，update 可改写）：一句话说清这个任务做什么，用于任务列表展示与搜索" },
                "cron_expr": { "type": "string", "description": "cron 表达式（add 必填）。5 段 `分 时 日 月 周`（如 `0 9 * * *`）或 6 段含秒" },
                "mode": {
                    "type": "string",
                    "enum": ["new_session", "continuous_session"],
                    "description": "运行方式（add 必填，无默认）：new_session 每次触发新建独立会话；continuous_session 持续复用同一条会话"
                },
                "instruction": { "type": "string", "description": "触发指令（触发时作为用户消息发送给该会话）" },
                "workspace_path": { "type": "string", "description": "会话目录（绝对路径；不传 = 自动用 <用户目录>/schedule/<任务 id>）" },
                "session_mode": {
                    "type": "string",
                    "enum": ["plan", "execute", "auto"],
                    "description": "会话执行模式（不传 = execute 执行）。所选智能体自带的模式优先于此值"
                },
                "permission_mode": {
                    "type": "string",
                    "enum": ["manual", "auto"],
                    "description": "会话审批模式（不传 = manual 手动授权）。所选智能体自带的策略优先于此值"
                },
                "agent_type": { "type": "string", "description": "执行智能体：传 id（builtin-main / builtin-general / builtin-explore / 自定义 id）或名称（如 General）；不传 = 内置主智能体" },
                "query": { "type": "string", "description": "搜索关键词（search 使用，匹配任务名称与描述，大小写不敏感；留空 = 全部任务）" }
            },
            "required": ["action", "description"]
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        let args: ScheduleArgs =
            serde_json::from_value(params).map_err(|e| format!("参数解析失败: {e}"))?;
        match args.action.as_str() {
            "list" => execute_list().await,
            "search" => execute_search(&args).await,
            "progress" => execute_progress(&args).await,
            "add" => {
                ensure_not_plan(ctx, "add")?;
                execute_add(&args).await
            }
            "update" => {
                ensure_not_plan(ctx, "update")?;
                execute_update(&args).await
            }
            "remove" => {
                ensure_not_plan(ctx, "remove")?;
                let id = required_id(&args)?;
                scheduler()?.remove_task(&id).await?;
                Ok(format!("✅ 定时任务已删除（{id}）"))
            }
            "enable" => {
                ensure_not_plan(ctx, "enable")?;
                let id = required_id(&args)?;
                scheduler()?.enable_task(&id, true).await?;
                Ok(format!("✅ 定时任务已启用（{id}），将按 cron 触发"))
            }
            "disable" => {
                ensure_not_plan(ctx, "disable")?;
                let id = required_id(&args)?;
                scheduler()?.enable_task(&id, false).await?;
                Ok(format!("✅ 定时任务已停用（{id}），不再按 cron 触发"))
            }
            "run" => {
                ensure_not_plan(ctx, "run")?;
                execute_run(&args).await
            }
            other => Err(format!(
                "不支持的操作: {other}（可用: list / search / progress / add / update / remove / enable / disable / run）"
            )),
        }
    }
}

async fn execute_list() -> Result<String, String> {
    let tasks = scheduler()?.list_tasks().await?;
    if tasks.is_empty() {
        return Ok("暂无定时任务".into());
    }
    let lines: Vec<String> = tasks.iter().map(task_lines).collect();
    Ok(format!("## 定时任务列表\n\n{}", lines.join("\n")))
}

/// 单条任务的展示（两行）：第一行概览，第二行工作区 / 智能体 / 最近会话 / id。
fn task_lines(t: &ScheduledTask) -> String {
    format!(
        "- **{}** | `{}` | {} | {} | {} | {}\n  工作区: {} · 智能体: {} · 最近会话: {} · ID: {}",
        t.name,
        t.cron_expr,
        mode_label(&t.mode),
        session_mode_label(t.session_mode),
        permission_label(t.permission_mode),
        if t.enabled == 1 { "✅ 启用" } else { "⏸ 已停用" },
        if t.workspace_path.is_empty() { "(未设置)" } else { &t.workspace_path },
        agent_label(&t.agent_type),
        if t.session_id.is_empty() { "无" } else { &t.session_id },
        t.task_id
    )
}

async fn execute_add(args: &ScheduleArgs) -> Result<String, String> {
    let name = required_text(args.name.as_deref(), "name")?;
    let cron_expr = required_text(args.cron_expr.as_deref(), "cron_expr")?;
    // 运行方式必填：不给默认（页面默认「每次新建会话」，工具默认「持续复用」会让两边语义分叉）
    let mode = args
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or("add 需要 mode：new_session（每次新建会话）或 continuous_session（持续复用同一会话）")?
        .to_string();
    validate_mode(&mode)?;

    let req = ScheduledTaskRequest {
        id: None,
        name,
        description: args.description.clone().unwrap_or_default(),
        cron_expr,
        mode,
        instruction: args.instruction.clone().unwrap_or_default(),
        workspace_path: args.workspace_path.clone().unwrap_or_default(),
        session_mode: args.session_mode,
        permission_mode: args.permission_mode,
        agent_type: args.agent_type.clone(),
    };
    // 与页面同一入口：空目录落默认调度工作区、智能体 id/名称归一化、非法 cron 保存前报错
    let id = scheduler()?.add_from_request(&req).await?;
    Ok(format!(
        "✅ 定时任务已创建（默认停用），ID: {id}；cron 触发前需先 enable，或用 run 立即跑一次"
    ))
}

/// `update`：只覆盖「给了且非空」的字段 —— 工具与页面的差别在于页面每次提交整包表单，
/// 而模型常常只想改一项（例如只改 cron），整包覆盖会把描述 / 指令清空。
async fn execute_update(args: &ScheduleArgs) -> Result<String, String> {
    let id = required_id(args)?;
    let sched = scheduler()?;
    let mut t = sched
        .get_task_by_id(&id)
        .await
        .map_err(|_| format!("任务不存在: {id}"))?;
    let mut changed: Vec<String> = Vec::new();

    if let Some(v) = non_empty(args.name.as_deref()) {
        t.name = v.to_string();
        changed.push(format!("name → 「{v}」"));
    }
    if let Some(v) = non_empty(args.description.as_deref()) {
        t.description = v.to_string();
        changed.push("description".into());
    }
    if let Some(v) = non_empty(args.cron_expr.as_deref()) {
        t.cron_expr = v.to_string();
        changed.push(format!("cron → `{v}`"));
    }
    if let Some(v) = non_empty(args.mode.as_deref()) {
        validate_mode(v)?;
        t.mode = v.to_string();
        changed.push(format!("运行方式 → {}", mode_label(v)));
    }
    if let Some(v) = non_empty(args.instruction.as_deref()) {
        t.instruction = v.to_string();
        changed.push("instruction".into());
    }
    if let Some(v) = non_empty(args.workspace_path.as_deref()) {
        t.workspace_path = v.to_string();
        changed.push(format!("工作区 → {v}"));
    }
    if let Some(m) = args.session_mode {
        t.session_mode = Some(m);
        changed.push(format!("执行模式 → {}", session_mode_label(Some(m))));
    }
    if let Some(p) = args.permission_mode {
        t.permission_mode = Some(p);
        changed.push(format!("审批 → {}", permission_label(Some(p))));
    }
    if let Some(v) = non_empty(args.agent_type.as_deref()) {
        // 原样交给 scheduler 归一化（id 或名称 → id）
        t.agent_type = Some(v.to_string());
        changed.push(format!("智能体 → {v}"));
    }

    if changed.is_empty() {
        return Ok(
            "未提供要更新的字段；可选：name / description / cron_expr / mode / instruction / session_mode / permission_mode / agent_type / workspace_path"
                .into(),
        );
    }
    let req = ScheduledTaskRequest {
        id: Some(id.clone()),
        name: t.name.clone(),
        description: t.description.clone(),
        cron_expr: t.cron_expr.clone(),
        mode: t.mode.clone(),
        instruction: t.instruction.clone(),
        workspace_path: t.workspace_path.clone(),
        session_mode: t.session_mode,
        permission_mode: t.permission_mode,
        agent_type: t.agent_type.clone(),
    };
    sched.update_from_request(&req).await?;
    Ok(format!("✅ 已更新任务「{}」：{}", t.name, changed.join("；")))
}

/// `run`：立即跑一轮（复用 `run_task_now`，停用状态也照跑，与页面「立即执行」一致）。
async fn execute_run(args: &ScheduleArgs) -> Result<String, String> {
    let id = required_id(args)?;
    let sched = scheduler()?;
    sched.run_task_now(&id).await.map_err(|e| {
        if e.contains("task not found") {
            format!("任务不存在: {id}")
        } else {
            e
        }
    })?;
    Ok(format!(
        "✅ 已触发一次执行（{id}）：会话按任务参数新建或复用（即使任务处于停用状态也会跑这一轮）"
    ))
}

/// 关键词搜索：**与页面搜索同一口径** —— 名称或描述包含关键词（大小写不敏感）。
async fn execute_search(args: &ScheduleArgs) -> Result<String, String> {
    let query = args.query.clone().filter(|s| !s.is_empty()).unwrap_or_default();
    let needle = query.to_lowercase();
    let tasks = scheduler()?.list_tasks().await?;
    let hits: Vec<&ScheduledTask> = tasks
        .iter()
        .filter(|t| {
            needle.is_empty()
                || t.name.to_lowercase().contains(&needle)
                || t.description.to_lowercase().contains(&needle)
        })
        .collect();
    if hits.is_empty() {
        return Ok("未找到匹配的定时任务".into());
    }
    let lines: Vec<String> = hits
        .iter()
        .map(|t| {
            format!(
                "- **{}** | `{}` | {} | {} | ID: {}",
                t.name,
                t.cron_expr,
                mode_label(&t.mode),
                if t.enabled == 1 { "✅ 启用" } else { "⏸ 已停用" },
                t.task_id
            )
        })
        .collect();
    Ok(format!(
        "## 搜索结果（关键词：{}）\n\n{}",
        if query.is_empty() { "（空 = 全部）" } else { &query },
        lines.join("\n")
    ))
}

// ---------- 展示与校验小工具 ----------

/// 查看任务最新一次执行的进展：取该任务**最新会话**的最后 [`PROGRESS_TAIL`] 条消息，
/// 做删减后返回（工具结果压成「执行 xx 工具，执行时间 xx」，助手保留 think 与正文，
/// 工具调用只留名称、不给参数与结果正文）——避免把整棵工具输出灌进模型上下文。
async fn execute_progress(args: &ScheduleArgs) -> Result<String, String> {
    let id = required_id(args)?;
    let sched = scheduler()?;
    let task = sched
        .get_task_by_id(&id)
        .await
        .map_err(|_| format!("任务不存在: {id}"))?;

    let app = App::global();
    // 「最新一次执行」= 归属该任务的会话里创建时间最晚的一条
    // （每次新建会话的任务会积累多条；持续复用会话的任务只有一条，尾部即本轮）
    let mut own: Vec<_> = app
        .storage
        .list_sessions()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| s.scheduled_task_id.as_deref() == Some(id.as_str()))
        .collect();
    own.sort_by_key(|s| (s.created_at, s.id));
    let Some(session) = own.last() else {
        return Ok(format!(
            "任务「{}」还没有执行记录（尚未触发过）；可用 enable 等下次触发，或用 run 立即跑一次",
            task.name
        ));
    };

    let sid = session.id.to_string();
    let messages = app.storage.load_messages(&sid).map_err(|e| e.to_string())?;
    // 工具消息只带 tool_call_id，工具名要从 assistant 的 tool_calls 反查
    let names = tool_names(&messages);
    let kept: Vec<&MessageRecord> = messages.iter().filter(|m| m.role != "system").collect();
    let total = kept.len();
    let tail: Vec<&&MessageRecord> = kept.iter().rev().take(PROGRESS_TAIL).collect();
    let tail = tail.into_iter().rev();

    let mut out = format!("## 任务「{}」最新执行进展\n\n", task.name);
    out.push_str(&format!(
        "- 会话: {} · 状态: {} · 轮次: {} · 最后更新: {}\n",
        sid,
        status_label(session.status.clone()),
        session.current_round,
        fmt_unix(session.updated_at)
    ));
    out.push_str(&format!(
        "- 共 {} 条消息，以下为最后 {} 条（工具结果已压缩为一句；助手 think / 正文为原文）\n\n",
        total,
        total.min(PROGRESS_TAIL)
    ));
    for m in tail {
        out.push_str(&digest_line(m, &names));
        out.push('\n');
    }
    Ok(out)
}

/// 单条消息的删减展示。
fn digest_line(m: &MessageRecord, names: &HashMap<String, String>) -> String {
    match m.role.as_str() {
        "user" => format!("- 👤 用户：{}", m.content.trim()),
        "tool" => {
            let tool = m
                .tool_call_id
                .as_deref()
                .and_then(|c| names.get(c))
                .cloned()
                .unwrap_or_else(|| "未知工具".to_string());
            let (when, cost, status) = tool_meta(m);
            format!("- 🔧 执行 {tool} 工具，执行时间 {when}{cost}{status}")
        }
        "assistant" => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(think) = m.reasoning_content.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                parts.push(format!("🧠 think：{think}"));
            }
            let calls = tool_call_names(&m.tool_calls);
            if !calls.is_empty() {
                // 只说明调了哪些工具，参数与结果不给
                parts.push(format!("调用工具：{}（参数略）", calls.join("、")));
            }
            if !m.content.trim().is_empty() {
                parts.push(format!("正文：{}", m.content.trim()));
            }
            if parts.is_empty() {
                parts.push("（空消息）".to_string());
            }
            format!("- 🤖 助手：{}", parts.join("\n  "))
        }
        other => format!("- {other}：{}", m.content.trim()),
    }
}

/// 工具结果的执行时间 / 耗时 / 结果：`(执行时刻, 耗时, 结果)`。
///
/// `started_at` / `ended_at` 是 react-loop 写进 metadata 的 unix 毫秒；缺失时退回消息创建时间。
fn tool_meta(m: &MessageRecord) -> (String, String, String) {
    let meta = m.metadata.as_ref();
    let get_ms = |k: &str| meta.and_then(|v| v.get(k)).and_then(Value::as_i64);
    let (started, ended) = (get_ms("started_at"), get_ms("ended_at"));
    let when = started
        .and_then(|ms| Local.timestamp_millis_opt(ms).single())
        .map(|d| d.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| fmt_unix(m.created_at));
    let cost = match (started, ended) {
        (Some(a), Some(b)) if b >= a => format!("，耗时 {:.1}s", (b - a) as f64 / 1000.0),
        _ => String::new(),
    };
    let status = match meta.and_then(|v| v.get("status")).and_then(Value::as_str) {
        Some("error") => "，结果：失败",
        Some("ok") => "，结果：成功",
        _ => "",
    };
    (when, cost, status.to_string())
}

/// tool_call_id → 工具名（assistant `tool_calls` 是 OpenAI 形状：`{id, type, function:{name}}`）。
fn tool_names(messages: &[MessageRecord]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for m in messages.iter().filter(|m| m.role == "assistant") {
        let Some(calls) = m.tool_calls.as_array() else {
            continue;
        };
        for (id, name) in calls.iter().filter_map(call_id_and_name) {
            map.insert(id, name);
        }
    }
    map
}

/// 本条消息调用了哪些工具（只取名称）。
fn tool_call_names(tool_calls: &Value) -> Vec<String> {
    tool_calls
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| call_id_and_name(c).map(|(_, name)| name))
                .collect()
        })
        .unwrap_or_default()
}

/// 取出一次工具调用的 `(id, 名称)`；兼容 `{function:{name}}` 与扁平 `{id, name}` 两种形状。
fn call_id_and_name(call: &Value) -> Option<(String, String)> {
    let id = call.get("id")?.as_str()?.to_string();
    let name = call
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .or_else(|| call.get("name").and_then(Value::as_str))?
        .to_string();
    Some((id, name))
}

/// 会话状态中文标签（后端 [`astorm_common::types::record::SessionStatus`] 的 serde 取值）。
fn status_label(status: astorm_common::types::record::SessionStatus) -> &'static str {
    use astorm_common::types::record::SessionStatus as S;
    match status {
        S::Running => "运行中",
        S::Done => "已完成",
        S::Error => "失败",
        S::Cancelled => "已取消",
        S::Interrupted => "已中断",
        _ => "空闲",
    }
}

/// unix 秒 → 本地 `YYYY-MM-DD HH:MM:SS`。
fn fmt_unix(sec: i64) -> String {
    Local
        .timestamp_opt(sec, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn mode_label(mode: &str) -> &'static str {
    if mode == MODE_CONTINUOUS_SESSION {
        "持续复用会话"
    } else {
        "每次新建会话"
    }
}

/// 缺省（未指定）= 执行：与调度器 [`crate::scheduler`] 的兜底一致（详见 `effective_session_mode`）。
fn session_mode_label(m: Option<SessionMode>) -> &'static str {
    match m {
        Some(SessionMode::Plan) => "规划",
        Some(SessionMode::Auto) => "自动",
        _ => "执行",
    }
}

fn permission_label(p: Option<PermissionMode>) -> &'static str {
    match p {
        Some(PermissionMode::Auto) => "自动授权",
        _ => "手动授权",
    }
}

/// 智能体展示：`id（名称）`；未指定 = 内置主智能体；解析不到名称时只显示 id。
fn agent_label(agent_type: &Option<String>) -> String {
    let Some(id) = agent_type.as_deref().filter(|s| !s.trim().is_empty()) else {
        return "默认主智能体".into();
    };
    match App::try_global().and_then(|app| app.agents.get(id).ok().flatten()) {
        Some(a) => format!("{}（{}）", a.agent_id, a.name),
        None => id.to_string(),
    }
}

fn validate_mode(mode: &str) -> Result<(), String> {
    if mode == MODE_NEW_SESSION || mode == MODE_CONTINUOUS_SESSION {
        Ok(())
    } else {
        Err(format!(
            "mode 取值无效: {mode}（可用: new_session（每次新建会话）/ continuous_session（持续复用同一会话））"
        ))
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn required_text(value: Option<&str>, field: &str) -> Result<String, String> {
    non_empty(value)
        .map(str::to_string)
        .ok_or_else(|| format!("{field} 不能为空"))
}

/// Plan 模式只允许只读操作（list / search）。
fn ensure_not_plan(ctx: &ToolContext, action: &str) -> Result<(), String> {
    if session_mode(&ctx.session_id) == SessionMode::Plan {
        return Err(format!(
            "当前模式为 Plan，不允许执行「{action}」操作。Plan 模式只允许查看定时任务（list / search），请切换到 Execute 或 Auto 模式后重试"
        ));
    }
    Ok(())
}

fn required_id(args: &ScheduleArgs) -> Result<String, String> {
    args.id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "缺少 id 参数".into())
}

/// 全局调度器句柄。
fn scheduler() -> Result<Arc<Scheduler>, String> {
    App::try_global()
        .map(|app| Arc::clone(&app.scheduler))
        .ok_or_else(|| "App 未初始化，无法访问调度器".into())
}
