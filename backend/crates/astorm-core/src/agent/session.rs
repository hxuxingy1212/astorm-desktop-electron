//! 会话运行时管理：会话注册表 + 会话内存状态 + 门面实现。
//!
//! 持久化经由注入的 [`Storage`]（`astorm_common::types`，由 napi 装配层组合具体后端）：
//! 元数据走 sqlite、消息历史走 jsonl 追加。`Session` 自身驱动 ReAct 循环
//! （见 `run`：`Session::run` 消费队列驱动引擎；`hook`：`ReactLoop` 事件编排）。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app::llm::LlmRegistryHandle;
use crate::app::{App, CompactConfig, GoalConfig, SummaryConfig};
use astorm_common::session::interrupt::InterruptResolution;
use astorm_common::session::req::{CreateSessionRequest, SessionSettings};
use astorm_common::session::Session as SessionFacade;
use astorm_common::types::agent::{AgentRecord, builtin_main_agent};
use astorm_common::types::record::{
    GoalState, GoalStatus, MessageRecord, SessionRecord, SessionStatus, SessionType,
};
use astorm_common::types::Storage;
use llm_provider::{ChatMessage, TokenUsage};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// 当前 unix 时间（秒），用于记录时间戳。
pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 会话输入：drain 循环按 FIFO 消费的三种「一轮」起点。
///
/// 决议与用户消息共用同一条队列，从而保持严格顺序（先到的先处理）；
/// 二者都不会插队——中断只发生于循环已停之后。
#[derive(Debug)]
pub(crate) enum SessionInput {
    /// 普通用户消息（携带前端对话框设置；会追加一条 user 消息并落盘）
    Message(CreateSessionRequest),
    /// 中断决议（审批同意/拒绝、表单回答/忽略）：**不追加 user 消息**，
    /// 只就地把决议写回被中断的 tool 消息（决议即 tool 结果），随后续跑。
    Resolution {
        pending: crate::tool::interrupt::PendingInterrupt,
        resolution: InterruptResolution,
    },
    /// 「启动」：把**已有历史**直接当作本轮请求跑起来（看板「待办」卡片的「启动」）。
    ///
    /// 专为「仅创建」（`auto_run = false`）的会话而设：那条指令在创建时就已写进历史末尾，
    /// 这里**不追加 user 消息**（与 `Resolution` 同一手法，见 run.rs）——否则同一条指令
    /// 会在历史里出现两遍，模型也就看到两遍。
    Start,
}

/// 会话信息事件：会话记录每次持久化后广播完整记录（camelCase），
/// 前端会话信息卡片据此实时刷新（标题 / 模式 / 轮次 / 用量 / 更新时间等）。
pub(crate) fn session_info_event(session_id: &str, record: &SessionRecord) -> Value {
    let mut info = serde_json::to_value(record).unwrap_or(Value::Object(Default::default()));
    if let Value::Object(map) = &mut info {
        map.insert("type".into(), "session_info".into());
        map.insert("session_id".into(), session_id.into());
    }
    info
}

/// 默认会话工作区名：创建时刻精确到秒 `yyyy_MM_dd_HH_mm_ss`（本地时区）。
/// 全部用下划线分隔（Windows 文件名不允许 `:`，用 `-` 会与日期衔接混淆；
/// 统一下划线便于任务目录在资源管理器中辨识），每条任务目录互不共享。
fn default_workspace_name() -> Option<String> {
    Some(chrono::Local::now().format("%Y_%m_%d_%H_%M_%S").to_string())
}

/// 智能体卡片的**可见工具集**（会话的 `tool_list` 即由此物化）。
///
/// - 内置默认主智能体：**不限制** → 全量内置工具（与引入智能体前的行为一致）。
/// - 自定义智能体：`tool_ids` 白名单 ∩ 当前注册中心（**空 = 无工具可调**）。
///   App 未初始化（测试等）时返回空列表。
pub(crate) fn agent_visible_tools(card: &AgentRecord) -> Vec<String> {
    let names = crate::app::App::try_global()
        .map(|app| app.tools.names())
        .unwrap_or_default();
    if card.is_builtin_main() {
        return names;
    }
    names
        .into_iter()
        .filter(|name| card.tool_ids.iter().any(|id| id == name))
        .collect()
}

/// 子会话工作区：`父会话工作区/.astorm/subagent_{子会话id}`（绝对路径字符串）。
///
/// 用**子会话 id** 命名，使同一父会话下的多个子会话目录互不覆盖；
/// 父会话工作区经 [`crate::tool::resolve_workspace`] 解析（相对名归入 `<用户目录>/workspace/`）。
fn subagent_workspace(parent_id: &Uuid, child_id: &Uuid) -> String {
    crate::tool::resolve_workspace(&parent_id.to_string())
        .join(".astorm")
        .join(format!("subagent_{child_id}"))
        .to_string_lossy()
        .to_string()
}

/// 会话：持久化记录 + 内存运行时状态（进程内跨 napi 调用共享）。
pub struct Session {
    /// 持久化记录（循环内回写：用量 / 轮次 / 更新时间；经 Mutex 提供内部可变性）。
    pub(crate) record: Mutex<SessionRecord>,
    /// 取消令牌：取消当前循环 / 终止会话（用户取消后可经 send_message 换新，恢复驱动）。
    pub cancel: RwLock<CancellationToken>,
    /// 全局事件总线发送端（创建会话时注入；Session 经此间接转发事件）。
    pub(crate) event_tx: broadcast::Sender<Value>,
    /// 会话完整历史（user / assistant / tool 消息，经 hook 落库）。
    pub(crate) history: Mutex<Vec<ChatMessage>>,
    /// 当前是否有循环在跑（串行调度，避免同会话并发）。
    pub(crate) running: Mutex<bool>,
    /// 用户消息 / 中断决议队列（由 Session::run 出队消费，FIFO 保序）。
    pub(crate) queue: Mutex<VecDeque<SessionInput>>,
    /// 打断队列（「立即发送」：优先于普通队列执行；入队同时打断当前循环）。
    pub(crate) interrupt_queue: Mutex<VecDeque<SessionInput>>,
    /// 存储门面（消息追加 / 元数据回写）。
    pub(crate) storage: Arc<dyn Storage>,
    /// 多供应商 LLM 注册表句柄（客户端路由 + 模型名解析；可热更新，每轮按当前注册表解析）。
    pub(crate) llm: LlmRegistryHandle,
    /// 会话摘要配置（共享热更值：前端保存后 `update_config` 写入，运行中会话下轮生效）。
    pub(crate) summary: Arc<RwLock<SummaryConfig>>,
    /// 会话消息剪支配置（共享热更值：前端保存后 `update_config` 写入，运行中会话下轮生效）。
    pub(crate) compact: Arc<RwLock<CompactConfig>>,
    /// 目标循环配置（共享热更值：`/goal` 外循环的 token / 迭代预算）。
    pub(crate) goal_cfg: Arc<RwLock<GoalConfig>>,
    /// 会话是否已被删除：删除后运行中的循环跳过继续持久化(upsert/追加),
    /// 避免「删除后又被循环复活」。
    pub(crate) deleted: AtomicBool,
    /// 下一条消息的持久化序号（**会话内单调递增**）。
    /// 不按「内存历史长度」推导：摘要把历史压缩后长度会回退，若据此取号，归档段
    /// （旧消息 seq 已到很大）与压缩后新消息（seq 从小重来）在重启回读按 seq 排序时
    /// 会交错错位。冷启动从「已加载记录最大 seq + 1」续接。
    pub(crate) next_seq: AtomicI64,
    /// 待执行的撤回请求（保留的用户消息条数）：循环运行时由 napi 置入 + 取消令牌，
    /// drain 循环在本轮结束后、发终态事件前执行，避免与在飞的引擎轮次竞态。
    pub(crate) pending_rewind: Mutex<Option<u32>>,
}

/// 会话管理门面实现：持有注册表句柄 + 引擎编排器 + 业务 runtime 句柄 + 存储门面。
pub struct SessionImpl {
    sessions: RwLock<HashMap<Uuid, Arc<Session>>>,
    llm: LlmRegistryHandle,
    event_tx: broadcast::Sender<Value>,
    handle: Handle,
    storage: Arc<dyn Storage>,
    /// 会话摘要配置（共享热更值；随 Session 下发同一 Arc）。
    summary: Arc<RwLock<SummaryConfig>>,
    /// 会话消息剪支配置（共享热更值；随 Session 下发同一 Arc）。
    compact: Arc<RwLock<CompactConfig>>,
    /// 目标循环配置（共享热更值；随 Session 下发同一 Arc）。
    goal: Arc<RwLock<GoalConfig>>,
    /// 子会话用量聚合的**待冲刷**状态（父会话维度）：见 [`SessionImpl::add_child_usage`]。
    child_usage_pending: Mutex<HashMap<Uuid, PendingChildUsage>>,
}

