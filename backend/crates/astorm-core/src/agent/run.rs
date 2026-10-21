//! 会话 drain 循环：[`Session::run`] 消费「用户消息 / 中断决议」队列，每条驱动一次完整 ReAct 循环。
//!
//! 两类输入共用一条 FIFO 队列（[`SessionInput`]）：
//! - `Message`：追加 user 消息 → 落盘 → 跑 LLM
//! - `Resolution`：就地把用户决议写回被中断的 tool 消息（决议即 tool 结果，**不追加** user 消息）
//!   → 跑 LLM（表单「忽略」不跑，省 token）。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use astorm_common::session::req::CreateSessionRequest;
use astorm_common::types::record::SessionStatus;
use llm_provider::ChatClient;
use react_loop::ReactEngine;
use tokio_util::sync::CancellationToken;

use crate::agent::context::build_loop_context;
use crate::agent::session::{Session, SessionInput};

impl Session {
    /// 更新会话运行状态（侧栏状态图标的唯一真相源）：内存记录 + 落盘 + 广播 `session_info`。
    ///
    /// **幂等**（状态未变不写盘不广播）；锁中毒 / 落盘失败静默跳过（状态变更不应影响主流程）。
    /// **已删除的会话整段跳过**：删除会取消循环，但循环收尾仍可能走到这里（done / cancelled /
    /// error），不拦就会把删掉的那行 upsert 回存储、并广播一条属于已删会话的 `session_info`。
    pub(crate) fn set_status(&self, status: SessionStatus) {
        if self.is_deleted() {
            return;
        }
        let record = {
            let Ok(mut guard) = self.record.lock() else {
                return;
            };
            if guard.status == status {
                return;
            }
            guard.status = status;
            guard.status_updated_at = crate::agent::session::unix_now();
            guard.clone()
        };
        if self.storage.upsert_session(&record).is_err() {
            return;
        }
        let sid = record.id.to_string();
        let _ = self
            .event_tx
            .send(crate::agent::session::session_info_event(&sid, &record));
    }

    /// 追加一条用户消息到会话历史并落盘，返回追加后的历史克隆（本轮请求历史）。
    ///
    /// 两条路径共用：
    /// - drain 循环收到 `SessionInput::Message`（正常执行路径）；
    /// - 创建会话时 `auto_run = false`（「仅创建」）：不起循环，但指令要留在历史里，
    ///   之后打开会话能看到这条 user 消息（引擎只持久化 assistant/tool，user 必须在这里补）。
    ///
    /// 附件（图片）在此复制进会话工作区并并入本条 user 消息（多模态），
    /// 元数据 `metadata.attachments` 随消息落盘（冷启动重建 / 前端缩略图据此恢复）。
    /// 历史锁中毒时返回 `None`（调用方据此收手）。
    pub(crate) fn append_user_message(
        &self,
        sid: &str,
        request: &CreateSessionRequest,
    ) -> Option<Vec<llm_provider::ChatMessage>> {
        let mut history = self.history.lock().ok()?;
        let base = crate::agent::attachment::build_user_message(
            sid,
            &request.user_message,
            request.attachments.as_deref(),
        );
        let workspace = self
            .record
            .lock()
            .ok()
            .and_then(|r| r.workspace.clone());
        // 「引用说明」= 结构化提及指令 + 元素围栏原文（@文件 / $智能体 / 浏览器元素）：
        // 正文只保留用户可见的「原文」，说明作为元数据落盘（**不渲染**）。
        let note = crate::agent::attachment::render_refs_note(
            request.mentions.as_deref().unwrap_or(&[]),
            request.elements.as_deref().unwrap_or(&[]),
            request.office_refs.as_deref().unwrap_or(&[]),
            request.quotes.as_deref().unwrap_or(&[]),
            workspace.as_deref(),
        );
        // 落盘 = 原文 + 附件元数据 + 引用说明（metadata.refsNote）：冷启动重建据此拼回正文。
        // seq 取会话内单调递增计数器（摘要压缩历史后不回退，见 Session.next_seq）
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let mut record = crate::agent::history::message_to_record(&base, sid, seq);
        crate::agent::attachment::set_refs_note(&mut record.metadata, &note);
        if let Err(e) = self
            .storage
            .append_messages(sid, std::slice::from_ref(&record))
        {
            tracing::error!("[session {sid}] 用户消息落盘失败: {e}");
        }
        // 喂模型 = 原文 + 引用说明（仅本级副本，不进 UI 回显）
        let user_msg = crate::agent::attachment::append_note(base, &note);
        history.push(user_msg);
        let snapshot = history.clone();
        drop(history);
        // 用户提问次数 +1：每追加一条用户消息（普通发送 / 仅创建 / goal 续跑）即计数，
        // 并立即落盘 + 广播 `session_info`，前端会话信息卡片实时刷新。
        self.bump_question_times(sid);
        Some(snapshot)
    }

