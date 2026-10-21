//! git_stage：暂存指定文件。

use astorm_common::git::Git;
use napi_derive::napi;

/// 暂存指定文件。
#[napi]
pub fn git_stage(dir: String, files: Vec<String>) -> napi::Result<String> {
    crate::app()?
        .git()
        .git_stage(dir, files)
        .map_err(|e| napi::Error::from_reason(e))
}