//! 会话摘要能力：判断会话历史是否达到摘要阈值，达到时把旧轮次历史发送 LLM
//! 生成结构化摘要，并以「摘要消息 + 最近消息尾部」重写会话消息（请求消息与历史同步），
//! 使循环内下一轮推理直接站在摘要上下文中继续。
//!
//! 阈值判断参考 astorm-desktop 的摘要实现：以估算 token（字符数 / 4 的启发式，
//! 与其 `apply_summary` 中 `context_usage = sum(len)/4` 同一口径）对比阈值。
//! 触发阈值 / 最少条数 / 保留尾部的轮数与 token 预算均来自 [`AppConfig.summary`]
//! （前端 JSON 可配，经 Session 下发），默认值见 `SummaryConfig::default`。
//! 保留尾部按「最近 N 轮」与「≤ keepTokenThreshold 累计 token」两种口径取保留最多的消息。
//!
//! 摘要仅在**循环开始**触发（`loop_start`，每条用户消息的每一轮 LLM 前做阈值检测），
//! 已超标的历史在进入本轮 LLM 前压缩。摘要完成后把**压缩结果写回活动日志**：
//! 新的活动日志由「系统提示词 + 摘要消息 + 保留的消息」组成，**它即会话历史**
//! （回读只读活动日志）；替换前的活动日志留作备份段 `<session>/message_NNN.jsonl`
//! （NNN = 摘要次数），仅作备份与检索，不再参与回读/上下文。
//! 摘要仅是 Session 上的一个能力（函数），**不存在智能体实体**；react-loop / llm-provider
//! 零改动。（历史紧凑化 / 工具结果归档的 compact 能力暂不实现。）

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use llm_provider::{ChatClient, ChatMessage};
use react_loop::LoopContext;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use astorm_common::session::req::CreateSessionRequest;
use astorm_common::types::MessageRecord;

use super::context::build_loop_context;
use super::session::{session_info_event, unix_now, Session};

/// 摘要系统提示词（要求严格 JSON 输出，字段与 [`SessionSummary`] 对齐）。
const SUMMARY_SYSTEM_PROMPT: &str = r#"#工作流程
你的任务是从多轮对话历史中提取结构化摘要，帮助后续对话快速理解上下文和提取长期记忆沉淀。请生成结构化摘要，严格按以下字段内容，输出JSON格式：

- user_requirement（文本，必填）：一句话概括用户的原始需求。
- current_task（文本，必填）：描述当前对话正在执行的任务细节。
- pending_tasks（列表，可选）：对话中明确提到但未完成的事项。
- history_process（列表，可选）：按时间顺序的过程步骤，明确包含用户提出的每一个需求，决策过程，处理过程，成功和失败的经验，尽可能具体详实。
- key_decisions（列表，可选）：已做出的关键决策，包含背景，备选方案，最终选择，决策理由，约束和代价，关联信息与资料。
- resource_files（列表，可选）：涉及的关键文件路径。
- user_infos（列表，可选）：用户画像沉淀，提取用户表达习惯、主要关注领域、知识水平，沟通风格、学习方式、决策模式，提出的约束或偏好。

#注意事项
- 严格基于对话：不编造对话中不存在的信息，不确定的内容不要写入。
- 区分角色：[系统] 的内容是指令而非用户需求，不要把系统提示误识为用户意图。
- 合并概括：多个相似的同一类型操作应合并描述（如"多次搜索了相关资料"），不逐条罗列。
- 忽略控制标记：消息中的 [思考] 仅辅助理解助手行为，摘要时提炼要点而非逐字引用。
- 空值处理：可选字段如果没有对应信息，输出 null，不要输出空字符串或空数组。

严格按以下 JSON 结构输出，不要输出任何额外文字或 markdown 围栏：
{
  "user_requirement": "用户需求一句话概括",
  "current_task": "当前任务细节",
  "pending_tasks": ["未完成事项"] 或 null,
  "history_process": ["过程步骤"] 或 null,
  "key_decisions": ["关键决策"] 或 null,
  "resource_files": ["关键文件路径"] 或 null,
  "user_infos": ["用户画像"] 或 null
}

