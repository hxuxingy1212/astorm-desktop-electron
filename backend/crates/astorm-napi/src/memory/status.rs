//! memory_status：记忆库状态卡（概念数 / 待编译数 / 分类统计 / 上次编译时间）。

use napi_derive::napi;

use crate::memory::dto::MemoryStatus;

/// 读取记忆库状态（含「待编译 N 个」提示；不读会话正文、不花 token）。
#[napi]
pub fn memory_status() -> napi::Result<MemoryStatus> {
    crate::app()?
        .memory()
        .status()
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
