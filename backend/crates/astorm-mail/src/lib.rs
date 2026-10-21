//! astorm-mail：多账户邮件服务（IMAP 收信 + SMTP 发信 + 加密账号库 + 本地缓存）。
//!
//! 由 astorm-plugin-mail 的 `mail-core` / `mail-cli`（axum Web 层）移植而来：
//! - 无 HTTP 层：原 `web.rs` 的连接池/缓存/同步逻辑收敛为 [`service::MailService`]
//! - 数据目录由调用方注入（应用数据目录下的 `mail/`），不再固定 `~/.mail-proxy`
//! - 无 CLI 二进制：命令语义并入 [`service::MailService`] 的方法

pub mod account;
pub mod crypto;
pub mod db;
pub mod error;
pub mod imap_client;
pub mod preset;
pub mod service;
pub mod smtp_client;
pub mod store;

pub use error::{MailError, MailResult};
pub use service::{
    AccountView, AddAccountInput, AttachmentBlob, FolderCountView, MailService, MessagesQuery,
    MetaUpdate, MsgListView, SendInput, StateView, UpdateAccountInput,
};
