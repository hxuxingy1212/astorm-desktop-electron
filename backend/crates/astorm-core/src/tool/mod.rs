//! 工具集：为 ReAct 循环提供可控的操作工具（实现 `react-loop` 的 [`Tool`] trait）。
//!
//! - [`ToolRegistry`]：内置工具注册中心（挂于 `AppState`，经 `App::global()` 可达）
//! - `file`：`file_operation`（读 / 写 / 编辑 / 追加 / 列目录 / 正则 / glob 搜索）
//! - `shell`：`bash`（工作区内执行命令，超时 / 取消 / 输出截断）
//! - `chat`：`chat_with_user`（分部表单向用户收集信息，返回 `form` 中断信封打断循环等待回答）
//! - `task_list`：线性任务计划管理（sqlite `plans` 表持久化）
//! - `delivery`：`submit_deliverables`（每轮收尾上报交付物 / 改动文件 / 有用记忆 id，
//!   富集后写 sqlite `session_deliverables` 表，前端会话结束后拉表渲染卡片）
//! - `load`：`tool_load`（按需加载延迟工具：定时任务 / 邮件 / 内置浏览器初始不下发，
//!   加载态持久化在会话记录，下一轮回复起携带新工具 schema；见 `agent::dynamic_tools`）
//! - `search`：统一搜索路由（对外仅暴露 `search`，内部实现见 `search/` 目录：
//!   `websearch` / `webfetch` / `github_trending` / `arxiv_search` / `history_search`）
//! - `read_images`：图片理解（本地 / 远程图片按 DeepSeek 多模态格式作为 user 消息注入，仅本轮可见）
//! - `skills`：技能管理（load / search / register；见 `skill/` 目录）
//! - `schedule_task`：定时任务管理（Cron；见 `scheduler/` 目录）
//! - `browser`：内置浏览器「AI 代理操控」（iframe 作用域 CDP 桥 + `agent-browser` CLI；见 `browser/`）
//! - `office`：astorm-office 五 CLI（统一入口 + 四格式；Word/Excel/PPT/PDF 生成·编辑·转换；随包分发，见 [`office`]）
//! - `tabs`：操控前端右侧工作区的页签（打开 / 列出 / 切换 / 关闭；见 [`tabs`]）
//!
//! 中断（等待用户）统一走 `Tool::interrupt_payload`：安全审批与提问同构，均返回中断信封。
//! 安全拦截的分类统一走 [`denial`]（拒绝读 / 拒绝写 / 危险命令 / 网络 / 资源）：进审批信封
//! `approval.category`、进工具结果元数据（`Tool::result_meta` → tool_result 事件的 `denial`
//! 字段 + 消息 `metadata.denial`）与审计记录的 `category`。
//!
//! 工具执行仅依赖 [`react_loop::ToolContext`]（`session_id` / `cancel` / `tool_call_id`）+ 经
//! `App::global()` 解析会话工作区与配置，无全局可变单例；react-loop / llm-provider 零改动。

pub mod audit;
pub mod browser;
pub mod chat;
pub mod denial;
pub mod delivery;
pub mod diff;
pub mod file;
pub mod interrupt;
pub mod load;
pub mod mail;
pub mod memory;
pub mod net_intent;
pub mod office;
pub mod policy;
pub mod read_images;
pub mod schedule_task;
pub mod search;
pub mod session_allow;
pub mod shell;
pub mod skills;
pub mod sub_session;
pub mod tabs;
pub mod task_list;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use astorm_common::session::types::SessionMode;
use astorm_common::tool::{ToolInfo, ToolSource};
use astorm_common::types::record::SessionRecord;
use astorm_common::types::Storage;
use react_loop::{Tool, ToolContext, ToolMap};

