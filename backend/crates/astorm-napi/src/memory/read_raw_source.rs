//! memory_read_raw_source：读取单个源会话（归一化后的 Markdown）。

use napi_derive::napi;

/// 读取源会话原文（`stem` 为 `raw/sessions/<stem>.md` 的文件名主干）。
#[napi]
pub fn memory_read_raw_source(stem: String) -> napi::Result<String> {
    crate::app()?
        .memory()
        .read_raw_source(&stem)
        .map_err(|e| napi::Error::from_reason(e))
}