若上下文出现「上一次会话摘要」（一条 assistant 消息），请以其为基线，结合后续新对话
合并梳理，生成一份更完整的摘要；新摘要应涵盖旧摘要的全部信息与自上次以来的新进展。"#;

/// 摘要请求的用户消息。
const SUMMARY_USER_PROMPT: &str = "请根据以上对话历史，生成结构化摘要，严格输出 JSON 格式。";

/// 结构化会话摘要（LLM 输出；可选字段缺失或 null 时 serde 隐式为 None）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub user_requirement: String,
    pub current_task: String,
    pub pending_tasks: Option<Vec<String>>,
    pub history_process: Option<Vec<String>>,
    pub key_decisions: Option<Vec<String>>,
    pub resource_files: Option<Vec<String>>,
    pub user_infos: Option<Vec<String>>,
}

impl SessionSummary {
    /// 转为可读的结构化文本（注入历史时使用，供 LLM 续接理解）。
    pub fn to_summary_text(&self) -> String {
        let mut parts = vec![
            format!("**用户需求**: {}", self.user_requirement),
            format!("**当前任务**: {}", self.current_task),
        ];
        if let Some(ref v) = self.pending_tasks {
            if !v.is_empty() {
                parts.push(format!("**待处理**:\n- {}", v.join("\n- ")));
            }
        }
        if let Some(ref v) = self.history_process {
            if !v.is_empty() {
                parts.push(format!("**历史处理**:\n- {}", v.join("\n- ")));
            }
        }
        if let Some(ref v) = self.key_decisions {
            if !v.is_empty() {
                parts.push(format!("**关键决策**:\n- {}", v.join("\n- ")));
            }
        }
        if let Some(ref v) = self.resource_files {
            if !v.is_empty() {
                parts.push(format!("**资源文件**:\n- {}", v.join("\n- ")));
            }
        }
        if let Some(ref v) = self.user_infos {
            if !v.is_empty() {
                parts.push(format!("**用户画像**:\n- {}", v.join("\n- ")));
            }
        }
        parts.join("\n\n")
    }
}

/// 对完整轮次消息序列做结构化摘要。
///
/// `prev_summary` 存在时作为「上一次会话摘要」基线注入，令模型以此为基线、
/// 结合新历史合并成一份更完整的摘要（可重复累加）。初次摘要传 [`None`]。
/// 发送前会清理孤立 tool_calls（避免上游因缺少 tool 响应返回 400）；
/// 流式 / 强制 json_object / 剥离 metadata 由 `complete_json` 统一处理。
pub async fn summarize(
    client: &ChatClient,
    model: &str,
    prev_summary: Option<&str>,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
) -> Result<SessionSummary, String> {
    let sanitized = sanitize_messages(messages);

    // 摘要请求消息：系统提示 + [上一次摘要基线] + 历史 + 摘要请求
    let mut llm_messages: Vec<ChatMessage> = Vec::with_capacity(sanitized.len() + 3);
    llm_messages.push(message("system", SUMMARY_SYSTEM_PROMPT));
    if let Some(prev) = prev_summary {
        llm_messages.push(ChatMessage {
            role: "assistant".into(),
            content: Some(format!("【上一次会话摘要】\n{prev}")),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            metadata: Some(serde_json::json!({ "prev_summary": true })),
        });
    }
    llm_messages.extend(sanitized);
    llm_messages.push(message("user", SUMMARY_USER_PROMPT));

    let request = llm_provider::ChatRequest::new(llm_messages).with_model(model);
    let result = crate::util::llm_util::complete_json(client, request, cancel).await?;
    let original = result.value;
    serde_json::from_value(original.clone())
        .map_err(|e| format!("摘要解析失败: {e}，原始返回: {original}"))
}

/// 清理孤立 tool_calls：移除没有对应 tool 响应的 assistant tool_calls。
fn sanitize_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let tool_result_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.tool_call_id.clone())
        .collect();

    messages
        .into_iter()
        .filter_map(|mut msg| {
            if msg.role == "assistant" {
                if let Some(ref tcs) = msg.tool_calls {
                    let has_all_results = tcs.iter().all(|tc| tool_result_ids.contains(&tc.id));
                    if !has_all_results {
                        msg.tool_calls = None;
                    }
                }
            }
            if msg.content.is_none() && msg.tool_calls.is_none() {
                None
            } else {
                Some(msg)
            }
        })
        .collect()
}

