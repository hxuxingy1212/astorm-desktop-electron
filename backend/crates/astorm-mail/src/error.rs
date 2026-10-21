//! 邮件服务错误（跨 FFI / 工具边界统一为可读文本）。

use std::fmt;

/// 邮件服务错误：只保留人类可读消息，便于转成 napi error / 工具结果字符串。
#[derive(Debug, Clone)]
pub struct MailError {
    pub message: String,
}

impl MailError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MailError {}

impl From<anyhow::Error> for MailError {
    fn from(e: anyhow::Error) -> Self {
        // `{:#}` 展开 anyhow 的 context 链，保留完整因果
        Self::new(format!("{e:#}"))
    }
}

impl From<String> for MailError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for MailError {
    fn from(message: &str) -> Self {
        Self::new(message.to_string())
    }
}

/// 邮件服务结果别名。
pub type MailResult<T> = Result<T, MailError>;
