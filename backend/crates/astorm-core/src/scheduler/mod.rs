//! 定时任务系统（独立目录）：cron 触发 + SQLite 持久化 + 会话执行。
//!
//! - 记录（[`astorm_common::types::ScheduledTask`]）经 [`Storage`] 门面落 sqlite `scheduled_tasks` 表
//! - 运行期由 `tokio-cron-scheduler` 提供 cron 调度：`start` 注册全部启用任务，`enable/update`
//!   热增删 job（按 `task_id → job uuid` 映射）
//! - 触发时经 `App` 会话门面新建 / 复用会话并注入指令（`new_session` / `continuous_session`），
//!   会话类型标记为 [`astorm_common::types::record::SessionType::Scheduled`]
//! - 表达式统一走 [`normalize_cron`]：5 段（分 时 日 月 周）补秒为 6 段，时区取本地（`Local`），
//!   与前端 `cron-parser` 的「下次执行」预览保持一致

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use astorm_common::schedule::{GeneratedWorkspace, ScheduledTaskRequest};
use astorm_common::session::req::CreateSessionRequest;
use astorm_common::session::types::SessionMode;
use astorm_common::session::Session as SessionFacade;
use astorm_common::types::record::SessionType;
use astorm_common::types::{ScheduledTask, Storage};
use chrono::{Local, Utc};
use tokio::sync::Mutex as AsyncMutex;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

#[cfg(test)]
mod tests;

/// 执行模式：每次触发新建会话。
pub const MODE_NEW_SESSION: &str = "new_session";
/// 执行模式：复用最近一次触发的会话。
pub const MODE_CONTINUOUS_SESSION: &str = "continuous_session";

/// Cron 定时任务调度器（`AppState.scheduler`；进程内单例）。
pub struct Scheduler {
    /// 存储门面（scheduled_tasks 表读写）。
    storage: Arc<dyn Storage>,
    /// 运行中的 JobScheduler 句柄（`start` 后 Some；未启动时热注册静默跳过）。
    runtime: AsyncMutex<Option<JobScheduler>>,
    /// 已注册的 cron job 映射：task_id → job uuid。
    jobs: std::sync::Mutex<HashMap<String, Uuid>>,
    /// 默认调度工作区根目录（`<data_dir>/scheduler`）。
    workspace_base: PathBuf,
}

impl Scheduler {
    /// 构建调度器（`workspace_base` 为默认调度工作区根目录）。
    pub fn new(storage: Arc<dyn Storage>, workspace_base: PathBuf) -> Self {
        Self {
            storage,
            runtime: AsyncMutex::new(None),
            jobs: std::sync::Mutex::new(HashMap::new()),
            workspace_base,
        }
    }

    /// 启动后台调度：加载全部启用任务注册 cron job 并启动（须在 tokio runtime 内调用）。
    pub async fn start(&self) -> Result<(), String> {
        let job_sched = JobScheduler::new().await.map_err(|e| format!("构建调度器失败: {e}"))?;
        let tasks = self.storage.list_scheduled_tasks().map_err(|e| e.to_string())?;
        for task in tasks {
            if task.enabled != 1 {
                continue;
            }
            match register_job(&task, &job_sched).await {
                Ok(uuid) => {
                    if let Ok(mut jobs) = self.jobs.lock() {
                        jobs.insert(task.task_id.clone(), uuid);
                    }
                }
                Err(e) => tracing::warn!("[scheduler] 任务 {} 注册失败: {e}", task.task_id),
            }
        }
        job_sched.start().await.map_err(|e| format!("调度器启动失败: {e}"))?;
        let registered = self.jobs.lock().map(|j| j.len()).unwrap_or(0);
        *self.runtime.lock().await = Some(job_sched);
        tracing::info!("[scheduler] 已启动（{} 个启用任务）", registered);
        Ok(())
    }

    /// 列出全部定时任务（按创建时间倒序）。
    pub async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, String> {
        self.storage.list_scheduled_tasks().map_err(|e| e.to_string())
    }

    /// 读取单个定时任务。
    pub async fn get_task_by_id(&self, task_id: &str) -> Result<ScheduledTask, String> {
        self.storage
            .get_scheduled_task(task_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("task not found: {task_id}"))
    }

