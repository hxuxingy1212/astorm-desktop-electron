//! 配置热更新：供应商 / 模型等运行时可改配置，不中断会话（见 astorm_core 的
//! [`astorm_core::app::App::update_config`]）。

use astorm_core::app::{App, AppConfig};
use napi_derive::napi;

/// 热更新应用配置。
///
/// 当前生效维度：**LLM 供应商 / 模型 / 默认供应商**（整体替换注册表）。
/// 进行中会话的下一轮推理立即读到新配置，无需 shutdown（不中断会话、不重开存储）。
#[napi]
pub fn update_config(config: String) -> napi::Result<()> {
    let app_config = AppConfig::from_json(&config)
        .map_err(|e| napi::Error::new(napi::Status::InvalidArg, e))?;
    App::update_config(app_config).map_err(|e| napi::Error::from_reason(e))?;
    Ok(())
}