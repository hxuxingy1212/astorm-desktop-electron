//! get_git_diff_summary：差异摘要（文件列表 + 增删计数）。

use astorm_common::git::Git;
use napi_derive::napi;

use crate::git::dto::DiffFileEntry;

/// 差异摘要。`staged = true` 取索引与 HEAD 的差异；未跟踪文件只在 `staged = false` 时计入。
#[napi]
pub fn get_git_diff_summary(dir: String, staged: bool) -> napi::Result<Vec<DiffFileEntry>> {
    crate::app()?
        .git()
        .get_git_diff_summary(dir, staged)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}