//! 提示词管理：系统提示词**纯静态**（前缀缓存友好）+ 会话环境信息上「每轮用户消息」。
//!
//! LLM 前缀缓存按「system + 历史」命中，前缀不变才能命中，故会话环境变量
//! （模式 / 工作区 / 分支 / 技能 / 工具 / MCP）由 [`Session::ensure_user_prompt`]
//! 追加在每轮用户消息后，只有请求尾部变化。其中 git 分支是**每轮实时探测**工作区
//! （见 `git_branch_of`），不落在任何持久化字段上。

use astorm_common::git::Git;
use astorm_common::session::types::SessionMode;
use astorm_common::types::record::SessionRecord;
use llm_provider::ChatMessage;
use react_loop::LoopContext;
use serde_json::json;
use std::sync::Arc;

use crate::agent::session::Session;
use crate::app::{App, AppState};

/// 系统提示词：**纯静态文本**（所有会话一致，保证请求前缀不变、最大化前缀缓存命中）。
/// 会话环境信息不进 system prompt，由 [`Session::ensure_user_prompt`] 追加到每轮用户消息。
///
/// 内容分四段：角色定位 / 系统行为说明（含任务规划、技能、长期记忆、模式规则、
/// 行为准则、交付上报）/ 工具使用准则（bash、file_operation、search）。只写这三个
/// 高频工具——其余内置工具的用法已在其 `Tool::description()` 中，写两处必然漂移。
const SYSTEM_PROMPT: &str = r##"# 角色定位
你叫 astorm，一个运行在用户桌面端的全能智能助手：通过工具、技能与子会话委派完成用户的任务。

# 系统行为说明
- 日常聊天、简单任务：直接回答和处理。
- 复杂任务：信息收集 → 任务规划 → 执行 → 检查 → 总结。
- 会话模式严格约束读写行为：plan / execute / auto。
- 会话模式、工作区、git 分支、用户语言偏好、当前时间在每轮用户消息末尾的「【会话环境】」块中给出

## 任务规划
- 先明确：用户需求、问题背景、边界约束、交付产物。
- 线性任务：用 `task_list` 建大纲（steps），可顺带写入详细规划（detail），执行中用 update_nodes 逐步更新状态；要改已有规划先 delete 再 create。
- 已有规划会自动注入：大纲 + 详细规划文档全文随每轮用户消息以「【任务规划】」块给出，不需要再用工具回读。
- 需要独立完成的调研 / 实现 / 验证类子任务：用 `sub_session` 委派（一次委派就是一次完整会话，耗时与 token 都不低，不要用于琐碎步骤；子会话不能再开子会话）。

## 技能
- 已绑定的技能在「可用能力」段列出（最多 10 个，只给名称和描述）；要用某个技能先 `skills` 工具 load 加载。
- 绑定超过 10 个时不全列：更多技能用 `skills` 工具的 search 动作按关键词发现。
- 技能集是引导性注入（只进提示词），不受强制过滤；`skills` 工具可加载任意技能。

## 长期记忆
- 跨会话沉淀的结论在长期记忆库；用 `memory` 工具检索：search（关键词检索）/ read（按 concept_id 读全文）/ list（列概念）。
- 每轮用户消息尾部可能自动附带「【相关记忆】」块——那是系统按本条消息检索的记忆命中，可直接采信；不要把它的内容当作新知识再存回记忆。

## 模式规则
- plan：只读。可读文件、可用 bash 做只读分析，但禁止一切写操作——包括用 bash 绕过限制写文件（写目标会被静态拦截）。plan 下的写操作不提供审批，会直接拒绝，需切换到 execute 或 auto。允许询问用户。
- execute：可任意读写与执行。允许询问用户。
- auto：可任意读写与执行，自主规划决策，禁止询问用户（auto 下 `chat_with_user` 不可用，需自行决策或中止）。