/// 内置工具注册中心（进程级，挂于 `AppState`）。
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// 构建全部内置工具。
    pub(crate) fn builtin() -> Self {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(file::FileOperationTool),
            Arc::new(shell::ShellTool),
            Arc::new(chat::UserChatTool),
            Arc::new(task_list::TaskListTool),
            Arc::new(delivery::DeliveryTool),
            Arc::new(load::ToolLoadTool),
            Arc::new(search::SearchTool),
            Arc::new(read_images::ReadImagesTool),
            Arc::new(skills::SkillsTool),
            Arc::new(schedule_task::ScheduleTool),
            Arc::new(sub_session::SubSessionTool),
            Arc::new(browser::BrowserTool),
            Arc::new(office::OfficeTool),
            Arc::new(tabs::TabsTool),
            Arc::new(memory::MemoryTool),
            Arc::new(mail::MailTool),
        ];
        Self { tools }
    }

    /// 导出为循环可用的 [`ToolMap`]（全量；未按智能体过滤时使用）。
    pub(crate) fn tool_map(&self) -> ToolMap {
        ToolMap::from_tools(self.tools.clone())
    }

    /// 按名称白名单导出循环可用的 [`ToolMap`]（**智能体可见工具集**）。
    ///
    /// 会话记录的 `tool_list` 即白名单（创建 / 冷启动自愈时按智能体卡片物化）：
    /// - 空列表 → 空 [`ToolMap`]（LLM 请求不带 tools，即智能体「无工具」语义）；
    /// - 白名单里已不存在的工具名静默忽略（卡片可能残留被移除的工具名）。
    pub(crate) fn tool_map_for(&self, names: &[String]) -> ToolMap {
        let tools: Vec<Arc<dyn Tool>> = self
            .tools
            .iter()
            .filter(|t| names.iter().any(|n| n == t.name()))
            .cloned()
            .collect();
        ToolMap::from_tools(tools)
    }

    /// 内置工具名列表（写入会话记录的 `tool_list` / 环境提示词）。
    pub(crate) fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// 工具目录快照（`list_tools` / `search_tools` 数据源；当前全部为 builtin 来源）。
    pub(crate) fn infos(&self) -> Vec<ToolInfo> {
        self.tools
            .iter()
            .map(|t| ToolInfo {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters_schema: t.parameters_schema(),
                source: ToolSource::Builtin,
                // 「按需加载」标记：清单真相在 agent::dynamic_tools::DYNAMIC_TOOLS，
                // 前端设置页据此把动态工具独立分组展示
                dynamic: crate::agent::dynamic_tools::is_dynamic_tool(t.name()),
            })
            .collect()
    }
}

/// 解析会话工作区目录（会话记录 `workspace` 优先，回退全局默认值；App 未初始化时给占位）。
///
/// 用户目录根 = `AppConfig.workspace`（系统设置可配，默认 `~/astorm`）。
/// 会话指定为**相对路径**（如默认时间戳工作区 `2026_09_03_22_30_00`）时，归入
/// `<用户目录>/workspace/` 下并**幂等创建目录**；显式绝对路径原样使用。
pub(crate) fn resolve_workspace(session_id: &str) -> PathBuf {
    let workspace_root = crate::app::App::try_global()
        .map(|app| crate::app::paths::AppDir::new(app.config()).workspace_dir)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join("astorm")
                .join("workspace")
        });
    let workspace = match session_record(session_id)
        .and_then(|r| r.workspace.clone().filter(|w| !w.trim().is_empty()))
    {
        Some(w) => {
            let p = Path::new(&w);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                workspace_root.join(w)
            }
        }
        None => workspace_root,
    };
    let _ = std::fs::create_dir_all(&workspace);
    workspace
}

/// 由会话记录判定**根会话 id**（纯函数，便于测试）：有 `pid` 即父会话，否则是自己。
///
/// 子会话深度固定为 1（`sub_session` 禁止嵌套），故一层即可到位。
pub(crate) fn root_of(record: Option<&SessionRecord>, session_id: &str) -> String {
    record
        .and_then(|r| r.pid)
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| session_id.to_string())
}

/// 会话的**根会话 id**：子会话归一化到父会话（子会话无上级时原样返回）。
///
/// 用途：**与父会话共享同一份 UI 状态**的工具。页签镜像（`tool::tabs::TABS_STATE`）与
/// 浏览器桥端点都按会话归属，且后端只持有「当前激活会话」的新鲜镜像——子会话若按自身 id
/// 去读写，会既读不到镜像（从未上报过）、又被浏览器端点的归属校验拒绝
/// （`agent_cli::read_endpoint`：「当前浏览器页签属于另一个会话」）。
/// 归一化后子会话与主会话共享同一个浏览器实例与同一套页签。
pub(crate) fn root_session_id(session_id: &str) -> String {
    root_of(session_record(session_id).as_ref(), session_id)
}

/// 读取会话记录快照（工具读取会话模式 / 权限等；App 未初始化时返回 None）。
pub(crate) fn session_record(session_id: &str) -> Option<SessionRecord> {
    crate::app::App::try_global()
        .and_then(|app| app.session().record_of(session_id))
}

/// 会话「规划详情」目录：`{会话工作区}/.astorm/plan`（幂等创建由写方负责）。
pub(crate) fn plan_docs_dir(session_id: &str) -> PathBuf {
    resolve_workspace(session_id).join(".astorm").join("plan")
}

