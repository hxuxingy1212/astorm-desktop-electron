//! Chrome Cookie 值解密：按平台派生密钥并解出明文。
//!
//! 各平台算法完全不同（macOS 钥匙串 + AES-128-CBC，Windows DPAPI + AES-256-GCM），
//! 因此每个平台一个子模块，对外统一暴露 [`PlatformDecryptor`]；
//! 不支持的平台由 [`unsupported`] 顶上，保证跨平台可编译、可如实报错。
//!
//! 平台无关的两件事收在本模块：
//! - 去掉明文前的 32 字节域名哈希（Chrome 130+ 的 CookieValuePrefix）；
//! - PKCS7 去填充。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::PlatformDecryptor;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::PlatformDecryptor;
#[cfg(target_os = "windows")]
pub use windows::PlatformDecryptor;

use sha2::{Digest, Sha256};

/// 单条 Cookie 解密失败的原因。
#[derive(Debug, Clone)]
pub enum DecryptError {
    /// 该 cookie 用了当前平台支持不了的加密（如 Windows 的 app-bound v20）。
    Unsupported(String),
    /// 解密过程出错（密钥不可用、密文损坏等）。
    Failed(String),
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(m) => write!(f, "不支持的加密方式: {m}"),
            Self::Failed(m) => write!(f, "解密失败: {m}"),
        }
    }
}

/// 平台解密器的统一构造入口：各平台实现为固有方法
/// `PlatformDecryptor::for_profile(profile_dir)`，签名一致，调用方无需按平台分支。
///
/// 去掉 Chrome 130+ 附在明文前的 32 字节域名哈希。
///
/// 精确路径是用 `SHA256(host_key)` 比对；但该特性只在部分版本/平台开启，且历史实现
/// 用「域」而非 `host_key` 做哈希，故再加一条启发式兜底：前 32 字节不是合法 UTF-8
/// 而其后是 —— cookie 值本身几乎总是可打印 ASCII，出现这种形态只可能是哈希前缀。
pub fn strip_domain_prefix<'a>(host_key: &str, plain: &'a [u8]) -> &'a [u8] {
    if plain.len() <= 32 {
        return plain;
    }
    let digest = Sha256::digest(host_key.as_bytes());
    if plain.starts_with(&digest[..]) {
        return &plain[32..];
    }
    if std::str::from_utf8(&plain[..32]).is_err() && std::str::from_utf8(&plain[32..]).is_ok() {
        return &plain[32..];
    }
    plain
}

/// PKCS7 去填充；填充非法时原样返回（个别版本不加填充或长度不整）。
pub fn unpad_pkcs7(data: &[u8]) -> &[u8] {
    let Some(&last) = data.last() else {
        return data;
    };
    let pad = last as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return data;
    }
    if data[data.len() - pad..].iter().all(|&b| b as usize == pad) {
        &data[..data.len() - pad]
    } else {
        data
    }
}

/// 密文前 3 字节的版本标记（`v10` / `v11` / `v20`…）。
pub fn blob_version(blob: &[u8]) -> Option<&str> {
    let tagged = blob.len() >= 3 && blob[0] == b'v' && blob[1].is_ascii_digit() && blob[2].is_ascii_digit();
    tagged.then(|| std::str::from_utf8(&blob[..3]).ok())?
}

/// 去掉版本标记，返回密文本体。
pub fn blob_body(blob: &[u8]) -> &[u8] {
    match blob_version(blob) {
        Some(_) => &blob[3..],
        None => blob,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkcs7_padding_is_stripped() {
        assert_eq!(unpad_pkcs7(b"abc\x02\x02"), b"abc");
        // 整块 16 字节：1 字节数据 + 15 字节填充
        let mut block = b"a".to_vec();
        block.extend(std::iter::repeat_n(15u8, 15));
        assert_eq!(unpad_pkcs7(&block), b"a");
        // 长度 16 的整块填充 → 全空
        let full: Vec<u8> = vec![16u8; 16];
        assert_eq!(unpad_pkcs7(&full), b"");
        // 非法填充原样返回
        assert_eq!(unpad_pkcs7(b"abc\x03\x04"), b"abc\x03\x04");
        // 填充字节数超过数据长度同样视为非法(不 panic、不越界)
        assert_eq!(unpad_pkcs7(b"a\x0f\x0f\x0f"), b"a\x0f\x0f\x0f");
        assert_eq!(unpad_pkcs7(b""), b"");
    }

    #[test]
    fn domain_prefix_is_stripped_by_exact_hash() {
        let host = ".example.com";
        let mut plain = Sha256::digest(host.as_bytes()).to_vec();
        plain.extend_from_slice(b"session-token");
        assert_eq!(strip_domain_prefix(host, &plain), b"session-token");
    }

    #[test]
    fn domain_prefix_falls_back_to_heuristic() {
        // 前缀不是 SHA256(host_key)(例如历史实现哈希的是域),但形态符合
        let mut plain = vec![0xffu8; 32];
        plain.extend_from_slice(b"value");
        assert_eq!(strip_domain_prefix("example.com", &plain), b"value");
    }

    #[test]
    fn plain_values_are_left_alone() {
        // 纯值:不误伤
        let short = b"just-a-value";
        assert_eq!(strip_domain_prefix("example.com", short), short);
        let long = b"a-fairly-long-cookie-value-that-exceeds-32-bytes";
        assert_eq!(strip_domain_prefix("example.com", long), long);
    }

    #[test]
    fn blob_version_and_body() {
        assert_eq!(blob_version(b"v10abc"), Some("v10"));
        assert_eq!(blob_body(b"v10abc"), b"abc");
        assert_eq!(blob_version(b"ab"), None);
        // 无版本标记:整体即密文
        assert_eq!(blob_body(b"\x01\x02\x03\x04"), b"\x01\x02\x03\x04");
    }
}