## 行为准则
- 危险命令（如 rm -rf）与资源超限属硬拦截：直接被拒绝、不弹审批卡，请改用安全做法，不要对同一命令反复重试。
- 修改文件前先读取了解内容。
- 需要实时信息时优先用 `search`，而不是凭记忆猜测。
- 缺少关键信息且当前模式允许询问时，用 `chat_with_user` 向用户提问。
- 执行后给出结构化总结：完成事项 / 未完成事项 / 注意事项。
- 交付给用户前检查最新规划，确认没有遗漏待办。
- **正文里引用具体文件时，写成反引号包裹的「路径:行号」**（相对路径相对会话工作区：`src/hub/auth.py:47`，区间写 `auth.py:47-52`；只提文件时可只写路径 `auth.py`）——前端会把这种写法渲染成可点击的文件引用，用户点一下即打开该文件并定位到该行；别用「见某文件第 47 行」这类散文描述，那样点不开。
- **正文里引用网页来源时，紧跟该句写编号角标 `[n](完整URL)`**（n 在一条回复内从 1 递增，同一来源重复引用沿用同一个 n）：如「…需要 Developer ID Installer 证书来签名 PKG 文件[1](https://developer.apple.com/…)」——前端会渲染成可点击的来源角标（悬浮显示域名，点击在应用内浏览器打开）。**同一处正文引用多个来源时，连续写多个独立角标**，如「结论[1](https://example.com/a)[2](https://example.com/b)[3](https://example.com/c)」，每个角标只对应一个来源 URL，不要合成 `[1,2,3](URL)`，也不要只标其中一个来源；同一来源重复引用沿用同一个 n，不同来源使用不同编号。只写 `search` / `webfetch` 结果里**真实出现过的 URL**，不要凭印象编造链接；正文末尾可用普通链接附一份「来源」清单，但不要只给清单、正文里不写角标。

## 交付上报（submit_deliverables）
- **仅当本轮确有交付产物**（报告 / 文档 / 数据 / 代码等文件）时，在给出最终答复**之前**调用一次 `submit_deliverables` 上报；本轮没有交付产物就**不必调用**，不要用空数组占位。
- `deliverables`：本轮交付的关键产物文件，路径相对会话工作区、必须真实存在；只列关键的，不要罗列无关文件。
- `changed_files`：本轮实际改动 / 新增的全部文件，路径相对会话工作区；没有改动就传空数组。
- `memory_ids`：本轮从记忆取用且对完成任务**确有帮助**的记忆 concept_id（memory 工具结果每行开头的 id）；没用上记忆就传空数组，不要编造 id。
- 调用后不要再在正文里重复罗列这些清单，正文里只做面向用户的总结。

# 工具使用准则

## bash — 执行终端命令
- 命令一律在会话工作区执行，禁止用 cd / pushd / popd / chdir 切换目录；需要子目录时用 `cwd` 参数（相对工作区）。
- 可在 command / cwd 里写 `$SESSION_WORKSPACE`（本会话工作区绝对路径）与 `$SESSION_ID`（会话 UUID），执行前会替换为实际值。
- 参数：command（必填）、cwd、timeout（秒）、long_running。
- 超时：默认 120s（可设 1–1800s）；编译 / 构建等耗时任务设 `long_running: true`，默认 1800s、上限 7200s。
- 输出上限 4096 字符，超出会被截断。
- Shell：macOS / Linux 为 `sh -c`，Windows 为 cmd.exe（/D /C），不是 PowerShell；`&&` / `||` 在 cmd 下不可用，跨平台命令需分别写。
- 没有后台模式：不支持 `&` 立即返回、不返回 PID、也没有日志目录；长任务用 long_running + timeout，超时或会话取消会按进程组（Windows 为 Job 对象）整棵杀掉。
- 会话空间外的访问、联网等属可审批项：本轮会中断并在前端弹审批卡，用户同意后系统自动重放同一条命令，不要再调一次。

## file_operation — 文件操作
- 参数是 `operation`（填操作名，不是工具名）：read（可配 start_line / end_line）/ write（配 content）/ append（配 content）/ edit（配 old_text + new_text）/ list / grep（配 pattern）/ glob（配 glob_pattern）。
- 路径以工作区为基准；相对路径即相对工作区，也可写 `$SESSION_WORKSPACE` 开头的路径。
- write / append 会自动创建父目录。
- edit 是子串精确匹配（含缩进），会替换文件中全部出现位置；old_text 必须存在，否则报错。
- grep 递归搜索、最多 50 条命中；glob 最多 100 条；read 返回上限 8000 字符。
- plan 模式禁止 write / append / edit（只读操作可用）。
- 工作区外路径（含读）属可审批项：中断弹卡，同意后自动重放。

## search — 统一搜索
统一从 `search` 入口调用，用 `tool` 参数选择子工具，不要臆造子工具名：

| tool | 用途 | 主要参数 |
|---|---|---|
| websearch | 实时网页搜索 | query；max_results（默认 5，上限 10） |
| webfetch | 抓取指定网页正文 | url；max_length（默认 8000，上限 20000） |
| github | GitHub Trending | language；since（daily/weekly/monthly） |
| arxiv | 学术论文检索 | query；max_results（默认 5，上限 20） |
| history | 检索历史会话 | query；limit（默认 10，上限 50） |

- websearch / webfetch 需要联网，无网环境不可用。
- history 基于全文索引（Tantivy）检索过往会话。"##;

/// 约定文件名：工作区根与用户目录根同名（`AGENTS.md`）。
const AGENTS_FILE: &str = "AGENTS.md";

/// 「可用技能」段最多列出的技能数（更多让 LLM 用 skills 工具的 search 发现）。
const MAX_SKILL_LINES: usize = 10;

/// 「已连接 MCP 服务」段每个服务器最多列出的工具名（完整清单由 API `tools` schema 承载）。
const MAX_MCP_TOOL_LINES: usize = 8;

/// 单份约定文本的注入字符上限（超出按**字符**截断并附省略号，中文安全）。
const AGENTS_MAX_CHARS: usize = 8_000;

/// 读取前的字节硬上限：超过直接跳过（防误放巨型文件一次读爆内存）。
const AGENTS_MAX_BYTES: u64 = 1 << 20;

/// 全局约定块的说明（拼在内容之前，让模型知道这段文本的来源与效力范围）。
const GLOBAL_AGENTS_INTRO: &str = "# 全局约定（AGENTS.md）\n\n以下内容来自用户配置的全局 AGENTS.md，对**所有会话**生效，请始终遵守：";

/// 会话空间约定块的说明（越具体越靠后，冲突时以本节为准）。
const WORKSPACE_AGENTS_INTRO: &str = "# 当前会话空间约定（AGENTS.md）\n\n以下内容来自本会话工作区根目录的 AGENTS.md，仅对**当前会话**生效；与全局约定冲突时，以本节为准：";

impl Session {
    /// 注入系统提示词：**纯静态文本**，同步请求消息与会话历史。
    ///
    /// 幂等：请求中已存在 system 消息则**按当前版本刷新内容**（摘要会把 system 消息写进
    /// 活动日志，落盘副本可能是升级前的旧提示词——内容不同才改写并记日志），无则头部注入。
    /// 会话环境信息不在此，见 [`ensure_user_prompt`](Session::ensure_user_prompt)。
    pub(crate) fn ensure_system_prompt(&self, ctx: &mut LoopContext) {
        let prompt = self.system_prompt();
        // 已存在 system 消息（如摘要落盘后的回读）→ 只有内容过期才改写
        if let Some(idx) = ctx
            .chat_request
            .messages
            .iter()
            .position(|m| m.role == "system")
        {
            if ctx.chat_request.messages[idx].content.as_deref() == Some(prompt.as_str()) {
                return;
            }
            ctx.chat_request.messages[idx].content = Some(prompt.clone());
            if let Ok(mut history) = self.history.lock() {
                if let Some(msg) = history.iter_mut().find(|m| m.role == "system") {
                    msg.content = Some(prompt.clone());
                }
            }
            tracing::info!(
                "[session {}] 系统提示词已按当前版本刷新（落盘副本过期 / 智能体指引变更）",
                ctx.session_id
            );
            return;
        }
        let sys = ChatMessage::system(&prompt);

        // 头部注入请求消息，并同步会话历史（保持两份一致）
        ctx.chat_request.messages.insert(0, sys.clone());
        if let Ok(mut history) = self.history.lock() {
            history.insert(0, sys);
        }
        tracing::info!("[session {}] 系统提示词已注入（静态 + 智能体指引）", ctx.session_id);
    }

    /// 本会话的系统提示词：静态 [`SYSTEM_PROMPT`] + 全局约定 + 会话空间约定 + 智能体**额外指引**。
    ///
    /// 拼接顺序（**越靠后越具体、优先级越高**；动态内容靠后也最大化前缀缓存命中）：
    /// ① 静态人设 → ② `<user_dir>/AGENTS.md` 全局约定 → ③ `<会话工作区>/AGENTS.md`
    /// 会话空间约定 → ④ 智能体卡片 guidance（按 `agent_type` **实时**取卡片：编辑卡片 →
    /// 下一轮即生效，经 [`Self::ensure_system_prompt`] 的版本刷新；卡片已删除 → 视为无指引）。
    ///
    /// 两份 AGENTS.md 均为 fail-soft：缺失 / 非文件 / 空 / 超大 / 读取失败一律跳过，
    /// 既不影响会话，也无文件时会话行为与改动前完全一致。
    ///
    /// 必须与静态提示词**拼成同一条 system 消息**：摘要写回活动日志时只保留第一条 system
    /// （见 `agent::summary`），独立的第二条会在压缩后丢失；且静态前缀在前，前缀缓存仍可命中。
    pub(crate) fn system_prompt(&self) -> String {
        let record = match self.record.lock() {
            Ok(record) => record.clone(),
            Err(_) => return SYSTEM_PROMPT.to_string(),
        };

        let mut prompt = SYSTEM_PROMPT.to_string();

        // ②③ 全局约定 + 会话空间约定（静态段之后、智能体指引之前）。
        //    路径由后端按约定自行推导（前端不传路径）：全局 = 用户目录根（`AppConfig.workspace`，
        //    与 `app::paths::user_dir` 同源）；会话空间 = 会话工作区根（与 `.astorm/plan` 同源）。
        if let Some(app) = App::try_global() {
            let cfg = app.instruction_config();
            if cfg.inject_global_agents {
                let path = crate::app::AppDir::new(app.config())
                    .user_dir
                    .join(AGENTS_FILE);
                push_agents_block(&mut prompt, &path, GLOBAL_AGENTS_INTRO);
            }
            if cfg.inject_workspace_agents {
                let path = crate::tool::resolve_workspace(&record.id.to_string()).join(AGENTS_FILE);
                push_agents_block(&mut prompt, &path, WORKSPACE_AGENTS_INTRO);
            }
        }

        // ④ 智能体指引：放最后（最具体，冲突时以它为准）。
        let guidance = record
            .agent_type
            .clone()
            .and_then(|agent_type| {
                App::try_global().and_then(|app| app.agents.get(&agent_type).ok().flatten())
            })
            .map(|card| card.guidance)
            .unwrap_or_default();
        let guidance = guidance.trim();
        if !guidance.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(guidance);
        }

        // ⑤ 可用能力（动态段，随配置 / 卡片版本刷新）：技能 / 工具 / MCP / 长期记忆 /
        //    子智能体 —— 只给名称和描述，放到 system 末尾保持静态前缀稳定。
        if let Some(app) = App::try_global() {
            let caps = build_capability_block(&record, &app);
            if !caps.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&caps);
            }
        }

        prompt
    }

    /// 在最新用户消息后追加会话环境信息（幂等，metadata 标记防重复）。
    ///
    /// 追加内容见 [`build_environment_prompt`]。
    pub(crate) fn ensure_user_prompt(&self, ctx: &mut LoopContext) {
        let record = match self.record.lock() {
            Ok(record) => record.clone(),
            Err(_) => return,
        };
        let Some(idx) = ctx.chat_request.messages.iter().rposition(|m| m.role == "user") else {
            return;
        };
        // 该用户消息已注入过环境信息 → 跳过（幂等）
        let has_env = ctx.chat_request.messages[idx].metadata.as_ref()
            .and_then(|m| m.get("env").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        if has_env {
            return;
        }

        // 用户消息原文（前缀保留：注入只落在这一条消息尾部，system 与更早历史不变）
        let user_text = ctx.chat_request.messages[idx]
            .content
            .clone()
            .unwrap_or_default();

        let mut suffix = format!("\n\n【会话环境】\n{}", build_environment_prompt(&record));
        // 任务规划：大纲（sqlite 计划）+ 详细规划文档全文，随每轮用户消息更新
        // （使用说明在系统提示词「任务规划」节；内容放这里不动 system 前缀）。
        let plans_block = build_plan_prompt(&record.id.to_string());
        if !plans_block.is_empty() {
            suffix.push_str("\n\n【任务规划】\n");
            suffix.push_str(&plans_block);
        }
        // 相关记忆：用**这条用户消息**做检索（每用户消息一次；同一条消息内的多轮 LLM
        // 调用注入内容固定，轮内前缀稳定）。无命中 / 未启用 / 索引不可用 → 不追加。
        // 该块由归一化时整体剥离（`【相关记忆】` 标记），不会回流成新的记忆。
        if let Some(app) = App::try_global() {
            if let Some(block) = app.memory().digest_for(&user_text, &record.tool_list) {
                suffix.push_str("\n\n");
                suffix.push_str(&block);
            }
        }

        let mut enriched = ctx.chat_request.messages[idx].clone();
        enriched.content = Some(match enriched.content.take() {
            Some(content) => format!("{content}{suffix}"),
            None => suffix.trim_start().to_string(),
        });
        // 标记合并（不整体替换）：保留消息原有 metadata（如 user 附件的 attachments，
        // 前端缩略图 / 冷启动重建依赖它），只补 env 标记。
        let mut md = enriched
            .metadata
            .take()
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| json!({}));
        if let Some(obj) = md.as_object_mut() {
            obj.insert("env".into(), json!(true));
        }
        enriched.metadata = Some(md);
        ctx.chat_request.messages[idx] = enriched.clone();
        // 同步会话历史（保持两份一致；索引在 before_loop 阶段与 ctx 对齐）
        if let Ok(mut history) = self.history.lock() {
            if idx < history.len() {
                history[idx] = enriched;
            }
        }
    }
}