/// 估算单条消息的 token 数（字符数 / 4，与摘要阈值判断同一启发式口径）。
fn estimate_tokens(msg: &ChatMessage) -> usize {
    let text_len = msg.content.as_deref().map(|c| c.len()).unwrap_or(0);
    let args_len: usize = msg
        .tool_calls
        .as_ref()
        .map(|tcs| {
            tcs.iter()
                .map(|tc| tc.function.arguments.len())
                .sum::<usize>()
        })
        .unwrap_or(0);
    (text_len + args_len) / 4
}

/// 阈值判断（**真实上下文 token 优先**，缺省回退字符估算）。
///
/// `real_tokens` 为上一轮 LLM 请求的真实 `prompt_tokens`（>0 时即为当前上下文占用），
/// 以其对比阈值；provider 未返回用量（=0）时回退 `estimate_tokens` 字符估算
/// （兼容不支持 usage 的供应商）。两种口径都要求历史条数 ≥ `min_messages`。
fn should_summarize(
    history: &[ChatMessage],
    threshold: usize,
    min_messages: usize,
    real_tokens: usize,
) -> bool {
    if history.len() < min_messages {
        return false;
    }
    let total_tokens = if real_tokens > 0 {
        real_tokens
    } else {
        history.iter().map(estimate_tokens).sum()
    };
    total_tokens >= threshold
}

/// 按轮数计算保留尾部：统计最近 `keep_rounds` 个 assistant 消息，
/// 保留其起点（含该 assistant 消息）往后的消息数。
///
/// 统计口径与 astorm-desktop 一致（以 assistant 消息代表"一轮"，其后跟随的
/// tool 响应归属同一轮）；`ChatMessage` 无轮次字段，故按 role 计数回溯。
/// assistant 不足 `keep_rounds` 时返回 **0**（该口径不适用），由 token 预算口径
/// 决定保留——避免"全保留"把 `split` 顶成 0 导致摘要被跳过。
pub(crate) fn tail_len_by_rounds(history: &[ChatMessage], keep_rounds: usize) -> usize {
    if keep_rounds == 0 {
        return 0;
    }
    let mut assistants = 0usize;
    for (i, msg) in history.iter().enumerate().rev() {
        if msg.role == "assistant" {
            assistants += 1;
            if assistants == keep_rounds {
                return history.len() - i;
            }
        }
    }
    0 // assistant 不足 keep_rounds：轮数口径不适用
}

/// 按 token 预算计算保留尾部：从尾部往回累积估算 token，超过 `keep_token_threshold` 截止。
fn tail_len_by_tokens(history: &[ChatMessage], keep_token_threshold: usize) -> usize {
    let mut total = 0usize;
    let mut count = 0usize;
    for msg in history.iter().rev() {
        total += estimate_tokens(msg);
        count += 1;
        if total >= keep_token_threshold {
            break;
        }
    }
    count
}

/// 计算「保留尾部」的起点（安全边界）。
///
/// 起点不能落在 `assistant(tool_calls)` 与其 `tool` 响应之间：否则被保留的尾部会以
/// `tool` 消息开头，而其 assistant 已被摘要掉——上送时上游报
/// 「Messages with role 'tool' must be a response to a preceding message with 'tool_calls'」(400)。
/// 因此从 `history.len() - keep` 往回退到非 `tool` 消息（即该组 tool 的 assistant），
/// 把整组「assistant + tool 响应」留在尾部；此时前缀也不会以缺响应的 assistant 结尾。
fn safe_split(history: &[ChatMessage], keep: usize) -> usize {
    let mut split = history.len().saturating_sub(keep);
    while split > 0 && history.get(split).map(|m| m.role == "tool").unwrap_or(false) {
        split -= 1;
    }
    split
}

