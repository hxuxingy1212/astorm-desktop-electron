//! IMAP client: connect (SSL / STARTTLS / plain), list folders, list and fetch messages.

use std::collections::{HashMap, HashSet};
use std::fmt;

use anyhow::{anyhow, Context, Result};
use async_imap::imap_proto::{BodyContentCommon, BodyStructure, MessageSection, SectionPath};
use async_imap::types::{Fetch, Flag, Name};
use async_imap::{Client, Session};
use futures_util::TryStreamExt;
use mail_parser::{Address, MessageParser, MimeHeaders};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::store::{Account, TlsMode};

/// Stream types usable by the IMAP session (plain TCP and TLS both qualify).
pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug> AsyncReadWrite for T {}

pub type Conn = Box<dyn AsyncReadWrite>;
pub type ImapSession = Session<Conn>;

const SUMMARY_QUERY: &str =
    "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE)])";
const FULL_QUERY: &str = "(UID FLAGS BODY.PEEK[])";

#[derive(Debug, Clone, serde::Serialize)]
pub struct MailSummary {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub size: u64,
    /// Sort key (UTC seconds from the Date header); not sent to clients.
    #[serde(skip)]
    pub ts: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailAttachment {
    pub filename: String,
    pub size: usize,
}

/// An attachment's decoded bytes plus metadata, used by the download endpoint.
#[derive(Debug, Clone)]
pub struct AttachmentData {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize)]
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
    pub attachments: Vec<MailAttachment>,
    /// 正文是否被截断（由缓存层按 2 MiB 上限设置；解析层恒为 false）。
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct FolderStats {
    /// Total messages in the mailbox.
    pub total: u32,
    /// Number of unseen (unread) messages.
    pub unread: u32,
}

/// A page of message summaries plus folder totals (returned by `list_messages`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MessagePage {
    pub messages: Vec<MailSummary>,
    pub total: u32,
    pub unread: u32,
    /// Whether older messages exist beyond this page.
    pub has_more: bool,
}

pub struct ImapClient {
    session: ImapSession,
    /// 当前已 `SELECT` 的文件夹：同一文件夹连续操作时跳过重复 SELECT（省一次往返）。
    selected: Option<String>,
    /// 上次 SELECT 时的 `EXISTS`（消息总数），供 `list_messages` 复用。
    selected_exists: u32,
}

async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream> {
    TcpStream::connect((host, port))
        .await
        .with_context(|| format!("tcp connect {host}:{port} failed"))
}

async fn tls_upgrade(host: &str, tcp: TcpStream) -> Result<TlsStreamOf> {
    async_native_tls::TlsConnector::new()
        .connect(host, tcp)
        .await
        .with_context(|| format!("TLS handshake with {host} failed"))
}

type TlsStreamOf = async_native_tls::TlsStream<TcpStream>;

async fn login(
    client: Client<Conn>,
    acct: &Account,
    password: &str,
) -> Result<ImapSession> {
    client.login(&acct.username, password).await.map_err(|(e, _)| {
        anyhow!(
            "IMAP login failed for '{}' on {}:{}: {e}",
            acct.username,
            acct.imap_host,
            acct.imap_port
        )
    })
}

impl ImapClient {
    pub async fn connect(acct: &Account, password: &str) -> Result<Self> {
        let host = acct.imap_host.as_str();
        let session = match acct.imap_tls {
            TlsMode::Ssl => {
                let tcp = connect_tcp(host, acct.imap_port).await?;
                let stream = tls_upgrade(host, tcp).await?;
                login(
                    Client::new(Box::new(stream) as Conn),
                    acct,
                    password,
                )
                .await?
            }
            TlsMode::StartTls => {
                let tcp = connect_tcp(host, acct.imap_port).await?;
                let mut plain = Client::new(tcp);
                plain
                    .run_command_and_check_ok("STARTTLS", None)
                    .await
                    .context("STARTTLS command rejected")?;
                let tcp = plain.into_inner();
                let stream = tls_upgrade(host, tcp).await?;
                login(
                    Client::new(Box::new(stream) as Conn),
                    acct,
                    password,
                )
                .await?
            }
            TlsMode::None => {
                let tcp = connect_tcp(host, acct.imap_port).await?;
                login(
                    Client::new(Box::new(tcp) as Conn),
                    acct,
                    password,
                )
                .await?
            }
        };
        Ok(Self { session, selected: None, selected_exists: 0 })
    }

