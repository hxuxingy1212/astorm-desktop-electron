//! 应用门面 + 全局单例（进程基础设施：runtime / 客户端 / 事件总线 / 会话注册表）。

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use astorm_common::types::Storage;
use serde_json::Value;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

use crate::agent::session::SessionImpl;
use crate::scheduler::Scheduler;
use crate::skill::{SkillIndex, SkillManager};
use crate::tool::ToolRegistry;
use self::paths::resolve_log_dir;

/// 应用配置：前端 `start` 传入的 JSON 解析结果（见 [`config`] 模块）。
pub mod config;

pub use config::{AppConfig, CompactConfig, GoalConfig, InstructionConfig, LlmConfig, LlmModel, LlmProvider, LogConfig, SummaryConfig};

/// 长期记忆配置（记忆管道的配置契约由 `astorm-memory` 拥有，core 只保存 + 热更）。
pub use astorm_memory::config::MemoryConfig;

/// 多供应商 LLM 客户端注册表（客户端路由 + 模型名解析；见 [`llm`] 模块）。
pub mod llm;

pub use llm::{LlmRegistry, LlmRegistryHandle};

/// 单次 LLM 补全（插件 SDK `llm.chat` 复用本应用 LLM 调用）。
pub mod ask_llm;

pub use ask_llm::AskLlmRequest;

/// 应用路径（平台数据根 + 路径集解析；见 [`paths`] 模块）。
pub mod paths;

pub use paths::{AppDir, user_data_dir};

/// 全局事件总线容量（超出后最慢订阅者丢事件，见 subscribe 的 lag 处理）。
const EVENT_CAPACITY: usize = 4096;

/// 应用进程级状态：全局单例（App 门面背后的实际数据）。
///
/// `runtime` / `llm` 仅为保持其生命周期（runtime 线程驱动循环、注册表持有各供应商
/// 客户端），不直接读取，故关闭 dead_code 提示。
#[allow(dead_code)]
pub struct AppState {
    pub(crate) config: AppConfig,
    pub(crate) runtime: Runtime,
    pub(crate) llm: LlmRegistryHandle,
    pub(crate) event_tx: broadcast::Sender<Value>,
    pub(crate) sessions: SessionImpl,
    /// 剪支配置（共享热更值：前端保存后 `update_config` 写入，运行中会话下轮生效）。
    pub(crate) compact: Arc<RwLock<CompactConfig>>,
    /// 摘要配置（共享热更值：前端保存后 `update_config` 写入，运行中会话下轮生效）。
    pub(crate) summary: Arc<RwLock<SummaryConfig>>,
    /// 目标循环配置（共享热更值：`/goal` 外循环的 token / 迭代预算）。
    pub(crate) goal: Arc<RwLock<GoalConfig>>,
    /// 安全配置（共享热更值：沙箱防护策略，`bash_protection` / file 空间矩阵即时读取）。
    pub(crate) security: Arc<RwLock<crate::app::config::SecurityConfig>>,
    /// 长期记忆配置（共享热更值：前端保存后 `update_config` 写入；下次编译/查询即生效）。
    pub(crate) memory_cfg: Arc<RwLock<MemoryConfig>>,
    /// 约定文件配置（共享热更值：AGENTS.md 注入开关；运行中会话下一轮提示词即生效）。
    pub(crate) instruction_cfg: Arc<RwLock<InstructionConfig>>,
    /// 页面渲染服务配置（共享热更值：websearch / webfetch 共用的内置 Chromium 渲染）。
    pub(crate) web_render_cfg: Arc<RwLock<crate::app::config::WebRenderConfig>>,
    /// 页面渲染服务（常驻单实例：预热 / 并发上限 / 排队 / 节流 / 空闲回收）。
    pub(crate) web_render: Arc<crate::tool::search::WebRender>,
    /// 存储门面（会话元数据 / 消息历史 / 全文检索；由装配层组合注入）。
    pub(crate) storage: Arc<dyn Storage>,
    /// 内置工具注册中心（构建各会话循环的 `ctx.tools`）。
    pub(crate) tools: Arc<ToolRegistry>,
    /// MCP 服务器管理器（配置读写 / 连接 / 工具发现与调用；工具按当前连接进入会话循环）。
    pub(crate) mcp: Arc<crate::mcp::McpManager>,
    /// 服务端浏览器后端（容器形态；宿主启动后注册，桌面为 None → browser 工具走 CLI 桥）。
    pub(crate) native_browser: RwLock<Option<Arc<dyn crate::tool::browser::native::NativeBrowser>>>,
    /// 技能管理器（SKILL.md 索引；`skills` 工具与提示词注入经它访问）。
    pub(crate) skill: Arc<SkillManager>,
    /// 智能体管理器（智能体卡片：工具集 / 技能集 / 额外系统指引；会话创建经它解析）。
    pub(crate) agents: Arc<crate::agent_registry::AgentRegistry>,
    /// 定时任务调度器（cron；`schedule_task` 工具经它访问）。
    pub(crate) scheduler: Arc<Scheduler>,
    // 应用模块门面（接口骨架：file / git / tool / skills / schedule / taskplan，实现待补充）
    pub(crate) file: crate::facade::FileImpl,
    pub(crate) git: crate::facade::GitImpl,
    pub(crate) tool: crate::facade::ToolImpl,
    pub(crate) skills: crate::facade::SkillImpl,
    pub(crate) agent: crate::facade::AgentImpl,
    pub(crate) schedule: crate::facade::ScheduleImpl,
    pub(crate) terminal: crate::facade::TerminalImpl,
    pub(crate) taskplan: crate::facade::TaskPlanImpl,
    /// 终端 PTY 会话管理器(交互式 shell,xterm.js 前端对接)。
    pub(crate) pty: crate::facade::PtyManager,
    /// 知识库门面（真实文件树 + 元数据清单,持有根目录）。
    pub(crate) knowledge: Arc<crate::facade::KnowledgeImpl>,
    /// 邮件服务（多账号 IMAP/SMTP + 本地缓存；mail 工具与前端邮箱界面共用）。
    pub(crate) mail: Arc<astorm_mail::MailService>,
    /// 长期记忆门面（OKF 编译管道：配置解析 / 来源收集 / 进度转发 / 取消）。
    pub(crate) memory: Arc<crate::facade::MemoryImpl>,
    /// 安全审计日志写入器（`<log_dir>/audit/{session_id}.jsonl`）。
    pub(crate) audit: Arc<crate::tool::audit::AuditLog>,
    /// 审批台账（文件 / bash 需审批时登记 + 用户同意/拒绝 + 一次性重放消费）。
    pub(crate) interrupts: Arc<crate::tool::interrupt::InterruptRegistry>,
    /// 会话级内存白名单（审批卡「当前会话允许」；**重启丢失**，按会话隔离）。
    pub(crate) session_allow: Arc<crate::tool::session_allow::SessionAllowRegistry>,
    /// 日志 WorkerGuard：shutdown 时 drop 以 flush 文件 appender。
    pub(crate) log_guard: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
}

