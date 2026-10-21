//! 邮件服务 napi 接口：账号管理 / 文件夹 / 邮件列表与正文 / 标记整理 / 附件 / 发信。
//!
//! 委托 [`astorm_core::app::AppState::mail`]（[`astorm_mail::MailService`]）；本模块只做
//! DTO 形状转换与错误映射，不含业务逻辑。

use std::collections::HashMap;

use astorm_mail::service::{
    AccountView as CoreAccountView, AddAccountInput as CoreAddAccount,
    AttachmentBlob as CoreAttachmentBlob, FolderCountView as CoreFolderCount,
    MessagesQuery as CoreMessagesQuery, MetaUpdate as CoreMetaUpdate, MsgListView as CoreMsgList,
    SendInput as CoreSendInput, StateView as CoreState, UpdateAccountInput as CoreUpdateAccount,
};
use astorm_mail::db::CachedMessage as CoreCachedMessage;
use astorm_mail::imap_client::MailDetail as CoreMailDetail;
use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use serde::Serialize;

fn err(e: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(e.to_string())
}

// ---------------------------------------------------------------------------
// 输出 DTO
// ---------------------------------------------------------------------------

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAccount {
    pub name: String,
    pub display_name: Option<String>,
    pub username: String,
    pub imap_host: String,
    pub imap_port: u32,
    pub imap_tls: String,
    pub smtp_host: String,
    pub smtp_port: u32,
    pub smtp_tls: String,
}

