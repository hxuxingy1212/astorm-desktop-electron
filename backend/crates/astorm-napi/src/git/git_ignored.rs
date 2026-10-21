//! git_ignored：被 git 忽略的路径列表（文件树淡色渲染用）。

use astorm_common::git::Git;
use napi_derive::napi;

/// 被 git 忽略的路径列表（相对仓库根;目录如 node_modules）。
#[napi]
pub fn git_ignored(dir: String) -> napi::Result<Vec<String>> {
    crate::app()?
        .git()
        .git_ignored(dir)
        .map_err(|e| napi::Error::from_reason(e))
}