static APP: OnceLock<Arc<AppState>> = OnceLock::new();

/// 应用门面（零大小类型，仅提供静态入口）。
pub struct App;

impl App {
    /// 获取全局单例（未初始化则 panic）。
    pub fn global() -> &'static Arc<AppState> {
        APP.get().expect("App 未初始化：请先调用 start / App::init")
    }

    /// 获取全局单例（未初始化返回 None）。
    pub fn try_global() -> Option<&'static Arc<AppState>> {
        APP.get()
    }

    /// 初始化：日志订阅器、多线程 runtime、LLM 客户端、全局事件总线、会话注册表与存储门面。
    ///
    /// `storage` 由装配层（napi startup）组合具体后端（StorageManager）后注入，core 不依赖存储实现。
    pub fn init(config: AppConfig, storage: Arc<dyn Storage>) -> Result<(), String> {
        // ① 日志订阅器（进程内仅一次；失败即中止启动）。日志目录按应用路径解析
        //   （相对 / 未配置落到应用根下，默认 `<数据根>/log`），并确保目录存在。
        let log_dir = resolve_log_dir(&user_data_dir(), &config);
        let log_guard = init_tracing(&config.log.level, &log_dir)?;

        // ② 自建多线程 runtime
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("构建 tokio runtime 失败: {e}"))?;

        // ③ LLM 客户端注册表（多供应商 × 多模型；会话按提供商路由）+ 全局事件总线
        let (event_tx, _rx) = broadcast::channel(EVENT_CAPACITY);
        let llm = LlmRegistryHandle::new(LlmRegistry::from_config(&config.llm));
        let handle = runtime.handle().clone();

        let compact = Arc::new(RwLock::new(config.compact.clone()));
        let summary = Arc::new(RwLock::new(config.summary.clone()));
        let goal = Arc::new(RwLock::new(config.goal.clone()));
        let security = Arc::new(RwLock::new(config.security.clone()));
        let memory_cfg = Arc::new(RwLock::new(config.memory.clone()));
        let instruction_cfg = Arc::new(RwLock::new(config.instructions.clone()));
        // 页面渲染服务配置（websearch / webfetch 共用；热更经 update_config）
        let web_render_cfg = Arc::new(RwLock::new(config.web_render.clone()));
        // 常驻渲染服务（复用内置 Chromium）：预热 + 并发上限 + 排队 + 节流 + 空闲回收
        let web_render =
            crate::tool::search::WebRender::new(Arc::clone(&web_render_cfg), handle.clone());
        web_render.warm();
        let sessions = SessionImpl::new(
            Arc::clone(&storage),
            llm.clone(),
            event_tx.clone(),
            handle,
            Arc::clone(&summary),
            Arc::clone(&compact),
            Arc::clone(&goal),
        );
        let tools = Arc::new(ToolRegistry::builtin());

        // ④ 技能系统：SKILL.md 索引（data_dir/skill_index）+ 用户技能目录（AppDir.skills_dir）
        let dirs = AppDir::new(&config);
        // MCP 服务器管理器（<config_dir>/mcp_servers.json；启用项的连接在 App 单例就绪后建立）
        let mcp = Arc::new(crate::mcp::McpManager::new(&dirs.config_dir));
        let skill_index_dir = dirs.data_dir.join("skill_index");
        let mut skill_index = SkillIndex::new(&skill_index_dir, &dirs.skills_dir)
            .map_err(|e| format!("初始化技能索引失败: {e}"))?;
        // 内建技能（skill-creator）：缺失或仍为 builtin 来源时用内嵌资源覆盖安装，
        // 使资源内容随版本更新分发；用户同名导入（source=user）不覆盖，启停状态保留。
        let existing_builtin = skill_index
            .get_by_name("skill-creator")
            .map_err(|e| e.to_string())?;
        let should_install = existing_builtin
            .as_ref()
            .map(|d| d.source == "builtin")
            .unwrap_or(true);
        if should_install {
            let content = include_str!("../../resources/skills/skill-creator/SKILL.md");
            if let Ok(mut doc) =
                SkillIndex::parse_skill_content(content, Path::new("builtin/skill-creator/SKILL.md"))
            {
                doc.source = "builtin".into();
                doc.source_path = "builtin:skill-creator".into();
                doc.enabled = existing_builtin.map(|d| d.enabled).unwrap_or(true);
                let _ = skill_index.index_skill(&doc);
            }
        }
        // 内建技能（astorm-office）：随 office-kit 分发（resources/office/skill）。总纲引用的
        // 格式分册 / references 靠相对路径读盘，且用户技能目录是 bash/file 的只读白名单，
        // 故先把技能树**复制到用户技能目录**再以真实路径注册（builtin 来源；每次启动按 kit
        // 内容刷新，用户启停状态保留）。kit 未同步（bin/skill 缺一）则整块跳过：技能不注入、
        // office 工具自行报「未同步」，二者同时缺位即功能不存在。
        let office_bin_dir = config
            .office
            .bin_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let office_skill_src = config
            .office
            .skill_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let (Some(bin_dir), Some(skill_src)) = (office_bin_dir, office_skill_src) {
            install_builtin_office_skill(&mut skill_index, &dirs.skills_dir, skill_src, bin_dir);
        }
        // 发现并导入项目 `.astorm/skills` / 全局技能（损坏文件跳过，仅告警）
        if let Ok(discovered) = skill_index.discover() {
            for d in discovered {
                if d.error.is_none() {
                    let _ = skill_index.import_from_path(Path::new(&d.path));
                }
            }
        }
        let _ = skill_index.commit();
        let skill = Arc::new(SkillManager::new(Arc::new(Mutex::new(skill_index))));

        // ⑤ 定时任务调度器（后台 cron；`start` 在 App 单例就绪后驱动）。
        //    默认任务工作区基座 = 用户目录 schedule/（如 ~/astorm/schedule；每个任务一个
        //    子目录，用户内容），任务状态仍在 sqlite data/
        let scheduler = Arc::new(Scheduler::new(
            Arc::clone(&storage),
            dirs.schedule_dir.clone(),
        ));

        // ⑥ 智能体管理器（内置主智能体播种；存储不支持 agents 表时仅告警——
        //    `resolve(None)` 有内存回退，「不指定智能体 = 不限制」始终成立）
        let agents = Arc::new(crate::agent_registry::AgentRegistry::new(Arc::clone(&storage)));
        if let Err(e) = agents.ensure_builtin() {
            tracing::warn!("[agents] 内置智能体播种失败（不影响启动）: {e}");
        }

        // 启动日志：供应商摘要（空列表时提示未配置）
        let llm_providers = if config.llm.providers.is_empty() {
            "(空)".to_string()
        } else {
            let list: Vec<String> = config
                .llm
                .providers
                .iter()
                .map(|p| format!("{}（models: {}）", p.name, p.models.len()))
                .collect();
            format!("[{}]", list.join(", "))
        };
        tracing::info!(
            "[app] 启动装配完成: workspace={}, log.level={}, log.dir={}, llm.providers={}, storage=已注入",
            config.workspace,
            config.log.level,
            log_dir.display(),
            llm_providers,
        );

        // PTY 管理器持有事件发送端 clone(输出经总线推送)
        let pty = crate::facade::PtyManager::new(event_tx.clone());

        // 知识库（真实文件树;根 = 用户目录 knowledge/,跟随设置;根缺失时写入演示树）
        let knowledge = Arc::new(crate::facade::KnowledgeImpl::new());

        // 邮件服务（数据目录 = <app_dir>/data/mail；账号库 + 缓存库）
        let mail = Arc::new(
            astorm_mail::MailService::open(dirs.data_dir.join("mail"))
                .map_err(|e| format!("初始化邮件服务失败: {e}"))?,
        );

        let state = Arc::new(AppState {
            config,
            runtime,
            llm,
            event_tx,
            sessions,
            compact,
            summary,
            goal,
            security,
            memory_cfg,
            instruction_cfg,
            web_render_cfg,
            web_render,
            storage,
            tools,
            mcp,
            native_browser: RwLock::new(None),
            skill,
            agents,
            scheduler,
            file: crate::facade::FileImpl,
            git: crate::facade::GitImpl,
            tool: crate::facade::ToolImpl,
            skills: crate::facade::SkillImpl,
            agent: crate::facade::AgentImpl,
            schedule: crate::facade::ScheduleImpl,
            taskplan: crate::facade::TaskPlanImpl,
            terminal: crate::facade::TerminalImpl,
            pty,
            knowledge,
            mail,
            memory: Arc::new(crate::facade::MemoryImpl::new()),
            audit: Arc::new(crate::tool::audit::AuditLog::new(dirs.log_dir.clone())),
            interrupts: Arc::new(crate::tool::interrupt::InterruptRegistry::new()),
            session_allow: Arc::new(crate::tool::session_allow::SessionAllowRegistry::new()),
            log_guard: Mutex::new(log_guard),
        });

        APP.set(state).map_err(|_| "App 已初始化".to_string())?;

        // 后台启动 cron 调度（execute_task 依赖已就绪的 App 单例；失败仅告警不影响启动）
        let sched = Arc::clone(&App::global().scheduler);
        App::global().runtime.handle().spawn(async move {
            if let Err(e) = sched.start().await {
                tracing::error!("[scheduler] 启动失败: {e}");
            }
        });

        // 后台连接启用中的 MCP 服务器（配置里 enabled 的项启动即可用；单个失败仅告警）
        let mcp = Arc::clone(&App::global().mcp);
        App::global().runtime.handle().spawn(async move {
            mcp.connect_enabled().await;
        });

        // 注意：邮件服务的 IMAP 连接池**不在此预热**。
        // 这里运行在 App 自建 runtime 上，而 napi 异步调用运行在 napi-rs 的 runtime 上；
        // 池化 socket 是绑定到「创建它的 runtime」的 I/O 资源，跨 runtime 轮询会不稳定
        // （表现为每次请求超时/极慢）。让所有邮件 I/O 统一发生在实际调用它的 napi runtime 上。

        Ok(())
    }

    /// 热更新 LLM 配置（供应商 / 模型 / 默认供应商）：整体替换 LlmRegistry 句柄，
    /// 进行中会话的下一轮推理立即读到新配置；不需要 shutdown（不中断会话、不重开存储）。
    ///
    /// 同时热更新剪支配置（共享 Arc<RwLock>），使运行中会话下一轮剪支立即生效。
    ///
    /// 同时热更新摘要配置（共享 Arc<RwLock>）：摘要阈值 / 保留尾部调整后，
    /// 运行中会话下一轮即按新阈值判定（会话信息「上下文容量」分母同源）。
    ///
    /// 同时热更新安全配置（共享 Arc<RwLock>）：沙箱防护策略（文件黑白名单 / 网络
    /// 名单 / 危险命令 / 资源限制）在工具下一次构造 `bash_protection` 时即时生效。
    ///
    /// 同时热更新记忆配置（共享 Arc<RwLock>）：开关 / 分类表 / 各类预算在下次
    /// 记忆编译或查询时即时生效（进行中的编译不受影响，保持一次运行的配置一致性）。
    pub fn update_config(config: AppConfig) -> Result<(), String> {
        let app = App::try_global().ok_or("应用未初始化：请先调用 start")?;
        app.llm.swap(LlmRegistry::from_config(&config.llm));
        if let Ok(mut compact) = app.compact.write() {
            *compact = config.compact.clone();
        }
        if let Ok(mut summary) = app.summary.write() {
            *summary = config.summary.clone();
        }
        if let Ok(mut goal) = app.goal.write() {
            *goal = config.goal.clone();
        }
        if let Ok(mut security) = app.security.write() {
            *security = config.security.clone();
        }
        if let Ok(mut memory) = app.memory_cfg.write() {
            *memory = config.memory.clone();
        }
        if let Ok(mut instructions) = app.instruction_cfg.write() {
            *instructions = config.instructions.clone();
        }
        if let Ok(mut wr) = app.web_render_cfg.write() {
            *wr = config.web_render.clone();
        }
        // 渲染服务配置变化（bin/script/profile 等）→ 停止当前实例，下次请求按新配置重启
        app.web_render.on_config_changed();
        tracing::info!(
            "[app] 配置已热更新: providers={}, compact_threshold={}, summary_threshold={}, security.enabled={}, memory.enabled={}, inject_global_agents={}, inject_workspace_agents={}",
            config.llm.providers.len(),
            config.compact.compact_threshold,
            config.summary.token_threshold,
            config.security.enabled,
            config.memory.enabled,
            config.instructions.inject_global_agents,
            config.instructions.inject_workspace_agents,
        );
        Ok(())
    }
}

