//! git_commit：提交（先暂存 files 再 commit）。

use astorm_common::git::Git;
use napi_derive::napi;

/// 提交（先暂存 files 再 commit message）。
#[napi]
pub fn git_commit(dir: String, message: String, files: Vec<String>) -> napi::Result<String> {
    crate::app()?
        .git()
        .git_commit(dir, message, files)
        .map_err(|e| napi::Error::from_reason(e))
}