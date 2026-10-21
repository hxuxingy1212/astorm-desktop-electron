//! kb_rename：重命名（文件夹同步迁移后代元数据）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

use crate::knowledge::dto::KbItem;

/// 重命名条目（相对路径变更时同步迁移清单）。
#[napi]
pub fn kb_rename(path: String, new_name: String) -> napi::Result<KbItem> {
    crate::app()?
        .knowledge()
        .rename(path, new_name)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
