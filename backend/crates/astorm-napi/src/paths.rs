//! get_data_paths：系统目录布局快照（无需 start()，主进程启动早期重定向 userData /
//! 放置配置文件时使用；与 AppDir 同源，保证前后端路径一致）。

use astorm_core::app::{AppConfig, AppDir};
use napi_derive::napi;

/// 系统目录布局（camelCase）。
#[napi(object)]
pub struct DataPaths {
    /// 应用根目录（config/ data/ log/ plugins/ skills/ electron/ 的父目录）
    pub app_dir: String,
    /// 配置目录（providers.json / backend-settings.json / security.json / mcp_servers.json / config.json）
    pub config_dir: String,
    /// 数据目录（sqlite.db / session/ / index/ / skill_index/）
    pub data_dir: String,
    /// 日志目录
    pub log_dir: String,
    /// 插件目录
    pub plugins_dir: String,
    /// Chromium 内部文件目录（主进程 userData 重定向目标）
    pub electron_dir: String,
}

/// 获取系统目录布局（纯路径计算，不初始化应用）。
#[napi]
pub fn get_data_paths() -> napi::Result<DataPaths> {
    let dirs = AppDir::new(&AppConfig::default());
    Ok(DataPaths {
        app_dir: dirs.app_dir.to_string_lossy().into_owned(),
        config_dir: dirs.config_dir.to_string_lossy().into_owned(),
        data_dir: dirs.data_dir.to_string_lossy().into_owned(),
        log_dir: dirs.log_dir.to_string_lossy().into_owned(),
        plugins_dir: dirs.plugins_dir.to_string_lossy().into_owned(),
        electron_dir: dirs.electron_dir.to_string_lossy().into_owned(),
    })
}