/// 摘要主入口（由 `ReactLoop::before_loop` 在循环开始调用，每条用户请求至多触发一次）：
/// 阈值达标 → LLM 结构化摘要 → 以「system（若有）+ 摘要消息 + 保留尾部」重写
/// 请求消息与会话历史 → 压缩结果写回活动日志（原日志留作 `<session>/message_NNN.jsonl`）→ 摘要计数回写存储。
///
/// 保留尾部取轮数 / token 预算两种口径的**更多者**（少丢新近上下文）。
///
/// `force=true`（`/compact` 手动触发）跳过阈值判断，直接摘要；仍受「可拆分点 > 0」约束
/// （历史太短 / 全是需保留的尾部时返回 `false`，不做无意义摘要）。
///
/// 返回 `true` 表示本次完成了摘要（上下文已压缩）；未达标返回 `false`，失败上抛由调用方策略处理。
pub async fn summarize_if_needed(
    session: &Session,
    ctx: &mut LoopContext,
    force: bool,
) -> Result<bool, String> {
    // ① 阈值判断 + 拆出待摘要前缀、新近尾部与 system 消息（历史锁内完成，锁外执行 LLM）
    //    尾部保留口径：轮数（最近 N 个 assistant 消息及其后）与 token 预算
    //    （累计估算 token ≤ keep_token_threshold），取保留消息更多的口径，少丢新近上下文
    let cfg = session
        .summary
        .read()
        .map_err(|_| "会话摘要配置锁中毒".to_string())?
        .clone();
    let (system_msg, prefix, tail, injected) = {
        let history = session
            .history
            .lock()
            .map_err(|_| "会话历史锁中毒".to_string())?;
        // 真实上下文 token（上一轮请求的 prompt_tokens）优先；为 0 时回退字符估算。
        let real_tokens = ctx.last_prompt_tokens as usize;
        if force {
            // 手动 /compact：上下文占用低于阈值 **50%** 视为太短，直接拒绝（避免无意义摘要）
            if !should_summarize(&history, cfg.token_threshold / 2, cfg.min_messages, real_tokens) {
                return Err("当前上下文较短，无需生成摘要".into());
            }
        } else if !should_summarize(
            &history,
            cfg.token_threshold,
            cfg.min_messages,
            real_tokens,
        ) {
            return Ok(false);
        }
        let keep = tail_len_by_rounds(&history, cfg.keep_tail_rounds)
            .max(tail_len_by_tokens(&history, cfg.keep_token_threshold));
        // 安全边界：起点不得落在 assistant(tool_calls) 与其 tool 响应之间（否则上送 400）
        let split = safe_split(&history, keep);
        if split == 0 {
            return Ok(false);
        }
        // 工具注入消息（read_images 图片等，只进 chat_request、不进会话历史；
        // 引擎已标 metadata.ephemeral）：摘要重写会丢这些消息，先取出，重写后追加回请求载荷
        let injected: Vec<ChatMessage> = ctx
            .chat_request
            .messages
            .iter()
            .filter(|m| is_ephemeral_msg(m))
            .cloned()
            .collect();
        (
            // 保留 system prompt（before_loop 注入），重写后仍放头部
            history.iter().find(|m| m.role == "system").cloned(),
            history[..split].to_vec(),
            history[split..].to_vec(),
            injected,
        )
    };

    // ② LLM 结构化摘要（模型沿用本轮请求；取消令牌级联会话生命周期）。
    //    多供应商：客户端优先取本轮上下文路由的（request.provider），缺省回退会话默认供应商；
    //    均缺失（理论上 create/send 已校验有供应商）时跳过本次摘要。
    //
    //    累加基线：读最近一次摘要快照（summary_*.md）作为「上一次摘要」，令模型与其合并；
    //    同时剔除待摘要历史中的旧摘要消息（其信息已由快照代表，避免重复喂入）。
    let dir = session_data_dir(&ctx.session_id);
    let prev = latest_summary(&dir).map(|(_, text)| text);
    let prefix: Vec<ChatMessage> = prefix.into_iter().filter(|m| !is_summary_msg(m)).collect();

    let model = ctx.chat_request.model.clone();
    let cancel = session
        .cancel
        .read()
        .map_err(|_| "会话取消锁中毒".to_string())?
        .child_token();
    let client = ctx.client.clone().or_else(|| session.llm.default_client());
    let Some(client) = client else {
        return Ok(false);
    };
    // 广播「摘要压缩中」：自动摘要发生在 loop_start（两次 LLM 请求之间），界面上原本无感——
    // 前端据此在过程末尾状态行显示流光「正在摘要压缩...」；完成后补 `summary_done`
    // 让状态回落到「等待模型回复」（随后的正文/思考事件也会自然覆盖）。
    let _ = session.event_tx.send(serde_json::json!({
        "type": "summarizing",
        "session_id": ctx.session_id,
    }));
    let title_cancel = cancel.clone();
    let summary = summarize(&client, &model, prev.as_deref(), prefix, cancel).await?;
    // 摘要后按「当前任务」重生成中英文标题：摘要代表会话的最新状态，旧标题可能已偏离。
    // 失败只告警，保留旧标题，不阻断摘要。
    let title_source = format!(
        "用户需求：{}\n当前任务：{}",
        summary.user_requirement, summary.current_task
    );
    let new_titles =
        match crate::agent::title::generate_titles(&client, &model, &title_source, title_cancel)
            .await
        {
            Ok(titles) => Some(titles),
            Err(e) => {
                tracing::warn!("[session {}] 摘要后标题生成失败: {e}", ctx.session_id);
                None
            }
        };
    let _ = session.event_tx.send(serde_json::json!({
        "type": "summary_done",
        "session_id": ctx.session_id,
    }));

    // ③ 注入：system（若有）+ 摘要消息（assistant，metadata 标记）+ 新近尾部，
    //    同时重写请求消息（循环内下一轮真正发给 LLM 的消息）与会话历史。
    //    尾部同样剔除旧摘要消息，保证历史中只保留最新一条摘要 assistant。
    let mut new_history: Vec<ChatMessage> = Vec::with_capacity(tail.len() + 2);
    if let Some(sys) = system_msg {
        new_history.push(sys);
    }
    let tail: Vec<ChatMessage> = tail.into_iter().filter(|m| !is_summary_msg(m)).collect();
    let tail_len = tail.len();
    new_history.push(summary_message(&summary.to_summary_text()));
    new_history.extend(tail);
    *session
        .history
        .lock()
        .map_err(|_| "会话历史锁中毒".to_string())? = new_history.clone();
    // 请求载荷 = 压缩后历史 + 工具注入消息（图片等仅本轮循环可见，不进会话历史）
    ctx.chat_request.messages = {
        let mut payload = new_history.clone();
        payload.extend(injected);
        payload
    };
    // ③′ 压缩后旧的上下文计数已失效：置 0 让下次判定回退为「压缩后历史的字符估算」，
    //     而不是继续按压缩前的真实峰值反复触发摘要（同轮 after_llm 会用真实 prompt_tokens 回填）。
    ctx.last_prompt_tokens = 0;

    // ④ 落盘：**新的活动日志就是会话历史**——由「系统提示词 + 摘要 + 保留消息」组成，
    //    替换前的活动日志留作备份段 `<session>/message_NNN.jsonl`（NNN=摘要次数，仅备份/检索，
    //    回读不再参与）；另写摘要快照 summary_NNN.md（人读 + 下次摘要基线）+
    //    摘要计数 / 上下文计数回写记录并广播（会话信息卡即时刷新）。
    let snapshot = {
        let mut record = session
            .record
            .lock()
            .map_err(|_| "会话记录锁中毒".to_string())?;
        record.summary_times += 1;
        record.context_tokens = 0;
        record.updated_at = unix_now();
        // 摘要后替换会话标题：title 保留中文兼容旧消费者，title_zh/title_en 供前端跟随语言
        if let Some(titles) = &new_titles {
            record.title = Some(titles.zh.clone());
            record.title_zh = Some(titles.zh.clone());
            record.title_en = Some(titles.en.clone());
        }
        record.clone()
    };
    let seq = snapshot.summary_times as u32;
    // 摘要快照尽力而为：失败仅告警，不阻断摘要（上下文压缩已生效，快照可由下次重试补写）
    if let Err(e) = save_summary(&dir, seq, &summary.to_summary_text()) {
        tracing::warn!("[session {}] 摘要快照写入失败: {e}", ctx.session_id);
    }
    // seq 从会话内单调递增计数器**连续分配**：回读按 seq 排序即得
    // 系统提示词 → 摘要 → 尾部 的顺序，且后续新消息续写其后（不随历史压缩回退）。
    let base = session.next_seq.load(Ordering::SeqCst);
    let records = compressed_records(&new_history, &ctx.session_id, base);
    // 落盘失败仅告警：压缩仍在内存生效，重启后回到压缩前日志（下次会再摘要，自愈）
    match session
        .storage
        .rewrite_live_messages(&ctx.session_id, seq, &records)
    {
        Ok(()) => session
            .next_seq
            .store(base + records.len() as i64, Ordering::SeqCst),
        Err(e) => tracing::warn!(
            "[session {}] 压缩历史落盘失败（本次仅在内存生效，重启会回到压缩前日志）: {e}",
            ctx.session_id
        ),
    }
    session
        .storage
        .upsert_session(&snapshot)
        .map_err(|e| format!("摘要计数回写失败: {e}"))?;
    // 会话信息即时广播：前端据此刷新摘要次数 / 上下文容量，并重载该会话消息（旧对话不再展示）
    let _ = session
        .event_tx
        .send(session_info_event(&ctx.session_id, &snapshot));
    // 摘要后的新标题单独广播一次（title 事件供前端即时替换，无需等下次列表刷新）
    if let Some(titles) = &new_titles {
        let _ = session
            .event_tx
            .send(crate::agent::title::title_event(&ctx.session_id, titles));
    }

    tracing::info!(
        "[session {}] 会话摘要完成: 前缀 {} 条 → 活动日志「系统提示词 + 摘要 + 尾部 {} 条」, summary_times={}, 快照={}",
        ctx.session_id,
        snapshot.summary_times - 1,
        tail_len,
        snapshot.summary_times,
        format!("summary_{seq:03}.md"),
    );
    Ok(true)
}

