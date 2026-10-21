//! 应用生命周期：`start` / `shutdown` / `is_running` / `version`。
//!
//! 配置由前端以 JSON 字符串传入 [`start`]，后端解析为 [`AppConfig`]（含路径配置）
//! 后交给业务门面 [`App::init`]（日志初始化已下沉到业务层）。

use std::sync::{Arc, OnceLock, RwLock};

use astorm_common::types::Storage;
use astorm_core::app::{App, AppConfig, AppDir};
use astorm_storage::StorageManager;
use napi_derive::napi;

/// 应用生命周期标志：由 [`start`] 置位，[`shutdown`] 复位。
struct GlobalState {
    /// 是否已完成初始化（start 置位，shutdown 复位）
    initialized: bool,
    /// 是否运行中
    started: bool,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self {
            initialized: false,
            started: false,
        }
    }
}

/// 进程级全局状态单例（惰性初始化）。
static STATE: OnceLock<RwLock<GlobalState>> = OnceLock::new();

/// 获取全局状态单例。
fn read() -> &'static RwLock<GlobalState> {
    STATE.get_or_init(|| RwLock::new(GlobalState::default()))
}

/// 启动：接收前端传入的 JSON 配置，解析为 [`AppConfig`]（含路径配置）后
/// 初始化业务门面（日志 / runtime / LLM 客户端 / 事件总线 / 会话注册表 / 存储）。
///
/// 幂等：已初始化时返回 `false`。配置由前端管理：日志级别/目录、工作目录、
/// LLM 连接参数均来自该 JSON，缺失字段回退默认值。
/// 存储根目录由后端按平台约定解析（应用根目录下 `data/`，见 [`astorm_core::app::AppDir`]），
/// 无需前端传入。
#[napi]
pub fn start(config: String) -> napi::Result<bool> {
    let mut state = read()
        .write()
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;

    if state.initialized {
        return Ok(false);
    }

    // 解析完整应用配置（前端管理：名称 / 日志 / 工作目录 / LLM）
    let app_config = AppConfig::from_json(&config)
        .map_err(|e| napi::Error::new(napi::Status::InvalidArg, e))?;

    // 组合存储后端（应用根目录下 data/：sqlite 元数据 + jsonl 消息 + tantivy 索引）
    let dirs = AppDir::new(&app_config);
    let manager = StorageManager::open_default(&dirs.data_dir)
        .map_err(|e| napi::Error::from_reason(format!(
            "初始化存储失败（{}）: {e}",
            dirs.data_dir.display()
        )))?;
    let storage: Arc<dyn Storage> = Arc::new(manager);

    // 初始化业务门面（日志 / runtime / 客户端 / 事件总线 / 会话注册表 / 存储）
    App::init(app_config, storage).map_err(|e| napi::Error::from_reason(e))?;

    state.initialized = true;
    state.started = true;
    Ok(true)
}

/// 关闭：先停业务门面（强杀残存 PTY shell + 取消所有会话循环 + flush 日志），再复位标志。
#[napi]
pub fn shutdown() {
    if let Some(app) = App::try_global() {
        app.pty().kill_all();
        app.shutdown();
    }
    if let Ok(mut state) = read().write() {
        state.initialized = false;
        state.started = false;
    }
}

/// 是否运行中。
#[napi]
pub fn is_running() -> bool {
    read().read().map(|s| s.started).unwrap_or(false)
}

/// napi 层版本号。
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
