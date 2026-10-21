//! 本地账号存储（密码 AES-256-GCM 加密落盘）。
//!
//! 目录由调用方注入（应用数据目录下的 `mail/`），内含：
//! - `key`：32 字节主密钥（0600，首次使用时创建）
//! - `accounts.json`：账户列表，密码字段为密文（0600）

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::crypto;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    #[default]
    Ssl,
    StartTls,
    None,
}

impl TlsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TlsMode::Ssl => "ssl",
            TlsMode::StartTls => "starttls",
            TlsMode::None => "none",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub username: String,
    pub imap_host: String,
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    #[serde(default)]
    pub imap_tls: TlsMode,
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_tls: TlsMode,
    #[serde(default)]
    pub password_enc: String,
}

fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    465
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

/// 文件系统账号库。
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create dir {}", dir.display()))?;
        make_dir_private(&dir);
        Ok(Self { dir })
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("key")
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("accounts.json")
    }

    pub fn load_or_create_key(&self) -> Result<Vec<u8>> {
        let path = self.key_path();
        if path.exists() {
            let key = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            anyhow::ensure!(
                key.len() == crypto::KEY_LEN,
                "corrupt key file {} (expected {} bytes)",
                path.display(),
                crypto::KEY_LEN
            );
            Ok(key)
        } else {
            let key = crypto::generate_key()?;
            write_private(&path, &key)?;
            Ok(key)
        }
    }

    pub fn load(&self) -> Result<ConfigFile> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(ConfigFile::default());
        }
        let data =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, cfg: &ConfigFile) -> Result<()> {
        let data = serde_json::to_string_pretty(cfg)?;
        write_private(&self.config_path(), data.as_bytes())
    }

    pub fn add_account(&self, mut acct: Account, password: &str) -> Result<()> {
        anyhow::ensure!(!acct.name.trim().is_empty(), "account name must not be empty");
        let key = self.load_or_create_key()?;
        let mut cfg = self.load()?;
        anyhow::ensure!(
            !cfg.accounts.iter().any(|a| a.name == acct.name),
            "account '{}' already exists",
            acct.name
        );
        acct.password_enc = crypto::encrypt(&key, password)?;
        let was_empty = cfg.accounts.is_empty();
        cfg.accounts.push(acct);
        if was_empty && cfg.active.is_none() {
            cfg.active = Some(cfg.accounts[0].name.clone());
        }
        self.save(&cfg)
    }

    pub fn remove_account(&self, name: &str) -> Result<bool> {
        let mut cfg = self.load()?;
        let before = cfg.accounts.len();
        cfg.accounts.retain(|a| a.name != name);
        if cfg.accounts.len() == before {
            return Ok(false);
        }
        if cfg.active.as_deref() == Some(name) {
            cfg.active = cfg.accounts.first().map(|a| a.name.clone());
        }
        self.save(&cfg)?;
        Ok(true)
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        Ok(self.load()?.accounts)
    }

    pub fn get_account(&self, name: &str) -> Result<Account> {
        self.load()?
            .accounts
            .into_iter()
            .find(|a| a.name == name)
            .with_context(|| format!("account '{}' not found", name))
    }

    /// 原地替换账户（保留位置与激活选择）。`new_password` 为 `Some` 时重新加密，
    /// 否则保留原密文。
    pub fn update_account(&self, acct: Account, new_password: Option<&str>) -> Result<()> {
        let mut cfg = self.load()?;
        let idx = cfg
            .accounts
            .iter()
            .position(|a| a.name == acct.name)
            .with_context(|| format!("account '{}' not found", acct.name))?;
        let mut updated = acct;
        if let Some(pw) = new_password {
            let key = self.load_or_create_key()?;
            updated.password_enc = crypto::encrypt(&key, pw)?;
        } else {
            updated.password_enc = cfg.accounts[idx].password_enc.clone();
        }
        cfg.accounts[idx] = updated;
        self.save(&cfg)
    }

    pub fn set_active(&self, name: &str) -> Result<()> {
        let mut cfg = self.load()?;
        anyhow::ensure!(
            cfg.accounts.iter().any(|a| a.name == name),
            "account '{}' not found",
            name
        );
        cfg.active = Some(name.to_string());
        self.save(&cfg)
    }

    pub fn active_name(&self) -> Result<Option<String>> {
        Ok(self.load()?.active)
    }

    pub fn active_account(&self) -> Result<Option<Account>> {
        let cfg = self.load()?;
        Ok(cfg
            .active
            .as_deref()
            .and_then(|n| cfg.accounts.iter().find(|a| a.name == n))
            .cloned())
    }

    pub fn decrypt_password(&self, acct: &Account) -> Result<String> {
        let key = self.load_or_create_key()?;
        crypto::decrypt(&key, &acct.password_enc)
    }
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    // 同目录临时文件（0600）+ rename：崩溃不会留下截断的 accounts.json。
    astorm_common::fsx::write_atomic_mode(path, data, Some(0o600))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn make_dir_private(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn make_dir_private(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(name: &str) -> Account {
        Account {
            name: name.to_string(),
            display_name: None,
            username: format!("{name}@example.com"),
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 465,
            smtp_tls: TlsMode::Ssl,
            password_enc: String::new(),
        }
    }

    #[test]
    fn add_list_remove_active_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(dir.path()).unwrap();

        st.add_account(acct("a"), "secret-a").unwrap();
        st.add_account(acct("b"), "secret-b").unwrap();
        assert_eq!(st.list_accounts().unwrap().len(), 2);
        assert_eq!(st.active_name().unwrap().as_deref(), Some("a"));

        st.set_active("b").unwrap();
        assert_eq!(st.active_account().unwrap().unwrap().name, "b");

        assert_eq!(
            st.decrypt_password(&st.get_account("a").unwrap()).unwrap(),
            "secret-a"
        );
        assert_eq!(
            st.decrypt_password(&st.get_account("b").unwrap()).unwrap(),
            "secret-b"
        );

        assert!(st.remove_account("a").unwrap());
        assert_eq!(st.active_name().unwrap().as_deref(), Some("b"));
        assert!(!st.remove_account("a").unwrap());
    }

    #[test]
    fn duplicate_name_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(dir.path()).unwrap();
        st.add_account(acct("dup"), "p1").unwrap();
        assert!(st.add_account(acct("dup"), "p2").is_err());
    }

    #[test]
    fn update_account_keeps_or_replaces_password() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(dir.path()).unwrap();
        st.add_account(acct("a"), "secret-a").unwrap();

        let mut a = st.get_account("a").unwrap();
        a.display_name = Some("Alice".to_string());
        st.update_account(a.clone(), None).unwrap();
        let cur = st.get_account("a").unwrap();
        assert_eq!(cur.display_name.as_deref(), Some("Alice"));
        assert_eq!(st.decrypt_password(&cur).unwrap(), "secret-a");

        st.update_account(a, Some("new-pass")).unwrap();
        let cur = st.get_account("a").unwrap();
        assert_eq!(cur.display_name.as_deref(), Some("Alice"));
        assert_eq!(st.decrypt_password(&cur).unwrap(), "new-pass");
        assert_eq!(st.active_name().unwrap().as_deref(), Some("a"));
    }

    #[test]
    fn update_account_unknown_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(dir.path()).unwrap();
        st.add_account(acct("a"), "p").unwrap();
        assert!(st.update_account(acct("nope"), None).is_err());
    }

    #[test]
    fn set_active_unknown_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(dir.path()).unwrap();
        st.add_account(acct("a"), "p").unwrap();
        assert!(st.set_active("nope").is_err());
    }

    #[test]
    fn passwords_are_encrypted_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(dir.path()).unwrap();
        st.add_account(acct("x"), "plaintext-secret").unwrap();
        let raw = fs::read_to_string(st.config_path()).unwrap();
        assert!(!raw.contains("plaintext-secret"));
    }
}
