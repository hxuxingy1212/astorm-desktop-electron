//! 非 macOS / Windows 平台的占位解密器（Linux 等）。
//!
//! Linux 上 Chrome 用 Secret Service（gnome-keyring / kwallet）或固定口令 `peanuts`，
//! 本版本未实现。这里显式返回「不支持」而不是编译失败，保证：
//! - 平台无关的代码与单测可在任意开发机编译运行；
//! - 用户拿到的是明确错误，而不是"导入 0 条"这种误导性成功。

use std::path::Path;

use super::DecryptError;

pub struct PlatformDecryptor;

impl PlatformDecryptor {
    pub fn for_profile(_profile_dir: &Path) -> Result<Self, DecryptError> {
        Err(DecryptError::Unsupported(
            "当前平台暂不支持读取本机 Chrome Cookie（已支持 macOS / Windows）".to_string(),
        ))
    }

    pub fn decrypt(&self, _host_key: &str, _blob: &[u8]) -> Result<String, DecryptError> {
        Err(DecryptError::Unsupported(
            "当前平台暂不支持读取本机 Chrome Cookie".to_string(),
        ))
    }
}
