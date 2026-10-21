//! kb_update_meta：更新条目展示元数据（图标 / 颜色 / 标签）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

use crate::knowledge::dto::{KbItem, KbMetaUpdate};

/// 更新展示元数据（不影响文件系统，仅写清单）。
#[napi]
pub fn kb_update_meta(meta: KbMetaUpdate) -> napi::Result<KbItem> {
    crate::app()?
        .knowledge()
        .update_meta(meta.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
