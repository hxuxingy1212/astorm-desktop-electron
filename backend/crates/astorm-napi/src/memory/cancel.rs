//! memory_cancel：取消运行中的编译。

use napi_derive::napi;

/// 取消运行中的记忆编译（已落盘内容保留；未提交的索引改动由管道自身回滚）。
///
/// 返回 `true` 表示确有任务被取消。
#[napi]
pub fn memory_cancel() -> napi::Result<bool> {
    Ok(crate::app()?.memory().cancel())
}