    /// 确保已选中 `folder`；已是当前文件夹则跳过 SELECT（省一次往返），返回 `EXISTS`。
    async fn ensure_selected(&mut self, folder: &str) -> Result<u32> {
        if self.selected.as_deref() == Some(folder) {
            return Ok(self.selected_exists);
        }
        let mailbox = self
            .session
            .select(folder)
            .await
            .with_context(|| format!("cannot select folder '{folder}'"))?;
        self.selected = Some(folder.to_string());
        self.selected_exists = mailbox.exists as u32;
        Ok(self.selected_exists)
    }

    pub async fn logout(&mut self) -> Result<()> {
        self.session.logout().await.context("IMAP logout failed")
    }

    pub async fn list_folders(&mut self) -> Result<Vec<String>> {
        let names: Vec<Name> = self
            .session
            .list(None, Some("*"))
            .await
            .context("IMAP LIST failed")?
            .try_collect()
            .await
            .context("IMAP LIST response failed")?;
        let mut out: Vec<String> = names
            .iter()
            .map(|n| decode_mutf7(n.name()))
            .collect();
        out.sort();
        Ok(out)
    }

    /// List one page of messages (newest first) plus the folder's totals.
    /// `offset` skips that many of the newest messages, for paging.
    pub async fn list_messages(
        &mut self,
        folder: &str,
        limit: usize,
        offset: usize,
        unread_only: bool,
    ) -> Result<MessagePage> {
        let limit = limit.max(1);
        let total = self.ensure_selected(folder).await?;

        let mut out = Vec::new();
        let mut has_more = false;
        let unread;

        if unread_only {
            let found: HashSet<u32> =
                self.session.uid_search("UNSEEN").await.unwrap_or_default();
            unread = found.len() as u32;
            let mut uids: Vec<u32> = found.into_iter().collect();
            uids.sort_unstable();
            if uids.len() > limit {
                uids = uids[uids.len() - limit..].to_vec();
            }
            if !uids.is_empty() {
                let set = uids
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let fetches: Vec<Fetch> = self
                    .session
                    .uid_fetch(set, SUMMARY_QUERY)
                    .await
                    .context("UID FETCH failed")?
                    .try_collect()
                    .await
                    .context("UID FETCH response failed")?;
                for f in &fetches {
                    if let Some(m) = summarize(f) {
                        out.push(m);
                    }
                }
            }
        } else {
            // The folder is already selected, so STATUS avoids a second SELECT.
            unread = self
                .folder_stats(folder)
                .await
                .map(|s| s.unread)
                .unwrap_or(0);
            let total_n = total as usize;
            if total_n > offset {
                let hi = total_n - offset; // newest sequence number of this page
                let lo = hi.saturating_sub(limit - 1).max(1);
                let fetches: Vec<Fetch> = self
                    .session
                    .fetch(format!("{lo}:{hi}").as_str(), SUMMARY_QUERY)
                    .await
                    .context("FETCH failed")?
                    .try_collect()
                    .await
                    .context("FETCH response failed")?;
                for f in &fetches {
                    if let Some(m) = summarize(f) {
                        out.push(m);
                    }
                }
                has_more = lo > 1;
            }
        }

        // Newest message date first; ties broken by newest UID. (UID alone would be
        // arrival order, which misplaces messages copied in from other folders.)
        sort_by_date_desc(&mut out);
        Ok(MessagePage {
            messages: out,
            total,
            unread,
            has_more,
        })
    }