impl AppState {
    /// 全局配置（前端 `start` 传入，启动时生效）。
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// 应用自带 tokio runtime 的句柄（应用所有会话/工具都跑在这上面；
    /// 供无头宿主（astorm-server）把 HTTP 服务也挂到同一 runtime）。
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }

    /// 剪支配置快照（共享热更值：前端保存后经 `update_config` 刷新）。
    pub fn compact_config(&self) -> CompactConfig {
        self.compact.read().map(|c| c.clone()).unwrap_or_default()
    }

    /// 摘要配置快照（共享热更值：前端保存后经 `update_config` 刷新）。
    pub fn summary_config(&self) -> SummaryConfig {
        self.summary.read().map(|c| c.clone()).unwrap_or_default()
    }

    /// 目标循环配置快照（共享热更值：前端保存后经 `update_config` 刷新）。
    pub fn goal_config(&self) -> GoalConfig {
        self.goal.read().map(|c| c.clone()).unwrap_or_default()
    }

    /// 审计日志读取器（设置页「审计日志」模块按会话查询）。
    pub fn audit_log(&self) -> &crate::tool::audit::AuditLog {
        &self.audit
    }

    /// 使用统计：按「本地日期 × 模型」聚合 token 用量（`since_ts` 为 unix 秒下限）。
    pub fn usage_stats(&self, since_ts: i64) -> Result<Vec<astorm_common::types::UsageStatRow>, String> {
        self.storage.usage_stats(since_ts).map_err(|e| e.to_string())
    }

    /// 会话交付物行：`submit_deliverables` 工具逐轮落库的记录（按轮次升序；前端拉取渲染）。
    pub fn list_deliverables(
        &self,
        session_id: &str,
    ) -> Result<Vec<astorm_common::types::DeliverableRow>, String> {
        self.storage
            .list_deliverables(session_id)
            .map_err(|e| e.to_string())
    }

    /// 跨会话交付物行（按更新时间倒序限量；「交付物」页跨会话按时间分组展示用）。
    pub fn list_all_deliverables(
        &self,
        limit: i64,
    ) -> Result<Vec<astorm_common::types::DeliverableRow>, String> {
        self.storage
            .list_all_deliverables(limit)
            .map_err(|e| e.to_string())
    }

    /// 安全配置快照（共享热更值：沙箱防护策略；未初始化 App 时回退默认）。
    pub fn security_config(&self) -> crate::app::config::SecurityConfig {
        self.security
            .read()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// 记忆配置快照（共享热更值：前端保存后经 `update_config` 刷新）。
    pub fn memory_config(&self) -> Option<MemoryConfig> {
        self.memory_cfg.read().map(|c| c.clone()).ok()
    }

    /// 约定文件配置快照（共享热更值：AGENTS.md 注入开关；未初始化时回退默认 = 均开启）。
    pub fn instruction_config(&self) -> InstructionConfig {
        self.instruction_cfg
            .read()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// 会话级内存白名单（审批卡「当前会话允许」；重启丢失，按会话隔离）。
    pub fn session_allow(&self) -> &Arc<crate::tool::session_allow::SessionAllowRegistry> {
        &self.session_allow
    }

    /// 多供应商 LLM 注册表快照（客户端路由 / 模型名解析 / 默认模型）。
    pub fn llm(&self) -> Arc<LlmRegistry> {
        self.llm.snapshot()
    }

    /// 应用路径集（根目录 / 工作空间 / 配置 / 日志 / 数据文件）。
    pub fn dir(&self) -> AppDir {
        AppDir::new(&self.config)
    }

    /// 本地文件门面（list_directory / read_file_content / 应用路径）。
    pub fn file(&self) -> &crate::facade::FileImpl {
        &self.file
    }

    /// git 门面（init / status / stage / commit / push / diff）。
    pub fn git(&self) -> &crate::facade::GitImpl {
        &self.git
    }

    /// 工具目录门面（list / search）。
    pub fn tool(&self) -> &crate::facade::ToolImpl {
        &self.tool
    }

    /// MCP 服务器管理器（配置读写 / 连接 / 工具发现与调用）。
    pub fn mcp(&self) -> &Arc<crate::mcp::McpManager> {
        &self.mcp
    }

    /// 注册服务端浏览器后端（容器宿主启动时调用；桌面不注册 → browser 工具走 CLI 桥）。
    pub fn set_native_browser(&self, backend: Arc<dyn crate::tool::browser::native::NativeBrowser>) {
        if let Ok(mut slot) = self.native_browser.write() {
            *slot = Some(backend);
        }
    }

    /// 取服务端浏览器后端（未注册 → None）。
    pub fn native_browser(&self) -> Option<Arc<dyn crate::tool::browser::native::NativeBrowser>> {
        self.native_browser.read().ok().and_then(|s| s.clone())
    }

    /// 构建某会话本轮循环的工具集：智能体可见集（剔除未加载的动态工具，见
    /// [`crate::agent::dynamic_tools`]）+ **当前已连接的 MCP 服务器工具**。
    ///
    /// 两处调用（初始 `build_loop_context` 与每轮 `loop_start` 重建）共用同一口径，
    /// MCP 连接 / 断开的下一轮即生效。
    pub(crate) fn loop_tool_map(
        &self,
        tool_list: &[String],
        loaded: &[String],
    ) -> react_loop::ToolMap {
        let mut map = self.tools.tool_map_for(
            &crate::agent::dynamic_tools::effective_tool_names(tool_list, loaded),
        );
        for tool in self.mcp.connected_tools() {
            map.register(tool);
        }
        map
    }

    /// 诊断口径：某会话本轮循环**实际下发**的工具名（排序；含已连接 MCP 工具）。
    ///
    /// 与 [`Self::loop_tool_map`] 同源，供集成测试与宿主排查「模型到底看得到哪些工具」。
    pub fn session_tool_names(&self, tool_list: &[String], loaded: &[String]) -> Vec<String> {
        let mut names = self.loop_tool_map(tool_list, loaded).tool_names();
        names.sort();
        names
    }

    /// 技能门面（search / import / 增删改 / 会话绑定）。
    pub fn skills(&self) -> &crate::facade::SkillImpl {
        &self.skills
    }

    /// 智能体门面（list / get / create / update / remove）。
    pub fn agents(&self) -> &crate::facade::AgentImpl {
        &self.agent
    }

    /// 定时任务门面（list / add / toggle / run_now）。
    pub fn schedule(&self) -> &crate::facade::ScheduleImpl {
        &self.schedule
    }

    /// 终端门面（在会话工作空间内执行命令，前端终端页签使用）。
    pub fn terminal(&self) -> &crate::facade::TerminalImpl {
        &self.terminal
    }

    /// 终端 PTY 会话管理器（交互式 shell，xterm.js 前端对接）。
    pub fn pty(&self) -> &crate::facade::PtyManager {
        &self.pty
    }

    /// 任务规划门面（list / read 规划文件）。
    pub fn taskplan(&self) -> &crate::facade::TaskPlanImpl {
        &self.taskplan
    }

    /// 知识库门面（真实文件 CRUD：文件夹=目录、文档=文件）。
    pub fn knowledge(&self) -> &crate::facade::KnowledgeImpl {
        &self.knowledge
    }

    /// 长期记忆门面（状态查询 / 概念浏览 / 检索 / 校验 / 编译触发与取消）。
    pub fn memory(&self) -> &crate::facade::MemoryImpl {
        &self.memory
    }

    /// 邮件服务（账号管理 / 收件 / 发件 / 标记整理）。
    pub fn mail(&self) -> &Arc<astorm_mail::MailService> {
        &self.mail
    }

    /// 会话模块门面（create / list / delete / send / cancel）。
    pub fn session(&self) -> &SessionImpl {
        &self.sessions
    }

    /// 订阅全局事件总线（广播，多订阅者；无回放，需在会话创建前订阅）。
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.event_tx.subscribe()
    }

    /// 向全局事件总线广播一条**与会话无关**的事件（如「定时任务定义变更」）。
    ///
    /// 会话内的协议事件走 `SessionImpl::emit_event_of`（用 session_id 定位）；这类全局事件
    /// `session_id` 为空，前端按 `type` 分发。无订阅者时 `send` 返回 `Err`，属正常情况，忽略。
    pub fn emit(&self, event: Value) {
        let _ = self.event_tx.send(event);
    }

    /// 关闭：取消所有会话循环、停止定时调度、flush 日志并清空注册表（runtime 随进程退出释放）。
    pub fn shutdown(&self) {
        self.sessions.shutdown();
        // 停止定时调度（shutdown 可能在 runtime 线程内被调用，故 spawn 而非 block_on）
        let sched = Arc::clone(&self.scheduler);
        self.runtime.handle().spawn(async move {
            sched.stop().await;
        });
        // 停止页面渲染服务（drop Service → kill_on_drop 杀子进程）
        let render = Arc::clone(&self.web_render);
        self.runtime.handle().spawn(async move {
            render.shutdown().await;
        });
        // 显式 flush 日志（drop WorkerGuard）
        if let Ok(mut guard) = self.log_guard.lock() {
            *guard = None;
        }
    }
}

