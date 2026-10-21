//! kb_delete：删除条目（文件夹递归删除）。

use astorm_common::knowledge::Knowledge;
use napi_derive::napi;

/// 删除条目（文件夹递归删除；同步清理元数据；幂等）。
#[napi]
pub fn kb_delete(path: String) -> napi::Result<()> {
    crate::app()?
        .knowledge()
        .delete(path)
        .map_err(|e| napi::Error::from_reason(e))
}
