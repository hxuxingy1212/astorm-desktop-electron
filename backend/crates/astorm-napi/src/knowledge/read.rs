//! kb_read：读取文档内容（>1MB 拒绝）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

/// 读取文档内容。
#[napi]
pub fn kb_read(path: String) -> napi::Result<String> {
    crate::app()?
        .knowledge()
        .read(path)
        .map_err(|e| napi::Error::from_reason(e))
}
