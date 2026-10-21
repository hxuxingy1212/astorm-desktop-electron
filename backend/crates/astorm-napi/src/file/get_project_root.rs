//! get_project_root：项目根目录（当前工作目录）。

use astorm_common::file::File;
use napi_derive::napi;

/// 项目根目录（当前工作目录）。
#[napi]
pub fn get_project_root() -> napi::Result<String> {
    crate::app()?
        .file()
        .get_project_root()
        .map_err(|e| napi::Error::from_reason(e))
}