/// 读取约定文件并追加为提示词块（读取失败 / 空内容不追加，仅在有内容时记日志）。
fn push_agents_block(prompt: &mut String, path: &std::path::Path, intro: &str) {
    if let Some(block) = load_agents_block(path, intro, AGENTS_MAX_CHARS) {
        tracing::info!(
            "[prompt] 已注入约定块 {}（{} 字符）",
            path.display(),
            block.chars().count()
        );
        prompt.push_str("\n\n");
        prompt.push_str(&block);
    }
}

/// 读取 `<path>` 并渲染为约定提示词块；全程 **fail-soft**（任何异常一律返回 `None`）。
///
/// 守卫：必须是普通文件、字节数不超过 [`AGENTS_MAX_BYTES`]，内容非空白。
fn load_agents_block(path: &std::path::Path, intro: &str, max_chars: usize) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > AGENTS_MAX_BYTES {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    render_agents_block(&content, intro, max_chars)
}

/// 纯渲染（无 IO、不依赖 `App`，便于单测）：`intro` + 正文；正文空白返回 `None`，
/// 超 `max_chars` 按字符截断（附省略号）。
fn render_agents_block(content: &str, intro: &str, max_chars: usize) -> Option<String> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    Some(format!(
        "{intro}\n\n{}",
        crate::util::text::truncate_chars(content, max_chars)
    ))
}

