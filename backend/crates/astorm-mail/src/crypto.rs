//! AES-256-GCM encryption for account secrets.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

pub const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Generate a random 256-bit key.
pub fn generate_key() -> Result<Vec<u8>> {
    let mut key = vec![0u8; KEY_LEN];
    getrandom::fill(&mut key).context("generate key")?;
    Ok(key)
}

/// Encrypt plaintext, returning base64(nonce || ciphertext).
pub fn encrypt(key: &[u8], plaintext: &str) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key).context("init cipher")?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).context("generate nonce")?;
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), plaintext.as_bytes())
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(B64.encode(out))
}

/// Decrypt base64(nonce || ciphertext).
pub fn decrypt(key: &[u8], enc: &str) -> Result<String> {
    let data = B64.decode(enc).context("decode encrypted data")?;
    anyhow::ensure!(
        data.len() > NONCE_LEN,
        "encrypted data too short / corrupted"
    );
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(key).context("init cipher")?;
    let plaintext = cipher
        .decrypt(&Nonce::from(nonce), ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed (wrong key or corrupted data)"))?;
    String::from_utf8(plaintext).context("decrypted data is not valid utf-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = generate_key().unwrap();
        let enc = encrypt(&key, "hunter2 密码 ✓").unwrap();
        assert_ne!(enc, "hunter2 密码 ✓");
        assert_eq!(decrypt(&key, &enc).unwrap(), "hunter2 密码 ✓");
    }

    #[test]
    fn wrong_key_fails() {
        let k1 = generate_key().unwrap();
        let k2 = generate_key().unwrap();
        let enc = encrypt(&k1, "secret").unwrap();
        assert!(decrypt(&k2, &enc).is_err());
    }

    #[test]
    fn tampered_data_fails() {
        let key = generate_key().unwrap();
        let enc = encrypt(&key, "secret").unwrap();
        let mut raw = B64.decode(enc).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        assert!(decrypt(&key, &B64.encode(raw)).is_err());
    }
}