/// 手动触发摘要（`/compact`）：由会话记录构建一个脱离循环的上下文，强制摘要一次。
///
/// `Ok(true)` = 完成摘要；`Ok(false)` = 无可摘要内容（历史太短 / 全是保留尾部 / 无 client）。
/// 调用方负责与运行中的循环互斥（见 `SessionImpl::summarize_session`）。
pub(crate) async fn summarize_now(session: &Arc<Session>, force: bool) -> Result<bool, String> {
    if session.llm.is_empty() {
        return Err("未配置可用的模型供应商，无法生成摘要".into());
    }
    let record = session
        .record
        .lock()
        .map_err(|_| "会话记录锁中毒".to_string())?
        .clone();
    let history = session
        .history
        .lock()
        .map_err(|_| "会话历史锁中毒".to_string())?
        .clone();
    // 空请求：仅借道 build_loop_context 解析 provider / client / model 与续接 context_tokens
    let request = CreateSessionRequest {
        model: record.model.clone().unwrap_or_default(),
        provider: record.provider.clone(),
        pid: None,
        agent_type: None,
        session_type: None,
        scheduled_task_id: None,
        workspace: record.workspace.clone(),
        session_mode: None,
        permission_mode: None,
        reasoning_effort: None,
        user_message: String::new(),
        attachments: None,
        auto_run: None,
        mentions: None,
        elements: None,
        office_refs: None,
        quotes: None,
        goal: None,
        goal_lang: None,
    };
    let mut ctx = build_loop_context(session, &request, history)
        .ok_or_else(|| "构建摘要上下文失败".to_string())?;
    summarize_if_needed(session, &mut ctx, force).await
}

