//! kb_create：新建文件夹 / 文档（真实落盘）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

use crate::knowledge::dto::{KbCreateRequest, KbItem};

/// 新建文件夹 / 文档（文档缺扩展名自动补 `.md`）。
#[napi]
pub fn kb_create(req: KbCreateRequest) -> napi::Result<KbItem> {
    crate::app()?
        .knowledge()
        .create(req.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
