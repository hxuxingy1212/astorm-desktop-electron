//! `ReactLoop` hook 编排：`Session` 实现循环钩子的**薄壳**。
//!
//! 具体能力下沉到各模块：`prompt`（提示词注入）、`history`（tool 配对修复 / 消息落盘）、
//! `summary`（摘要压缩）、本文件 `persist_session_state`（每轮状态持久化）。
//! 同 trait + 同类型只能有一处 impl，故编排集中于此、各方法保持极薄。

use async_trait::async_trait;
use llm_provider::ChatMessage;
use react_loop::{LlmResult, LoopContext, ReactLoop};
use serde_json::Value;
use std::sync::atomic::Ordering;

use crate::agent::history::message_to_record;
use crate::agent::session::{session_info_event, unix_now, Session};

impl Session {
    /// 会话摘要：阈值达标则压缩旧历史（失败仅告警，不中断循环）。
    async fn try_summarize(&self, ctx: &mut LoopContext) {
        // 循环内按阈值自动摘要（force=false）；手动 /compact 走 summarize_now（force=true）
        if let Err(e) = crate::agent::summary::summarize_if_needed(self, ctx, false).await {
            tracing::warn!("[session {}] 会话摘要失败: {e}", ctx.session_id);
        }
    }

    /// 每轮 LLM 后持久化会话状态：轮次 / 累计用量 / 更新时间 → 元数据落盘。
    ///
    /// 调用时机在引擎保存 assistant 消息并调用 `append_message`（jsonl 落盘）**之后**，
    /// 与消息同点持久化——中途异常退出时，消息与元数据对齐，恢复时不会"文件有消息、
    /// 记录是旧的"。失败仅告警（下一轮会再次尝试）。
    fn persist_session_state(&self, ctx: &LoopContext) {
        // 会话已被删除:运行中循环的收尾持久化跳过,避免把已删记录插回存储
        if self.is_deleted() {
            return;
        }
        let (snapshot, prev_contributed, parent_id) = {
            let mut record = match self.record.lock() {
                Ok(record) => record,
                Err(_) => return,
            };
            record.current_round = ctx.round as i32;
            record.session_usage = ctx.total_usage.clone();
            // 上一轮请求的真实 prompt token（当前上下文占用；供会话信息进度条 + 摘要阈值）
            record.context_tokens = ctx.last_prompt_tokens;
            record.updated_at = unix_now();
            // 子会话：记录「已上报父会话」的累计，用于本次算增量（父会话自身忽略该字段）
            let prev_contributed = record.contributed_to_parent.clone();
            let parent_id = record.pid;
            record.contributed_to_parent = record.session_usage.clone();
            (record.clone(), prev_contributed, parent_id)
        };
        if let Err(e) = self.storage.upsert_session(&snapshot) {
            tracing::error!("[session {}] 会话状态落盘失败: {e}", ctx.session_id);
        }
        // 会话信息持久化(轮次 / 用量 / 更新时间已刷新)→ 广播最新记录
        let _ = self
            .event_tx
            .send(session_info_event(&ctx.session_id, &snapshot));
        // 子会话：把本轮新增 token 增量累加到父会话（父的 session_usage 与 /goal 预算据此计入子 agent 开销）
        if let Some(parent_id) = parent_id {
            let delta = snapshot.session_usage.delta_from(&prev_contributed);
            if !delta.is_zero()
                && let Some(app) = crate::app::App::try_global()
            {
                app.session().add_child_usage(&parent_id, &delta);
            }
        }
    }

    /// 记录一次轮次的 token 增量（使用统计用）：会话模型/供应商取当前记录。
    fn record_usage_event(&self, session_id: &str, result: &LlmResult) {
        let usage = &result.usage;
        if usage.prompt == 0 && usage.completion == 0 && usage.total == 0 {
            return;
        }
        let (model, provider) = match self.record.lock() {
            Ok(record) => (
                record.model.clone().unwrap_or_default(),
                record.provider.clone().unwrap_or_default(),
            ),
            Err(_) => return,
        };
        let event = astorm_common::types::UsageEvent {
            session_id: session_id.to_string(),
            model,
            provider,
            ts: unix_now(),
            prompt: usage.prompt,
            completion: usage.completion,
            cache_hit: usage.cache_hit,
            cache_miss: usage.cache_miss,
            total: usage.total,
        };
        if let Err(e) = self.storage.append_usage_event(&event) {
            tracing::warn!("[session {session_id}] 用量事件落盘失败: {e}");
        }
    }
}

#[async_trait]
impl ReactLoop for Session {
    async fn before_loop(&self, ctx: &mut LoopContext) -> Result<(), String> {
        // 循环前只做提示词注入（前缀稳定，最大化前缀缓存命中）。剪支 / 摘要等
        // 「发送前准备」下沉到 loop_start：每轮 LLM 前都执行，长任务中途也能压缩。
        // ① 系统提示词：纯静态注入（前缀不变，最大化前缀缓存命中）
        self.ensure_system_prompt(ctx);
        // ② 会话环境信息：追加到最新用户消息后（每轮用户提示词携带环境变量）
        self.ensure_user_prompt(ctx);
        Ok(())
    }