/// 会话模式 → ASCII 标识（会话环境信息用）。
fn mode_str(mode: &SessionMode) -> &'static str {
    match mode {
        SessionMode::Plan => "plan",
        SessionMode::Execute => "execute",
        SessionMode::Auto => "auto",
    }
}

/// 会话工作区**当前**的 git 分支：每个用户消息实时探测一次。
///
/// 刻意不读会话记录里的分支字段——那是建会话那一刻的快照，agent 自己在会话里
/// checkout、或用户在外部终端切分支都不会回写记录，快照必然过期，模型会被一直
/// 喂旧分支（这正是「新建会话显示 master，但实际 HEAD 是 test」那类问题的根源）。
/// 非仓库 / 无工作区 / git 不可用 / 分离 HEAD → 「未知」。
///
/// 成本：每个用户消息一次 git 子进程（约 5 ms）；注入点在用户消息尾部，
/// **不动 system 前缀**，不破坏前缀缓存。
fn git_branch_of(record: &SessionRecord) -> String {
    let Some(app) = App::try_global() else {
        return "未知".into();
    };
    let dir = crate::tool::resolve_workspace(&record.id.to_string());
    match app.git().git_current_branch(dir.to_string_lossy().into_owned()) {
        Ok(name) if !name.is_empty() => name,
        _ => "未知".into(),
    }
}