/// 初始化 tracing 订阅器（进程内仅一次）。返回文件 appender 的 WorkerGuard，
/// 需由调用方持有以保证退出时 flush。目录不存在时自动创建。
fn init_tracing(
    level: &str,
    dir: &Path,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("创建日志目录失败（{}）: {e}", dir.display()))?;
    let file_appender = tracing_appender::rolling::daily(dir, "astorm.log");
    let (non_blocking, worker_guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .try_init()
        .map_err(|e| format!("日志初始化失败: {e}"))?;
    Ok(Some(worker_guard))
}


/// 安装内建技能（astorm-office）：把 office-kit 的技能树复制进用户技能目录后注册。
///
/// - 目录内容与 kit 的 `skill/` 同构（总纲 `SKILL.md` + 四格式分册），相对引用可解析；
/// - 技能目录在 bash/file 防护里是**只读白名单**，模型读分册 / references 免审批；
/// - 每次 `start` 都按 kit 内容刷新（内容不同则整树重拷），注册为 `source="builtin"`、
///   `source_path=真实路径`（skills load 返回技能目录，模型按需读资产）；
/// - 用户同名导入（source=user）不覆盖；用户启停状态保留；
/// - 末尾追加「桌面端运行方式」附录（CLI 已预装 + `office` 工具用法），内容源仍是
///   office 仓库单源，附录仅由本端注入。
fn install_builtin_office_skill(
    index: &mut SkillIndex,
    skills_dir: &Path,
    src_dir: &str,
    bin_dir: &str,
) {
    let src = Path::new(src_dir);
    let md = src.join("SKILL.md");
    let content = match std::fs::read_to_string(&md) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[skills] astorm-office 技能资源缺失（功能不可用）: {e}");
            return;
        }
    };
    let dest_dir = skills_dir.join("astorm-office");
    let dest_md = dest_dir.join("SKILL.md");
    let stale = std::fs::read_to_string(&dest_md)
        .map(|c| c != content)
        .unwrap_or(true);
    if stale {
        let _ = std::fs::remove_dir_all(&dest_dir);
        if let Err(e) = copy_tree(src, &dest_dir) {
            tracing::warn!("[skills] astorm-office 技能树复制失败（保留旧版）: {e}");
            return;
        }
    }
    let existing = match index.get_by_name("astorm-office") {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[skills] 查询 astorm-office 失败: {e}");
            return;
        }
    };
    // 用户同名导入（source=user）不覆盖；否则缺失 / 仍为 builtin 都（重）安装
    if !existing.as_ref().map(|d| d.source == "builtin").unwrap_or(true) {
        return;
    }
    let new_content = match std::fs::read_to_string(&dest_md) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[skills] astorm-office 技能复制后不可读: {e}");
            return;
        }
    };
    if let Ok(mut doc) = SkillIndex::parse_skill_content(&new_content, &dest_md) {
        doc.source = "builtin".into();
        doc.source_path = dest_md.to_string_lossy().into_owned();
        doc.enabled = existing.map(|d| d.enabled).unwrap_or(true);
        // 桌面端分发约束置顶：CLI/技能资产随安装包分发，禁止模型尝试安装或下载
        doc.prompt = format!(
            "> **桌面端内置环境**：本技能依赖的五个 CLI、技能资产与预览组件均已随安装包分发，\n\
> **不要执行 `cargo install` / `npm install` 或任何下载安装**；直接按下方命令调用。\n\
> 若可执行文件缺失，说明安装包不完整，应报告错误而不是自行安装。\n\n{}",
            doc.prompt
        );
        doc.prompt.push_str(&office_skill_appendix(bin_dir));
        if let Err(e) = index.index_skill(&doc) {
            tracing::warn!("[skills] astorm-office 索引失败: {e}");
        }
    }
}