/// 构造摘要消息（assistant + `metadata.summary`）：既进内存历史，也随新活动日志落盘，
/// 使冷启动回读到与压缩时一致的上下文（`is_summary_msg` 据此识别）。
pub(crate) fn summary_message(text: &str) -> ChatMessage {
    ChatMessage {
        role: "assistant".into(),
        content: Some(text.to_string()),
        content_parts: None,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        metadata: Some(serde_json::json!({ "summary": true })),
    }
}

/// 压缩后的历史 → 待落盘记录：`seq` 从 `base` **连续分配**，使回读按 `seq` 排序
/// 即得「系统提示词 → 摘要 → 保留消息」的顺序（并据条数推进会话计数器）。
fn compressed_records(history: &[ChatMessage], session_id: &str, base: i64) -> Vec<MessageRecord> {
    history
        .iter()
        .enumerate()
        .map(|(i, m)| crate::agent::history::message_to_record(m, session_id, base + i as i64))
        .collect()
}

/// 构造基础 ChatMessage（无工具字段 / 无元数据）。
fn message(role: &str, content: impl Into<String>) -> ChatMessage {
    ChatMessage {
        role: role.into(),
        content: Some(content.into()),
        content_parts: None,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        metadata: None,
    }
}

/// 判断是否为会话摘要消息（由摘要注入，metadata.summary == true）。
fn is_summary_msg(msg: &ChatMessage) -> bool {
    msg.role == "assistant"
        && msg
            .metadata
            .as_ref()
            .and_then(|m| m.get("summary").and_then(|v| v.as_bool()))
            .unwrap_or(false)
}

