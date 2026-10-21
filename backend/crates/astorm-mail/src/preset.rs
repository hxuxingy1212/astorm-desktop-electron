//! 邮箱服务商预设（IMAP/SMTP 主机、端口、TLS）。

use crate::store::TlsMode;

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub imap_host: &'static str,
    pub imap_port: u16,
    pub imap_tls: TlsMode,
    pub smtp_host: &'static str,
    pub smtp_port: u16,
    pub smtp_tls: TlsMode,
}

pub const PRESETS: &[(&str, Preset)] = &[
    (
        "qq",
        Preset {
            imap_host: "imap.qq.com",
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.qq.com",
            smtp_port: 465,
            smtp_tls: TlsMode::Ssl,
        },
    ),
    (
        "163",
        Preset {
            imap_host: "imap.163.com",
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.163.com",
            smtp_port: 465,
            smtp_tls: TlsMode::Ssl,
        },
    ),
    (
        "126",
        Preset {
            imap_host: "imap.126.com",
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.126.com",
            smtp_port: 465,
            smtp_tls: TlsMode::Ssl,
        },
    ),
    (
        "gmail",
        Preset {
            imap_host: "imap.gmail.com",
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.gmail.com",
            smtp_port: 465,
            smtp_tls: TlsMode::Ssl,
        },
    ),
    (
        "outlook",
        Preset {
            imap_host: "outlook.office365.com",
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.office365.com",
            smtp_port: 587,
            smtp_tls: TlsMode::StartTls,
        },
    ),
    (
        "icloud",
        Preset {
            imap_host: "imap.mail.me.com",
            imap_port: 993,
            imap_tls: TlsMode::Ssl,
            smtp_host: "smtp.mail.me.com",
            smtp_port: 587,
            smtp_tls: TlsMode::StartTls,
        },
    ),
];

pub fn lookup(name: &str) -> Option<Preset> {
    PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
}

pub fn names() -> Vec<&'static str> {
    PRESETS.iter().map(|(n, _)| *n).collect()
}
