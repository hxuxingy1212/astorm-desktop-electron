//! 应用路径：平台数据根解析 + 应用路径集（[`AppDir`]）。
//!
//! 目录分两层（系统 = 应用自身；用户 = 用户产生的内容）：
//!
//! 系统目录（平台约定的用户数据根，随应用安装；Windows: `%APPDATA%\astorm`）：
//! ```text
//! <app_dir>/
//!   config/             # 配置目录（config.json + providers.json + backend-settings.json + security.json + mcp_servers.json）
//!   data/               # 持久化数据（sqlite.db / session/ / index/ / skill_index/）
//!   log/                # 日志目录（astorm.YYYY-MM-DD.log 按天滚动）
//!   plugins/            # 插件目录（Node 主进程加载，plugins.json 清单）
//!   skills/             # 技能 / 技能包目录（预留，后续使用）
//!   electron/           # Chromium 内部文件（主进程 app.setPath('userData') 重定向至此）
//! ```
//!
//! 用户目录（系统设置可配，默认 `~/astorm`；`AppConfig.workspace` 即此根）：
//! ```text
//! <user_dir>/
//!   workspace/          # 未指定会话空间时的任务工作区（时间戳目录归入其下）
//!   schedule/           # 定时任务会话工作区（每个任务一个子目录：<task_id> / <uuid>）
//!   knowledge/          # 知识库（真实文件树 + .manifest.json 元数据清单）
//! ```

use std::path::{Path, PathBuf};

use super::config::AppConfig;

/// 应用根目录（系统侧用户数据根）。
///
/// 平台约定位置：
/// - macOS：`~/Library/Application Support/astorm`
/// - Windows：`%APPDATA%\astorm`
/// - Linux：`~/.local/share/astorm`
///
/// `dirs::data_dir()` 无法解析（极少数环境）时回退 `cwd/astorm-data`。
pub fn user_data_dir() -> PathBuf {
    // 隔离重定向：`ASTORM_DATA_DIR` 非空时优先（集成测试 / 多实例场景，
    // 避免与运行中的应用抢 sqlite / tantivy skill_index 等文件锁）
    if let Ok(dir) = std::env::var("ASTORM_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::data_dir()
        .map(|d| d.join("astorm"))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|c| c.join("astorm-data"))
                .unwrap_or_else(|_| PathBuf::from("astorm-data"))
        })
}

/// 用户目录根：`AppConfig.workspace` 非空时原样使用；
/// 未配置时回退默认 `~/astorm`（与前端设置页默认值一致）。
pub fn user_dir(config: &AppConfig) -> PathBuf {
    let ws = config.workspace.trim();
    if ws.is_empty() {
        dirs::home_dir()
            .map(|h| h.join("astorm"))
            .unwrap_or_else(|| PathBuf::from("astorm"))
    } else {
        PathBuf::from(ws)
    }
}

/// 应用路径集（`AppState::dir()` 输出）：
/// 系统侧（config / data / log / plugins / skills / electron）+ 用户侧（workspace / knowledge）。
#[derive(Debug, Clone)]
pub struct AppDir {
    /// 应用根目录（系统侧：下辖 config/ data/ log/ plugins/ skills/ electron/）
    pub app_dir: PathBuf,
    /// 配置目录（`app_dir/config`，config.json 与主进程 JSON 配置所在）
    pub config_dir: PathBuf,
    /// 配置文件路径（`config_dir/config.json`，预留）
    pub config_file: PathBuf,
    /// 数据目录（`app_dir/data`，持久化数据：sqlite.db / session/ / index/ / skill_index/）
    pub data_dir: PathBuf,
    /// 数据文件路径（`data_dir/sqlite.db`，会话元数据等主数据落盘文件）
    pub data_file: PathBuf,
    /// 日志目录（`app_dir/log`，默认；`log.dir` 为绝对路径时原样使用）
    pub log_dir: PathBuf,
    /// 日志文件路径（`log_dir/astorm.log`，实际为按天滚动的 `astorm.YYYY-MM-DD.log`）
    pub log_file: PathBuf,
    /// 插件目录（`app_dir/plugins`，Node 主进程加载）
    pub plugins_dir: PathBuf,
    /// 技能目录（`app_dir/skills`，技能 / 技能包存放处；预留，后续使用）
    pub skills_dir: PathBuf,
    /// Chromium 内部文件目录（`app_dir/electron`，主进程 userData 重定向目标）
    pub electron_dir: PathBuf,
    /// 用户目录根（`AppConfig.workspace`，默认 `~/astorm`）
    pub user_dir: PathBuf,
    /// 用户工作区根（`user_dir/workspace`，未指定会话空间时的时间戳任务目录归入其下）
    pub workspace_dir: PathBuf,
    /// 定时任务工作区根（`user_dir/schedule`，任务未指定会话目录时的默认落点）
    pub schedule_dir: PathBuf,
    /// 知识库目录（`user_dir/knowledge`，真实文件树 + 元数据清单）
    pub knowledge_dir: PathBuf,
    /// 长期记忆 Bundle 根（`user_dir/memory`，OKF v0.2：`raw/` 来源 + `wiki/` 概念）
    pub memory_dir: PathBuf,
}

