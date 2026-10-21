//! get_knowledge_root：当前知识库根目录（markdown 相对资源经 astorm-kb:// 协议取用）。

use napi_derive::napi;

/// 返回当前知识库根目录绝对路径。
#[napi]
pub fn get_knowledge_root() -> napi::Result<String> {
    Ok(crate::app()?.knowledge().root().to_string_lossy().into_owned())
}
