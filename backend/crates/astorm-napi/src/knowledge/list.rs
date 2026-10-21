//! kb_list：列出知识库整棵树（真实文件系统 + 元数据合并）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

use crate::knowledge::dto::KbItem;

/// 列出知识库整棵树（文件夹在前、名称排序；根缺失时自动初始化）。
#[napi]
pub fn kb_list() -> napi::Result<Vec<KbItem>> {
    crate::app()?
        .knowledge()
        .list()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
