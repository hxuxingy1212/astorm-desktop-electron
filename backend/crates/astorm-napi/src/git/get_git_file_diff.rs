//! get_git_file_diff：单个文件差异全文 + 新侧总行数。

use astorm_common::git::Git;
use napi_derive::napi;

use crate::git::dto::DiffFileContent;

/// 单个文件差异全文 + 新侧总行数。`staged = true` 取索引与 HEAD 的差异。
#[napi]
pub fn get_git_file_diff(dir: String, path: String, staged: bool) -> napi::Result<DiffFileContent> {
    crate::app()?
        .git()
        .get_git_file_diff(dir, path, staged)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}