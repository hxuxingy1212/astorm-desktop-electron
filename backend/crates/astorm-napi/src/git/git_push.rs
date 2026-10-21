//! git_push：推送（SSH 密钥认证）。

use astorm_common::git::Git;
use napi_derive::napi;

/// 推送（SSH 密钥认证）。
#[napi]
pub fn git_push(dir: String) -> napi::Result<String> {
    crate::app()?
        .git()
        .git_push(dir)
        .map_err(|e| napi::Error::from_reason(e))
}