/// 从会话记录构建「会话环境」信息块（追加在用户消息后，随每轮用户提示词携带）。
///
/// 只保留**每轮都变 / 每会话不同**的最小事实：会话模式 / 工作区 / git 分支（实时探测）/
/// 用户语言偏好 / 当前时间。可用能力清单在系统提示词「可用能力」段（见
/// [`build_capability_block`]）；任务规划内容作为独立「【任务规划】」块随用户消息下发。
fn build_environment_prompt(record: &SessionRecord) -> String {
    let locale = App::try_global()
        .map(|app| app.config().locale.clone())
        .unwrap_or_else(|| "zh-CN".into());
    let now = chrono::Local::now();
    let weekday = match now.format("%u").to_string().as_str() {
        "1" => "一",
        "2" => "二",
        "3" => "三",
        "4" => "四",
        "5" => "五",
        "6" => "六",
        _ => "日",
    };
    format!(
        "- 会话模式：{}（plan=先规划后执行 / execute=直接执行 / auto=自动判断）\n- 工作区：{}\n- git 分支：{}\n- 用户语言偏好：{}\n- 当前时间：{}（周{}）",
        mode_str(&record.session_mode),
        record.workspace.clone().unwrap_or_else(|| "未知".into()),
        git_branch_of(record),
        locale,
        now.format("%Y-%m-%d %H:%M:%S"),
        weekday,
    )
}