    /// 新增定时任务：空工作区时落在默认调度工作区 `<workspace_base>/<task_id>`。
    /// 仅落盘，cron job 由 `enable_task` / `start` 注册。
    pub async fn add_task(&self, task: &ScheduledTask) -> Result<String, String> {
        let mut t = task.clone();
        // 表达式校验：非法值在落盘前就报错（而非注册时静默失败）。
        // 落库保持用户原样的 5 段（前端 `parseCronToFrequency` 与 dto 契约都按 5 段解析），
        // 6 段形式只在注册 cron job 时由 `normalize_cron` 生成。
        validate_cron(&t.cron_expr)?;
        t.cron_expr = t.cron_expr.trim().to_string();
        if t.workspace_path.is_empty() {
            t.workspace_path = self.default_workspace(&t.task_id)?;
        }
        self.storage.upsert_scheduled_task(&t).map_err(|e| e.to_string())?;
        Ok(t.task_id)
    }

    /// 默认调度工作区：`<workspace_base>/<task_id>`（基座 = 用户目录 `schedule/`，
    /// 即 `~/astorm/schedule/<task_id>`），幂等建目录。
    /// 同一任务的多次触发共用一个目录；要隔离时在任务里显式指定会话目录。
    fn default_workspace(&self, task_id: &str) -> Result<String, String> {
        let ws = self.workspace_base.join(task_id);
        std::fs::create_dir_all(&ws).map_err(|e| e.to_string())?;
        Ok(ws.to_string_lossy().into_owned())
    }

    /// 新增定时任务（请求体；`add_scheduled_task` / `schedule_task` 工具）：id 缺省生成，新建默认禁用。
    pub async fn add_from_request(&self, req: &ScheduledTaskRequest) -> Result<String, String> {
        let task = ScheduledTask {
            task_id: req.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            name: req.name.clone(),
            description: req.description.clone(),
            cron_expr: req.cron_expr.clone(),
            mode: if req.mode.is_empty() {
                MODE_NEW_SESSION.into()
            } else {
                req.mode.clone()
            },
            instruction: req.instruction.clone(),
            workspace_path: req.workspace_path.clone(),
            session_mode: req.session_mode,
            permission_mode: req.permission_mode,
            agent_type: resolve_agent_type(req.agent_type.as_deref())?,
            session_id: String::new(),
            enabled: 0,
            created_at: Utc::now().timestamp(),
            updated_at: Utc::now().timestamp(),
        };
        self.add_task(&task).await
    }

    /// 更新定时任务（请求体；`update_scheduled_task`）：按 id 合并字段后热替换 cron job。
    ///
    /// 文本字段为**整包覆盖**（表单每次提交都是全量）；会话参数为 `Option`，采用
    /// **增量语义**（缺省 = 保持原值），避免旧前端/工具不携带时把已配置的参数抹掉。
    /// （`schedule_task` 工具的 update 只给部分字段，由工具侧先读旧记录再合并成全量后调用。）
    pub async fn update_from_request(&self, req: &ScheduledTaskRequest) -> Result<(), String> {
        let id = req.id.clone().ok_or("task id 缺失")?;
        let mut t = self.get_task_by_id(&id).await?;
        t.name = req.name.clone();
        t.description = req.description.clone();
        t.cron_expr = req.cron_expr.clone();
        if !req.mode.is_empty() {
            t.mode = req.mode.clone();
        }
        t.instruction = req.instruction.clone();
        t.workspace_path = req.workspace_path.clone();
        // 会话参数：仅显式给出的才覆盖（空 = 保持）
        if req.session_mode.is_some() {
            t.session_mode = req.session_mode;
        }
        if req.permission_mode.is_some() {
            t.permission_mode = req.permission_mode;
        }
        if req.agent_type.is_some() {
            t.agent_type = resolve_agent_type(req.agent_type.as_deref())?;
        }
        self.update_task(&t).await
    }

    /// 生成调度专用工作区：`<workspace_base>/<uuid>` 并创建目录（`generate_scheduler_workspace`）。
    pub async fn generate_workspace(&self) -> Result<GeneratedWorkspace, String> {
        let id = Uuid::new_v4().to_string();
        let path = self.workspace_base.join(&id);
        std::fs::create_dir_all(&path).map_err(|e| format!("创建调度工作区失败: {e}"))?;
        Ok(GeneratedWorkspace {
            id,
            path: path.to_string_lossy().into_owned(),
        })
    }