impl AppDir {
    /// 由应用配置构建路径集（系统侧 = [`user_data_dir`]，用户侧 = `config.workspace`）。
    pub fn new(config: &AppConfig) -> Self {
        let app_dir = user_data_dir();
        let data_dir = app_dir.join("data");
        let config_dir = app_dir.join("config");
        let log_dir = resolve_log_dir(&app_dir, config);
        let user_dir = user_dir(config);
        Self {
            config_file: config_dir.join("config.json"),
            data_file: data_dir.join("sqlite.db"),
            log_file: log_dir.join("astorm.log"),
            plugins_dir: app_dir.join("plugins"),
            skills_dir: app_dir.join("skills"),
            electron_dir: app_dir.join("electron"),
            workspace_dir: user_dir.join("workspace"),
            schedule_dir: user_dir.join("schedule"),
            knowledge_dir: user_dir.join("knowledge"),
            memory_dir: user_dir.join("memory"),
            app_dir,
            config_dir,
            data_dir,
            log_dir,
            user_dir,
        }
    }
}

/// 解析日志目录：`log.dir` 为绝对路径时原样使用；相对路径或未配置时解析到应用根目录下
/// （默认 `app_dir/log`）。`init_tracing` 与 [`AppDir`] 共用，保证写入位置与报告一致。
pub fn resolve_log_dir(app_dir: &Path, config: &AppConfig) -> PathBuf {
    match &config.log.dir {
        Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => app_dir.join(dir),
        None => app_dir.join("log"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::{
        BrowserConfig, CompactConfig, GoalConfig, LlmConfig, LogConfig, OfficeConfig,
        SummaryConfig, WebRenderConfig, WebSearchConfig,
    };

    fn cfg(log_dir: Option<String>, workspace: &str) -> AppConfig {
        AppConfig {
            log: LogConfig {
                level: "info".into(),
                dir: log_dir,
            },
            workspace: workspace.into(),
            llm: LlmConfig::default(),
            summary: SummaryConfig::default(),
            compact: CompactConfig::default(),
            goal: GoalConfig::default(),
            web_render: WebRenderConfig::default(),
            browser: BrowserConfig::default(),
            office: OfficeConfig::default(),
            web_search: WebSearchConfig::default(),
            security: Default::default(),
            memory: Default::default(),
            instructions: Default::default(),
            locale: "zh-CN".into(),
        }
    }

    #[test]
    fn app_dir_composition() {
        let d = AppDir::new(&cfg(None, "/tmp/my-user-root"));
        // 系统侧：config/ data/ log/ plugins/ skills/ electron/ 扁平分层
        assert_eq!(d.config_dir, d.app_dir.join("config"));
        assert_eq!(d.config_file, d.config_dir.join("config.json"));
        assert_eq!(d.data_dir, d.app_dir.join("data"));
        assert_eq!(d.data_file, d.data_dir.join("sqlite.db"));
        assert_eq!(d.log_dir, d.app_dir.join("log"));
        assert_eq!(d.log_file, d.log_dir.join("astorm.log"));
        assert_eq!(d.plugins_dir, d.app_dir.join("plugins"));
        assert_eq!(d.skills_dir, d.app_dir.join("skills"));
        assert_eq!(d.electron_dir, d.app_dir.join("electron"));
        // 用户侧：设置目录下 workspace/ + schedule/ + knowledge/ + memory/
        assert_eq!(d.user_dir, PathBuf::from("/tmp/my-user-root"));
        assert_eq!(d.workspace_dir, d.user_dir.join("workspace"));
        assert_eq!(d.schedule_dir, d.user_dir.join("schedule"));
        assert_eq!(d.knowledge_dir, d.user_dir.join("knowledge"));
        assert_eq!(d.memory_dir, d.user_dir.join("memory"));
    }
}