/// 父会话待冲刷的子会话用量（内存聚合，节流落盘/广播）。
struct PendingChildUsage {
    usage: TokenUsage,
    last_flush: Instant,
}

/// 子用量落盘/广播的节流间隔（长子任务时把「每子轮一次」降到「每 ≥1s 一次」）。
const CHILD_USAGE_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// 待冲刷量超过此阈值立即冲刷（避免极长单轮迟迟不更新父总量）。
const CHILD_USAGE_FLUSH_TOKENS: u32 = 50_000;

impl SessionImpl {
    pub(crate) fn new(
        storage: Arc<dyn Storage>,
        llm: LlmRegistryHandle,
        event_tx: broadcast::Sender<Value>,
        handle: Handle,
        summary: Arc<RwLock<SummaryConfig>>,
        compact: Arc<RwLock<CompactConfig>>,
        goal: Arc<RwLock<GoalConfig>>,
    ) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            llm,
            event_tx,
            handle,
            storage,
            summary,
            compact,
            goal,
            child_usage_pending: Mutex::new(HashMap::new()),
        }
    }

    /// 关闭：取消全部会话循环。
    pub fn shutdown(&self) {
        if let Ok(sessions) = self.sessions.read() {
            for session in sessions.values() {
                if let Ok(cancel) = session.cancel.read() {
                    cancel.cancel();
                }
            }
        }
    }

    /// 读取会话记录快照（工具解析工作区 / 会话模式等）。
    pub(crate) fn record_of(&self, session_id: &str) -> Option<SessionRecord> {
        let id = Uuid::parse_str(session_id).ok()?;
        let session = self.sessions.read().ok()?.get(&id).cloned()?;
        session.record.lock().ok().map(|r| r.clone())
    }

    /// 子会话把新增 token **增量**累加到父会话（子会话 hook 每轮调用）。
    ///
    /// **节流**：只把增量累加进内存 `child_usage_pending`；距上次落盘/广播 ≥ [`CHILD_USAGE_FLUSH_INTERVAL`]
    /// 或待冲刷量超 [`CHILD_USAGE_FLUSH_TOKENS`] 时才真正写父记录 + 广播 `session_info`；
    /// 子会话结束（`after_loop`）会调用 [`SessionImpl::flush_child_usage`] 强制冲刷，保证终值落盘。
    pub(crate) fn add_child_usage(&self, parent_id: &Uuid, delta: &TokenUsage) {
        if delta.is_zero() {
            return;
        }
        let should_flush = {
            let mut pending = match self.child_usage_pending.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            let entry = pending.entry(*parent_id).or_insert_with(|| PendingChildUsage {
                usage: TokenUsage::default(),
                last_flush: Instant::now(),
            });
            entry.usage.accumulate(delta);
            entry.last_flush.elapsed() >= CHILD_USAGE_FLUSH_INTERVAL
                || entry.usage.total >= CHILD_USAGE_FLUSH_TOKENS
        };
        if should_flush {
            self.flush_child_usage(parent_id);
        }
    }

    /// 强制把某父会话的待冲刷子用量落盘并广播（子会话结束 / 必要时调用）。
    pub(crate) fn flush_child_usage(&self, parent_id: &Uuid) {
        let delta = {
            let mut pending = match self.child_usage_pending.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            match pending.get_mut(parent_id) {
                Some(entry) if !entry.usage.is_zero() => {
                    let d = entry.usage.clone();
                    entry.usage = TokenUsage::default();
                    entry.last_flush = Instant::now();
                    d
                }
                Some(entry) => {
                    entry.last_flush = Instant::now();
                    return;
                }
                None => return,
            }
        };
        self.apply_parent_usage(parent_id, &delta);
    }

    /// 把一份子用量增量落进父会话记录（`subagent_usage`）并广播 `session_info`。
    fn apply_parent_usage(&self, parent_id: &Uuid, delta: &TokenUsage) {
        let parent = match self.sessions.read().ok().and_then(|s| s.get(parent_id).cloned()) {
            Some(session) => session,
            // 后端重启后父会话可能尚未在注册表：从存储重建同一条会话
            None => match self.get_or_rehydrate(parent_id) {
                Ok(session) => session,
                Err(_) => return,
            },
        };
        let snapshot = {
            let mut record = match parent.record.lock() {
                Ok(record) => record,
                Err(_) => return,
            };
            record.subagent_usage.accumulate(delta);
            record.updated_at = unix_now();
            record.clone()
        };
        if let Err(e) = self.storage.upsert_session(&snapshot) {
            tracing::warn!("[session {}] 子会话用量累加落盘失败: {e}", parent_id);
            return;
        }
        let _ = parent
            .event_tx
            .send(session_info_event(&parent_id.to_string(), &snapshot));
    }

    /// 获取内存会话;注册表缺失(如后端重启后)时从存储记录重建**同一会话**(恢复 jsonl 历史),
    /// 避免「会话不存在」报错与前端走重建产生孤儿会话。
    pub fn get_or_rehydrate(&self, session_id: &Uuid) -> Result<Arc<Session>, String> {
        if let Some(s) = self
            .sessions
            .read()
            .map_err(|_| "会话注册表锁中毒".to_string())?
            .get(session_id)
        {
            return Ok(s.clone());
        }
        let id = session_id.to_string();
        let mut record = self
            .storage
            .get_session(&id)
            .map_err(|e| format!("读取会话记录失败: {e}"))?
            .ok_or_else(|| format!("会话不存在: {session_id}"))?;
        // 工具列表自愈：`tool_list` 是按**智能体卡片**物化的可见工具集，而内置工具集会随版本增删
        // （如已移除的 ask_approval）。冷启动按当前注册中心重算：智能体卡片仍在 → 按其 `tool_ids`
        // 过滤后再更新；卡片已被删除 → **保留 record 快照**（不把限制放开成全量），
        // 避免系统提示词「可用能力」的工具清单与模型实际可调用的工具集不一致。
        if let Some(app) = crate::app::App::try_global() {
            let names = match record.agent_type.as_deref() {
                Some(_) => match app.agents.resolve(record.agent_type.as_deref()) {
                    Ok(card) => agent_visible_tools(&card),
                    Err(_) => {
                        tracing::warn!(
                            "[session {id}] 智能体已不存在，工具集沿用创建时快照: {:?}",
                            record.agent_type
                        );
                        record.tool_list.clone()
                    }
                },
                // 老记录（引入智能体前创建）无 agent_type → 默认主智能体语义 = 全量
                None => app.tools.names(),
            };
            // 动态加载态清洗：注册中心已下线的工具名从 loaded_tool_list 剔除（不重置加载语义，
            // 只去掉模型实际调不到的幻影名，保证系统提示词「可用能力」的工具清单与现实一致）。
            let (cleaned_loaded, loaded_changed) =
                crate::agent::dynamic_tools::sanitize_loaded(&record.loaded_tool_list, &app.tools.names());
            let tools_changed = record.tool_list != names;
            if tools_changed || loaded_changed {
                if tools_changed {
                    record.tool_list = names;
                }
                if loaded_changed {
                    record.loaded_tool_list = cleaned_loaded;
                }
                if let Err(e) = self.storage.upsert_session(&record) {
                    tracing::warn!("[session {id}] 工具列表自愈落盘失败: {e}");
                }
            }
        }
        // 历史 = 活动日志（摘要把压缩结果「系统提示词 + 摘要 + 保留消息」写回活动日志后，
        // 活动日志即会话历史；备份段 `message_NNN.jsonl` 只作备份/检索，不参与回读）。
        let recs = self.storage.load_messages(&id).unwrap_or_default();
        // 老记录兼容：升级前 `question_times` 缺省为 0，按活动日志里的用户消息条数一次性回填
        // （摘要消息 role 为 assistant，不会被计入；注入消息只在本轮局部，不落盘）。
        if record.question_times == 0 {
            let user_count = recs.iter().filter(|r| r.role == "user").count() as i32;
            if user_count > 0 {
                record.question_times = user_count;
                if let Err(e) = self.storage.upsert_session(&record) {
                    tracing::warn!("[session {id}] 用户提问次数回填落盘失败: {e}");
                }
            }
        }
        // 序号续接：从已加载记录最大 seq + 1 起（压缩写回已占用更高 seq，故新消息仍递增）
        let next_seq = recs.iter().map(|r| r.seq).max().map(|m| m + 1).unwrap_or(0);
        let history: Vec<ChatMessage> = recs
            .iter()
            .filter_map(crate::agent::history::record_to_message)
            .collect();
        let session = Arc::new(Session {
            record: Mutex::new(record),
            cancel: RwLock::new(CancellationToken::new()),
            event_tx: self.event_tx.clone(),
            history: Mutex::new(history),
            running: Mutex::new(false),
            queue: Mutex::new(VecDeque::new()),
            interrupt_queue: Mutex::new(VecDeque::new()),
            storage: Arc::clone(&self.storage),
            llm: self.llm.clone(),
            summary: self.summary.clone(),
            compact: self.compact.clone(),
            deleted: AtomicBool::new(false),
            next_seq: AtomicI64::new(next_seq),
            pending_rewind: Mutex::new(None),
            goal_cfg: self.goal.clone(),
        });
        // 冷启动恢复中断态：历史中 `metadata.interrupt=pending` 重建台账（前端可重新审批/回答）、
        // `replaying` 覆盖为「结果未知」（保守不重跑）。幂等；此后本会话留在内存注册表。
        crate::agent::interrupt::recover_pending_interrupts(&session);
        self.sessions
            .write()
            .map_err(|_| "会话注册表锁中毒".to_string())?
            .insert(*session_id, Arc::clone(&session));
        Ok(session)
    }

    /// 按会话 id 更新运行状态（工具层等只有 session_id 的调用方用；会话不在内存注册表则跳过）。
    ///
    /// 具体写入见 [`Session::set_status`]（幂等：落盘 + 广播 `session_info`）。
    pub(crate) fn set_status_of(&self, session_id: &str, status: SessionStatus) {
        let Ok(uuid) = Uuid::parse_str(session_id) else {
            return;
        };
        let session = self
            .sessions
            .read()
            .ok()
            .and_then(|map| map.get(&uuid).cloned());
        if let Some(session) = session {
            session.set_status(status);
        }
    }

    /// 按会话 id 广播一条**协议事件**（工具层等只有 session_id 的调用方用；会话不在内存
    /// 注册表则静默跳过）。
    ///
    /// 与 [`SessionImpl::set_status_of`] 同构：只按 id 取会话的全局事件总线发送端，
    /// 不改动会话状态。事件由业务侧定义构造器（如 `task_list` 的计划变更事件）。
    pub(crate) fn emit_event_of(&self, session_id: &str, event: Value) {
        let Ok(uuid) = Uuid::parse_str(session_id) else {
            return;
        };
        let session = self
            .sessions
            .read()
            .ok()
            .and_then(|map| map.get(&uuid).cloned());
        if let Some(session) = session {
            let _ = session.event_tx.send(event);
        }
    }

    /// 动态工具加载写回（`tool_load` 工具调用；按会话 id 定位，会话不在内存则报错）。
    ///
    /// `load`：校验后把新名合并进 `record.loaded_tool_list`（去重幂等），落盘 + 广播
    /// `session_info`，返回已加载全集供工具回执；校验失败（不在可加载清单 / 注册中心没有）
    /// 返回带可加载清单的错误文案（模型据此纠正）。`unload`：从加载集移除（未加载的忽略）。
    ///
    /// 只改记录：本轮**不**重建 `ctx.tools`（下一轮生效）——会话 hook 的 `loop_start`
    /// 每轮 LLM 前按记录重建循环工具集（见 `agent::dynamic_tools`）。
    pub fn mutate_loaded_tools(
        &self,
        session_id: &str,
        names: &[String],
        load: bool,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        let uuid = Uuid::parse_str(session_id).map_err(|_| format!("会话不存在: {session_id}"))?;
        let session = self
            .get_or_rehydrate(&uuid)
            .map_err(|e| format!("会话不存在: {e}"))?;
        if session.is_deleted() {
            return Err(format!("会话已删除: {session_id}"));
        }
        let known = App::try_global()
            .map(|app| app.tools.names())
            .unwrap_or_default();
        let (new_loaded, newly) = {
            let mut guard = session
                .record
                .lock()
                .map_err(|_| "会话记录锁中毒".to_string())?;
            let (new_loaded, newly) = if load {
                let (merged, newly, _) = crate::agent::dynamic_tools::apply_load(
                    names,
                    &guard.loaded_tool_list,
                    &known,
                )?;
                (merged, newly)
            } else {
                (
                    crate::agent::dynamic_tools::apply_unload(names, &guard.loaded_tool_list),
                    Vec::new(),
                )
            };
            if new_loaded != guard.loaded_tool_list {
                guard.loaded_tool_list = new_loaded.clone();
                guard.updated_at = unix_now();
            }
            (new_loaded, newly)
        };
        // 落盘 + 广播（幂等值同样广播：前端 session_info 刷新无害，回执需要最新全集）
        let record = session
            .record
            .lock()
            .map_err(|_| "会话记录锁中毒".to_string())?
            .clone();
        self.storage
            .upsert_session(&record)
            .map_err(|e| format!("动态工具加载态落盘失败: {e}"))?;
        let _ = session
            .event_tx
            .send(session_info_event(session_id, &record));
        Ok((new_loaded, newly))
    }

    /// 读取会话的动态工具加载态（`tool_load` 的 status 动作；会话不在则空）。
    pub fn loaded_tools_of(&self, session_id: &str) -> Vec<String> {
        let Ok(uuid) = Uuid::parse_str(session_id) else {
            return Vec::new();
        };
        let session = self
            .sessions
            .read()
            .ok()
            .and_then(|map| map.get(&uuid).cloned());
        match session {
            Some(s) => s
                .record
                .lock()
                .map(|r| r.loaded_tool_list.clone())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// 会话模式 / 审批策略 / 模型跟随最新请求：UI 下拉每条消息随发，显式携带时更新记录并落盘 +
    /// 广播 session_info（前端会话卡片实时同步，工具策略下次执行即读到新值）。
    ///
    /// 模型 / 供应商一并落到记录：**中断决议没有请求体**（`submitInterrupt` 只带决议），
    /// 续跑那一轮按记录里的模型路由，取「最近一次用户请求」才能与当前选择一致。
    ///
    /// 前端已改为「设置是会话属性、经 `update_session_settings` 单独改写」，发消息不再携带
    /// 这些字段（缺省 = 保持）；这里保留承接旧客户端与其他调用方（如定时任务）显式携带的值。
    fn sync_mode_permission(
        &self,
        session: &Session,
        session_id: &str,
        request: &CreateSessionRequest,
    ) {
        let settings = SessionSettings::from(request);
        if settings.is_empty() || session.is_deleted() {
            return;
        }
        let record = {
            // 锁内原地更新内存快照（工具策略层经 record_of 读的就是它），再克隆落盘广播
            let mut guard = match session.record.lock() {
                Ok(record) => record,
                Err(_) => return,
            };
            if !settings.apply_to(&mut guard) {
                return;
            }
            guard.updated_at = unix_now();
            guard.clone()
        };
        if self.storage.upsert_session(&record).is_err() {
            return;
        }
        let _ = session.event_tx.send(session_info_event(session_id, &record));
    }

    /// 设置会话目标（`/goal`，随消息原子下发）：`request.goal` 非空时写入 `record.goal`
    /// 并重置预算（token 基线 = 当前累计用量、迭代 0）。缺省 = 不改动既有目标。
    ///
    /// 在 `create_session` / `send_message` 跑循环**之前**调用，保证本轮结束判定时目标已生效。
    fn sync_goal(&self, session: &Session, session_id: &str, request: &CreateSessionRequest) {
        let Some(text) = request
            .goal
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        if session.is_deleted() {
            return;
        }
        let record = {
            let mut guard = match session.record.lock() {
                Ok(record) => record,
                Err(_) => return,
            };
            guard.goal = Some(GoalState {
                text: text.to_string(),
                lang: request.goal_lang.clone().unwrap_or_default(),
                tokens_used: 0,
                // 基线包含已有子 agent 用量（goal 预算口径 = 自身 + 子 agent）
                last_total: (guard.session_usage.total + guard.subagent_usage.total) as u64,
                iterations: 0,
                status: GoalStatus::Active,
            });
            guard.updated_at = unix_now();
            guard.clone()
        };
        if self.storage.upsert_session(&record).is_err() {
            return;
        }
        let _ = session.event_tx.send(session_info_event(session_id, &record));
    }

    /// 立即发送（打断）：请求进入**打断消息队列**(`interrupt_queue`),并取消当前循环,
    /// 当前循环收尾后 run 优先消费打断消息(先于普通队列)。未运行(异常)时直接驱动。
    pub fn interrupt_message(&self, session_id: &Uuid, request: CreateSessionRequest) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;
        // 供应商校验（显式指定不存在的供应商 → 拒绝入队）
        if let Some(p) = request.provider.as_deref() {
            if !self.llm.has_provider(p) {
                return Err(format!("供应商不存在: {p}"));
            }
        }
        // 会话模式 / 审批策略跟随最新请求（立即发送同样携带 UI 当前选择）
        self.sync_mode_permission(&session, &session_id.to_string(), &request);

        // 锁序 running → interrupt_queue，与 Session::run 一致（避免双循环竞态）
        let spawn = {
            let mut running = session
                .running
                .lock()
                .map_err(|_| "运行锁中毒".to_string())?;
            let mut iq = session
                .interrupt_queue
                .lock()
                .map_err(|_| "打断队列锁中毒".to_string())?;
            iq.push_back(SessionInput::Message(request));
            if *running {
                false
            } else {
                *running = true;
                true
            }
        };
        // 打断当前循环：取消令牌 → 当前轮 break/错误退出 → run 循环下一轮优先进打断队列
        if let Ok(cancel) = session.cancel.read() {
            cancel.cancel();
        }
        // 入队即视为运行中（侧栏状态图标；run 出队后仍会再置一次，幂等）
        session.set_status(SessionStatus::Running);
        if spawn {
            let _ = self.handle.spawn(session.run());
        }
        Ok(())
    }

    /// 提交中断决议（napi `submitInterrupt`）：审批同意/拒绝、表单回答/忽略。
    ///
    /// **入队前即取走台账待决项**（原子校验）：不存在/已处理 → 报错回前端，
    /// 避免重复决议与失效决议静默丢失。决议随普通队列 FIFO 处理，天然与用户消息保序。
    /// 实际改写历史发生在出队时（`agent::interrupt::resume_interrupt`）。
    pub fn resolve_interrupt(
        &self,
        session_id: &Uuid,
        resolution: InterruptResolution,
    ) -> Result<(), String> {
        resolution.validate()?;
        let session = self.get_or_rehydrate(session_id)?;
        let sid = session_id.to_string();
        let pending = crate::app::App::try_global()
            .and_then(|app| app.interrupts.take(&sid, &resolution.request_id))
            .ok_or_else(|| {
                format!(
                    "中断请求不存在或已失效（可能已被处理或取代）: {}",
                    resolution.request_id
                )
            })?;
        if pending.interrupt_type != resolution.interrupt_type {
            // 类型不符（前端串用）：放回待决项后报错，避免这条决议被错误应用
            let actual = pending.interrupt_type;
            if let Some(app) = crate::app::App::try_global() {
                app.interrupts.register_pending(&sid, pending);
            }
            return Err(format!(
                "中断类型不符：期望 {}，收到 {actual}",
                resolution.interrupt_type
            ));
        }

        // 锁序 running → queue，与 Session::run 保持一致（避免双循环竞态）
        let spawn = {
            let mut running = session
                .running
                .lock()
                .map_err(|_| "运行锁中毒".to_string())?;
            let mut queue = session
                .queue
                .lock()
                .map_err(|_| "队列锁中毒".to_string())?;
            queue.push_back(SessionInput::Resolution { pending, resolution });
            if *running {
                false
            } else {
                *running = true;
                true
            }
        };
        // 决议入队不置 Running：是否续跑（表单忽略不跑 LLM）由 run 出队处判定
        if spawn {
            let _ = self.handle.spawn(session.run());
        }
        Ok(())
    }

    /// 标题顺序编排：LLM 生成标题 → 元数据落盘（全字段 upsert）→ 发送 `title` 事件。
    /// 任一步失败均不阻塞主循环（落盘失败仅 error 日志，事件照发保证前端即时刷新）。
    pub(crate) async fn generate_title_and_emit(
        session: &Arc<Session>,
        id: &str,
        model: &str,
        provider: Option<&str>,
        message: &str,
    ) {
        // 会话已被删除：标题是异步生成的，可能在删除之后才回来 —— 不拦就会把删掉的行
        // 连同新标题一起 upsert 回存储（前端有墓碑不显示，但存储里确实复活了）
        if session.is_deleted() {
            return;
        }
        let cancel = session
            .cancel
            .read()
            .map(|c| c.child_token())
            .unwrap_or_else(|_| CancellationToken::new());
        // 按请求供应商路由客户端 + 解析 API 模型名（引用 id → name，未匹配原样透传）
        let Some(client) = session.llm.client_for(provider) else {
            tracing::warn!("[session {id}] 供应商客户端不可用，跳过标题生成");
            return;
        };
        let api_model = session.llm.resolve_model_name(provider, model);
        let titles = match crate::agent::title::generate_titles(&client, &api_model, message, cancel).await {
            Ok(titles) => titles,
            Err(e) => {
                tracing::warn!("[session {id}] 会话标题生成失败: {e}");
                return;
            }
        };
        // ② 元数据落盘：锁 record 写双标题 + updated_at，锁外全字段 upsert（sqlite 整条入 data 列）
        let snapshot = {
            let mut record = match session.record.lock() {
                Ok(r) => r,
                Err(_) => return,
            };
            record.title = Some(titles.zh.clone());
            record.title_zh = Some(titles.zh.clone());
            record.title_en = Some(titles.en.clone());
            record.updated_at = unix_now();
            record.clone()
        };
        if let Err(e) = session.storage.upsert_session(&snapshot) {
            tracing::error!("[session {id}] 标题落盘失败: {e}");
        } else {
            tracing::info!("[session {id}] 生成标题: zh={} / en={}", titles.zh, titles.en);
        }
        // ③ 发送标题事件（落盘失败也照发）
        let _ = session.event_tx.send(crate::agent::title::title_event(id, &titles));
        // ④ 会话信息持久化（标题 + 更新时间已更新）→ 广播最新记录
        let _ = session.event_tx.send(session_info_event(id, &snapshot));
    }

    /// 基于已有会话 fork 出新会话：新 session id、**共用源会话工作区**、继承部分历史。
    ///
    /// 继承内容 = 源活动日志中「最后一个无工具调用的助手消息」**之前**的消息（含 `system` /
    /// 最近一次摘要记录作上下文前缀）；`before_final` 指定在「无工具助手消息」中 fork 在其
    /// 之前的下标（0-based，供逐条 fork），`None` = 最后一条。多次摘要时活动日志本身就是
    /// 「系统提示词 + 最近摘要 + 保留消息」，故天然只继承最近摘要之后的部分。
    pub fn fork_session(&self, source_id: &Uuid, before_final: Option<u32>) -> Result<Uuid, String> {
        let src = self.get_or_rehydrate(source_id)?;
        let src_id = source_id.to_string();
        let src_record = src
            .record
            .lock()
            .map_err(|_| "会话记录锁中毒".to_string())?
            .clone();
        let records = self
            .storage
            .load_messages(&src_id)
            .map_err(|e| format!("读取源会话历史失败: {e}"))?;
        let cut = compute_fork_cut(&records, before_final)?;
        let inherited = &records[..cut];

        // 新会话记录:共用工作区(原样复制字符串,解析后即同一目录);继承能力配置
        let new_id = Uuid::new_v4();
        let new_sid = new_id.to_string();
        let now = unix_now();
        let title = format!(
            "fork from {}",
            src_record
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "新对话".into())
        );
        let record = SessionRecord {
            id: new_id,
            pid: None,
            session_type: src_record.session_type,
            scheduled_task_id: None,
            agent_type: src_record.agent_type.clone(),
            title: Some(title),
            title_zh: None,
            title_en: None,
            model: src_record.model.clone(),
            provider: src_record.provider.clone(),
            workspace: src_record.workspace.clone(),
            session_mode: src_record.session_mode,
            reasoning_effort: src_record.reasoning_effort,
            permission_mode: src_record.permission_mode,
            skill_list: src_record.skill_list.clone(),
            tool_list: src_record.tool_list.clone(),
            mcp_list: src_record.mcp_list.clone(),
            // fork 继承源会话的动态加载态（对话语义连续：模型已熟悉哪些工具可用）
            loaded_tool_list: src_record.loaded_tool_list.clone(),
            session_usage: TokenUsage::default(),
            contributed_to_parent: Default::default(),
            subagent_usage: Default::default(),
            context_tokens: 0,
            summary_times: src_record.summary_times,
            question_times: src_record.question_times,
            current_round: 0,
            created_at: now,
            updated_at: now,
            status: SessionStatus::Idle,
            status_updated_at: now,
            goal: None,
        };
        self.storage
            .upsert_session(&record)
            .map_err(|e| format!("fork 会话元数据落盘失败: {e}"))?;

        // 复制消息:新会话 id / 连续 seq / 重算消息 id;清除待决中断标记避免复活审批卡
        let copied: Vec<MessageRecord> = inherited
            .iter()
            .enumerate()
            .map(|(i, r)| copy_record_for_fork(r, &new_sid, i as i64))
            .collect();
        if !copied.is_empty() {
            self.storage
                .append_messages(&new_sid, &copied)
                .map_err(|e| format!("fork 会话历史落盘失败: {e}"))?;
        }

        // 注册到内存(加载刚写入的历史 + 续接 next_seq + 中断恢复)并广播
        let _ = self.get_or_rehydrate(&new_id)?;
        let _ = self.event_tx.send(session_info_event(&new_sid, &record));
        // 共用工作区目录(幂等):fork 后即使不触发工具也保证目录存在
        let _ = crate::tool::resolve_workspace(&new_sid);
        Ok(new_id)
    }

    /// 手动摘要（`/compact`）：会话空闲时异步执行一次**强制**摘要；运行中直接拒绝。
    ///
    /// 复用 `running` 做互斥：置位后当前没有循环在跑；摘要期间到达的消息照常入队但**不启动**
    /// （`send_message` 见 running=true 只入队），摘要结束后若队列非空再 `spawn` 驱动，
    /// 避免与摘要并发写会话历史、也避免丢消息。
    pub async fn summarize_session(&self, session_id: &Uuid) -> Result<bool, String> {
        let session = self.get_or_rehydrate(session_id)?;
        {
            let mut running = session
                .running
                .lock()
                .map_err(|_| "运行锁中毒".to_string())?;
            if *running {
                return Err("会话正在生成，请稍后再试".into());
            }
            *running = true;
        }
        let result = crate::agent::summary::summarize_now(&session, true).await;
        // 收尾：队列空 → 复位；摘要期间入队了消息 → 保持 running 并驱动循环
        let spawn = {
            let mut running = session
                .running
                .lock()
                .map_err(|_| "运行锁中毒".to_string())?;
            let queue = session.queue.lock().map_err(|_| "队列锁中毒".to_string())?;
            if queue.is_empty() {
                *running = false;
                false
            } else {
                true
            }
        };
        if spawn {
            let _ = self.handle.spawn(session.run());
        }
        result
    }
}