    /// 更新定时任务（落盘 + cron job 热替换：已启用则重注册，否则移除）。
    pub async fn update_task(&self, task: &ScheduledTask) -> Result<(), String> {
        let mut t = task.clone();
        // 同 `add_task`：只校验，落库保持用户原样（5 段）
        validate_cron(&t.cron_expr)?;
        t.cron_expr = t.cron_expr.trim().to_string();
        // 会话目录被清空 → 回落默认调度工作区（与新增同一规则，保证目录字段始终有值）
        if t.workspace_path.trim().is_empty() {
            t.workspace_path = self.default_workspace(&t.task_id)?;
        }
        self.storage.upsert_scheduled_task(&t).map_err(|e| e.to_string())?;
        self.remove_job(&t.task_id).await;
        if t.enabled == 1 {
            self.register_job_for(&t).await;
        }
        Ok(())
    }

    /// 删除定时任务（落盘 + 移除 cron job）。
    pub async fn remove_task(&self, task_id: &str) -> Result<(), String> {
        self.storage.delete_scheduled_task(task_id).map_err(|e| e.to_string())?;
        self.remove_job(task_id).await;
        Ok(())
    }

    /// 启用 / 禁用：落盘 + 热增删 cron job。
    pub async fn enable_task(&self, task_id: &str, enabled: bool) -> Result<(), String> {
        let mut t = self.get_task_by_id(task_id).await?;
        t.enabled = if enabled { 1 } else { 0 };
        t.updated_at = Utc::now().timestamp();
        self.storage.upsert_scheduled_task(&t).map_err(|e| e.to_string())?;
        if enabled {
            self.register_job_for(&t).await;
        } else {
            self.remove_job(task_id).await;
        }
        Ok(())
    }

    /// 立即执行一次（不走 cron 排队）。
    pub async fn run_task_now(&self, task_id: &str) -> Result<(), String> {
        let task = self.get_task_by_id(task_id).await?;
        execute_task(&task).await
    }

    /// 停止调度（shutdown JobScheduler + 清空映射；应用退出时调用）。
    pub async fn stop(&self) {
        let mut runtime = self.runtime.lock().await;
        if let Some(mut job_sched) = runtime.take() {
            let _ = job_sched.shutdown().await;
            tracing::info!("[scheduler] 已停止");
        }
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.clear();
        }
    }

    /// 热注册 cron job（调度器未启动时静默跳过，等 `start` 统一加载）。
    async fn register_job_for(&self, task: &ScheduledTask) {
        let runtime = self.runtime.lock().await;
        let Some(job_sched) = runtime.as_ref() else {
            return;
        };
        match register_job(task, job_sched).await {
            Ok(uuid) => {
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.insert(task.task_id.clone(), uuid);
                }
            }
            Err(e) => tracing::warn!("[scheduler] 任务 {} 热注册失败: {e}", task.task_id),
        }
    }

    /// 移除 cron job（按 task_id → job uuid 映射）。
    async fn remove_job(&self, task_id: &str) {
        let uuid = self
            .jobs
            .lock()
            .map(|mut j| j.remove(task_id))
            .unwrap_or(None);
        if let Some(uuid) = uuid {
            let runtime = self.runtime.lock().await;
            if let Some(job_sched) = runtime.as_ref() {
                let _ = job_sched.remove(&uuid).await;
            }
        }
    }
}

/// 校验 cron 表达式（不改变用户写法）：保存路径用，非法值当场报错。
/// 归一化后的 6 段形式只在注册 cron job 时生成（见 [`normalize_cron`]）。
fn validate_cron(expr: &str) -> Result<(), String> {
    normalize_cron(expr).map(|_| ())
}