/// 判断是否为工具注入的临时消息（`metadata.ephemeral == true`）：
/// 仅进本轮请求载荷、不进会话历史；摘要重写请求载荷时据此保留。
pub(crate) fn is_ephemeral_msg(msg: &ChatMessage) -> bool {
    msg.metadata
        .as_ref()
        .and_then(|m| m.get("ephemeral").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// 会话消息目录（纯 fs 逻辑）：`<base>/session/<session_id>`。
fn summary_dir(base: &Path, session_id: &str) -> PathBuf {
    base.join("session").join(session_id)
}

/// 当前会话的消息目录（摘要快照 / 剪支归档共用）：
/// 优先 `App::try_global()` 的应用数据目录，回退平台数据根。
pub(crate) fn session_data_dir(session_id: &str) -> PathBuf {
    let base = crate::app::App::try_global()
        .map(|app| app.dir().data_dir)
        .unwrap_or_else(crate::app::user_data_dir);
    summary_dir(&base, session_id)
}

/// 写入一份摘要快照：`<dir>/summary_{seq:03}.md`（seq 从 1 起，与 summary_times 对齐）。
fn save_summary(dir: &Path, seq: u32, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建摘要目录失败: {e}"))?;
    let path = dir.join(format!("summary_{seq:03}.md"));
    std::fs::write(&path, text).map_err(|e| format!("写摘要文件失败: {e}"))
}

/// 读取历史摘要快照中**最新一份**（最大 seq 且可读），返回 `(seq, 文本)`。
/// 无文件 / 全部损坏 / 解析失败时返回 [`None`]（当作无基线，按初次摘要处理）。
fn latest_summary(dir: &Path) -> Option<(u32, String)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(u32, String)> = None;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        let Some(seq_str) = name
            .strip_prefix("summary_")
            .and_then(|n| n.strip_suffix(".md"))
        else {
            continue;
        };
        let Ok(seq) = seq_str.parse::<u32>() else {
            continue;
        };
        if best.as_ref().map(|(b, _)| seq <= *b).unwrap_or(false) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        best = Some((seq, content));
    }
    best
}

/// 清理会话摘要目录（删除会话时调用，尽力而为）：`<data_dir>/session/<session_id>`。
pub(crate) fn cleanup_summary_dir(session_id: &str) {
    let dir = session_data_dir(session_id);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        if !is_not_found(&e) {
            tracing::warn!("[session {session_id}] 清除摘要目录失败: {e}");
        }
    }
}