    /// 用户提问次数 +1：内存记录 → 落盘 → 广播 `session_info`（失败仅告警，不影响消息落盘）。
    ///
    /// **已删除的会话整段跳过**：与 `set_status` 同口径，避免把删掉的记录 upsert 回存储。
    fn bump_question_times(&self, sid: &str) {
        if self.is_deleted() {
            return;
        }
        let snapshot = {
            let Ok(mut record) = self.record.lock() else {
                return;
            };
            record.question_times += 1;
            record.updated_at = crate::agent::session::unix_now();
            record.clone()
        };
        if let Err(e) = self.storage.upsert_session(&snapshot) {
            tracing::warn!("[session {sid}] 用户提问次数落盘失败: {e}");
        }
        let _ = self
            .event_tx
            .send(crate::agent::session::session_info_event(sid, &snapshot));
    }

    /// 写回用户提问次数（撤回历史后回退到保留条数）：值未变则跳过；落盘 + 广播 `session_info`。
    pub(crate) fn set_question_times(&self, value: i32) {
        if self.is_deleted() {
            return;
        }
        let snapshot = {
            let Ok(mut record) = self.record.lock() else {
                return;
            };
            if record.question_times == value {
                return;
            }
            record.question_times = value;
            record.updated_at = crate::agent::session::unix_now();
            record.clone()
        };
        let sid = snapshot.id.to_string();
        if let Err(e) = self.storage.upsert_session(&snapshot) {
            tracing::warn!("[session {sid}] 用户提问次数落盘失败: {e}");
        }
        let _ = self
            .event_tx
            .send(crate::agent::session::session_info_event(&sid, &snapshot));
    }
    /// 会话 drain 循环：逐条消费用户消息队列，每条驱动一次完整 ReAct 循环。
    ///
    /// 每条循环结束后（工具调用完毕 / 无工具调用的助手消息之后）重新读取队列：
    /// 有排队请求则作为 user 消息继续下一轮循环，队列清空则结束。
    pub async fn run(self: Arc<Self>) {
        // 引擎默认客户端：会话默认供应商（create/send 已校验存在供应商；空兜底客户端
        // 不会被真正调用——本轮请求路由到的客户端经 LoopContext.client 覆盖，见 build_loop_context）
        let engine = ReactEngine::new(
            self.llm
                .default_client()
                .unwrap_or_else(|| Arc::new(ChatClient::new("", ""))),
            self.clone(),
        );

        loop {
            // 出队一条用户请求（打断队列优先进口：「立即发送」打断当前循环后优先执行）。
            // 暂停(令牌已取消且无打断消息)时不出队:消息留在队列,走收尾复位 running,
            // 待 send_message 的 reset_cancel + spawn 重新驱动——否则换新令牌会让暂停零效果。
            // 注意：此处只出队、不碰历史——审批重放需要 await（持 std 锁跨 await 会死锁）。
            let mut paused = false;
            let request = {
                let mut queue = match self.queue.lock() {
                    Ok(queue) => queue,
                    Err(_) => return,
                };
                let mut interrupt_queue = match self.interrupt_queue.lock() {
                    Ok(iq) => iq,
                    Err(_) => return,
                };
                let from_interrupt = !interrupt_queue.is_empty();
                let token_cancelled = self.cancel.read().map(|c| c.is_cancelled()).unwrap_or(false);
                if token_cancelled && !from_interrupt {
                    // 用户暂停 / 标题生成窗口取消:停止消费,不出队不入历史
                    paused = true;
                    None
                } else {
                    // 打断消息需换新已取消的令牌(打断语义 = 取消旧循环,优先跑打断消息)
                    if from_interrupt && token_cancelled {
                        if let Ok(mut cancel) = self.cancel.write() {
                            *cancel = CancellationToken::new();
                        }
                    }
                    interrupt_queue.pop_front().or_else(|| queue.pop_front())
                }
            };
            let Some(input) = request else {
                if paused {
                    // 暂停收尾:复位 running 并退出(只取 running 锁,符合锁序),
                    // 后续 send_message 见 running=false 会 spawn 新循环恢复消费
                    if let Ok(mut running) = self.running.lock() {
                        *running = false;
                    }
                    // 用户暂停 = 取消态（侧栏不显示图标；与「跑一半被中断」区分开）
                    self.set_status(SessionStatus::Cancelled);
                }
                break;
            };

            let sid = self.record.lock().map(|r| r.id.to_string()).unwrap_or_default();
            // 本轮开始：会话进入运行中（侧栏转圈；决议续跑 / 排队续跑同样途经此处）
            self.set_status(SessionStatus::Running);
            // 本轮起点：普通用户消息（追加 user 消息）或中断决议（就地改写被中断的 tool 消息）。
            // 决议**不追加 user 消息**——决议本身就是 tool 结果，模型据此纯 ReAct 续跑。
            let (request, history, should_run) = match input {
                SessionInput::Message(request) => {
                    // 普通消息到达：未决中断（待审批 / 待回答）置 superseded，避免模型反复重试
                    crate::agent::interrupt::supersede_pending(&self, &sid);
                    // 追加 user 消息到历史并落盘（内含附件处理，见 append_user_message）；
                    // 拿到的克隆即本轮请求历史。历史锁拿不到（中毒）时结束循环（与原行为一致）。
                    let Some(history) = self.append_user_message(&sid, &request) else {
                        return;
                    };
                    (request, history, true)
                }
                SessionInput::Resolution { pending, resolution } => {
                    // 决议：就地把结果写回被中断的 tool 消息（审批重放 / 表单回答 / 忽略）。
                    // 表单「忽略」返回 false —— 不发起 LLM 轮次（省 token），但走同一收尾块复位 running。
                    let (should_run, injected) = crate::agent::interrupt::resume_interrupt(
                        &self,
                        &sid,
                        &pending,
                        &resolution,
                    )
                    .await;
                    let mut history = match self.history.lock() {
                        Ok(history) => history.clone(),
                        Err(_) => return,
                    };
                    // 重放工具注入的追加消息（如 read_images 的图片 user 消息）：紧跟被重放的
                    // tool 消息之后并入本轮载荷。只进本轮局部 history，不写回会话历史（仅本轮可见）。
                    history.extend(injected);
                    // 决议没有请求体；沿用会话记录里的模型 / 供应商（仅 should_run 时才会用到）
                    let request = CreateSessionRequest {
                        model: self
                            .record
                            .lock()
                            .ok()
                            .and_then(|r| r.model.clone())
                            .unwrap_or_default(),
                        provider: self.record.lock().ok().and_then(|r| r.provider.clone()),
                        // 决议续跑不新建会话：pid / agent_type / session_type 只影响创建，取缺省即可
                        // （工具集与指引按会话记录 `record` 解析，见 build_loop_context / ensure_system_prompt）
                        pid: None,
                        agent_type: None,
                        session_type: None,
                        scheduled_task_id: None,
                        workspace: None,
                        session_mode: None,
                        permission_mode: None,
                        reasoning_effort: None,
                        user_message: String::new(),
                        attachments: None,
                        // 决议续跑：构造完立刻跑本轮，显式声明执行
                        auto_run: Some(true),
                        // 决议续跑无新提及 / 无新目标
                        mentions: None,
                        elements: None,
                        office_refs: None,
                        quotes: None,
                        goal: None,
                        goal_lang: None,
                    };
                    if should_run {
                        self.set_status(SessionStatus::Running);
                    } else if crate::app::App::try_global()
                        .map(|app| app.interrupts.has_session_pending(&sid))
                        .unwrap_or(false)
                    {
                        // 续跑（deferred）中又有调用需要审批：登记时已置「等待用户处理」态，
                        // 保持不动，避免被覆盖成 Idle（前端据此显示待审批图标）
                    } else {
                        // 表单「忽略」：不续跑，会话回到「已忽略提问」态（侧栏灰点）——
                        // 它既不是完成也不是失败：那条提问仍在历史里，可经 reopenInterrupt 继续填写
                        self.set_status(SessionStatus::Ignored);
                    }
                    (request, history, should_run)
                }
                SessionInput::Start => {
                    // 「启动」（看板「待办」卡片）：**不追加 user 消息** —— 指令早在「仅创建」
                    // 时就落在了历史末尾，直接把它当作本轮请求历史（与上面决议同一手法）。
                    // 历史形态已由 start_session 体检过（非空、末尾是 user），此处不再判。
                    let history = match self.history.lock() {
                        Ok(history) => history.clone(),
                        Err(_) => return,
                    };
                    // 没有请求体：沿用会话记录里的模型 / 供应商；思考强度 / 模式 / 审批
                    // 都留空 = 用记录值（见 build_loop_context / SessionSettings 的「缺省即保持」）
                    let request = CreateSessionRequest {
                        model: self
                            .record
                            .lock()
                            .ok()
                            .and_then(|r| r.model.clone())
                            .unwrap_or_default(),
                        provider: self.record.lock().ok().and_then(|r| r.provider.clone()),
                        pid: None,
                        agent_type: None,
                        session_type: None,
                        scheduled_task_id: None,
                        workspace: None,
                        session_mode: None,
                        permission_mode: None,
                        reasoning_effort: None,
                        // 指令已在历史里，这里刻意留空：本轮不产生新的 user 消息
                        user_message: String::new(),
                        attachments: None,
                        auto_run: Some(true),
                        mentions: None,
                        elements: None,
                        office_refs: None,
                        quotes: None,
                        goal: None,
                        goal_lang: None,
                    };
                    (request, history, true)
                }
            };

            // 决议不续跑（表单忽略）时不发起 LLM 轮次：决议已写入历史，
            // 直接落到下方队列收尾（队列空 → 复位 running 结束），不消耗 token。
            if should_run {
                // 会话无标题时补生成标题:创建时首条消息的标题生成被 LLM 限流/失败跳过,
                // 或会话以空消息创建(如任务默认「新任务」)时不会触发,发消息后这里重试;
                // 用历史首条用户消息(最稳定,回退当前消息),并发生成不阻塞主循环,失败仅告警
                let needs_title = self.record.lock().map(|r| r.title.is_none()).unwrap_or(false);
                if needs_title {
                    let title_src = history
                        .iter()
                        .find(|m| m.role == "user")
                        .and_then(|m| m.content.clone())
                        .unwrap_or_else(|| request.user_message.clone());
                    if !title_src.trim().is_empty() {
                        let sid = self.record.lock().map(|r| r.id.to_string()).unwrap_or_default();
                        let model = request.model.clone();
                        let provider = request.provider.clone();
                        let title_session = self.clone();
                        let _ = tokio::spawn(async move {
                            crate::agent::session::SessionImpl::generate_title_and_emit(
                                &title_session,
                                &sid,
                                &model,
                                provider.as_deref(),
                                &title_src,
                            )
                            .await;
                        });
                    }
                }

                // 以该请求的设置构建完整初始化的循环上下文（历史 / 模型 / 思考强度 +
                // 会话状态续接 + 全量元数据；引擎的工具事件已由 emit_event hook 转发）
                let mut ctx = match build_loop_context(&self, &request, history) {
                    Some(ctx) => ctx,
                    None => break,
                };

                // 驱动本轮 ReAct 循环（终态事件在循环结束后由下方统一判定发送）
                let run_result = engine.run(&mut ctx).await;

                // 撤回请求：本轮结束后立即回退历史（内存 + 磁盘）到目标用户消息之前。
                // 必须在发终态事件前完成，保证前端收到 cancelled/done 时两端已一致。
                if let Some(keep) = self.take_pending_rewind() {
                    if let Err(e) = self.rewind_now(keep) {
                        tracing::warn!("[session] 撤回失败: {e}");
                    }
                }

                // 终态事件由编排器统一发送（hook 的 after_loop 无队列视图）：
                // 仍有后续消息（打断 / 排队，含本轮运行期间新入队的）将续跑 → 不发终态，
                // 前端流式状态保持，全部跑完才发唯一 done；否则令牌已取消（用户暂停）→
                // cancelled，正常 → done。引擎出错时不会触发 after_loop，error 亦在此补发
                // （取消导致的错误按取消收尾）
                let session_id = self
                    .record
                    .lock()
                    .map(|r| r.id.to_string())
                    .unwrap_or_else(|_| "未知".into());
                let token_cancelled = self.cancel.read().map(|c| c.is_cancelled()).unwrap_or(false);

                // 目标循环（`/goal`）：本轮正常结束、未取消、无待决中断、且队列为空（无用户后续消息）时，
                // 由单次判定决定是否继续推进。判定为「继续」会注入一条用户消息到队列 → 下方 has_followup=true
                // → 不发终态、继续下一轮（与打断/排队同一抑制机制）。
                if run_result.is_ok() && !token_cancelled {
                    let has_goal = self
                        .record
                        .lock()
                        .map(|r| r.goal.as_ref().is_some_and(|g| g.is_active()))
                        .unwrap_or(false);
                    if has_goal {
                        let has_pending = crate::app::App::try_global()
                            .map(|app| app.interrupts.has_session_pending(&session_id))
                            .unwrap_or(false);
                        let queued = {
                            let q_empty = self.queue.lock().map(|q| q.is_empty()).unwrap_or(false);
                            let iq_empty = self
                                .interrupt_queue
                                .lock()
                                .map(|q| q.is_empty())
                                .unwrap_or(false);
                            !(q_empty && iq_empty)
                        };
                        if !has_pending && !queued {
                            let _ = self.goal_continue(&session_id).await;
                        }
                    }
                }

                // 锁序与出队块一致：queue → interrupt_queue
                let has_followup = {
                    let queue = match self.queue.lock() {
                        Ok(queue) => queue,
                        Err(_) => return,
                    };
                    let interrupt_queue = match self.interrupt_queue.lock() {
                        Ok(iq) => iq,
                        Err(_) => return,
                    };
                    !interrupt_queue.is_empty() || (!queue.is_empty() && !token_cancelled)
                };
                if !has_followup {
                    // 终态事件 + 终态落盘（前端侧栏图标冷启动据此恢复）
                    let (event, status) = match run_result {
                        Ok(()) if !token_cancelled => {
                            (react_loop::event::done(&session_id), SessionStatus::Done)
                        }
                        Ok(()) => (
                            react_loop::event::cancelled(&session_id),
                            SessionStatus::Cancelled,
                        ),
                        Err(_) if token_cancelled => (
                            react_loop::event::cancelled(&session_id),
                            SessionStatus::Cancelled,
                        ),
                        Err(e) => (
                            react_loop::event::error(&session_id, &e),
                            SessionStatus::Error,
                        ),
                    };
                    let _ = self.event_tx.send(event);
                    // 停在中途等用户决定（待审批 / 待填表）时不落终态：登记待决中断时
                    // `register_pending_interrupt` 已把状态置为 `awaiting_*`（内存 + 落盘 + 广播），
                    // 引擎「因中断而退出」不是任务完成。若无条件写 Done，落盘状态被覆盖 →
                    // 重启后侧栏由「等你处理」橙点退化成绿点（前端只能等打开会话才补回）。
                    // 事件照发：前端需要 done 收尾流式态（停转圈 / 定格耗时）。
                    let has_pending = crate::app::App::try_global()
                        .map(|app| app.interrupts.has_session_pending(&session_id))
                        .unwrap_or(false);
                    if let Some(terminal) = terminal_status(status, has_pending) {
                        self.set_status(terminal);
                    }
                    // 长期记忆：会话真正收尾后按需跑一次增量编译（`memory.autoCompile`，
                    // **默认关**）。只在「正常结束且无待决中断」时触发：停在审批/填表时
                    // 对话还没结束，此刻编译只会把半截会话吃进记忆（后续续跑还得重编一次）。
                    // 钩子自身永不阻塞、永不报错——自动编译是后台增益，不该打断会话收尾。
                    if status == SessionStatus::Done
                        && !has_pending
                        && let Some(app) = crate::app::App::try_global()
                    {
                        app.memory().auto_compile_after_session(&session_id);
                    }
                }

            }

            // 兜底：未起轮次（暂停 / 无 should_run）的路径也要落地撤回（take 幂等）
            if let Some(keep) = self.take_pending_rewind() {
                if let Err(e) = self.rewind_now(keep) {
                    tracing::warn!("[session] 撤回失败: {e}");
                }
            }

            // 本轮循环结束：队列非空则继续（running 置回 true），否则收尾（打断队列同样计入）
            let has_next = {
                // 锁序：running → queue / interrupt_queue，与 enqueue 保持一致（避免双循环竞态）
                let mut running = match self.running.lock() {
                    Ok(running) => running,
                    Err(_) => return,
                };
                let queue = match self.queue.lock() {
                    Ok(queue) => queue,
                    Err(_) => return,
                };
                let interrupt_queue = match self.interrupt_queue.lock() {
                    Ok(iq) => iq,
                    Err(_) => return,
                };
                *running = false;
                if queue.is_empty() && interrupt_queue.is_empty() {
                    false
                } else {
                    *running = true;
                    true
                }
            };
            if !has_next {
                break;
            }
        }
    }
}

