//! 账户构建：合并预设默认值与显式覆盖，产出可入库的 [`Account`]。

use anyhow::{Context, Result};

use crate::preset;
use crate::store::{Account, TlsMode};

pub fn parse_tls(s: &str) -> Result<TlsMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "ssl" | "tls" => Ok(TlsMode::Ssl),
        "starttls" | "start-tls" => Ok(TlsMode::StartTls),
        "none" | "plain" => Ok(TlsMode::None),
        _ => Err("expected one of: ssl | starttls | none".to_string()),
    }
}

/// 原始输入（CLI `account add` 与前端 `mail_add_account` 共用同一套逻辑）。
pub struct AddFields {
    pub preset: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
    /// 密码 / 授权码；`None` 时回退环境变量 `MAIL_PROXY_PASSWORD`。
    pub password: Option<String>,
    pub display_name: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub imap_tls: Option<TlsMode>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_tls: Option<TlsMode>,
}

fn need<T>(v: &Option<T>, field: &str, missing: &mut Vec<String>) {
    if v.is_none() {
        missing.push(field.to_string());
    }
}

/// 合并预设与显式覆盖 + 默认值，产出 [`Account`] 与明文密码。
pub fn build_account(fields: AddFields) -> Result<(Account, String)> {
    let preset_cfg = match &fields.preset {
        Some(n) => Some(preset::lookup(n).with_context(|| {
            format!(
                "unknown preset '{n}' (available: {})",
                preset::names().join(", ")
            )
        })?),
        None => None,
    };

    let mut missing: Vec<String> = Vec::new();
    let imap_host = fields
        .imap_host
        .clone()
        .or_else(|| preset_cfg.map(|x| x.imap_host.to_string()));
    let smtp_host = fields
        .smtp_host
        .clone()
        .or_else(|| preset_cfg.map(|x| x.smtp_host.to_string()));
    need(&fields.name, "--name", &mut missing);
    need(&fields.username, "--user", &mut missing);
    need(&imap_host, "--imap-host (or --preset)", &mut missing);
    need(&smtp_host, "--smtp-host (or --preset)", &mut missing);
    let password = fields
        .password
        .clone()
        .or_else(|| std::env::var("MAIL_PROXY_PASSWORD").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if password.is_none() {
        missing.push("--password or MAIL_PROXY_PASSWORD".to_string());
    }

    anyhow::ensure!(
        missing.is_empty(),
        "missing required args: {}",
        missing.join(", ")
    );

    let name = fields.name.unwrap();
    let acct = Account {
        name: name.clone(),
        display_name: fields
            .display_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        username: fields.username.unwrap(),
        imap_host: imap_host.unwrap(),
        imap_port: fields
            .imap_port
            .or(preset_cfg.map(|x| x.imap_port))
            .unwrap_or(993),
        imap_tls: fields
            .imap_tls
            .or(preset_cfg.map(|x| x.imap_tls))
            .unwrap_or(TlsMode::Ssl),
        smtp_host: smtp_host.unwrap(),
        smtp_port: fields
            .smtp_port
            .or(preset_cfg.map(|x| x.smtp_port))
            .unwrap_or(465),
        smtp_tls: fields
            .smtp_tls
            .or(preset_cfg.map(|x| x.smtp_tls))
            .unwrap_or(TlsMode::Ssl),
        password_enc: String::new(),
    };
    Ok((acct, password.unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> AddFields {
        AddFields {
            preset: Some("qq".to_string()),
            name: Some(name.to_string()),
            username: Some(format!("{name}@qq.com")),
            password: Some("authcode".to_string()),
            display_name: None,
            imap_host: None,
            imap_port: None,
            imap_tls: None,
            smtp_host: None,
            smtp_port: None,
            smtp_tls: None,
        }
    }

    #[test]
    fn build_account_applies_preset_defaults() {
        let (acct, password) = build_account(base("a")).unwrap();
        assert_eq!(acct.imap_host, "imap.qq.com");
        assert_eq!(acct.smtp_host, "smtp.qq.com");
        assert_eq!(acct.imap_port, 993);
        assert_eq!(acct.smtp_port, 465);
        assert_eq!(acct.imap_tls, TlsMode::Ssl);
        assert_eq!(acct.username, "a@qq.com");
        assert_eq!(password, "authcode");
    }

    #[test]
    fn build_account_overrides_preset() {
        let mut f = base("b");
        f.imap_port = Some(1993);
        f.smtp_host = Some("alt.example.com".to_string());
        f.imap_tls = Some(TlsMode::StartTls);
        let (acct, _) = build_account(f).unwrap();
        assert_eq!(acct.imap_port, 1993);
        assert_eq!(acct.smtp_host, "alt.example.com");
        assert_eq!(acct.imap_tls, TlsMode::StartTls);
    }

    #[test]
    fn build_account_requires_missing_fields() {
        let mut f = base("c");
        f.username = None;
        let err = build_account(f).unwrap_err().to_string();
        assert!(err.contains("--user"));
    }
}
