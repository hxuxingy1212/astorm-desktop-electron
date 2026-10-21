//! read_file_content：读取文件内容。

use astorm_common::file::File;
use napi_derive::napi;

/// 读取文件内容（1MB 上限；二进制扩展名按约定处理）。
#[napi]
pub fn read_file_content(path: String) -> napi::Result<String> {
    crate::app()?
        .file()
        .read_file_content(path)
        .map_err(|e| napi::Error::from_reason(e))
}