/// 判断文件系统错误是否为「路径不存在」（目录本就无摘要文件时静默忽略）。
fn is_not_found(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("astorm_summary_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_and_latest_summary_roundtrip() {
        let dir = tmp_dir("roundtrip");
        assert!(save_summary(&dir, 1, "第一版").is_ok());
        assert!(save_summary(&dir, 2, "第二版").is_ok());

        let (seq, text) = latest_summary(&dir).expect("应有最新摘要");
        assert_eq!(seq, 2);
        assert_eq!(text, "第二版");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_summary_none_when_empty() {
        let dir = tmp_dir("empty");
        assert!(latest_summary(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_summary_ignores_invalid_and_skips_older() {
        let dir = tmp_dir("invalid");
        assert!(save_summary(&dir, 1, "旧版").is_ok());
        // 无关文件与会话摘要文件同目录，应被跳过
        std::fs::write(dir.join("readme.md"), "hi").unwrap();
        let (seq, text) = latest_summary(&dir).expect("应有最新摘要");
        assert_eq!(seq, 1);
        assert_eq!(text, "旧版");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 压缩结果落盘：seq 从 base 连续分配，回读按 seq 排序即得
    /// 「系统提示词 → 摘要 → 保留消息」；摘要消息带 metadata.summary（回读可识别）。
    #[test]
    fn compressed_records_assign_contiguous_seq_in_order() {
        let history = vec![
            ChatMessage::system("sys"),
            summary_message("结构化摘要"),
            ChatMessage::user("最近一句"),
        ];
        let records = compressed_records(&history, "sess-1", 42);
        assert_eq!(records.len(), 3);
        assert_eq!(
            records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![42, 43, 44],
            "seq 必须连续且按数组顺序"
        );
        assert_eq!(records[0].role, "system");
        assert_eq!(records[1].content, "结构化摘要");
        assert_eq!(
            records[1].metadata.as_ref().unwrap()["summary"],
            serde_json::json!(true),
            "摘要消息须带 metadata.summary"
        );
        assert_eq!(records[2].role, "user");
        // 记录 id 唯一（回读按 id 去重，重复 id 会丢消息）
        let mut ids: Vec<_> = records.iter().map(|r| r.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    /// 阈值判定：真实 token（上一轮 prompt_tokens）优先；为 0 时回退字符估算；
    /// 条数不足时任何口径都不触发。
    #[test]
    fn should_summarize_prefers_real_tokens() {
        let history: Vec<ChatMessage> = (0..8)
            .map(|i| ChatMessage::user(format!("m{i}")))
            .collect();
        // 字符估算仅 16/4=4 token，但真实上下文 token 超标 → 触发
        assert!(should_summarize(&history, 100, 8, 200));
        // 真实 token 未达标 → 不触发
        assert!(!should_summarize(&history, 100, 8, 50));
        // 无真实 token（0）→ 回退字符估算（远低于阈值）→ 不触发
        assert!(!should_summarize(&history, 100, 8, 0));
        // 条数不足：即使真实 token 超标也不触发
        assert!(!should_summarize(&history[..2], 100, 8, 200));
    }

    /// 摘要切分点不得落在 assistant(tool_calls) 与其 tool 响应之间
    /// （否则尾部以孤立 tool 开头，上送 400）。
    #[test]
    fn safe_split_never_breaks_tool_group() {
        let a1 = ChatMessage::assistant("")
            .with_tool_calls(vec![llm_provider::ToolCall::new("c1", "t", "{}")]);
        let t1 = ChatMessage::tool("r1", "c1");
        let t2 = ChatMessage::tool("r2", "c2");
        let a2 = ChatMessage::assistant("")
            .with_tool_calls(vec![llm_provider::ToolCall::new("c3", "t", "{}")]);
        let t3 = ChatMessage::tool("r3", "c3");
        let history = vec![ChatMessage::system("sys"), a1, t1, t2, a2, t3];

        // 起点落在 tool 上 → 回退到该组 tool 的 assistant
        assert_eq!(safe_split(&history, 1), 4);
        assert_eq!(safe_split(&history, 2), 4);
        assert_eq!(safe_split(&history, 3), 1);
        assert_eq!(safe_split(&history, 4), 1);
        assert_eq!(safe_split(&history, 5), 1);
        // 起点已在安全边界（assistant / system）→ 保持不变
        assert_eq!(safe_split(&history, 6), 0);
    }
}