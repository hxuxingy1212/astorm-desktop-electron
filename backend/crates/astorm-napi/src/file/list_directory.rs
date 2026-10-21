//! list_directory：列出目录树。

use astorm_common::file::File;
use napi_derive::napi;

use crate::file::dto::FileNode;

/// 列出目录树（排除 node_modules/target/.git 等噪音目录）。
#[napi]
pub fn list_directory(dir: String) -> napi::Result<Vec<FileNode>> {
    crate::app()?
        .file()
        .list_directory(dir)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}