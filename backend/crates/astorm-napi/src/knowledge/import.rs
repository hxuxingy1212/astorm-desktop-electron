//! kb_import：从本地磁盘导入文件 / 目录到知识库（真实拷贝）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

use crate::knowledge::dto::KbItem;

/// 导入本地文件 / 目录到指定父目录（空串 = 根层;目录递归拷贝;重名自动加序号）。
#[napi]
pub fn kb_import(parent: String, paths: Vec<String>) -> napi::Result<Vec<KbItem>> {
    crate::app()?
        .knowledge()
        .import(parent, paths)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
