//! git_status：工作区状态（staged / unstaged）。

use astorm_common::git::Git;
use napi_derive::napi;

use crate::git::dto::GitStatus;

/// 工作区状态（暂存 / 未暂存）。
#[napi]
pub fn git_status(dir: String) -> napi::Result<GitStatus> {
    crate::app()?
        .git()
        .git_status(dir)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}