/// 记录是否带工具调用（`null` 或空数组 = 无）。
fn record_has_tool_calls(r: &MessageRecord) -> bool {
    match &r.tool_calls {
        Value::Null => false,
        Value::Array(a) => !a.is_empty(),
        _ => true,
    }
}

/// 记录是否为会话摘要消息（assistant + `metadata.summary == true`）。
fn record_is_summary(r: &MessageRecord) -> bool {
    r.role == "assistant"
        && r.metadata
            .as_ref()
            .and_then(|m| m.get("summary"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// 计算 fork 继承的切点：返回 `records[..cut]` 为继承集。
///
/// - 只在最近一次摘要**之后**的「无工具调用的助手消息」中选切点（摘要记录自身不算候选）；
///   无摘要时范围为全体。
/// - `Some(n)` → 在第 n 个候选（0-based）之前切；越界报错。
/// - `None` → 在最后一个候选之前切。
/// - 无候选 → 继承全部（返回 `records.len()`）。
///
/// 切点之前的 `system` / 摘要记录作为上下文前缀一并保留。
fn compute_fork_cut(records: &[MessageRecord], before_final: Option<u32>) -> Result<usize, String> {
    let floor = records.iter().rposition(record_is_summary);
    let candidates: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(i, r)| {
            let after_summary = floor.map_or(true, |p| *i > p);
            after_summary && r.role == "assistant" && !record_is_summary(r) && !record_has_tool_calls(r)
        })
        .map(|(i, _)| i)
        .collect();

    match before_final {
        Some(n) => candidates
            .get(n as usize)
            .copied()
            .ok_or_else(|| "fork 目标不存在".to_string()),
        None => Ok(candidates.last().copied().unwrap_or(records.len())),
    }
}

/// 为 fork 复制一条消息记录：新会话 id、连续 seq、重算消息 id，保留其余字段。
fn copy_record_for_fork(src: &MessageRecord, new_session_id: &str, seq: i64) -> MessageRecord {
    let id = match src.tool_call_id.as_deref() {
        Some(tcid) if !tcid.is_empty() => {
            crate::agent::history::tool_message_id(new_session_id, tcid)
        }
        _ => Uuid::new_v4().to_string(),
    };
    let mut metadata = src.metadata.clone();
    if let Some(obj) = metadata.as_mut().and_then(|m| m.as_object_mut()) {
        obj.remove("interrupt");
    }
    MessageRecord {
        id,
        session_id: new_session_id.to_string(),
        role: src.role.clone(),
        content: src.content.clone(),
        tool_calls: src.tool_calls.clone(),
        seq,
        created_at: src.created_at,
        reasoning_content: src.reasoning_content.clone(),
        tool_call_id: src.tool_call_id.clone(),
        metadata,
    }
}

impl Session {
    /// 计算撤回切点并做边界校验（不修改任何状态）。
    ///
    /// - `keep_user_messages`：保留前 N 条用户消息，删除第 N 条（0-based）及其后全部。
    /// - 摘要边界：目标用户消息的 `seq` 必须大于会话摘要记录（`metadata.summary == true`）
    ///   的 `seq`，否则返回「不可跨摘要撤回」——摘要之前的对话已被压缩丢弃，不能回退过去。
    fn plan_rewind(&self, keep_user_messages: u32) -> Result<RewindPlan, String> {
        let sid = self
            .record
            .lock()
            .map_err(|_| "会话记录锁中毒".to_string())?
            .id
            .to_string();
        let recs = self
            .storage
            .load_messages(&sid)
            .map_err(|e| format!("读取会话历史失败: {e}"))?;
        let cutoff_seq = compute_rewind_cutoff(&recs, keep_user_messages)?;
        Ok(RewindPlan {
            keep_user_messages,
            cutoff_seq,
        })
    }

    /// 落地撤回：磁盘截断（无备份段）+ 内存历史截断 + 作废被删区间的待决中断。
    /// `next_seq` 不回退（单调递增，重发消息拿更大的 seq，回读排序安全）。
    fn apply_rewind(&self, plan: &RewindPlan) -> Result<(), String> {
        let sid = self
            .record
            .lock()
            .map_err(|_| "会话记录锁中毒".to_string())?
            .id
            .to_string();

        self.storage
            .truncate_messages(&sid, plan.cutoff_seq)
            .map_err(|e| format!("截断会话历史失败: {e}"))?;

        {
            let mut history = self.history.lock().map_err(|_| "历史锁中毒".to_string())?;
            let mut cut_idx = history.len();
            let mut seen = 0u32;
            for (i, m) in history.iter().enumerate() {
                if m.role != "user" {
                    continue;
                }
                if seen == plan.keep_user_messages {
                    cut_idx = i;
                    break;
                }
                seen += 1;
            }
            history.truncate(cut_idx);
        }

        // 待决中断无 TTL，必须显式清理，否则被删消息上的审批/提问卡片会残留
        if let Some(app) = crate::app::App::try_global() {
            let _ = app.interrupts.take_session_pending(&sid);
        }
        Ok(())
    }

    /// 撤回历史并同步截断磁盘（含边界校验）。
    pub(crate) fn rewind_now(&self, keep_user_messages: u32) -> Result<(), String> {
        let plan = self.plan_rewind(keep_user_messages)?;
        self.apply_rewind(&plan)?;
        // 用户提问次数回退到保留条数（撤回删掉了第 N 条及之后的用户消息）
        self.set_question_times(keep_user_messages as i32);
        // 交付物表同步清理：被撤回轮次（round > 保留条数）的行一并删除，与历史保持一致
        let sid = self.record.lock().map(|r| r.id.to_string()).unwrap_or_default();
        if !sid.is_empty()
            && let Some(storage) = crate::tool::storage()
            && let Err(e) = storage.delete_deliverables_from(&sid, keep_user_messages as i32 + 1)
        {
            tracing::warn!("[session {sid}] 撤回清理交付物行失败: {e}");
        }
        Ok(())
    }

    /// 取出待执行的撤回请求（drain 循环用；幂等）。
    pub(crate) fn take_pending_rewind(&self) -> Option<u32> {
        self.pending_rewind.lock().ok().and_then(|mut g| g.take())
    }
}

/// 撤回计划：保留 `seq < cutoff_seq` 的消息，内存历史保留前 `keep_user_messages` 条用户消息。
struct RewindPlan {
    keep_user_messages: u32,
    cutoff_seq: i64,
}

/// 纯函数：按持久化记录计算撤回切点 `seq`，并做摘要边界校验。
///
/// - 保留前 `keep_user_messages` 条用户消息，删除第 N 条（0-based）及其后全部；
/// - 目标用户消息必须位于会话摘要记录（`metadata.summary == true`）之后，
///   否则报「不可跨摘要撤回」（摘要前的对话已被压缩，无法回退）。
fn compute_rewind_cutoff(
    records: &[MessageRecord],
    keep_user_messages: u32,
) -> Result<i64, String> {
    let user_seqs: Vec<i64> = records.iter().filter(|r| r.role == "user").map(|r| r.seq).collect();
    let idx = keep_user_messages as usize;
    if idx >= user_seqs.len() {
        return Err("没有可撤回的用户消息".into());
    }
    let cutoff_seq = user_seqs[idx];

    let boundary_seq = records.iter().find_map(|r| {
        let is_summary = r
            .metadata
            .as_ref()
            .and_then(|m| m.get("summary"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        is_summary.then_some(r.seq)
    });
    if let Some(b) = boundary_seq {
        if cutoff_seq <= b {
            return Err("不可跨摘要撤回".into());
        }
    }
    Ok(cutoff_seq)
}

impl SessionFacade for SessionImpl {
    fn create_session(&self, mut request: CreateSessionRequest) -> Result<Uuid, String> {
        let id = Uuid::new_v4();
        let now = unix_now();
        // 首条消息即创建消息：取出供异步标题生成（request 随后被 move 进队列）
        let first_message = request.user_message.clone();
        // 创建后是否立即执行（缺省 = 执行）。`false` = 「仅创建」：状态落 idle、首条消息只落历史、
        // 不驱动 ReAct 循环（看板「仅创建」入口用；调度器 / 子会话工具必须保持执行）。
        let auto_run = request.auto_run.unwrap_or(true);
        // 供应商校验：未配置任何供应商 → 拒绝创建（具体供应商名在继承后校验，见下）
        if self.llm.is_empty() {
            return Err("未配置 LLM 供应商（AppConfig.llm.providers 为空），无法创建会话".into());
        }
        // 智能体解析（缺省 = 内置默认主智能体；指定但卡片不存在 → 拒绝创建）
        let card = match crate::app::App::try_global() {
            Some(app) => app.agents.resolve(request.agent_type.as_deref())?,
            // 测试等未装配场景：按内置主智能体语义（不限制）
            None => builtin_main_agent(now),
        };
        // 子会话（携带 pid）：继承父会话的模型 / 供应商 / 模式 / 权限 / 思考强度。
        // 继承只在这里做一次（单一真源），子会话工具无需逐字段复制父记录。
        if let Some(parent_id) = request.pid {
            let parent = self
                .storage
                .get_session(&parent_id.to_string())
                .map_err(|e| format!("读取父会话失败: {e}"))?
                .ok_or_else(|| format!("父会话不存在: {parent_id}"))?;
            if request.model.trim().is_empty() {
                request.model = parent.model.clone().unwrap_or_default();
            }
            if request.provider.is_none() {
                request.provider = parent.provider.clone();
            }
            if request.session_mode.is_none() {
                request.session_mode = Some(parent.session_mode);
            }
            if request.permission_mode.is_none() {
                request.permission_mode = Some(parent.permission_mode);
            }
            if request.reasoning_effort.is_none() {
                request.reasoning_effort = Some(parent.reasoning_effort);
            }
        }
        // 智能体卡片**创建时即决定**会话模式与权限（不保留"请求 vs 卡片"两套来源）：
        // - 执行模式：只读 → plan（工具层硬拒写）/ 读写 → execute / 自动 → auto；
        //   `继承` → 沿用请求（子会话即父会话）→ 缺省值。
        // - 权限模式：手动 / 自动；`继承` → 同上。
        let session_mode = card
            .mode
            .to_session()
            .or(request.session_mode)
            .unwrap_or_default();
        let permission_mode = card
            .permission
            .to_session()
            .or(request.permission_mode)
            .unwrap_or_default();
        let model = request.model.clone();
        let provider = request.provider.clone();
        // 供应商校验（在子会话继承之后：显式指定不存在的供应商 → 拒绝创建）
        if let Some(p) = provider.as_deref() {
            if !self.llm.has_provider(p) {
                return Err(format!(
                    "供应商不存在: {p}（已配置: {}）",
                    self.llm.provider_names().join(", ")
                ));
            }
        }
        // 可见能力按智能体卡片物化：工具集强制过滤；技能集仅进提示词（`skills` 工具不受限）
        let tool_list = agent_visible_tools(&card);
        let skill_list = card.skill_ids.clone();
        // 工作区：显式目录优先；子会话缺省落在 `父会话工作区/.astorm/subagent_{本会话id}/`，
        // 普通会话缺省按创建时刻精确到秒命名（每条任务目录唯一）
        let workspace = request
            .workspace
            .clone()
            .filter(|w| !w.trim().is_empty())
            .or_else(|| match request.pid {
                Some(parent_id) => Some(subagent_workspace(&parent_id, &id)),
                None => default_workspace_name(),
            });
        // 会话类型：携带 pid 即子会话（**强制**，调用方无需重复声明，避免与 pid 漂移）；
        // 其余按调用方声明，缺省普通会话（前端「任务」分组）。
        let session_type = if request.pid.is_some() {
            SessionType::Child
        } else {
            request.session_type.unwrap_or_default()
        };
        let record = SessionRecord {
            id,
            pid: request.pid,
            session_type,
            // 归属定时任务（仅调度器注入；其余调用方为 None）
            scheduled_task_id: request.scheduled_task_id.clone(),
            agent_type: Some(card.agent_id.clone()),
            title: None,
            title_zh: None,
            title_en: None,
            model: Some(model.clone()),
            provider: provider.clone(),
            workspace,
            session_mode,
            reasoning_effort: request.reasoning_effort.unwrap_or_default(),
            permission_mode,
            skill_list,
            tool_list,
            mcp_list: Vec::new(),
            // 新会话无动态加载态：可加载集（schedule_task / mail / browser）由模型经
            // `tool_load` 显式加载后才进入 `loaded_tool_list`（见 agent::dynamic_tools）
            loaded_tool_list: Vec::new(),
            session_usage: TokenUsage::default(),
            contributed_to_parent: Default::default(),
            subagent_usage: Default::default(),
            context_tokens: 0,
            summary_times: 0,
            question_times: 0,
            current_round: 0,
            created_at: now,
            updated_at: now,
            // 首条请求已入队并立即 spawn run → 会话自创建即为「运行中」（侧栏转圈由此驱动）；
            // `auto_run=false`（仅创建）时不起循环，落 `Idle` —— 前端把它当「无状态标记」，
            // 看板正好归到「待办（新建未执行）」。
            status: if auto_run {
                SessionStatus::Running
            } else {
                SessionStatus::Idle
            },
            status_updated_at: now,
            goal: None,
        };
        self.storage
            .upsert_session(&record)
            .map_err(|e| format!("会话元数据落盘失败: {e}"))?;
        // 会话信息持久化 → 广播事件（前端会话信息卡片随创建即展示最新记录）
        let _ = self
            .event_tx
            .send(session_info_event(&id.to_string(), &record));

        let session = Arc::new(Session {
            record: Mutex::new(record),
            cancel: RwLock::new(CancellationToken::new()),
            event_tx: self.event_tx.clone(),
            history: Mutex::new(Vec::new()),
            // 首条请求已入队并立即 spawn run：先置 running=true，避免紧随的
            // send_message 再 spawn 一个并发 run 造成双循环竞态。
            // 仅创建时留 false：这个会话没有循环在跑，之后 send_message 才 spawn。
            running: Mutex::new(auto_run),
            // 仅创建：首条请求不入队（想跑时用户在该会话里再发一条消息即可）
            queue: Mutex::new(if auto_run {
                VecDeque::from([SessionInput::Message(request.clone())])
            } else {
                VecDeque::new()
            }),
            interrupt_queue: Mutex::new(VecDeque::new()),
            storage: Arc::clone(&self.storage),
            llm: self.llm.clone(),
            summary: self.summary.clone(),
            compact: self.compact.clone(),
            deleted: AtomicBool::new(false),
            next_seq: AtomicI64::new(0),
            pending_rewind: Mutex::new(None),
            goal_cfg: self.goal.clone(),
        });

        self.sessions
            .write()
            .map_err(|_| "会话注册表锁中毒".to_string())?
            .insert(id, Arc::clone(&session));

        let id_str = id.to_string();

        // 目标（`/goal`）：随创建请求原子设置，须在首轮循环跑完前生效
        self.sync_goal(&session, &id_str, &request);

        // 确保任务工作区目录已创建（「从资源管理器打开」/ 工具执行都依赖该目录；
        // resolve_workspace 幂等建目录，纯文本会话不触发工具时也不会缺失）。
        // 主线程内同步执行：create_session 返回时目录已落盘。
        let _ = crate::tool::resolve_workspace(&id_str);

        // 仅创建：首条消息**写进会话历史**（引擎只持久化 assistant/tool，这里补上 user），
        // 之后打开会话能看到这条指令；想跑时在里面再发一条消息即可。
        // 附件也在这条路径上完成「复制进会话工作区 + 写 metadata」。
        if !auto_run {
            let _ = session.append_user_message(&id_str, &request);
        }

        // 顺序编排：先生成标题 → 元数据落盘 → 发 title 事件 → 再驱动主循环。
        // 首条请求已入队且 running 预置 true：send_message 在此窗口只入队不 spawn，
        // run 迟到启动不会引发双循环竞态。
        let message = first_message.clone();
        let title_session = Arc::clone(&session);
        let _ = self.handle.spawn(async move {
            // ① 首条消息非空时先生成标题（失败仅告警，不阻塞主循环）
            if !message.trim().is_empty() {
                Self::generate_title_and_emit(&title_session, &id_str, &model, provider.as_deref(), &message)
                    .await;
            }
            // ② 仅创建（auto_run=false）：标题已落，到此为止 —— 不驱动 ReAct 循环
            if !auto_run {
                return;
            }
            // ④ 再驱动首轮 ReAct 循环（队列首条即本次请求）
            session.run().await;
        });

        Ok(id)
    }

    /// 会话列表（含**运行状态修正**）。
    ///
    /// 落盘的 `status` 可能陈旧：进程重启后不可能还有会话在跑，故：
    /// - 内存注册表里有该会话且 `running == true` → `Running`（本进程真实运行中）
    /// - 否则落盘 `Running` 视为**上次被进程退出打断** → `Interrupted`
    ///
    /// 只修正返回值，**不回写磁盘**（避免读接口带写副作用；真正写入由下一次状态变更完成）。
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, String> {
        let mut records = self
            .storage
            .list_sessions()
            .map_err(|e| format!("读取会话列表失败: {e}"))?;
        let live: Vec<Uuid> = {
            let Ok(map) = self.sessions.read() else {
                return Ok(records);
            };
            map.iter()
                .filter(|(_, s)| s.running.lock().map(|r| *r).unwrap_or(false))
                .map(|(id, _)| *id)
                .collect()
        };
        for record in &mut records {
            if record.status != SessionStatus::Running {
                continue;
            }
            if !live.contains(&record.id) {
                record.status = SessionStatus::Interrupted;
            }
        }
        Ok(records)
    }

    fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, String> {
        let sid = session_id.to_string();
        // UI 展示回填：摘要后补回「摘要之前最近 ≤2 轮完整轮次」的原始消息（含用户气泡），
        // 让用户仍能看到被压缩掉的前文；seq 单调递增保证拼接顺序自然正确。
        // 仅显示路径拼接——喂模型 / 重放仍只读活动日志（load_messages）。
        let mut out = self
            .storage
            .load_archived_tail(&sid, 2)
            .map_err(|e| format!("读取会话历史归档失败: {e}"))?;
        out.extend(
            self.storage
                .load_messages(&sid)
                .map_err(|e| format!("读取会话消息失败: {e}"))?,
        );
        Ok(out)
    }

    fn archived_messages_before(
        &self,
        session_id: &Uuid,
        before_seq: i64,
        max_user_turns: u32,
    ) -> Result<Vec<MessageRecord>, String> {
        self.storage
            .load_archived_before(&session_id.to_string(), before_seq, max_user_turns as usize)
            .map_err(|e| format!("读取会话归档失败: {e}"))
    }

    fn system_prompt(&self, session_id: &Uuid) -> Result<String, String> {
        // 系统提示词按当前配置实时拼装（含 AGENTS.md / 智能体指引），不读落盘副本（可能过期）
        let session = self.get_or_rehydrate(session_id)?;
        Ok(session.system_prompt())
    }

    fn delete_session(&self, session_id: &Uuid) -> Result<(), String> {
        let removed = self
            .sessions
            .write()
            .map_err(|_| "会话注册表锁中毒".to_string())?
            .remove(session_id);
        if let Some(session) = removed {
            // 标记已删除:运行中循环后续 persist/append 跳过,避免复活
            session.deleted.store(true, Ordering::SeqCst);
            if let Ok(cancel) = session.cancel.read() {
                cancel.cancel();
            }
        }
        self.storage
            .delete_session(&session_id.to_string())
            .map_err(|e| format!("删除会话持久化失败: {e}"))?;
        // 清除会话摘要快照目录（尽力而为，避免删除后残留孤儿摘要文件）
        crate::agent::summary::cleanup_summary_dir(&session_id.to_string());
        // 清除会话级内存白名单（审批卡「当前会话允许」的记录，避免残留到同 id 新会话）
        if let Some(app) = crate::app::App::try_global() {
            app.session_allow().clear_session(&session_id.to_string());
        }
        Ok(())
    }

    fn send_message(&self, session_id: &Uuid, request: CreateSessionRequest) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;

        // 供应商校验（显式指定不存在的供应商 → 拒绝入队）
        if let Some(p) = request.provider.as_deref() {
            if !self.llm.has_provider(p) {
                return Err(format!("供应商不存在: {p}"));
            }
        }

        // 用户取消后恢复：cancel_session 置位的一次性取消令牌会阻断后续所有循环，
        // 新消息入队前换新 token，使会话可重新驱动（参考 desktop reset_cancel）
        if session.cancel.read().map(|c| c.is_cancelled()).unwrap_or(false) {
            *session
                .cancel
                .write()
                .map_err(|_| "会话取消锁中毒".to_string())? = CancellationToken::new();
        }

        // 会话模式 / 审批策略跟随最新请求（UI 每条消息随发，运行时切换即时生效）。
        // 中断决议不走本入口（见 `resolve_interrupt`），故无需再特判审批标记。
        let sid = session_id.to_string();
        self.sync_mode_permission(&session, &sid, &request);
        // 目标（`/goal`）：随本条消息原子设置
        self.sync_goal(&session, &sid, &request);

        // 锁序 running → queue，与 Session::run 保持一致（避免双循环竞态）
        let spawn = {
            let mut running = session
                .running
                .lock()
                .map_err(|_| "运行锁中毒".to_string())?;
            let mut queue = session
                .queue
                .lock()
                .map_err(|_| "队列锁中毒".to_string())?;
            queue.push_back(SessionInput::Message(request));
            if *running {
                false
            } else {
                *running = true;
                true
            }
        };
        // 入队即视为运行中（侧栏状态图标；run 出队后仍会再置一次，幂等）
        session.set_status(SessionStatus::Running);
        if spawn {
            let _ = self.handle.spawn(session.run());
        }
        Ok(())
    }

    fn start_session(&self, session_id: &Uuid) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;

        // 历史体检在拿 running/queue 之前做（锁序是 running → queue，history 不在这条链上，
        // 先拿先放，不与它们嵌套）。末尾必须是 user 消息 —— 只有「仅创建」会留下这个形态：
        // 指令已在历史里、却没跑过。其余形态一律拒绝，免得发出一条模型看不懂的请求。
        {
            let history = session.history.lock().map_err(|_| "历史锁中毒".to_string())?;
            match history.last() {
                Some(last) if last.role == "user" => {}
                Some(_) => {
                    return Err("该会话没有待执行的指令（历史末尾不是用户消息）".into());
                }
                None => return Err("该会话没有待执行的指令（历史为空）".into()),
            }
        }

        // 与 send_message 同款：取消令牌是一次性的，重新驱动前换新 token
        if session.cancel.read().map(|c| c.is_cancelled()).unwrap_or(false) {
            *session
                .cancel
                .write()
                .map_err(|_| "会话取消锁中毒".to_string())? = CancellationToken::new();
        }

        // 锁序 running → queue，与 Session::run 保持一致；正在跑就直接拒绝（不排队）：
        // 「启动」是对一条**没跑过**的会话发起的，跑着的时候再点一次没有意义。
        {
            let mut running = session
                .running
                .lock()
                .map_err(|_| "运行锁中毒".to_string())?;
            if *running {
                return Err("会话正在运行".into());
            }
            let mut queue = session
                .queue
                .lock()
                .map_err(|_| "队列锁中毒".to_string())?;
            queue.push_back(SessionInput::Start);
            *running = true;
        }
        // 入队即视为运行中（侧栏转圈；run 出队后仍会再置一次，幂等）
        session.set_status(SessionStatus::Running);
        let _ = self.handle.spawn(session.run());
        Ok(())
    }

    fn cancel_session(&self, session_id: &Uuid) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;
        if let Ok(cancel) = session.cancel.read() {
            cancel.cancel();
        }
        Ok(())
    }

    fn rewind_to_user_message(
        &self,
        session_id: &Uuid,
        keep_user_messages: u32,
    ) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;
        let running = session.running.lock().map(|r| *r).unwrap_or(false);
        if running {
            // 同步做边界/目标校验，把错误直接回给前端（不必等循环）；再挂起 + 打断当前循环，
            // 由 drain 循环在本轮结束后、发终态事件前落地截断（避免与在飞轮次竞态）。
            session.plan_rewind(keep_user_messages)?;
            *session
                .pending_rewind
                .lock()
                .map_err(|_| "撤回锁中毒".to_string())? = Some(keep_user_messages);
            if let Ok(cancel) = session.cancel.read() {
                cancel.cancel();
            }
            Ok(())
        } else {
            session.rewind_now(keep_user_messages)?;
            session.set_status(SessionStatus::Done);
            Ok(())
        }
    }

    /// 异步改写会话级设置（模式 / 审批 / 思考强度 / 模型 / 供应商）。
    ///
    /// 与 `sync_mode_permission` 共用 `SessionSettings::apply_to`，但**不依赖任何消息**，
    /// 并且把落盘失败如实回给前端（前端会回滚乐观值），而不是静默保持旧值。
    /// 只改内存活记录即生效——工具策略按工具调用读它；**不打断**当前循环，也不动
    /// 已在等待的审批卡（切到自动授权只影响后续工具调用）。
    fn update_settings(&self, session_id: &Uuid, settings: SessionSettings) -> Result<(), String> {
        if settings.is_empty() {
            return Ok(()); // 全缺省 = 无操作，不落盘不广播
        }
        // 供应商校验与 send_message 同口径：显式指定不存在的供应商 → 拒绝（前端回滚乐观值并提示），
        // 而不是把坏供应商落进记录、等到下一轮推理才以更含糊的错误暴露
        if let Some(p) = settings.provider.as_deref() {
            if !self.llm.has_provider(p) {
                return Err(format!("供应商不存在: {p}"));
            }
        }
        let session = self.get_or_rehydrate(session_id)?;
        let record = {
            let mut guard = session
                .record
                .lock()
                .map_err(|_| "会话记录锁中毒".to_string())?;
            if !settings.apply_to(&mut guard) {
                return Ok(()); // 幂等：值没变（含并发下已被同值写入）→ 不落盘不广播
            }
            guard.updated_at = unix_now();
            guard.clone()
        };
        self.storage
            .upsert_session(&record)
            .map_err(|e| format!("会话设置落盘失败: {e}"))?;
        let _ = session
            .event_tx
            .send(session_info_event(&session_id.to_string(), &record));
        Ok(())
    }

    fn update_goal(
        &self,
        session_id: &Uuid,
        goal: Option<String>,
        goal_lang: Option<String>,
    ) -> Result<(), String> {
        let session = self.get_or_rehydrate(session_id)?;
        if session.is_deleted() {
            return Err("会话已删除".into());
        }
        let text = goal
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let record = {
            let mut guard = session
                .record
                .lock()
                .map_err(|_| "会话记录锁中毒".to_string())?;
            match text {
                Some(text) => {
                    // 修改 / 新设目标：预算归零，基线 = 当前累计用量（含子 agent）
                    guard.goal = Some(GoalState {
                        text,
                        lang: goal_lang.unwrap_or_default(),
                        tokens_used: 0,
                        last_total: (guard.session_usage.total + guard.subagent_usage.total) as u64,
                        iterations: 0,
                        status: GoalStatus::Active,
                    });
                }
                None => {
                    if guard.goal.is_none() {
                        return Ok(()); // 幂等：本来就没有目标
                    }
                    guard.goal = None;
                }
            }
            guard.updated_at = unix_now();
            guard.clone()
        };
        self.storage
            .upsert_session(&record)
            .map_err(|e| format!("目标落盘失败: {e}"))?;
        let _ = session
            .event_tx
            .send(session_info_event(&session_id.to_string(), &record));
        Ok(())
    }
}

