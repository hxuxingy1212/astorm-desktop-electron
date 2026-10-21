//! check_git_env：git 环境检查。

use astorm_common::git::Git;
use napi_derive::napi;

use crate::git::dto::GitEnvInfo;

/// git 环境检查（是否可用 / 是否仓库内）。
#[napi]
pub fn check_git_env(dir: String) -> napi::Result<GitEnvInfo> {
    crate::app()?
        .git()
        .check_git_env(dir)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}