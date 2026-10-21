//! `POST /api/v1/bridge`：Web 前端 `backend.invoke(command, args)` 的服务端路由。
//!
//! 与桌面版 `main/backend.ts` 的命令表对齐：
//! - **napi 命令**（~85 个）→ core facade 直调（同一批 astorm-common trait）；
//! - **主进程本地命令**（settings / AGENTS.md / MCP 配置文件）→ 服务端等价实现；
//! - **明确降级**：插件运行时、Chrome cookie 导入未移植，返回可读错误
//! - **MCP 服务器管理**（list / 配置读写 / 启停 / 连接）→ core 的 `mcp` 模块（stdio 常驻
//!   子进程 + HTTP JSON-RPC，工具连接后直接进会话工具集）。
//!
//! 参数与返回值直接走 `serde_json::Value`（前端剥离 reactive 后已是纯数据）；
//! facade 返回类型均 `Serialize`，错误统一映射 APIError。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use astorm_common::agent::Agent as _;
use astorm_common::file::File as _;
use astorm_common::git::Git as _;
use astorm_common::knowledge::Knowledge as _;
use astorm_common::schedule::Schedule as _;
use astorm_common::session::Session as _;
use astorm_common::skill::Skill as _;
use astorm_common::taskplan::TaskPlan as _;
use astorm_common::tool::Tools as _;

use crate::error::ApiError;
use crate::local;
use crate::state::ServerState;

type R = Result<Json<Value>, ApiError>;

fn app() -> Result<std::sync::Arc<astorm_core::app::AppState>, ApiError> {
    astorm_core::app::App::try_global().cloned().ok_or_else(|| ApiError::internal("应用未初始化"))
}

fn ser<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn s(a: &Value, key: &str) -> String {
    a.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn opt_s(a: &Value, key: &str) -> Option<String> {
    a.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// args 字段（或 args 本身）→ 强类型反序列化。
fn de<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, ApiError> {
    serde_json::from_value(v.clone())
        .map_err(|e| ApiError::bad_request(format!("参数解析失败: {e}")))
}

fn opt_de<T: serde::de::DeserializeOwned>(a: &Value, key: &str) -> Result<Option<T>, ApiError> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(de(v)?)),
    }
}

/// 未移植能力的统一降级错误。
fn not_ported(what: &str) -> ApiError {
    ApiError::bad_request(format!("{what} 暂未在云端服务端实现（Phase B 移植中）"))
}