/// 终态落盘判定：本轮跑完该往盘上写哪个状态（`None` = 不写，保持当前状态）。
///
/// 有未决中断（待审批 / 待填表）时**不写终态**：等待态是 `register_pending_interrupt` 登记的，
/// 比本轮的「跑完了」更权威（见 `run` 终态块注释）。
///
/// 规则单独成函数只为可测：drain 循环要真的跑出中断，必须先有真实 LLM 回复去发起工具调用，
/// 测试里驱动不到那一步（`astorm-tests` 的 e2e 需真实凭据且 `#[ignore]`）。
fn terminal_status(planned: SessionStatus, has_pending_interrupt: bool) -> Option<SessionStatus> {
    (!has_pending_interrupt).then_some(planned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_status_writes_terminal_when_no_pending_interrupt() {
        for planned in [
            SessionStatus::Done,
            SessionStatus::Cancelled,
            SessionStatus::Error,
        ] {
            assert_eq!(
                terminal_status(planned, false),
                Some(planned),
                "没有待决中断时应正常落终态"
            );
        }
    }

    #[test]
    fn terminal_status_keeps_awaiting_when_interrupt_pending() {
        // 待决中断在场：awaiting_form / awaiting_approval 必须留在盘上（返回 None = 不写），
        // 否则重启后侧栏由橙点退化成绿点 / 红点（前端只能等打开会话才补回）
        for planned in [
            SessionStatus::Done,
            SessionStatus::Cancelled,
            SessionStatus::Error,
        ] {
            assert_eq!(
                terminal_status(planned, true),
                None,
                "有未决中断时不得覆盖等待态"
            );
        }
    }
}