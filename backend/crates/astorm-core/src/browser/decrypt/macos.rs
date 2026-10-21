//! macOS Chrome Cookie 解密。
//!
//! Chrome 在 macOS 上不使用机器绑定密钥，而是一个固定口令：
//! 口令存在**登录钥匙串**（generic password，service `Chrome Safe Storage`，
//! account `Chrome`），经 `PBKDF2-HMAC-SHA1(salt="saltysalt", 1003 轮, 16 字节)`
//! 派生出 AES-128-CBC 密钥，IV 为 16 个空格；密文前 3 字节是 `v10` 版本标记。
//!
//! 首次读取钥匙串会弹出系统授权框（"astorm 想使用钥匙串中的机密信息"）。因此
//! 调用方必须走 worker 线程（见 napi 绑定），不要把同步调用放在 Electron 主线程。
//! 授权被拒 → 报 [`DecryptError::Failed`]，由上层如实提示，不静默吞掉。

use std::path::Path;

use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use security_framework::os::macos::keychain::SecKeychain;

use super::{DecryptError, blob_body, strip_domain_prefix, unpad_pkcs7};

/// 钥匙串中 Chrome 主密钥的条目名（Chrome 全平台统一用这两个常量）。
const SERVICE: &str = "Chrome Safe Storage";
const ACCOUNT: &str = "Chrome";
/// Chrome 的 PBKDF2 参数（与 Chromium `os_crypt_mac.mm` 一致）。
const SALT: &[u8] = b"saltysalt";
const ITERATIONS: u32 = 1003;
const KEY_LEN: usize = 16;
/// IV 固定为 16 个空格（Chromium 的历史约定）。
const IV: [u8; 16] = [b' '; 16];
/// AES 分组大小。
const BLOCK: usize = 16;

type Aes128Cbc = cbc::Decryptor<aes::Aes128>;

pub struct PlatformDecryptor {
    key: [u8; KEY_LEN],
}

impl PlatformDecryptor {
    /// macOS 的密钥与 Profile 无关，`profile_dir` 仅用于保持跨平台签名一致。
    pub fn for_profile(_profile_dir: &Path) -> Result<Self, DecryptError> {
        let password = keychain_password()?;
        let mut key = [0u8; KEY_LEN];
        pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), SALT, ITERATIONS, &mut key);
        Ok(Self { key })
    }

    /// 解密一条 Cookie 的密文，返回明文值。
    pub fn decrypt(&self, host_key: &str, blob: &[u8]) -> Result<String, DecryptError> {
        let body = blob_body(blob);
        if body.is_empty() {
            return Ok(String::new());
        }
        if body.len() % BLOCK != 0 {
            return Err(DecryptError::Failed(format!(
                "密文长度 {} 不是分组大小 {BLOCK} 的整数倍",
                body.len()
            )));
        }
        let mut buf = body.to_vec();
        // 用 NoPadding 只做分组解密(不做填充校验),填充交给宽松的 unpad_pkcs7:
        // 个别历史值不带合法 PKCS7 填充,标准解填充会直接报错丢掉整条 cookie。
        let plain = Aes128Cbc::new_from_slices(&self.key, &IV)
            .map_err(|e| DecryptError::Failed(format!("初始化 AES 失败: {e}")))?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|e| DecryptError::Failed(format!("AES 解密失败: {e}")))?;
        let plain = strip_domain_prefix(host_key, unpad_pkcs7(plain));
        Ok(String::from_utf8_lossy(plain).into_owned())
    }
}

/// 取出钥匙串里的 Chrome 主密钥口令。
fn keychain_password() -> Result<String, DecryptError> {
    let keychain = SecKeychain::default()
        .map_err(|e| DecryptError::Failed(format!("打开登录钥匙串失败: {e}")))?;
    let (password, _item) = keychain
        .find_generic_password(SERVICE, ACCOUNT)
        .map_err(|e| DecryptError::Failed(format!("读取钥匙串 `{SERVICE}` 失败（可能拒绝了授权）: {e}")))?;
    // 口令是 ASCII 串;用 lossy 避免个别异常字节直接失败
    Ok(String::from_utf8_lossy(&password).into_owned())
}

#[cfg(test)]
mod tests {
    use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::browser::decrypt::blob_version;

    /// 用同一套参数加密一次，验证 decrypt 的完整链路（不触碰钥匙串）。
    fn encrypt_for_test(key: &[u8; KEY_LEN], plain: &[u8]) -> Vec<u8> {
        // 手工做 PKCS7 补齐:cipher 的 alloc feature 未开启,拿不到 encrypt_padded_vec_mut
        let pad = BLOCK - plain.len() % BLOCK;
        let mut buf = plain.to_vec();
        buf.extend(std::iter::repeat_n(pad as u8, pad));
        let msg_len = buf.len();
        cbc::Encryptor::<aes::Aes128>::new(key.into(), (&IV).into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, msg_len)
            .expect("NoPadding 加密不应失败");
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&buf);
        blob
    }

    #[test]
    fn aes_cbc_roundtrip_with_domain_prefix() {
        let key = [7u8; KEY_LEN];
        let decryptor = PlatformDecryptor { key };
        let host = ".example.com";
        let mut plain = Sha256::digest(host.as_bytes()).to_vec();
        plain.extend_from_slice(b"hello-cookie");
        let blob = encrypt_for_test(&key, &plain);
        assert_eq!(blob_version(&blob), Some("v10"));
        assert_eq!(decryptor.decrypt(host, &blob).unwrap(), "hello-cookie");
    }

    #[test]
    fn aes_cbc_roundtrip_without_prefix() {
        let key = [1u8; KEY_LEN];
        let decryptor = PlatformDecryptor { key };
        let blob = encrypt_for_test(&key, b"plain-value");
        assert_eq!(decryptor.decrypt("example.com", &blob).unwrap(), "plain-value");
    }

    #[test]
    fn wrong_key_does_not_panic() {
        let decryptor = PlatformDecryptor { key: [0u8; KEY_LEN] };
        let blob = encrypt_for_test(&[9u8; KEY_LEN], b"secret");
        // 解出乱码是预期结果;关键是不得 panic
        assert!(decryptor.decrypt("example.com", &blob).is_ok());
    }

    #[test]
    fn empty_and_malformed_blobs_are_handled() {
        let decryptor = PlatformDecryptor { key: [0u8; KEY_LEN] };
        assert_eq!(decryptor.decrypt("example.com", b"").unwrap(), "");
        assert_eq!(decryptor.decrypt("example.com", b"v10").unwrap(), "");
        // 长度不整 → 明确报错而非 panic
        assert!(matches!(
            decryptor.decrypt("example.com", b"v10xyz"),
            Err(DecryptError::Failed(_))
        ));
    }
}