    /// Fetch short plain-text previews for the given message UIDs (one batched
    /// `UID FETCH`). Kept separate from `list_messages` because fetching even a
    /// partial body makes the server read the whole message, which is very costly
    /// for messages with large attachments — callers should request only the
    /// messages they actually show.
    pub async fn fetch_previews(
        &mut self,
        folder: &str,
        uids: &[u32],
    ) -> Result<HashMap<u32, String>> {
        if uids.is_empty() {
            return Ok(HashMap::new());
        }
        self.ensure_selected(folder).await?;
        let set = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(set, "(UID BODY.PEEK[TEXT]<0.2048>)")
            .await
            .context("UID FETCH previews failed")?
            .try_collect()
            .await
            .context("UID FETCH previews response failed")?;
        Ok(fetches
            .iter()
            .filter_map(|f| Some((f.uid?, f.text().map(make_preview).unwrap_or_default())))
            .collect())
    }

    pub async fn fetch_message(&mut self, folder: &str, uid: u32) -> Result<MailDetail> {
        let (body, seen) = self.fetch_raw(folder, uid).await?;
        detail_from_bytes(uid, &body, seen)
    }

    /// 拉取整封原始报文（`BODY.PEEK[]`，不标记已读），返回（原始字节, 是否已读）。
    /// 正文解析与附件解码共用这一份原始报文，避免同一封邮件多次全量下载。
    pub async fn fetch_raw(&mut self, folder: &str, uid: u32) -> Result<(Vec<u8>, bool)> {
        self.ensure_selected(folder).await?;
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uid.to_string(), FULL_QUERY)
            .await
            .context("UID FETCH failed")?
            .try_collect()
            .await
            .context("UID FETCH response failed")?;
        let f = fetches
            .first()
            .ok_or_else(|| anyhow!("uid {uid} not found in folder '{folder}'"))?;
        Ok((f.body().unwrap_or(&[]).to_vec(), is_seen(f)))
    }

    /// Fetch attachment `index` of a message (bytes + filename + MIME type).
    pub async fn fetch_attachment(
        &mut self,
        folder: &str,
        uid: u32,
        index: usize,
    ) -> Result<AttachmentData> {
        self.ensure_selected(folder).await?;
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uid.to_string(), FULL_QUERY)
            .await
            .context("UID FETCH failed")?
            .try_collect()
            .await
            .context("UID FETCH response failed")?;
        let f = fetches
            .first()
            .ok_or_else(|| anyhow!("uid {uid} not found in folder '{folder}'"))?;
        attachment_from_bytes(f.body().unwrap_or(&[]), index)
    }

    /// 只取指定附件的 MIME 分片（不下载整封）：`BODYSTRUCTURE` 定位附件分片号 →
    /// `BODY.PEEK[part.MIME]` + `BODY.PEEK[part]` 仅拉该分片 → 合成单附件 multipart 交
    /// mail-parser 解码。`expected` 为已知文件名，用于校验索引与分片是否对得上；
    /// 任何不确定（找不到分片 / 名字对不上）返回 `Ok(None)`，由调用方回退全量拉取。
    pub async fn fetch_attachment_fast(
        &mut self,
        folder: &str,
        uid: u32,
        index: usize,
        expected: Option<&str>,
    ) -> Result<Option<AttachmentData>> {
        self.ensure_selected(folder).await?;

        // 1) 取 BODYSTRUCTURE，按文档顺序收集「附件类」叶子分片号
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uid.to_string(), "(BODYSTRUCTURE)")
            .await
            .context("UID FETCH BODYSTRUCTURE failed")?
            .try_collect()
            .await
            .context("UID FETCH BODYSTRUCTURE response failed")?;
        let Some(structure) = fetches.first().and_then(|f| f.bodystructure()) else {
            return Ok(None);
        };
        let mut parts: Vec<Vec<u32>> = Vec::new();
        collect_attachment_parts(structure, Vec::new(), &mut parts);
        let Some(part) = parts.get(index).cloned() else {
            return Ok(None);
        };

        // 2) 只拉该分片的 MIME 头 + 内容
        let dotted = part
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(".");
        let query = format!("(BODY.PEEK[{dotted}.MIME] BODY.PEEK[{dotted}])");
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uid.to_string(), query.as_str())
            .await
            .context("UID FETCH part failed")?
            .try_collect()
            .await
            .context("UID FETCH part response failed")?;
        let Some(f) = fetches.first() else {
            return Ok(None);
        };
        let mime_path = SectionPath::Part(part.clone(), Some(MessageSection::Mime));
        let body_path = SectionPath::Part(part.clone(), None);
        let headers = f.section(&mime_path).unwrap_or(&[]);
        let body = f.section(&body_path).unwrap_or(&[]);
        if body.is_empty() {
            return Ok(None);
        }

        // 3) 合成单附件 multipart/mixed，复用 mail-parser 解码（文件名 / 传输编码 / 内容）
        const BOUNDARY: &str = "----astorm-part-boundary";
        let mut synthetic = Vec::with_capacity(headers.len() + body.len() + 128);
        synthetic.extend_from_slice(
            format!(
                "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"{BOUNDARY}\"\r\n\r\n--{BOUNDARY}\r\n"
            )
            .as_bytes(),
        );
        synthetic.extend_from_slice(headers);
        synthetic.extend_from_slice(b"\r\n\r\n");
        synthetic.extend_from_slice(body);
        synthetic.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        let data = attachment_from_bytes(&synthetic, 0)?;

        // 4) 文件名校验：已知期望名且不一致 → 放弃（回退全量，避免错配）
        if let Some(exp) = expected {
            if !exp.is_empty() && !data.filename.is_empty() && data.filename != exp {
                return Ok(None);
            }
        }
        Ok(Some(data))
    }

    /// Total and unseen counts for a folder.
    ///
    /// Uses `STATUS` (one command, no SELECT, leaves the current mailbox untouched)
    /// and falls back to `SELECT` + `UID SEARCH UNSEEN` when a server does not
    /// report UNSEEN in STATUS.
    pub async fn folder_stats(&mut self, folder: &str) -> Result<FolderStats> {
        if let Ok(m) = self.session.status(folder, "(MESSAGES UNSEEN)").await {
            return Ok(FolderStats {
                total: m.exists,
                unread: m.unseen.unwrap_or(0),
            });
        }
        let total = self.ensure_selected(folder).await?;
        let unread = self
            .session
            .uid_search("UNSEEN")
            .await
            .unwrap_or_default()
            .len() as u32;
        Ok(FolderStats { total, unread })
    }

    /// Set or clear the IMAP `\Flagged` flag on a message.
    pub async fn set_flagged(&mut self, folder: &str, uid: u32, on: bool) -> Result<()> {
        self.ensure_selected(folder).await?;
        let op = if on { "+FLAGS" } else { "-FLAGS" };
        self.session
            .uid_store(uid.to_string(), &format!("{op} (\\Flagged)"))
            .await
            .context("UID STORE flag failed")?
            .try_collect::<Vec<_>>()
            .await
            .context("UID STORE flag response failed")?;
        Ok(())
    }

    /// Set or clear the IMAP `\Seen` flag on a message (mark read / unread).
    pub async fn set_seen(&mut self, folder: &str, uid: u32, on: bool) -> Result<()> {
        self.ensure_selected(folder).await?;
        let op = if on { "+FLAGS" } else { "-FLAGS" };
        self.session
            .uid_store(uid.to_string(), &format!("{op} (\\Seen)"))
            .await
            .context("UID STORE seen flag failed")?
            .try_collect::<Vec<_>>()
            .await
            .context("UID STORE seen flag response failed")?;
        Ok(())
    }

    /// Move a message from `src_folder` to `dest_folder`
    /// (used by 删除 → 废纸篓 and 垃圾邮件).
    /// async-imap 0.11 exposes no UID MOVE, so this does COPY + mark \Deleted + EXPUNGE,
    /// which copies the message into the destination and removes it from the source.
    pub async fn move_message(&mut self, src_folder: &str, uid: u32, dest_folder: &str) -> Result<()> {
        let set = uid.to_string();
        self.ensure_selected(src_folder).await?;
        self.session
            .uid_copy(&set, dest_folder)
            .await
            .context("UID COPY failed")?;
        self.session
            .uid_store(&set, "+FLAGS (\\Deleted)")
            .await
            .context("UID STORE failed")?
            .try_collect::<Vec<_>>()
            .await
            .context("UID STORE response failed")?;
        self.session
            .expunge()
            .await
            .context("EXPUNGE failed")?
            .try_collect::<Vec<_>>()
            .await
            .context("EXPUNGE response failed")?;
        Ok(())
    }
}