/// 归一化 cron 表达式为调度器要求的 6 段形式（`秒 分 时 日 月 周`），并校验可解析。
///
/// 前端表单与产品文档一律使用 **5 段**（分 时 日 月 周），而 `tokio-cron-scheduler` 的
/// `Job::new_async*` 以 `with_seconds_required()` 解析 croner 表达式 → 5 段直接报错
/// （结果是任务存得下、显示正常，但 cron job 注册失败、永不触发）。这里统一补 `0 ` 前缀
/// （秒 = 0），并用与注册**完全同一条**解析路径校验，使非法值在保存时就暴露。
pub fn normalize_cron(expr: &str) -> Result<String, String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err("cron 表达式不能为空".into());
    }
    // 顺带把内部多空格归一为单空格（前端 Cron 输入框允许随意空格）
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let normalized = match parts.len() {
        5 => format!("0 {}", parts.join(" ")),
        6 => parts.join(" "),
        n => {
            return Err(format!(
                "cron 表达式段数非法（{n} 段）：应为 5 段「分 时 日 月 周」或 6 段「秒 分 时 日 月 周」"
            ));
        }
    };
    // 真解析一次（与 register_job 同规则：秒必填 + DOM/DOW 组合语义）
    Job::new_async_tz(normalized.as_str(), Local, |_uuid, _lock| Box::pin(async {}))
        .map_err(|e| format!("cron 表达式无效（{trimmed}）: {e}"))?;
    Ok(normalized)
}

/// 注册单个 cron job：触发时执行 `execute_task`（日志告警，不阻塞调度）。
///
/// 时区取 `Local`：前端「下次执行」用 `cron-parser` 的本地时区语义计算，两边必须一致，
/// 否则 UTC 机器上「每天 9 点」会实际在 17 点触发。（`tokio-cron-scheduler` 在 job 创建时
/// 固定偏移量，跨 DST 的时区切换后偏移会陈旧——国内无 DST，暂不处理。）
async fn register_job(task: &ScheduledTask, job_sched: &JobScheduler) -> Result<Uuid, String> {
    let cron_expr = normalize_cron(&task.cron_expr)?;
    let task_clone = task.clone();
    let job = Job::new_async_tz(&cron_expr, Local, move |_uuid, _lock| {
        let t = task_clone.clone();
        Box::pin(async move {
            if let Err(e) = execute_task(&t).await {
                tracing::warn!("[scheduler] 定时任务执行失败 ({}): {e}", t.task_id);
            }
        })
    })
    .map_err(|e| format!("job create error: {e}"))?;
    job_sched.add(job).await.map_err(|e| format!("job add error: {e}"))
}

/// 执行定时任务：按模式新建 / 复用会话，注入指令（`App` 未初始化时调用方在启动后保证已就绪）。
async fn execute_task(task: &ScheduledTask) -> Result<(), String> {
    let app = crate::app::App::global();
    let request = scheduled_request(task)?;
    let now_title = Local::now().format("%Y-%m-%d %H:%M").to_string();

    let sid: String = match task.mode.as_str() {
        MODE_CONTINUOUS_SESSION if !task.session_id.is_empty() => {
            // 复用既有会话：仅追加指令
            let uuid = Uuid::parse_str(&task.session_id).map_err(|e| e.to_string())?;
            app.session().send_message(&uuid, request).map_err(|e| e.to_string())?;
            task.session_id.clone()
        }
        _ => {
            // 新建会话（new_session / continuous_session 首次触发）；首条请求即指令，create 即驱动循环
            let uuid = app.session().create_session(request).map_err(|e| e.to_string())?;
            let sid = uuid.to_string();
            // 回写 task.session_id（continuous_session 后续复用）
            if let Ok(Some(mut t)) = app.storage.get_scheduled_task(&task.task_id) {
                t.session_id = sid.clone();
                t.updated_at = Utc::now().timestamp();
                let _ = app.storage.upsert_scheduled_task(&t);
            }
            sid
        }
    };

    // 会话标题标记触发时间
    if let Ok(Some(mut rec)) = app.storage.get_session(&sid) {
        rec.title = Some(now_title);
        rec.updated_at = crate::agent::session::unix_now();
        let _ = app.storage.upsert_session(&rec);
    }
    Ok(())
}

