//! kb_write：写入文档内容。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

/// 写入文档内容（目标必须是文档）。
#[napi]
pub fn kb_write(path: String, content: String) -> napi::Result<()> {
    crate::app()?
        .knowledge()
        .write(path, content)
        .map_err(|e| napi::Error::from_reason(e))
}
