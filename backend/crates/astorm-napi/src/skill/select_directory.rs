//! select_directory：目录选择（原生对话框；接口保留，实现可能交由 Electron 端）。

use astorm_common::skill::Skill;
use napi_derive::napi;

/// 目录选择（原生对话框；接口保留，实现可能交由 Electron 端）。
#[napi]
pub fn select_directory() -> napi::Result<Vec<String>> {
    crate::app()?
        .skills()
        .select_directory()
        .map_err(|e| napi::Error::from_reason(e))
}