fn is_seen(f: &Fetch) -> bool {
    f.flags().any(|fl| matches!(fl, Flag::Seen))
}

fn is_flagged(f: &Fetch) -> bool {
    f.flags().any(|fl| matches!(fl, Flag::Flagged))
}

/// Decode IMAP modified UTF-7 folder names (e.g. QQ Mail Chinese folders).
/// Returns the input unchanged if it is plain ASCII or malformed.
fn decode_mutf7(name: &str) -> String {
    const ESCAPE: char = '&';
    if !name.contains(ESCAPE) {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        // find end of base64 section (terminated by '-')
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'-' {
            end += 1;
        }
        if end == start {
            // "&-" is a literal '&'
            out.push('&');
            i = end + 1;
            continue;
        }
        let b64 = &name[start..end];
        // mUTF-7 base64: ',' instead of '/', no padding
        let normalized = b64.replace(',', "/");
        let padded = match normalized.len() % 4 {
            2 => format!("{normalized}=="),
            3 => format!("{normalized}="),
            _ => normalized.clone(),
        };
        match engine_decode(&padded) {
            Ok(raw) => {
                // raw is UTF-16BE
                let units: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                out.push_str(&String::from_utf16_lossy(&units));
            }
            Err(_) => {
                // keep original section on decode failure
                out.push_str(&name[i..end.min(bytes.len())]);
            }
        }
        i = end + 1;
    }
    out
}

