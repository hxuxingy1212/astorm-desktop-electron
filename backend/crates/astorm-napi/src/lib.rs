//! astorm-napi：Electron 后端的 napi 绑定层。
//!
//! 只承载「应用生命周期 + 事件订阅 + 各应用模块接口」胶水能力，**不写任何业务实现**：
//! - [`lifecycle`]：`start`（接收前端 JSON 配置并启动）/ `shutdown` / `is_running` / `version`
//! - [`subscribe`]：全局事件总线订阅（Web ReadableStream）
//! - [`session`]：会话接口（委托 [`astorm_core::app::App`] 业务门面）
//! - [`agent`]：智能体卡片接口（智能体设置的增删改查）
//! - [`browser_data`]：本机 Chrome Cookie 导入（设置页「自动化控制」）
//! - [`memory`]：长期记忆（OKF v0.2 Bundle：状态 / 概念浏览 / 检索 / 校验 / 编译触发）
//! - [`file`] / [`git`] / [`tool`] / [`skill`] / [`schedule`] / [`taskplan`]：
//!   应用模块接口（接口骨架，委托 core 门面，业务实现待补充）

pub mod agent;
pub mod browser_data;
pub mod config;
pub mod file;
pub mod git;
pub mod knowledge;
pub mod lifecycle;
pub mod mail;
pub mod mcp;
pub mod memory;
pub mod paths;
mod localenv;
pub mod schedule;
pub mod session;
pub mod pty;
mod audit;
mod security;
pub mod skill;
pub mod subscribe;
pub mod tabs;
pub mod taskplan;
pub mod terminal;
pub mod tool;
pub mod usage;

pub use agent::{
    create_agent, get_agent, list_agents, remove_agent, update_agent, AgentCard, AgentExecMode,
    AgentPermissionMode, AgentUpdate, CreateAgentRequest,
};
pub use browser_data::{import_chrome_cookies, ChromeCookie, ChromeCookieImport};
pub use config::update_config;
pub use file::{get_app_path, get_project_root, list_directory, read_file_content, FileNode};
pub use git::{
    check_git_env, get_git_diff_summary, get_git_file_diff, git_commit, git_ignored, git_init,
    git_push, git_stage, git_status, git_unstage, DiffFileContent, DiffFileEntry, GitEnvInfo,
    GitFileChange, GitStatus,
};
pub use knowledge::{
    get_knowledge_root, kb_create, kb_delete, kb_import, kb_list, kb_read, kb_read_buffer,
    kb_rename, kb_update_meta, kb_write,
    KbCreateRequest, KbItem, KbMetaUpdate,
};
pub use lifecycle::{is_running, shutdown, start, version};
pub use localenv::{detect_local_env, LocalEnv};
pub use mail::{
    mail_activate_account, mail_add_account, mail_attachment, mail_attachment_to_temp,
    mail_folder_counts, mail_folders, mail_message, mail_messages, mail_move, mail_previews,
    mail_remove_account, mail_send, mail_set_meta, mail_state, mail_update_account, MailAccount,
    MailAttachmentBlob, MailAttachmentMeta, MailAddAccount, MailDetail, MailFolderCount, MailList,
    MailMessage, MailMessagesQuery, MailMeta, MailPreview, MailSendInput, MailState,
    MailUpdateAccount,
};
pub use mcp::{
    mcp_connect_server, mcp_disconnect_server, mcp_list_servers, mcp_read_config_file,
    mcp_toggle_server, mcp_write_config_file, McpServerSummary,
};
pub use memory::{
    memory_cancel, memory_compile, memory_list_concepts, memory_list_raw_sources, memory_read_log,
    memory_read_raw_source, memory_reindex, memory_root, memory_search, memory_show_concept,
    memory_status, memory_validate, MemoryCategoryStat, MemoryConcept, MemoryConceptDetail,
    MemoryIssue, MemoryLogEntry, MemoryRawSource, MemorySourceRef, MemoryStatus,
    MemoryValidateReport,
};
pub use paths::{get_data_paths, DataPaths};
pub use pty::{pty_close, pty_open, pty_resize, pty_write};
pub use audit::read_audit_log;
pub use security::{
    builtin_danger_patterns, toolchain_allowances, ToolchainAllowanceReport, ToolchainAllowanceTool,
};
pub use schedule::{
    add_scheduled_task, delete_scheduled_task, generate_scheduler_workspace,
    list_scheduled_tasks, run_scheduled_task_now, toggle_scheduled_task, update_scheduled_task,
    GeneratedWorkspace, ScheduledTask, ScheduledTaskRequest,
};
pub use session::{
    archived_messages_before, ask_llm, cancel_session, create_session, delete_session,
    fork_session, get_system_prompt,
    interrupt_message, list_all_deliverables, list_messages, list_session_deliverables,
    list_sessions, reopen_interrupt, rewind_to_user_message, send_message, start_session,
    submit_interrupt, summarize_session, update_session_goal, update_session_settings,
    CreateSessionRequest,
    InterruptType, MessageRecord, PermissionMode, ReasoningEffort, SessionMode, SessionRecord,
    SessionType, SubmitInterruptRequest, UpdateSessionSettingsRequest,
};
pub use skill::{
    create_skill, discover_skills, get_session_skills, get_skill, import_skill_package,
    import_skills, list_skills, remove_skill, search_skills, select_directory,
    set_session_skills, toggle_skill, update_skill, CreateSkillRequest, DiscoveredSkill, SkillDoc,
    SkillSummary, SkillUpdate,
};
pub use subscribe::subscribe;
pub use tabs::update_tabs_state;
pub use taskplan::{
    list_plan_files, read_plan_file, PlanFile, PlanStep, PlanSummary, ReadPlanFileRequest,
};
pub use tool::{list_mcp_server_tools, list_tools, search_tools, ToolInfo, ToolSource};
pub use usage::usage_stats;

/// 获取业务门面单例（未初始化时返回 napi 错误，避免 panic 跨 FFI 边界）。
pub(crate) fn app() -> napi::Result<&'static std::sync::Arc<astorm_core::app::AppState>> {
    astorm_core::app::App::try_global()
        .ok_or_else(|| napi::Error::from_reason("应用未初始化：请先调用 start"))
}