#[cfg(test)]
mod rewind_tests {
    use super::*;

    fn rec(seq: i64, role: &str, summary: bool) -> MessageRecord {
        MessageRecord {
            id: format!("m{seq}"),
            session_id: "s".into(),
            role: role.into(),
            content: format!("c{seq}"),
            tool_calls: serde_json::Value::Null,
            seq,
            created_at: seq,
            reasoning_content: None,
            tool_call_id: None,
            metadata: if summary {
                Some(serde_json::json!({ "summary": true }))
            } else {
                None
            },
        }
    }

    #[test]
    fn cutoff_keeps_before_target_user() {
        let recs = vec![
            rec(0, "user", false),
            rec(1, "assistant", false),
            rec(2, "user", false),
            rec(3, "assistant", false),
        ];
        // 保留前 1 条 user → 删第 2 条（seq=2）及其后
        assert_eq!(compute_rewind_cutoff(&recs, 1).unwrap(), 2);
        // 保留 0 条 → 删第一条 user（seq=0）
        assert_eq!(compute_rewind_cutoff(&recs, 0).unwrap(), 0);
        // 越界
        assert!(compute_rewind_cutoff(&recs, 2).is_err());
    }

    #[test]
    fn cutoff_allows_after_summary_but_rejects_crossing() {
        // 摘要之后仍有 user：可撤回（切点 > 摘要 seq）
        let after = vec![
            rec(0, "system", false),
            rec(1, "assistant", true), // 摘要
            rec(2, "user", false),
            rec(3, "assistant", false),
        ];
        assert_eq!(compute_rewind_cutoff(&after, 0).unwrap(), 2);

        // 摘要之前存在 user（异常/旧记录）：撤回会跨摘要 → 拒绝
        let crossing = vec![
            rec(0, "user", false),
            rec(1, "assistant", true), // 摘要
            rec(2, "user", false),
        ];
        let err = compute_rewind_cutoff(&crossing, 0).unwrap_err();
        assert!(err.contains("不可跨摘要撤回"), "err: {err}");
    }