fn engine_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
}

/// Newest message date first; ties broken by newest UID.
fn sort_by_date_desc(msgs: &mut [MailSummary]) {
    msgs.sort_by(|a, b| b.ts.cmp(&a.ts).then(b.uid.cmp(&a.uid)));
}

fn summarize(f: &Fetch) -> Option<MailSummary> {
    let uid = f.uid?;
    let headers = f.header()?;
    let parsed = MessageParser::default().parse(headers)?;
    Some(MailSummary {
        uid,
        seen: is_seen(f),
        flagged: is_flagged(f),
        from: addr_to_string(parsed.from()),
        to: addr_to_string(parsed.to()),
        subject: parsed.subject().unwrap_or_default().to_string(),
        date: parsed.date().map(|d| d.to_rfc3339()).unwrap_or_default(),
        size: f.size.unwrap_or(0) as u64,
        ts: parsed.date().map(|d| d.to_timestamp()).unwrap_or(0),
    })
}

/// Best-effort plain-text preview from the first bytes of a message body,
/// fetched with `BODY.PEEK[TEXT]<0.2048>`.
fn make_preview(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let plain = strip_markup(&first_text_part(&text));
    truncate_chars(&plain.split_whitespace().collect::<Vec<_>>().join(" "), 200)
}

/// Return the decoded text of the first `text/*` leaf part in a (possibly
/// truncated) MIME body. Parts are scanned flatly by boundary, which also
/// works for nested multiparts.
fn first_text_part(s: &str) -> String {
    let chunks = boundary_chunks(s);
    // Only treat the body as multipart if the chunks actually carry MIME headers;
    // a plain-text mail may legitimately contain a "-- " signature separator.
    let is_multipart = chunks.iter().any(|c| {
        let h = split_headers(c).0.to_ascii_lowercase();
        h.contains("content-type") || h.contains("content-transfer-encoding")
    });
    if !is_multipart {
        return decode_bytes(s.as_bytes(), None);
    }
    for chunk in chunks {
        let (head, content) = split_headers(chunk);
        let hl = head.to_ascii_lowercase();
        if hl.contains("multipart/") {
            continue; // container part; its children are chunks of their own
        }
        if hl.contains("text/") || hl.is_empty() {
            let bytes = decode_transfer(&hl, content);
            let out = decode_bytes(&bytes, charset_of(&hl).as_deref());
            if !out.trim().is_empty() {
                return out;
            }
        }
    }
    String::new()
}

/// Split a body into the chunks between MIME boundary lines (lines starting with "--").
fn boundary_chunks(s: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start: Option<usize> = None;
    let mut offset = 0usize;
    for line in s.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).starts_with("--") {
            if let Some(st) = start {
                chunks.push(&s[st..offset]);
            }
            start = Some(offset + line.len());
        }
        offset += line.len();
    }
    if let Some(st) = start {
        chunks.push(&s[st..]);
    }
    chunks
}

