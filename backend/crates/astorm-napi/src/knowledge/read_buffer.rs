//! kb_read_buffer：读取文档字节（前端预览组件用,保留原始多色/二进制结构）。

use astorm_common::knowledge::Knowledge;
use napi::bindgen_prelude::Buffer;
use napi_derive::napi;

/// 读取文档字节（>50MB 拒绝）。
#[napi]
pub fn kb_read_buffer(path: String) -> napi::Result<Buffer> {
    crate::app()?
        .knowledge()
        .read_bytes(path)
        .map(Buffer::from)
        .map_err(|e| napi::Error::from_reason(e))
}