    fn rec_tools(seq: i64, role: &str, tools: serde_json::Value, summary: bool) -> MessageRecord {
        let mut r = rec(seq, role, summary);
        r.tool_calls = tools;
        r
    }

    #[test]
    fn fork_cut_before_last_final_assistant() {
        // 无摘要:[user0, final1, user2, final3];候选=[1,3]
        let recs = vec![
            rec(0, "user", false),
            rec(1, "assistant", false),
            rec(2, "user", false),
            rec(3, "assistant", false),
        ];
        // None → 最后一个 final 之前 → 继承 [0,1,2]
        assert_eq!(compute_fork_cut(&recs, None).unwrap(), 3);
        // Some(0) → 第一个 final 之前 → 继承 [0]
        assert_eq!(compute_fork_cut(&recs, Some(0)).unwrap(), 1);
        // Some(1) → 第二个 final 之前 → 继承 [0,1,2]
        assert_eq!(compute_fork_cut(&recs, Some(1)).unwrap(), 3);
        // 越界
        assert!(compute_fork_cut(&recs, Some(2)).is_err());
    }

    #[test]
    fn fork_cut_skips_tool_call_assistants() {
        // 带工具调用的 assistant 不算「无工具 final」
        let recs = vec![
            rec(0, "user", false),
            rec_tools(1, "assistant", serde_json::json!([{ "id": "t1" }]), false),
            rec(2, "tool", false),
            rec(3, "assistant", false), // 唯一 final
        ];
        assert_eq!(compute_fork_cut(&recs, None).unwrap(), 3);
        assert_eq!(compute_fork_cut(&recs, Some(0)).unwrap(), 3);
    }

    #[test]
    fn fork_cut_keeps_summary_prefix_and_only_considers_after_it() {
        // 摘要后:[user2, final3];摘要记录自身不作为候选;继承含 system+摘要前缀
        let recs = vec![
            rec(0, "system", false),
            rec(1, "assistant", true), // 摘要
            rec(2, "user", false),
            rec(3, "assistant", false),
        ];
        // None → 摘要后最后 final(3)之前 → 继承 [0,1,2]
        assert_eq!(compute_fork_cut(&recs, None).unwrap(), 3);
        // 无 final 候选 → 继承全部
        let no_final = vec![
            rec(0, "system", false),
            rec(1, "assistant", true),
            rec(2, "user", false),
            rec_tools(3, "assistant", serde_json::json!([{ "id": "t1" }]), false),
        ];
        assert_eq!(compute_fork_cut(&no_final, None).unwrap(), 4);
    }
}