/// 由定时任务构造会话请求：执行模式 / 审批模式 / 执行智能体 / 会话目录来自任务配置；
/// **模型不选**——一律用设置里配置的默认模型（`llm.defaultModel`，缺省 = 默认供应商的第一个模型），
/// provider 与思考强度也不指定，交给会话默认值。
///
/// 注意：所选智能体卡片自带的执行模式 / 审批模式**优先于**这里的 `session_mode` /
/// `permission_mode`（`create_session` 现有语义，卡片非 `inherit` 时以卡片为准）。
fn scheduled_request(task: &ScheduledTask) -> Result<CreateSessionRequest, String> {
    let app = crate::app::App::global();
    // 定时任务的模型固定取设置里的默认模型：任务不再携带模型选择，
    // 用户改「设置 → 模型」的默认值后，所有任务的下一次触发立即生效。
    let model = app.llm().default_model().unwrap_or_default();
    if model.is_empty() {
        return Err("未配置默认模型（AppConfig.llm.defaultModel 或供应商模型表为空），无法执行定时任务".into());
    }
    // 会话目录：新增 / 编辑时已保证非空（空则填默认调度工作区 `~/astorm/schedule/<task_id>`）
    let workspace = Some(task.workspace_path.clone());
    Ok(CreateSessionRequest {
        model,
        // 不指定供应商：由会话按模型名解析（默认模型所属供应商）
        provider: None,
        // 定时任务会话是顶层会话：无父会话
        pid: None,
        agent_type: task.agent_type.clone(),
        // 会话类型标记：前端侧栏据此归组并显示「定时任务」徽标
        session_type: Some(SessionType::Scheduled),
        // 归属：任务卡片下按时间列出该任务的执行记录
        scheduled_task_id: Some(task.task_id.clone()),
        workspace,
        session_mode: effective_session_mode(task),
        permission_mode: task.permission_mode,
        // 不指定思考强度：用会话默认（默认 = 不上送 provider 的 reasoning_effort）
        reasoning_effort: None,
        user_message: task.instruction.clone(),
        attachments: None,
        // 调度器触发的会话必须执行（建了就跑）
        auto_run: Some(true),
        // 任务指令无前端提及 / 无元素 / 无目标
        mentions: None,
        elements: None,
        office_refs: None,
        quotes: None,
        goal: None,
        goal_lang: None,
    })
}

/// 任务生效的会话执行模式：任务显式指定优先，缺省用**执行模式**。
///
/// 不能直接透传 `task.session_mode`（`None`）：`create_session` 对 `None` 取
/// `SessionMode::default()` = Plan，会让定时任务落在只读模式下（写文件 / 改代码被工具层硬拒），
/// 而页面与工具建任务的默认都是「执行」。所选智能体卡片自带的模式仍**优先于**此值。
fn effective_session_mode(task: &ScheduledTask) -> Option<SessionMode> {
    Some(task.session_mode.unwrap_or(SessionMode::Execute))
}

/// 解析任务里的执行智能体：**id 或名称都认**（模型不知道 id 时可以传「General」这类名称）。
///
/// - 命中的是名称 → 归一化成 `agent_id` 落库（任务里始终存 id）；
/// - 都不命中 → 报错并列出可用项（`id（名称）`），把「智能体不存在」从"触发时才失败"
///   提前到"创建/更新任务时就能看到"；
/// - 名称命中多个 → 提示改用 id（避免歧义）；
/// - 空值 = 不指定（`None`，运行时解析到内置默认主智能体）；App 未初始化（测试等）时跳过校验。
fn resolve_agent_type(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let Some(app) = crate::app::App::try_global() else {
        return Ok(Some(raw.to_string()));
    };
    if app.agents.get(raw)?.is_some() {
        return Ok(Some(raw.to_string()));
    }
    let agents = app.agents.list()?;
    let matches: Vec<&astorm_common::types::AgentRecord> =
        agents.iter().filter(|a| a.name.eq_ignore_ascii_case(raw)).collect();
    match matches.as_slice() {
        [one] => Ok(Some(one.agent_id.clone())),
        [] => {
            let available = agents
                .iter()
                .map(|a| format!("{}（{}）", a.agent_id, a.name))
                .collect::<Vec<_>>()
                .join("、");
            Err(format!(
                "未找到智能体「{raw}」；可用：{}",
                if available.is_empty() { "（无：尚未播种内置智能体）".to_string() } else { available }
            ))
        }
        many => Err(format!(
            "智能体名称「{raw}」对应 {} 个卡片（{}），请改用 id 指定",
            many.len(),
            many.iter().map(|a| a.agent_id.as_str()).collect::<Vec<_>>().join("、")
        )),
    }
}