    /// 每轮 LLM 推理前的发送前准备（每条用户消息的每一轮都会执行）：
    /// 动态工具同步 → 剪支 → 摘要 → 修复 tool 配对。失败仅告警，不中断循环。
    ///
    /// 顺序说明：动态工具同步放最前（`tool_load` 上一轮加载的工具，本轮请求即携带 schema）；
    /// compact 需在消息与历史仍逐条对齐时先做（前缀原地替换，见 compact）；
    /// 摘要会按自身口径重写请求载荷（丢弃临时注入与占位修复）；**配对修复放最后**，
    /// 保证最终上送载荷中 assistant(tool_calls) 与 tool 响应成对（缺失会被上游 400）。
    async fn loop_start(&self, ctx: &mut LoopContext) {
        // ⓪ 动态工具同步：按会话记录重建循环工具集（LLM 实际可用集 = 可见集 −
        //    未加载的可加载集 ∪ 已加载集 ∪ 已连接 MCP 工具，见 agent::dynamic_tools）。
        //    `tool_load` 工具上一轮写完记录，本轮这里重建 → 下一轮请求即携带新工具 schema；
        //    MCP 连接 / 断开同样在下一轮生效。
        if let Some(app) = crate::app::App::try_global() {
            let snapshot = self.record.lock().ok().map(|r| {
                (r.tool_list.clone(), r.loaded_tool_list.clone())
            });
            if let Some((tool_list, loaded)) = snapshot {
                ctx.tools = app.loop_tool_map(&tool_list, &loaded);
            }
        }
        // ① 剪支：对较旧且超长的工具参数/结果截断归档（不改 jsonl，仅内存 + 发送载荷）
        if let Err(e) = crate::agent::compact::compact_if_needed(self, ctx).await {
            tracing::warn!("[session {}] 剪支失败: {e}", ctx.session_id);
        }
        // ② 摘要阈值检测：用上一轮真实上下文 token（缺省回退字符估算）判定，超标即压缩
        self.try_summarize(ctx).await;
        // ③ tool / tool_result 必须成对：修复孤立的 assistant tool_calls
        //    （缺失 tool 响应上游会返回 400；放在摘要之后，对最终载荷生效）
        self.repair_tool_calls(ctx);
    }

    async fn after_llm(&self, ctx: &mut LoopContext, result: &LlmResult) {
        // 逐轮记录 token 增量（使用统计按天/模型聚合；失败仅告警，不影响循环）
        self.record_usage_event(&ctx.session_id, result);
        // 每轮 LLM 结束即持久化会话状态：本轮消息已由 append_message 落 jsonl，
        // 此处同点补元数据（轮次 / 累计用量 / 更新时间），中途异常也能对齐恢复
        self.persist_session_state(ctx);
    }

    fn loop_end(&self, _ctx: &mut LoopContext) {}

    /// 循环结束（仅成功路径）：推送用量统计与 done 结束信号（前端据此判定会话本轮完成）。
    /// 会话状态（轮次 / 用量 / 更新时间）已由 `after_llm` 逐轮持久化，此处不再落盘。
    async fn after_loop(&self, ctx: &mut LoopContext) -> Result<(), String> {
        // 子会话结束：强制冲刷父会话的待聚合用量（节流窗口未到也要把终值落盘/广播）
        let parent_id = self.record.lock().ok().and_then(|r| r.pid);
        if let Some(pid) = parent_id
            && let Some(app) = crate::app::App::try_global()
        {
            app.session().flush_child_usage(&pid);
        }
        let usage = &ctx.total_usage;
        let _ = self.event_tx.send(react_loop::event::session_stats(
            &ctx.session_id,
            usage.prompt,
            usage.completion,
            usage.total,
        ));
        // 交付物 / 变更清单不再经事件下发：`submit_deliverables` 工具每轮收尾时已落
        // sqlite `session_deliverables` 表，前端在会话结束后拉表渲染（见 tool::delivery）。
        // 终态事件（done / cancelled）由 run 编排器统一发送：只有它同时知道取消令牌
        // 与打断 / 排队队列状态——本轮结束后仍有后续消息续跑时不能发 done，否则前端
        // 会提前收尾流式状态（立即发送场景：被打断轮的收尾会让停止按钮提前回落）
        Ok(())
    }

    fn append_message(&self, ctx: &LoopContext, message: ChatMessage) {
        // 会话已被删除:跳过追加(内存历史与 jsonl),避免复活
        if self.is_deleted() {
            return;
        }
        // 中断请求（tool 结果为中断信封 JSON：安全拦截待审批 / 向用户提问）
        // → 附 metadata.interrupt 标记：中断态随消息流持久化（冷启动重建台账 / 前端重绘卡片）
        let message = crate::agent::history::attach_interrupt_marker(message);
        // 内存历史先推；seq 取会话内**单调递增计数器**（摘要把历史压缩后长度会回退，
        // 不能按 history.len()-1 取号，否则归档段与压缩后新消息重启回读会按 seq 交错）
        let seq = {
            let mut history = match self.history.lock() {
                Ok(history) => history,
                Err(_) => return,
            };
            history.push(message.clone());
            self.next_seq.fetch_add(1, Ordering::SeqCst)
        };
        // 追加式 JSONL 落盘（会话消息历史；转换逻辑在 history 模块）
        let record = message_to_record(&message, &ctx.session_id, seq);
        if let Err(e) = self
            .storage
            .append_messages(&ctx.session_id, std::slice::from_ref(&record))
        {
            tracing::error!("[session {}] 消息落盘失败: {e}", ctx.session_id);
        }
    }

    fn emit_event(&self, _ctx: &LoopContext, event: Value) {
        let _ = self.event_tx.send(event);
    }
}