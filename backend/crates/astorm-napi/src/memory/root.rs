//! memory_root：长期记忆 Bundle 根目录。

use napi_derive::napi;

/// 返回当前记忆 Bundle 根目录绝对路径（`<用户目录>/memory`）。
#[napi]
pub fn memory_root() -> napi::Result<String> {
    Ok(crate::app()?.memory().root().to_string_lossy().into_owned())
}
