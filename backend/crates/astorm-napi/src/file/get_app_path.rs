//! get_app_path：应用可执行文件所在目录。

use astorm_common::file::File;
use napi_derive::napi;

/// 应用可执行文件所在目录。
#[napi]
pub fn get_app_path() -> napi::Result<String> {
    crate::app()?
        .file()
        .get_app_path()
        .map_err(|e| napi::Error::from_reason(e))
}