/// 内置 office 技能的「桌面端运行方式」附录（索引时追加到总纲正文）。
fn office_skill_appendix(bin_dir: &str) -> String {
    format!(
        "\n\n---\n\n## 桌面端运行方式（astorm-desktop 内置）\n\n\
五个 CLI 已**随应用预装**于 `{bin_dir}`（统一入口 astorm-office + json2docx / json2xlsx / json2pptx / json2pdf），无需 cargo：\n\
- 全部 CLI、技能模板与预览组件随安装包分发，**不需要联网下载或安装**；不要执行 `cargo` / `npm install`。\n\
- 优先用 `office` 工具调用：`bin` 选格式（docx/xlsx/pptx/pdf），`args` 原样传 CLI 子命令与参数，\n\
  在会话工作区执行（例：`bin=\"docx\", args=[\"view\", \"a.docx\", \"/part[1]\", \"text\"]`；\n\
  路径建议绝对路径）。也可经 bash 直接执行（写绝对路径），但 `serve` / `mcp`（常驻服务）不可用。\n\
- 跨格式批处理 / 统一入口用 `bin=\"office\"`：`args=[\"batch\", \"steps.txt\"]`（一行一条、可混排四格式），\n\
  或 `args=[\"word\", \"unpack\", \"a.docx\", \"-o\", \"prod/\"]`；省略格式时 `astorm-office` 按文件扩展名自动分发。\n\
- `view` 的 text 模式直出结构感知 Markdown（`/` 看整篇）；docx 修订用 `--revisions markup|original`，\n\
  导出图片 / 内嵌附件用 `--media DIR` / `--attachments DIR`（会写盘，走写操作审批）。\n\
- 调用建议带全局 `--json`：stdout 只出数据、错误为结构化 JSON（encrypted=密码保护，zip=损坏包）。\n\
- `render png` 的 Chromium 与 `convert office` 的 LibreOffice 已按本机配置注入环境变量。\n\
- 生成 / 修改完成后，前端会自动以高保真预览展示产物；用户可在预览中拾取元素路径回填输入框。\n"
    )
}

/// 递归复制目录（文件 + 子目录；不存在目标目录则创建）。
fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