/// 系统提示词「可用能力」段（动态，第 ⑤ 项）：技能 / 工具 / MCP / 长期记忆 / 子智能体，
/// **只给名称和描述**（技能最多 10 个，更多让 LLM 用 skills 工具的 search 发现）。
fn build_capability_block(record: &SessionRecord, app: &Arc<AppState>) -> String {
    let mut blocks: Vec<String> = Vec::new();

    // 可用技能（绑定集，≤10 个）：名称 + 描述；超出提示用 search 发现
    if !record.skill_list.is_empty() {
        if let Ok(skills) = app.skill.list_by_names(&record.skill_list) {
            if !skills.is_empty() {
                let cap = skills.len().min(MAX_SKILL_LINES);
                let lines: Vec<String> = skills[..cap]
                    .iter()
                    .map(|s| format!("- {}: {}", s.name, s.description))
                    .collect();
                let mut block = format!("## 可用技能\n\n{}", lines.join("\n"));
                if skills.len() > MAX_SKILL_LINES {
                    block.push_str(&format!(
                        "\n（共 {} 个技能，仅列前 {} 个；更多用 `skills` 工具的 search 动作按关键词发现）",
                        skills.len(),
                        MAX_SKILL_LINES
                    ));
                }
                blocks.push(block);
            }
        }
    }

    // 可加载（延迟加载）工具：常驻工具的调用契约由 API `tools` schema 承载，提示词不罗列；
    // 这里只列**尚未加载**的动态工具（可见集 ∩ 可加载集 − 已加载集），供模型知道
    // 可经 `tool_load` 加载什么；已加载的下一轮起已在 schema 里，不再重复。
    let known = app.tools.names();
    let loadable: Vec<&str> = known
        .iter()
        .filter(|n| crate::agent::dynamic_tools::is_dynamic_tool(n))
        .filter(|n| record.tool_list.iter().any(|t| t == *n))
        .filter(|n| !record.loaded_tool_list.iter().any(|t| t == *n))
        .map(String::as_str)
        .collect();
    if !loadable.is_empty() {
        let lines: Vec<String> = loadable
            .iter()
            .map(|name| {
                match crate::agent::dynamic_tools::DYNAMIC_TOOL_HELP
                    .iter()
                    .find(|(n, _)| n == name)
                {
                    Some((_, help)) => format!("- {name}：{help}"),
                    None => format!("- {name}"),
                }
            })
            .collect();
        blocks.push(format!(
            "## 可加载工具\n\n{}\n\n(以上工具默认不下发,需要时先用 `tool_load` 加载,\n加载后自下一轮起可直接调用)",
            lines.join("\n")
        ));
    }

    // 已连接 MCP 服务：以**运行时连接**为准（连接即工具可用，见 `AppState::loop_tool_map`），
    // 会话记录里的静态清单（桌面端写入）作为补充；工具名一并列出，便于模型知道能调什么。
    let mut mcp_names = app.mcp().connected_servers();
    for name in &record.mcp_list {
        if !mcp_names.contains(name) {
            mcp_names.push(name.clone());
        }
    }
    if !mcp_names.is_empty() {
        let lines: Vec<String> = mcp_names
            .iter()
            .map(|name| {
                let tools: Vec<String> = app
                    .mcp()
                    .list_tools(name)
                    .into_iter()
                    .map(|t| t.name)
                    .collect();
                if tools.is_empty() {
                    return format!("- {name}");
                }
                let cap = tools.len().min(MAX_MCP_TOOL_LINES);
                let mut line = format!("- {name}（{} 个工具：{}", tools.len(), tools[..cap].join("、"));
                if tools.len() > cap {
                    line.push_str(" 等");
                }
                line.push('）');
                line
            })
            .collect();
        blocks.push(format!(
            "## 已连接 MCP 服务\n\n{}\n\n(以上服务的工具已直接下发,可直接调用)",
            lines.join("\n")
        ));
    }

    // 长期记忆能力行（memory 工具在可用集里且库里有概念时出现）
    if let Some(line) = app.memory().capability_line(&record.tool_list) {
        blocks.push(line);
    }

    // 可用智能体：供模型填 `sub_session` 的 `agentType`（委派子任务）。
    // 子会话本身不能再开子会话（深度 = 1），故对子会话**不列**该块，避免诱导无效调用。
    if record.pid.is_none() {
        if let Ok(agents) = app.agents.list() {
            if !agents.is_empty() {
                let lines: Vec<String> = agents
                    .iter()
                    .map(|a| {
                        if a.description.trim().is_empty() {
                            format!("- {}（id: {}）", a.name, a.agent_id)
                        } else {
                            format!("- {}（id: {}）：{}", a.name, a.agent_id, a.description)
                        }
                    })
                    .collect();
                blocks.push(format!(
                    "## 可用智能体（`sub_session` 委派时填 id）\n\n{}",
                    lines.join("\n")
                ));
            }
        }
    }

    if blocks.is_empty() {
        return String::new();
    }
    format!("# 可用能力\n\n{}", blocks.join("\n\n"))
}