impl From<CoreAccountView> for MailAccount {
    fn from(a: CoreAccountView) -> Self {
        Self {
            name: a.name,
            display_name: a.display_name,
            username: a.username,
            imap_host: a.imap_host,
            imap_port: a.imap_port as u32,
            imap_tls: a.imap_tls,
            smtp_host: a.smtp_host,
            smtp_port: a.smtp_port as u32,
            smtp_tls: a.smtp_tls,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailState {
    pub active: Option<String>,
    pub presets: Vec<String>,
    pub accounts: Vec<MailAccount>,
}

impl From<CoreState> for MailState {
    fn from(s: CoreState) -> Self {
        Self {
            active: s.active,
            presets: s.presets,
            accounts: s.accounts.into_iter().map(Into::into).collect(),
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailFolderCount {
    pub folder: String,
    pub total: u32,
    pub unread: u32,
}

impl From<CoreFolderCount> for MailFolderCount {
    fn from(c: CoreFolderCount) -> Self {
        Self {
            folder: c.folder,
            total: c.total,
            unread: c.unread,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessage {
    pub uid: u32,
    pub folder: String,
    pub seen: bool,
    pub flagged: bool,
    pub pinned: bool,
    pub category: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub size: i64,
    pub ts: i64,
    pub preview: Option<String>,
}

impl From<CoreCachedMessage> for MailMessage {
    fn from(m: CoreCachedMessage) -> Self {
        Self {
            uid: m.uid,
            folder: m.folder,
            seen: m.seen,
            flagged: m.flagged,
            pinned: m.pinned,
            category: m.category,
            from: m.from,
            to: m.to,
            subject: m.subject,
            date: m.date,
            size: m.size as i64,
            ts: m.ts,
            preview: m.preview,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailList {
    pub folder: String,
    pub total: u32,
    pub unread: u32,
    pub has_more: bool,
    pub synced_at: i64,
    pub stale: bool,
    pub messages: Vec<MailMessage>,
}

impl From<CoreMsgList> for MailList {
    fn from(l: CoreMsgList) -> Self {
        Self {
            folder: l.folder,
            total: l.total,
            unread: l.unread,
            has_more: l.has_more,
            synced_at: l.synced_at,
            stale: l.stale,
            messages: l.messages.into_iter().map(Into::into).collect(),
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAttachmentMeta {
    pub filename: String,
    pub size: u32,
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailDetail {
    pub uid: u32,
    pub seen: bool,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub date: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<MailAttachmentMeta>,
    /// 正文是否因超过缓存上限（2 MiB）被截断。
    pub truncated: bool,
}

impl From<CoreMailDetail> for MailDetail {
    fn from(d: CoreMailDetail) -> Self {
        Self {
            uid: d.uid,
            seen: d.seen,
            from: d.from,
            to: d.to,
            cc: d.cc,
            subject: d.subject,
            date: d.date,
            text: d.text,
            html: d.html,
            attachments: d
                .attachments
                .into_iter()
                .map(|a| MailAttachmentMeta {
                    filename: a.filename,
                    size: a.size as u32,
                })
                .collect(),
            truncated: d.truncated,
        }
    }
}

#[napi(object)]
pub struct MailAttachmentBlob {
    pub filename: String,
    pub content_type: String,
    pub data: Buffer,
}

impl From<CoreAttachmentBlob> for MailAttachmentBlob {
    fn from(b: CoreAttachmentBlob) -> Self {
        Self {
            filename: b.filename,
            content_type: b.content_type,
            data: b.data.into(),
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailPreview {
    pub uid: u32,
    pub preview: String,
}

// ---------------------------------------------------------------------------
// 输入 DTO
// ---------------------------------------------------------------------------

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct MailAddAccount {
    pub preset: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub display_name: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u32>,
    pub imap_tls: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u32>,
    pub smtp_tls: Option<String>,
}

impl From<MailAddAccount> for CoreAddAccount {
    fn from(a: MailAddAccount) -> Self {
        CoreAddAccount {
            preset: a.preset,
            name: a.name,
            username: a.username,
            password: a.password,
            display_name: a.display_name,
            imap_host: a.imap_host,
            imap_port: a.imap_port.map(|n| n as u16),
            imap_tls: a.imap_tls,
            smtp_host: a.smtp_host,
            smtp_port: a.smtp_port.map(|n| n as u16),
            smtp_tls: a.smtp_tls,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct MailUpdateAccount {
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u32>,
    pub imap_tls: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u32>,
    pub smtp_tls: Option<String>,
}

impl From<MailUpdateAccount> for CoreUpdateAccount {
    fn from(a: MailUpdateAccount) -> Self {
        CoreUpdateAccount {
            display_name: a.display_name,
            username: a.username,
            password: a.password,
            imap_host: a.imap_host,
            imap_port: a.imap_port.map(|n| n as u16),
            imap_tls: a.imap_tls,
            smtp_host: a.smtp_host,
            smtp_port: a.smtp_port.map(|n| n as u16),
            smtp_tls: a.smtp_tls,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct MailMessagesQuery {
    pub folder: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub unread: Option<bool>,
    pub category: Option<String>,
    pub refresh: Option<bool>,
    pub account: Option<String>,
}

impl From<MailMessagesQuery> for CoreMessagesQuery {
    fn from(q: MailMessagesQuery) -> Self {
        CoreMessagesQuery {
            folder: q.folder,
            limit: q.limit.map(|n| n as usize),
            offset: q.offset.map(|n| n as usize),
            unread: q.unread,
            category: q.category,
            refresh: q.refresh,
            account: q.account,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct MailMeta {
    pub flagged: Option<bool>,
    pub pinned: Option<bool>,
    pub category: Option<String>,
    pub seen: Option<bool>,
}

impl From<MailMeta> for CoreMetaUpdate {
    fn from(m: MailMeta) -> Self {
        CoreMetaUpdate {
            flagged: m.flagged,
            pinned: m.pinned,
            category: m.category,
            seen: m.seen,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct MailSendInput {
    pub account: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub html: Option<String>,
    pub attachments: Vec<String>,
}

impl From<MailSendInput> for CoreSendInput {
    fn from(s: MailSendInput) -> Self {
        CoreSendInput {
            account: s.account,
            to: s.to,
            cc: s.cc,
            bcc: s.bcc,
            subject: s.subject,
            body: s.body,
            html: s.html,
            attachments: s.attachments,
        }
    }
}

// ---------------------------------------------------------------------------
// 接口
// ---------------------------------------------------------------------------

// 账号 CRUD 为 `async`：其内部做同步文件 IO + AES 解密，异步化后 napi 将其放到
// tokio 运行时执行，**不阻塞 JS 主线程**（`mail_state` 在邮箱页挂载时即被调用）。
#[napi]
pub async fn mail_state() -> napi::Result<MailState> {
    Ok(crate::app()?.mail().state().map_err(err)?.into())
}

#[napi]
pub async fn mail_add_account(input: MailAddAccount) -> napi::Result<MailState> {
    Ok(crate::app()?.mail().add_account(input.into()).map_err(err)?.into())
}

#[napi]
pub async fn mail_update_account(name: String, input: MailUpdateAccount) -> napi::Result<MailState> {
    Ok(crate::app()?
        .mail()
        .update_account(&name, input.into())
        .map_err(err)?
        .into())
}

#[napi]
pub async fn mail_remove_account(name: String) -> napi::Result<MailState> {
    Ok(crate::app()?.mail().remove_account(&name).map_err(err)?.into())
}

#[napi]
pub async fn mail_activate_account(name: String) -> napi::Result<MailState> {
    Ok(crate::app()?.mail().activate(&name).map_err(err)?.into())
}

#[napi]
pub async fn mail_folders(account: Option<String>) -> napi::Result<Vec<String>> {
    crate::app()?.mail().folders(account).await.map_err(err)
}

#[napi]
pub async fn mail_folder_counts(account: Option<String>) -> napi::Result<Vec<MailFolderCount>> {
    let list = crate::app()?.mail().folder_counts(account).await.map_err(err)?;
    Ok(list.into_iter().map(Into::into).collect())
}

#[napi]
pub async fn mail_messages(query: MailMessagesQuery) -> napi::Result<MailList> {
    Ok(crate::app()?
        .mail()
        .messages(query.into())
        .await
        .map_err(err)?
        .into())
}

#[napi]
pub async fn mail_previews(
    folder: Option<String>,
    account: Option<String>,
    uids: Vec<u32>,
) -> napi::Result<Vec<MailPreview>> {
    let map = crate::app()?
        .mail()
        .previews(folder, account, uids)
        .await
        .map_err(err)?;
    Ok(map
        .into_iter()
        .map(|(uid, preview)| MailPreview { uid, preview })
        .collect())
}

#[napi]
pub async fn mail_message(
    uid: u32,
    folder: Option<String>,
    account: Option<String>,
    refresh: Option<bool>,
) -> napi::Result<MailDetail> {
    Ok(crate::app()?
        .mail()
        .message(uid, folder, account, refresh.unwrap_or(false))
        .await
        .map_err(err)?
        .into())
}

#[napi]
pub async fn mail_set_meta(
    uid: u32,
    folder: Option<String>,
    account: Option<String>,
    body: MailMeta,
) -> napi::Result<()> {
    crate::app()?
        .mail()
        .set_meta(uid, folder, account, body.into())
        .await
        .map_err(err)
}

#[napi]
pub async fn mail_move(
    uid: u32,
    folder: Option<String>,
    account: Option<String>,
    dest: String,
) -> napi::Result<()> {
    crate::app()?
        .mail()
        .move_message(uid, folder, account, dest)
        .await
        .map_err(err)
}

#[napi]
pub async fn mail_attachment(
    uid: u32,
    index: u32,
    folder: Option<String>,
    account: Option<String>,
) -> napi::Result<MailAttachmentBlob> {
    let blob = crate::app()?
        .mail()
        .attachment(uid, index as usize, folder, account)
        .await
        .map_err(err)?;
    Ok(blob.into())
}

/// 附件写入临时文件并返回路径（前端文件预览直连渲染）。
#[napi]
pub async fn mail_attachment_to_temp(
    uid: u32,
    index: u32,
    folder: Option<String>,
    account: Option<String>,
) -> napi::Result<String> {
    crate::app()?
        .mail()
        .attachment_to_temp(uid, index as usize, folder, account)
        .await
        .map_err(err)
}

#[napi]
pub async fn mail_send(input: MailSendInput) -> napi::Result<String> {
    crate::app()?.mail().send(input.into()).await.map_err(err)
}

// 保留：供后续批量预览等扩展（HashMap 无法直接作为 napi 返回值，需转数组）
#[allow(dead_code)]
fn _hashmap_to_vec(map: HashMap<u32, String>) -> Vec<MailPreview> {
    map.into_iter()
        .map(|(uid, preview)| MailPreview { uid, preview })
        .collect()
}