/// Split a MIME part into (headers, body).
fn split_headers(chunk: &str) -> (&str, &str) {
    if let Some(p) = chunk.find("\r\n\r\n") {
        (&chunk[..p], &chunk[p + 4..])
    } else if let Some(p) = chunk.find("\n\n") {
        (&chunk[..p], &chunk[p + 2..])
    } else {
        ("", chunk)
    }
}

/// Transfer-decode a leaf part's content to raw bytes.
fn decode_transfer(headers_lower: &str, content: &str) -> Vec<u8> {
    use mail_parser::decoders::quoted_printable::quoted_printable_decode;
    let content = content.trim_start_matches(['\r', '\n']);
    if headers_lower.contains("base64") {
        if let Some(b) = decode_b64(content) {
            return b;
        }
    } else if headers_lower.contains("quoted-printable") {
        if let Some(b) = quoted_printable_decode(content.as_bytes()) {
            return b;
        }
    } else {
        if looks_like_base64(content) {
            if let Some(b) = decode_b64(content) {
                if !b.is_empty() {
                    return b;
                }
            }
        }
        if content.contains('=') {
            if let Some(b) = quoted_printable_decode(content.as_bytes()) {
                return b;
            }
        }
    }
    content.as_bytes().to_vec()
}

/// Decode bytes to a string using the declared charset (defaults to UTF-8).
fn decode_bytes(bytes: &[u8], charset: Option<&str>) -> String {
    if let Some(cs) = charset {
        if !cs.eq_ignore_ascii_case("utf-8") && !cs.eq_ignore_ascii_case("utf8") {
            if let Some(decode) =
                mail_parser::decoders::charsets::map::charset_decoder(cs.as_bytes())
            {
                return decode(bytes);
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Extract `charset=...` from lowercased MIME headers.
fn charset_of(headers_lower: &str) -> Option<String> {
    let i = headers_lower.find("charset")?;
    let rest = &headers_lower[i..];
    let eq = rest.find('=')?;
    let val = rest[eq + 1..].trim_start().trim_start_matches('"');
    let end = val
        .find(|c: char| c == '"' || c == ';' || c.is_whitespace())
        .unwrap_or(val.len());
    let cs = val[..end].trim();
    (!cs.is_empty()).then(|| cs.to_string())
}

fn decode_b64(s: &str) -> Option<Vec<u8>> {
    use mail_parser::decoders::base64::base64_decode;
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let n = cleaned.len() - cleaned.len() % 4;
    if n == 0 {
        return None;
    }
    base64_decode(cleaned[..n].as_bytes())
}

fn looks_like_base64(s: &str) -> bool {
    // Base64 bodies never contain spaces, and are a multiple of 4 chars.
    if s.contains(' ') {
        return false;
    }
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 24
        && compact.len() % 4 == 0
        && compact
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        && compact.chars().any(|c| c.is_ascii_alphabetic())
}

/// Strip HTML tags and decode a few common entities (best effort).
fn strip_markup(s: &str) -> String {
    let stripped = if s.contains('<') {
        let mut out = String::with_capacity(s.len());
        let mut in_tag = false;
        for c in s.chars() {
            match c {
                '<' => in_tag = true,
                '>' => {
                    in_tag = false;
                    out.push(' ');
                }
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out
    } else {
        s.to_string()
    };
    stripped
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 从原始 RFC822 报文解析出 [`MailDetail`]（与 `fetch_message` 共用解析逻辑）。
pub fn detail_from_bytes(uid: u32, body: &[u8], seen: bool) -> Result<MailDetail> {
    let parsed = MessageParser::default()
        .parse(body)
        .ok_or_else(|| anyhow!("failed to parse message uid {uid}"))?;
    let attachments = (0..parsed.attachment_count() as u32)
        .filter_map(|i| parsed.attachment(i))
        .map(|p| MailAttachment {
            filename: p.attachment_name().unwrap_or("unnamed").to_string(),
            // decoded size (not the raw base64 length on the wire)
            size: p.len(),
        })
        .collect();
    Ok(MailDetail {
        uid,
        seen,
        from: addr_to_string(parsed.from()),
        to: addr_to_string(parsed.to()),
        cc: addr_to_string(parsed.cc()),
        subject: parsed.subject().unwrap_or_default().to_string(),
        date: parsed.date().map(|d| d.to_rfc3339()).unwrap_or_default(),
        text: parsed.body_text(0).map(|c| c.into_owned()),
        html: parsed.body_html(0).map(|c| c.into_owned()),
        attachments,
        truncated: false,
    })
}

/// Extract attachment `index` (decoded bytes + MIME type) from a raw RFC822 message.
/// mail-parser decodes the transfer-encoding while parsing, so `contents()` yields
/// the original file bytes for base64/quoted-printable attachments.
pub fn attachment_from_bytes(body: &[u8], index: usize) -> Result<AttachmentData> {
    let parsed = MessageParser::default()
        .parse(body)
        .ok_or_else(|| anyhow!("failed to parse message"))?;
    let part = parsed
        .attachment(index as u32)
        .ok_or_else(|| anyhow!("attachment {index} not found"))?;
    let filename = part.attachment_name().unwrap_or("unnamed").to_string();
    // Prefer the declared MIME type, but QQ/163 often send generic
    // application/octet-stream — fall back to guessing from the filename then.
    let guessed = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();
    let content_type = part
        .content_type()
        .and_then(|ct| ct.subtype().map(|s| format!("{}/{}", ct.ctype(), s)))
        .filter(|ct| !ct.eq_ignore_ascii_case("application/octet-stream"))
        .unwrap_or(guessed);
    Ok(AttachmentData {
        filename,
        content_type,
        data: part.contents().to_vec(),
    })
}

/// 某分片是否算「附件」（有 attachment 处置，或带 name / filename 参数）。
fn is_attachment_part(common: &BodyContentCommon<'_>) -> bool {
    let disp = common.disposition.as_ref();
    let disp_attachment = disp
        .map(|d| d.ty.eq_ignore_ascii_case("attachment"))
        .unwrap_or(false);
    let has_name = param_value(&common.ty.params, "name").is_some()
        || disp.and_then(|d| param_value(&d.params, "filename")).is_some();
    disp_attachment || has_name
}

/// 从 IMAP `BodyParams` 里取参数值（大小写不敏感）。
fn param_value(
    params: &Option<Vec<(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)>>,
    key: &str,
) -> Option<String> {
    params
        .as_ref()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.to_string())
}

/// 按文档顺序收集附件分片号（IMAP 分片号：multipart 子项从 1 起、嵌套用点号）。
fn collect_attachment_parts(bs: &BodyStructure<'_>, prefix: Vec<u32>, out: &mut Vec<Vec<u32>>) {
    match bs {
        BodyStructure::Multipart { bodies, .. } => {
            for (i, child) in bodies.iter().enumerate() {
                let mut p = prefix.clone();
                p.push(i as u32 + 1);
                collect_attachment_parts(child, p, out);
            }
        }
        BodyStructure::Basic { common, .. }
        | BodyStructure::Text { common, .. }
        | BodyStructure::Message { common, .. } => {
            if is_attachment_part(common) {
                out.push(prefix);
            }
        }
        _ => {}
    }
}

fn addr_to_string(addr: Option<&Address<'_>>) -> String {
    let Some(addr) = addr else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    match addr {
        Address::List(list) => {
            for a in list {
                if let Some(mail) = a.address() {
                    match a.name().map(|n| n.trim().to_string()) {
                        Some(n) if !n.is_empty() => parts.push(format!("{n} <{mail}>")),
                        _ => parts.push(mail.to_string()),
                    }
                }
            }
        }
        Address::Group(groups) => {
            for g in groups {
                for a in &g.addresses {
                    if let Some(mail) = a.address() {
                        parts.push(mail.to_string());
                    }
                }
            }
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{attachment_from_bytes, decode_mutf7, make_preview, sort_by_date_desc, MailSummary};

    fn summary(uid: u32, date: &str, ts: i64) -> MailSummary {
        MailSummary {
            uid,
            seen: true,
            flagged: false,
            from: String::new(),
            to: String::new(),
            subject: String::new(),
            date: date.to_string(),
            size: 0,
            ts,
        }
    }

    #[test]
    fn preview_plain_text_is_collapsed() {
        assert_eq!(
            make_preview(b"Hello   world\r\nsecond\tline"),
            "Hello world second line"
        );
    }

    #[test]
    fn preview_multipart_base64() {
        let raw = b"--B\r\nContent-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8gd29ybGQ=\r\n--B--\r\n";
        assert_eq!(make_preview(raw), "Hello world");
    }

    #[test]
    fn preview_multipart_quoted_printable_html() {
        let raw = b"--B\r\nContent-Type: text/html; charset=utf-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\r\n<p>Hi=20there</p>\r\n--B--\r\n";
        assert_eq!(make_preview(raw), "Hi there");
    }

    #[test]
    fn preview_truncates_long_text() {
        let p = make_preview("a".repeat(300).as_bytes());
        assert!(p.chars().count() <= 201);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn preview_nested_multipart_after_preamble() {
        // QQ-style: preamble, then multipart/alternative wrapping the text part.
        let raw = b"This is a multi-part message in MIME format.\r\n\r\n\
------=_NextPart_X\r\nContent-Type: multipart/alternative; boundary=\"Y\"\r\n\r\n\
--Y\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n\
SGVsbG8gd29ybGQ=\r\n--Y--\r\n";
        assert_eq!(make_preview(raw), "Hello world");
    }

    #[test]
    fn preview_plain_text_with_dash_signature_is_not_multipart() {
        let raw = b"Hi there\r\n\r\n-- \r\nRegards";
        assert_eq!(make_preview(raw), "Hi there -- Regards");
    }

    #[test]
    fn sorts_by_date_not_by_uid() {
        // uid 300 (2020) was copied in last, so it has a high UID but an old date.
        let mut msgs = vec![
            summary(300, "2020-05-30T10:00:00+08:00", 1_590_804_000),
            summary(1, "2026-09-10T10:00:00+08:00", 1_788_998_400),
            summary(2, "2026-09-21T10:00:00+08:00", 1_789_493_200),
        ];
        sort_by_date_desc(&mut msgs);
        assert_eq!(msgs.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![2, 1, 300]);
    }

    #[test]
    fn sorts_ties_by_uid_desc() {
        let mut msgs = vec![summary(5, "x", 100), summary(9, "x", 100)];
        sort_by_date_desc(&mut msgs);
        assert_eq!(msgs.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![9, 5]);
    }

    #[test]
    fn mutf7_decode() {
        assert_eq!(decode_mutf7("INBOX"), "INBOX");
        assert_eq!(decode_mutf7("Sent Messages"), "Sent Messages");
        assert_eq!(decode_mutf7("&Ti1lhw-"), "中文");
        assert_eq!(decode_mutf7("QQ&Ti1lhw-X"), "QQ中文X");
        assert_eq!(decode_mutf7("&-"), "&");
        assert_eq!(decode_mutf7("A&B"), "A&B");
        assert_eq!(decode_mutf7("&U&-"), "&U&");
    }

    // "hello world" base64-encoded as an attachment named 测试.txt
    const RAW: &[u8] = b"From: a@b.com\r\n\
Subject: t\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\
\r\n\
--B\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
body\r\n\
--B\r\n\
Content-Type: text/plain; name=\"=?utf-8?B?5rWL6K+VLnR4dA==?=\"\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"=?utf-8?B?5rWL6K+VLnR4dA==?=\"\r\n\
\r\n\
aGVsbG8gd29ybGQ=\r\n\
--B--\r\n";

    #[test]
    fn attachment_bytes_decoded_and_named() {
        let a = attachment_from_bytes(RAW, 0).expect("attachment 0");
        assert_eq!(a.filename, "测试.txt");
        assert_eq!(a.content_type, "text/plain");
        assert_eq!(a.data, b"hello world");
    }

    #[test]
    fn attachment_index_out_of_range() {
        assert!(attachment_from_bytes(RAW, 9).is_err());
    }
}