/// 读取会话工作区 `.astorm/plan/` 下的规划:大纲步骤 + 详细 markdown 文档,拼为提示词块。
fn build_plan_prompt(session_id: &str) -> String {
    let dir = crate::tool::plan_docs_dir(session_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return String::new();
    };
    let mut blocks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "md" {
            continue;
        }
        let Some(task_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let detail = std::fs::read_to_string(&path).unwrap_or_default();
        let outline = crate::tool::storage()
            .and_then(|st| st.get_plan(session_id, task_id).ok().flatten());
        let mut block = String::new();
        match outline {
            Some(plan) => {
                let steps = plan
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(i, n)| format!("{}. {} - {}", i + 1, n.task_name, n.task_description))
                    .collect::<Vec<_>>()
                    .join("\n");
                block.push_str(&format!(
                    "### 任务 {task_id}（{}）\n大纲步骤（{} 步）:\n{}\n",
                    plan.title,
                    plan.nodes.len(),
                    steps
                ));
            }
            None => block.push_str(&format!("### 任务 {task_id}\n")),
        }
        if !detail.trim().is_empty() {
            block.push_str("\n详细规划:\n");
            block.push_str(&indent_plan_md(&detail));
        }
        blocks.push(block);
    }
    blocks.join("\n\n")
}