/// 单个规划详情的 markdown 文档路径：`{会话工作区}/.astorm/plan/{task_id}.md`。
///
/// task_id 由系统生成（UUID）；这里强制 UUID 形态，防止 `../../x` 或 Windows
/// 绝对路径借 `join` 删除/写入规划目录之外的文件。
pub(crate) fn plan_doc_path(session_id: &str, task_id: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(task_id)
        .map_err(|_| format!("task_id 必须是 UUID: {task_id}"))?;
    Ok(plan_docs_dir(session_id).join(format!("{task_id}.md")))
}

/// 会话模式（工具按模式约束行为，如 Plan 模式禁止写文件）。
pub(crate) fn session_mode(session_id: &str) -> SessionMode {
    session_record(session_id)
        .map(|r| r.session_mode)
        .unwrap_or_default()
}

/// 展开路径/命令中的会话变量：`$SESSION_WORKSPACE` / `$SESSION_ID`。
///
/// 用于**路径参数**（file/mail/read_images/skills 等）：原样替换，不加引号。
pub(crate) fn expand_session_vars(raw: &str, session_id: &str, workspace: &Path) -> String {
    let ws = workspace.to_string_lossy();
    raw.replace("$SESSION_WORKSPACE", &ws)
        .replace("${SESSION_WORKSPACE}", &ws)
        .replace("$SESSION_ID", session_id)
        .replace("${SESSION_ID}", session_id)
}

/// 展开 **shell 命令文本**中的会话变量。
///
/// `$SESSION_WORKSPACE` 按平台加引号（POSIX 单引号 / cmd 双引号），避免含空格
/// 的工作区被 shell 分词；若变量已在引号内，则只做对应引号上下文的转义。
/// `$SESSION_ID` 是 UUID，原样替换。
pub(crate) fn expand_session_vars_shell(
    raw: &str,
    session_id: &str,
    workspace: &Path,
) -> String {
    let ws = workspace.to_string_lossy();
    let expanded = expand_workspace_in_command(raw, &ws);
    expanded
        .replace("$SESSION_ID", session_id)
        .replace("${SESSION_ID}", session_id)
}

fn expand_workspace_in_command(raw: &str, ws: &str) -> String {
    const TOKENS: [&str; 2] = ["${SESSION_WORKSPACE}", "$SESSION_WORKSPACE"];
    let quoted = quote_shell_path(ws);
    let mut out = String::with_capacity(raw.len() + 8);
    let mut single = false;
    let mut double = false;
    let mut i = 0;
    while i < raw.len() {
        let rest = &raw[i..];
        if let Some(token) = TOKENS.iter().find(|t| rest.starts_with(**t)) {
            if single {
                out.push_str(&escape_single_quoted(ws));
            } else if double {
                out.push_str(&escape_double_quoted(ws));
            } else {
                out.push_str(&quoted);
            }
            i += token.len();
            continue;
        }
        let ch = rest.chars().next().unwrap();
        // 引号状态机：Windows cmd 只有双引号；POSIX 单/双引号都参与。
        match ch {
            '"' if !single => double = !double,
            '\'' if !double && !cfg!(windows) => single = !single,
            _ => {}
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(windows)]
fn quote_shell_path(path: &str) -> String {
    // cmd.exe：双引号；Windows 路径不能包含 `"`
    format!("\"{path}\"")
}

#[cfg(not(windows))]
fn quote_shell_path(path: &str) -> String {
    // POSIX sh：单引号，内部单引号用 '\'' 拼接
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn escape_single_quoted(path: &str) -> String {
    // cmd 不识别单引号；命令里出现单引号时按原样（调用方应使用双引号）
    path.to_string()
}

#[cfg(not(windows))]
fn escape_single_quoted(path: &str) -> String {
    path.replace('\'', "'\\''")
}

fn escape_double_quoted(path: &str) -> String {
    #[cfg(windows)]
    {
        // Windows 路径不能包含双引号；`%` 在 cmd 内展开，路径里极少见
        path.to_string()
    }
    #[cfg(not(windows))]
    {
        let mut s = String::with_capacity(path.len());
        for ch in path.chars() {
            if matches!(ch, '\\' | '"' | '$' | '`') {
                s.push('\\');
            }
            s.push(ch);
        }
        s
    }
}

/// 全局存储门面（工具读写会话元数据 / 历史全文检索 / 规划落盘）。
pub(crate) fn storage() -> Option<Arc<dyn Storage>> {
    crate::app::App::try_global().map(|app| Arc::clone(&app.storage))
}

#[cfg(test)]
mod tests;