/// `POST /api/v1/bridge`。
pub async fn invoke(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> R {
    let command = s(&body, "command");
    let a = body.get("args").cloned().unwrap_or(json!({}));
    let v = route(&state, &command, &a).await?;
    Ok(Json(v))
}

async fn route(state: &ServerState, command: &str, a: &Value) -> Result<Value, ApiError> {
    let app = app()?;

    /* ---------- 文件 / 审计 / 用量 / 交付物 ---------- */
    let v = match command {
        "list_directory" => ser(app.file().list_directory(s(a, "dir")).map_err(ApiError::from)?),
        "read_file_content" => ser(app.file().read_file_content(s(a, "path")).map_err(ApiError::from)?),
        /* ---------- 会话目标 ---------- */
        // 与 napi updateSessionGoal 同语义：goal=None/空白 = 删除，否则改写并广播 session_info。
        "update_session_goal" => {
            let id = uuid::Uuid::parse_str(&s(a, "session_id"))
                .map_err(|e| ApiError::internal(format!("session_id 非法: {e}")))?;
            app.session()
                .update_goal(&id, opt_s(a, "goal"), opt_s(a, "goal_lang"))
                .map_err(ApiError::from)?;
            json!(null)
        }
        "read_audit_log" => json!(app.audit_log().read_session_json(&s(a, "session_id"))),
        "usage_stats" => ser(
            app.usage_stats(a.get("since_ts").and_then(|v| v.as_i64()).unwrap_or(0))
                .map_err(ApiError::from)?,
        ),
        // napi 契约返回 JSON **字符串**（渲染层 `typeof raw === 'string' ? JSON.parse(raw) : []`）
        "session_deliverables" => json!(
            serde_json::to_string(&app.list_deliverables(&s(a, "session_id")).map_err(ApiError::from)?)
                .unwrap_or_else(|_| "[]".to_string())
        ),
        "all_deliverables" => json!(
            serde_json::to_string(
                &app.list_all_deliverables(a.get("limit").and_then(|v| v.as_i64()).unwrap_or(500))
                    .map_err(ApiError::from)?
            )
            .unwrap_or_else(|_| "[]".to_string())
        ),

        /* ---------- 安全配置 ---------- */
        "builtin_danger_patterns" => ser(astorm_core::tool::policy::builtin_danger_patterns()),
        "security_defaults" => ser(astorm_core::app::config::SecurityConfig::default()),

        /* ---------- 会话大纲 / 页签镜像 ---------- */
        "list_plan_files" => ser(app.taskplan().list_plan_files(s(a, "session_id")).map_err(ApiError::from)?),
        "read_plan_file" => ser(
            app.taskplan()
                .read_plan_file(astorm_common::taskplan::ReadPlanFileRequest {
                    session_id: s(a, "session_id"),
                    task_id: s(a, "task_id"),
                })
                .map_err(ApiError::from)?,
        ),
        "update_tabs_state" => {
            let tabs: astorm_core::tool::tabs::TabsState =
                serde_json::from_str(&s(a, "state_json"))
                    .map_err(|e| ApiError::bad_request(format!("tabs 状态解析失败: {e}")))?;
            astorm_core::tool::tabs::update_state(&s(a, "session_id"), tabs);
            json!(null)
        }

        /* ---------- 终端 PTY（输出经全局事件流 type:pty 回传，web 走 WS）---------- */
        "pty_open" => ser(
            app.pty()
                .open(
                    &s(a, "session_id"),
                    u16::try_from(a.get("cols").and_then(|v| v.as_u64()).unwrap_or(80)).unwrap_or(80),
                    u16::try_from(a.get("rows").and_then(|v| v.as_u64()).unwrap_or(24)).unwrap_or(24),
                )
                .map_err(ApiError::from)?,
        ),
        "pty_write" => ser(app.pty().write(&s(a, "session_id"), &s(a, "data")).map_err(ApiError::from)?),
        "pty_resize" => ser(
            app.pty()
                .resize(
                    &s(a, "session_id"),
                    u16::try_from(a.get("cols").and_then(|v| v.as_u64()).unwrap_or(80)).unwrap_or(80),
                    u16::try_from(a.get("rows").and_then(|v| v.as_u64()).unwrap_or(24)).unwrap_or(24),
                )
                .map_err(ApiError::from)?,
        ),
        "pty_close" => ser(app.pty().close(&s(a, "session_id")).map_err(ApiError::from)?),

        /* ---------- git（core facade）---------- */
        "git_init" => ser(app.git().git_init(s(a, "dir")).map_err(ApiError::from)?),
        "git_status" => ser(app.git().git_status(s(a, "dir")).map_err(ApiError::from)?),
        "git_ignored" => ser(app.git().git_ignored(s(a, "dir")).map_err(ApiError::from)?),
        "git_diff_summary" => ser(
            app.git()
                .get_git_diff_summary(s(a, "dir"), a.get("staged").and_then(|v| v.as_bool()).unwrap_or(false))
                .map_err(ApiError::from)?,
        ),
        "git_file_diff" => ser(
            app.git()
                .get_git_file_diff(s(a, "dir"), s(a, "path"), a.get("staged").and_then(|v| v.as_bool()).unwrap_or(false))
                .map_err(ApiError::from)?,
        ),
        "git_current_branch" => ser(app.git().git_current_branch(s(a, "dir")).map_err(ApiError::from)?),
        // 只读探测类 git 查询：**目录不存在或命令失败一律给空**——`run_git` 失败时第二项是
        // stderr 末行（"fatal: cannot change to '/data/workspace/…'" 之类），照原样当结果用会
        // 把错误文本渲染成「分支名」（普通任务的工作区目录被清掉后就会这样，实测踩过）。
        // 语义对齐桌面主进程 git:branch-state / git:list-branches（`!hasDir → 空`、`!ok → 空`）。
        "git_is_repo" => {
            let dir = s(a, "dir");
            let r = if has_dir(&dir) {
                run_git(&dir, &["rev-parse", "--is-inside-work-tree"])
            } else {
                (false, String::new())
            };
            json!(r.0 && r.1.trim() == "true")
        }
        "git_list_branches" => {
            let dir = s(a, "dir");
            let r = if has_dir(&dir) {
                run_git(&dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
            } else {
                (false, String::new())
            };
            json!(if r.0 {
                r.1.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>()
            } else {
                Vec::new()
            })
        }
        "git_branch_state" => {
            let dir = s(a, "dir");
            let empty = json!({ "current": "", "local": [], "remotes": [], "uncommitted": 0 });
            if !has_dir(&dir) {
                empty
            } else {
                let head = run_git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
                // HEAD 失败 = 非仓库 / 尚无提交（unborn HEAD）→ 全空，胶囊自然隐藏
                if !head.0 {
                    empty
                } else {
                    let local = run_git(&dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"]);
                    let remotes = run_git(&dir, &["for-each-ref", "--format=%(refname:short)", "refs/remotes/"]);
                    let status = run_git(&dir, &["status", "--porcelain"]);
                    json!({
                        "current": head.1.lines().next().unwrap_or("").trim(),
                        "local": if local.0 {
                            local.1.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        },
                        "remotes": if remotes.0 {
                            remotes.1.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        },
                        "uncommitted": if status.0 {
                            status.1.lines().filter(|l| !l.trim().is_empty()).count()
                        } else {
                            0
                        },
                    })
                }
            }
        }
        "git_log" => {
            let limit = usize::try_from(a.get("limit").and_then(|v| v.as_u64()).unwrap_or(50)).unwrap_or(50).clamp(1, 200);
            let r = run_git(
                &s(a, "dir"),
                &[
                    "log",
                    &format!("--max-count={limit}"),
                    "--date=iso",
                    "--pretty=format:%h%x1f%an%x1f%ad%x1f%ar%x1f%s%x1f%D%x1e",
                    &format!("refs/heads/{}", s(a, "branch")),
                ],
            );
            let mut commits = Vec::new();
            for raw in r.1.split('\u{1e}') {
                let rec = raw.trim_start();
                if rec.is_empty() {
                    continue;
                }
                let f: Vec<&str> = rec.split('\u{1f}').collect();
                commits.push(json!({
                    "hash": f.first().unwrap_or(&"").trim(),
                    "author": f.get(1).unwrap_or(&"").trim(),
                    "dateISO": f.get(2).unwrap_or(&"").trim(),
                    "dateRel": f.get(3).unwrap_or(&"").trim(),
                    "subject": f.get(4).unwrap_or(&""),
                    "decorations": f.get(5).unwrap_or(&"").split(',').map(|d| d.trim()).filter(|d| !d.is_empty()).collect::<Vec<_>>(),
                }));
            }
            if r.0 { json!({ "commits": commits }) } else { json!({ "commits": [], "error": r.1 }) }
        }
        "git_checkout" => {
            let r = run_git(&s(a, "dir"), &["checkout", "--", s(a, "branch").trim()]);
            if r.0 { json!({ "ok": true }) } else { json!({ "ok": false, "error": r.1 }) }
        }
        "git_create_branch" => {
            let r = run_git(&s(a, "dir"), &["checkout", "-b", &s(a, "name").trim(), "--"]);
            if r.0 { json!({ "ok": true }) } else { json!({ "ok": false, "error": r.1 }) }
        }

        /* ---------- skills ---------- */
        "list_skills" => ser(
            app.skills()
                .list_skills(a.get("enabledOnly").and_then(|v| v.as_bool()), opt_s(a, "category"))
                .map_err(ApiError::from)?,
        ),
        "search_skills" => ser(
            app.skills()
                .search_skills(s(a, "query"), a.get("limit").and_then(|v| v.as_u64()).map(|v| usize::try_from(v).unwrap_or(usize::MAX)))
                .map_err(ApiError::from)?,
        ),
        "get_skill" => ser(app.skills().get_skill(s(a, "name")).map_err(ApiError::from)?),
        "create_skill" => ser(app.skills().create_skill(de(a.get("req").unwrap_or(&json!({})))?).map_err(ApiError::from)?),
        "update_skill" => ser(
            app.skills()
                .update_skill(s(a, "name"), de(a.get("updates").unwrap_or(&json!({})))?)
                .map_err(ApiError::from)?,
        ),
        "toggle_skill" => ser(
            app.skills()
                .toggle_skill(s(a, "name"), a.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
                .map_err(ApiError::from)?,
        ),
        "remove_skill" => ser(app.skills().remove_skill(s(a, "name")).map_err(ApiError::from)?),
        "discover_skills" => ser(app.skills().discover_skills().map_err(ApiError::from)?),
        "import_skills" => ser(app.skills().import_skills(de(a.get("paths").unwrap_or(&json!([])))?).map_err(ApiError::from)?),
        "import_skill_package" => ser(app.skills().import_skill_package(s(a, "path")).map_err(ApiError::from)?),

        /* ---------- agents ---------- */
        "list_agents" => ser(app.agents().list_agents().map_err(ApiError::from)?),
        "get_agent" => ser(app.agents().get_agent(s(a, "id")).map_err(ApiError::from)?),
        "create_agent" => ser(app.agents().create_agent(de(a.get("req").unwrap_or(&json!({})))?).map_err(ApiError::from)?),
        "update_agent" => ser(
            app.agents()
                .update_agent(s(a, "id"), de(a.get("updates").unwrap_or(&json!({})))?)
                .map_err(ApiError::from)?,
        ),
        "remove_agent" => ser(app.agents().remove_agent(s(a, "id")).map_err(ApiError::from)?),

        /* ---------- tools ---------- */
        "list_tools" => ser(app.tool().list_tools(opt_s(a, "category")).map_err(ApiError::from)?),
        "search_tools" => ser(
            app.tool()
                .search_tools(s(a, "query"), a.get("limit").and_then(|v| v.as_u64()).map(|v| usize::try_from(v).unwrap_or(usize::MAX)))
                .map_err(ApiError::from)?,
        ),

        /* ---------- 知识库 ---------- */
        // 根目录（UI 新建/导入需要 parent 起点/memory 页展示根路径）
        "kb_root" => json!(app.knowledge().root().to_string_lossy()),
        "memory_root" => json!(app.memory().root().to_string_lossy()),
        "kb_list" => ser(app.knowledge().list().map_err(ApiError::from)?),
        "kb_create" => ser(app.knowledge().create(de(a.get("req").unwrap_or(&json!({})))?).map_err(ApiError::from)?),
        "kb_rename" => ser(app.knowledge().rename(s(a, "path"), s(a, "new_name")).map_err(ApiError::from)?),
        "kb_delete" => ser(app.knowledge().delete(s(a, "path")).map_err(ApiError::from)?),
        "kb_update_meta" => ser(app.knowledge().update_meta(de(a.get("meta").unwrap_or(&json!({})))?).map_err(ApiError::from)?),
        "kb_read" => ser(app.knowledge().read(s(a, "path")).map_err(ApiError::from)?),
        // 桌面 napi 返回 JSON 字符串（KB 文档图片渲染走 markdownImages 的 base64）
        "kb_read_buffer" => {
            let bytes = app.knowledge().read_bytes(s(a, "path")).map_err(ApiError::from)?;
            json!({
                "base64": base64_encode(&bytes),
                "mime": sniff_kb_mime(&s(a, "path")),
            })
        }
        // 桌面 napi/kb_import 契约是 `paths`；早期服务端写成了 `sources`。两者都接，
        // 避免 Web UI 导入静默 0 项。
        "kb_import" => {
            let sources = a
                .get("sources")
                .or_else(|| a.get("paths"))
                .cloned()
                .unwrap_or_else(|| json!([]));
            ser(app
                .knowledge()
                .import(s(a, "parent"), de(&sources)?)
                .map_err(ApiError::from)?)
        }
        "kb_write" => ser(app.knowledge().write(s(a, "path"), s(a, "content")).map_err(ApiError::from)?),

        /* ---------- 定时任务 ---------- */
        "list_scheduled_tasks" => ser(app.schedule().list_scheduled_tasks().map_err(ApiError::from)?),
        "generate_scheduler_workspace" => ser(app.schedule().generate_scheduler_workspace().map_err(ApiError::from)?),
        "add_scheduled_task" => ser(app.schedule().add_scheduled_task(de(a)?).map_err(ApiError::from)?),
        "update_scheduled_task" => ser(app.schedule().update_scheduled_task(de(a)?).map_err(ApiError::from)?),
        "delete_scheduled_task" => ser(app.schedule().delete_scheduled_task(s(a, "id")).map_err(ApiError::from)?),
        "toggle_scheduled_task" => ser(
            app.schedule()
                .toggle_scheduled_task(s(a, "id"), a.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
                .map_err(ApiError::from)?,
        ),
        "run_scheduled_task_now" => ser(app.schedule().run_scheduled_task_now(s(a, "id")).map_err(ApiError::from)?),

        /* ---------- 长期记忆 ---------- */
        "memory_status" => ser(app.memory().status().map_err(ApiError::from)?),
        "memory_list_concepts" => ser(app.memory().list_concepts().map_err(ApiError::from)?),
        "memory_show_concept" => ser(app.memory().show_concept(&s(a, "concept_id")).map_err(ApiError::from)?),
        "memory_search" => ser(
            app.memory()
                .search(&s(a, "query"), a.get("limit").and_then(|v| v.as_u64()).map(|v| usize::try_from(v).unwrap_or(usize::MAX)).unwrap_or(20))
                .map_err(ApiError::from)?,
        ),
        "memory_validate" => {
            let report = app.memory().validate();
            let issue = |i: &astorm_memory::validate::ValidateIssue| {
                json!({
                    "level": format!("{:?}", i.level).to_lowercase(),
                    "conceptId": i.concept_id,
                    "message": i.message,
                })
            };
            json!({
                "checked": report.checked,
                "errors": report.issues.iter().filter(|i| matches!(i.level, astorm_memory::validate::IssueLevel::Error)).map(issue).collect::<Vec<_>>(),
                "warnings": report.issues.iter().filter(|i| !matches!(i.level, astorm_memory::validate::IssueLevel::Error)).map(issue).collect::<Vec<_>>(),
            })
        }
        "memory_read_log" => ser(app.memory().read_log(a.get("limit").and_then(|v| v.as_u64()).map(|v| usize::try_from(v).unwrap_or(usize::MAX)).unwrap_or(50))),
        "memory_list_raw_sources" => ser(app.memory().list_raw_sources().map_err(ApiError::from)?),
        "memory_read_raw_source" => ser(app.memory().read_raw_source(&s(a, "stem")).map_err(ApiError::from)?),
        "memory_reindex" => ser(app.memory().reindex().map_err(ApiError::from)?),
        "memory_cancel" => ser(app.memory().cancel()),
        "memory_compile" => {
            let request = astorm_core::facade::memory::MemoryCompileRequest {
                force: a.get("force").and_then(|v| v.as_bool()).unwrap_or(false),
                dry_run: a.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false),
            };
            ser(app.memory().compile(request).map_err(ApiError::from)?)
        }

        /* ---------- 邮件（astorm_mail::MailService，async）---------- */
        "mail_state" => ser(app.mail().state().map_err(mail_err)?),
        "mail_add_account" => ser(app.mail().add_account(de(a)?).map_err(mail_err)?),
        "mail_update_account" => ser(
            app.mail()
                .update_account(&s(a, "name"), de(a.get("input").unwrap_or(&json!({})))?)
                .map_err(mail_err)?,
        ),
        "mail_remove_account" => ser(app.mail().remove_account(&s(a, "name")).map_err(mail_err)?),
        "mail_activate_account" => ser(app.mail().activate(&s(a, "name")).map_err(mail_err)?),
        "mail_folders" => ser(app.mail().folders(opt_s(a, "account")).await.map_err(mail_err)?),
        "mail_folder_counts" => ser(app.mail().folder_counts(opt_s(a, "account")).await.map_err(mail_err)?),
        "mail_messages" => ser(app.mail().messages(de(a)?).await.map_err(mail_err)?),
        "mail_previews" => ser(
            app.mail()
                .previews(
                    opt_s(a, "folder"),
                    opt_s(a, "account"),
                    de(a.get("uids").unwrap_or(&json!([])))?,
                )
                .await
                .map_err(mail_err)?,
        ),
        "mail_message" => ser(
            app.mail()
                .message(
                    u32::try_from(a.get("uid").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    opt_s(a, "folder"),
                    opt_s(a, "account"),
                    a.get("refresh").and_then(|v| v.as_bool()).unwrap_or(false),
                )
                .await
                .map_err(mail_err)?,
        ),
        "mail_set_meta" => ser(
            app.mail()
                .set_meta(
                    u32::try_from(a.get("uid").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    opt_s(a, "folder"),
                    opt_s(a, "account"),
                    de(a.get("body").unwrap_or(&json!({})))?,
                )
                .await
                .map_err(mail_err)?,
        ),
        "mail_move" => ser(
            app.mail()
                .move_message(
                    u32::try_from(a.get("uid").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    opt_s(a, "folder"),
                    opt_s(a, "account"),
                    s(a, "dest"),
                )
                .await
                .map_err(mail_err)?,
        ),
        "mail_attachment" => {
            let blob = app
                .mail()
                .attachment(
                    u32::try_from(a.get("uid").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    usize::try_from(a.get("index").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    opt_s(a, "folder"),
                    opt_s(a, "account"),
                )
                .await
                .map_err(mail_err)?;
            json!({ "filename": blob.filename, "base64": base64_encode(&blob.data) })
        }
        "mail_attachment_to_temp" => ser(
            app.mail()
                .attachment_to_temp(
                    u32::try_from(a.get("uid").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    usize::try_from(a.get("index").and_then(|v| v.as_u64()).unwrap_or(0)).unwrap_or(0),
                    opt_s(a, "folder"),
                    opt_s(a, "account"),
                )
                .await
                .map_err(mail_err)?,
        ),
        "mail_send" => ser(app.mail().send(de(a)?).await.map_err(mail_err)?),

        /* ---------- 服务端本地命令（settings / AGENTS.md / MCP 文件）---------- */
        // 设置聚合契约与桌面 backend.ts 一致：get 返回全量聚合（缺段补默认），
        // save 取 args.settings 整体覆盖（渲染层各设置页「读全量→改自己段→写全量」）
        "get_settings" => json!(local::settings_aggregate(&state.local)),
        "save_settings" => {
            let settings = a.get("settings").cloned().unwrap_or_else(|| a.clone());
            local::save_settings(&state.local, &settings)?;
            json!({ "ok": true })
        }
        "read_global_agents" => json!(local::read_global_agents()),
        "write_global_agents" => {
            local::write_global_agents(&s(a, "content"))?;
            json!({ "ok": true })
        }
        // 桌面 read 返回**原始文本**（渲染层编辑器直接展示）；缺失回退桌面同款初始文本
        "read_mcp_config_file" => json!(state.app.mcp().config_text()),
        // 桌面 write 收 {content: "<json 文本>"}：校验后落盘 → 重载 → 连接启用项
        "write_mcp_config_file" => {
            let text = a.get("content").and_then(|c| c.as_str()).unwrap_or("");
            state
                .app
                .mcp()
                .write_config(text)
                .await
                .map_err(ApiError::bad_request)?;
            json!({ "ok": true })
        }
        // list 返回行数组 {name, transportType, command, args, url, enabled, connected, authRequired}
        // （camelCase 与渲染层 DTO 一致；见 astorm-core 的 mcp 模块）
        "list_mcp_servers" => json!(state.app.mcp().list_servers()),
        // 桌面 toggle 只改标志（禁用即断开）；连接由 connect 命令或启动自动连接负责
        "toggle_mcp_server" => {
            let name = s(a, "name");
            let enabled = a.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
            state
                .app
                .mcp()
                .toggle(&name, enabled)
                .map_err(ApiError::bad_request)?;
            json!({ "ok": true })
        }
        "connect_mcp_server" => {
            let name = s(a, "name");
            let message = state
                .app
                .mcp()
                .connect(&name)
                .await
                .map_err(ApiError::bad_request)?;
            json!(message)
        }
        "disconnect_mcp_server" => {
            let name = s(a, "name");
            state
                .app
                .mcp()
                .disconnect(&name)
                .map_err(ApiError::bad_request)?;
            json!({ "ok": true })
        }
        "list_mcp_server_tools" => json!(state.app.mcp().list_tools(&s(a, "server"))),
        // 手动授权头（Bearer / 自定义 Header）云端同样可用；OAuth 浏览器授权需要与
        // 浏览器同机的回环回调，云容器无法接收，明确降级（设置页仍有手动表单）。
        "set_mcp_server_auth" => {
            let name = s(a, "name");
            let headers = a
                .get("headers")
                .and_then(|h| h.as_str())
                .unwrap_or("{}")
                .to_string();
            let message = state
                .app
                .mcp()
                .set_server_auth(&name, &headers)
                .await
                .map_err(ApiError::bad_request)?;
            json!(message)
        }
        "begin_mcp_oauth" => Err(not_ported("MCP OAuth 浏览器授权（云端需公网回调）"))?,
        "mcp_oauth_status" => json!({ "status": "none", "error": null }),

        /* ---------- 明确降级（插件 / 浏览器数据 / 本地环境探测）---------- */
        "plugin_scan" | "plugin_install" | "plugin_remove" | "plugin_set_enabled"
        | "plugin_save_config" | "plugin_dir" => Err(not_ported("插件系统"))?,
        "import_chrome_cookies" => Err(not_ported("Chrome cookie 导入（云容器无本机浏览器）"))?,
        "clear_browser_cache" | "clear_browser_data" => json!({ "ok": true }),
        "detect_local_env" => json!([]),
        "toolchain_allowances" => json!([]),

        _ => return Err(ApiError::bad_request(format!("未知命令: {command}"))),
    };
    Ok(v)
}

/// 运行 git（与桌面 ipc.ts runGit 同语义：8MB 缓冲 / 8s 超时 / (ok, stdout|stderr)）。
/// 目录存在且是目录（与桌面主进程 `hasDir` 同义）：git 命令对不存在的 `-C` 目标只会吐
/// `fatal: cannot change to …`，读起来像"分支名"；先挡一道。
fn has_dir(dir: &str) -> bool {
    std::fs::metadata(dir).map(|m| m.is_dir()).unwrap_or(false)
}

fn run_git(dir: &str, args: &[&str]) -> (bool, String) {
    let mut child = match std::process::Command::new("git")
        // 非 ASCII 文件名默认会被 quote 成八进制转义，导致后续按路径取 diff 失败
        .args(["-c", "core.quotePath=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (false, e.to_string()),
    };
    // 先并发抽干 stdout/stderr 再等退出：大仓库的 status/log 超过管道缓冲时，
    // 子进程会阻塞在 write，轮询等待只会等到超时。
    use std::io::Read;
    let stdout_thread = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_thread = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    // 同步调用阻塞 worker：轮询等待，8s 超时强杀（与桌面 execFile timeout 一致，
    // 防止大仓库操作卡死共享 runtime）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => return (false, e.to_string()),
        }
    };
    let out_bytes = stdout_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();
    let err_bytes = stderr_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();
    let stdout = String::from_utf8_lossy(&out_bytes);
    let stderr = String::from_utf8_lossy(&err_bytes);
    match status {
        Some(st) if st.success() => (true, stdout.into_owned()),
        Some(_) => {
            let last = stderr.trim().lines().last().unwrap_or("git 执行失败").to_string();
            (false, last)
        }
        None => (false, "git 执行超时（8s）".to_string()),
    }
}

/// 服务端 config 目录（settings/providers/mcp_servers.json 所在）。
fn config_dir() -> std::path::PathBuf {
    astorm_core::app::App::try_global()
        .map(|app| app.dir().config_dir)
        .unwrap_or_else(std::env::temp_dir)
}

/// KB 文件 MIME（按扩展名粗判；与桌面 kbFiles.ts 的 MIME 表同源的最小集）。
fn sniff_kb_mime(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "md" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn mail_err(e: astorm_mail::error::MailError) -> ApiError {
    ApiError::internal(e.to_string())
}

/// base64 编码（无新依赖：手写标准表）。
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}
