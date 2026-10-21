//! memory_validate：OKF v0.2 合规校验。

use napi_derive::napi;

use crate::memory::dto::MemoryValidateReport;

/// 校验 Bundle 合规性（Error = 破坏 OKF 契约；Warning = 建议修正）。
#[napi]
pub fn memory_validate() -> napi::Result<MemoryValidateReport> {
    Ok(crate::app()?.memory().validate().into())
}
