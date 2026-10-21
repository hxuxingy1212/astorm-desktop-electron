//! Windows Chrome Cookie 解密。
//!
//! 主密钥由 Chrome 随机生成后用 **DPAPI** 保护，存在 `<User Data>/Local State` 的
//! `os_crypt.encrypted_key`（base64，去掉 `DPAPI` 前缀后是 DPAPI 密文）。解出的是
//! 32 字节 AES-256-GCM 密钥；cookie 密文形如 `v10` + 12 字节 nonce + 密文 + 16 字节 tag。
//!
//! Chrome 127+ 起，部分 cookie 改用 **app-bound encryption（`v20`）**，密钥由 Chrome
//! 的提权服务派生，第三方无法解密 —— 这类条目返回 [`DecryptError::Unsupported`]，
//! 由上层计数并如实上报，不做静默丢弃。
//!
//! 注意：本模块只在 Windows 上编译；开发机为 macOS 时其正确性只能靠真机验证。

use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

use super::{DecryptError, blob_version, strip_domain_prefix, unpad_pkcs7};

/// AES-256-GCM 密钥长度。
const KEY_LEN: usize = 32;
/// GCM nonce 长度。
const NONCE_LEN: usize = 12;
/// GCM 认证标签长度。
const TAG_LEN: usize = 16;
/// DPAPI 后的密钥在 `Local State` 里带这个 ASCII 前缀。
const DPAPI_PREFIX: &[u8] = b"DPAPI";

pub struct PlatformDecryptor {
    cipher: Aes256Gcm,
}

impl PlatformDecryptor {
    /// `profile_dir` 为 `<User Data>/<Profile>`，其父目录下的 `Local State` 存放密钥。
    pub fn for_profile(profile_dir: &Path) -> Result<Self, DecryptError> {
        let root = profile_dir.parent().ok_or_else(|| {
            DecryptError::Failed(format!("Profile 目录异常: {}", profile_dir.display()))
        })?;
        let key = read_master_key(&root.join("Local State"))?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| DecryptError::Failed(format!("初始化 AES-GCM 失败: {e}")))?;
        Ok(Self { cipher })
    }

    /// 解密一条 Cookie 的密文，返回明文值。
    pub fn decrypt(&self, host_key: &str, blob: &[u8]) -> Result<String, DecryptError> {
        if blob.is_empty() {
            return Ok(String::new());
        }
        match blob_version(blob) {
            // Chrome 127+ 的 app-bound 加密:密钥在 Chrome 的提权服务里,第三方拿不到
            Some("v20") | Some("v21") => Err(DecryptError::Unsupported(format!(
                "Chrome app-bound 加密({})，需要 Chrome 提权服务",
                String::from_utf8_lossy(&blob[..3])
            ))),
            Some(_) => {
                let plain = self.decrypt_gcm(&blob[3..])?;
                let plain = strip_domain_prefix(host_key, unpad_pkcs7(&plain));
                Ok(String::from_utf8_lossy(plain).into_owned())
            }
            // 无版本标记的旧格式:整段密文交给 DPAPI
            None => {
                let plain = dpapi_unprotect(blob)?;
                let plain = strip_domain_prefix(host_key, unpad_pkcs7(&plain));
                Ok(String::from_utf8_lossy(plain).into_owned())
            }
        }
    }

    /// `v10` 分支：12 字节 nonce + 密文 + 16 字节 tag。
    fn decrypt_gcm(&self, body: &[u8]) -> Result<Vec<u8>, DecryptError> {
        if body.len() < NONCE_LEN + TAG_LEN {
            return Err(DecryptError::Failed(format!(
                "GCM 密文过短: {} 字节",
                body.len()
            )));
        }
        let (nonce, rest) = body.split_at(NONCE_LEN);
        let (ciphertext, tag) = rest.split_at(rest.len() - TAG_LEN);
        // aes-gcm 约定 ciphertext||tag 连续存放
        let mut sealed = ciphertext.to_vec();
        sealed.extend_from_slice(tag);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), sealed.as_ref())
            .map_err(|_| DecryptError::Failed("AES-GCM 认证失败（密钥不匹配或密文损坏）".to_string()))
    }
}

/// 从 `Local State` 取出并用 DPAPI 解开 32 字节 AES 主密钥。
fn read_master_key(local_state: &Path) -> Result<Vec<u8>, DecryptError> {
    use base64::Engine as _;

    let raw = std::fs::read_to_string(local_state)
        .map_err(|e| DecryptError::Failed(format!("读取 {} 失败: {e}", local_state.display())))?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| DecryptError::Failed(format!("解析 Local State 失败: {e}")))?;
    let encoded = json
        .get("os_crypt")
        .and_then(|v| v.get("encrypted_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| DecryptError::Failed("Local State 缺少 os_crypt.encrypted_key".to_string()))?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| DecryptError::Failed(format!("encrypted_key 不是合法 base64: {e}")))?;
    let payload = blob
        .strip_prefix(DPAPI_PREFIX)
        .ok_or_else(|| DecryptError::Failed("encrypted_key 缺少 DPAPI 前缀".to_string()))?;
    let key = dpapi_unprotect(payload)?;
    if key.len() != KEY_LEN {
        return Err(DecryptError::Failed(format!(
            "主密钥长度异常: {}（期望 {KEY_LEN}）",
            key.len()
        )));
    }
    Ok(key)
}

/// 调用 DPAPI 解保护一段数据。
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>, DecryptError> {
    if blob.is_empty() {
        return Err(DecryptError::Failed("DPAPI 输入为空".to_string()));
    }
    // 入参数组必须可变指针(API 不写入,但签名如此)
    let mut input_buf = blob.to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: input_buf.len() as u32,
        pbData: input_buf.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: 输入指针指向本函数栈上、生命周期覆盖调用的缓冲区;output 由 API 分配、
    // 下面用 LocalFree 释放,并在释放前完成拷贝。
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
            .map_err(|e| DecryptError::Failed(format!("DPAPI 解密失败: {e}")))?;
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        if !output.pbData.is_null() {
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        }
        Ok(plain)
    }
}
