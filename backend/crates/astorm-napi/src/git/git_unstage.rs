//! git_unstage：取消暂存指定文件。

use astorm_common::git::Git;
use napi_derive::napi;

/// 取消暂存指定文件。
#[napi]
pub fn git_unstage(dir: String, files: Vec<String>) -> napi::Result<String> {
    crate::app()?
        .git()
        .git_unstage(dir, files)
        .map_err(|e| napi::Error::from_reason(e))
}