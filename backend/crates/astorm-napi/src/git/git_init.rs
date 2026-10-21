//! git_init：初始化仓库。

use astorm_common::git::Git;
use napi_derive::napi;

/// 初始化仓库。
#[napi]
pub fn git_init(dir: String) -> napi::Result<String> {
    crate::app()?
        .git()
        .git_init(dir)
        .map_err(|e| napi::Error::from_reason(e))
}