/// markdown 逐行缩进两空格（嵌套进提示词块，与列表层级一致）。
fn indent_plan_md(md: &str) -> String {
    md.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独占子目录（同进程并行跑不互相覆盖）。
    fn temp_agents_file(tag: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("astorm_prompt_test_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join(AGENTS_FILE);
        std::fs::write(&path, content).expect("写临时文件失败");
        path
    }

    #[test]
    fn renders_intro_and_content() {
        let block = render_agents_block("  项目约定：先跑测试。\n", "【说明】", 100).expect("应渲染");
        assert!(block.starts_with("【说明】\n\n"), "说明头应在前: {block}");
        assert!(block.contains("项目约定：先跑测试。"));
    }

    #[test]
    fn blank_content_is_skipped() {
        assert!(render_agents_block("", "x", 100).is_none());
        assert!(render_agents_block("   \n\t ", "x", 100).is_none());
    }

    #[test]
    fn truncates_by_chars_not_bytes() {
        // 中文按字符截断，不能 panic 也不应超限
        let long: String = "中文约定".repeat(50);
        let block = render_agents_block(&long, "x", 10).expect("应渲染");
        let body = block.strip_prefix("x\n\n").expect("前缀");
        assert_eq!(body.chars().count(), 11, "10 字 + 省略号");
        assert!(body.ends_with('…'));
    }

    #[test]
    fn loads_file_and_skips_missing_or_empty() {
        let path = temp_agents_file("ok", "工作区约定内容");
        let block = load_agents_block(&path, "【工作区】", 100).expect("应读取成功");
        assert!(block.contains("工作区约定内容"));

        assert!(load_agents_block(&path.with_file_name("NOPE.md"), "x", 100).is_none());

        let empty = temp_agents_file("empty", "   \n");
        assert!(load_agents_block(&empty, "x", 100).is_none());
    }

    #[test]
    fn oversized_file_is_skipped() {
        let big = "a".repeat((AGENTS_MAX_BYTES + 1) as usize);
        let path = temp_agents_file("big", &big);
        assert!(
            load_agents_block(&path, "x", AGENTS_MAX_CHARS).is_none(),
            "超过字节上限应跳过"
        );
    }

    /// 正文引用的书写约定必须留在提示词里：前端把「路径:行号」渲染成可点击的文件引用 chip、
    /// 把「[n](url)」渲染成网页引用角标（判定见 `frontend/src/renderer/src/utils/fileRefs.ts`
    /// 与 `components/markdown/LinkRenderer.vue`），提示词里删掉这两句，模型就会退回散文写法，
    /// 引用不再可点。
    #[test]
    fn system_prompt_keeps_file_reference_convention() {
        assert!(
            SYSTEM_PROMPT.contains("路径:行号"),
            "系统提示词应保留文件引用书写约定（反引号包裹的 路径:行号）"
        );
        assert!(SYSTEM_PROMPT.contains("auth.py:47-52"), "应给出区间写法示例");
    }

    /// 网页引用角标同上一并守住（`[n](url)` + 编号递增 + 不编造 URL）。
    #[test]
    fn system_prompt_keeps_web_reference_convention() {
        assert!(
            SYSTEM_PROMPT.contains("编号角标 `[n](完整URL)`"),
            "系统提示词应保留网页引用角标写法"
        );
        assert!(SYSTEM_PROMPT.contains("不要凭印象编造链接"), "应禁止编造来源 URL");
        assert!(SYSTEM_PROMPT.contains("同一处正文引用多个来源时，连续写多个独立角标"));
        assert!(SYSTEM_PROMPT.contains("不要合成 `[1,2,3](URL)`"));
        assert!(SYSTEM_PROMPT.contains("同一来源重复引用沿用同一个 n，不同来源使用不同编号"));
    }
}