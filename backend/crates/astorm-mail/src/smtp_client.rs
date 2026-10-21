//! SMTP sending via lettre.

use std::path::Path;

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::store::{Account, TlsMode};

#[allow(clippy::too_many_arguments)]
pub async fn send_mail(
    acct: &Account,
    password: &str,
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
    body: &str,
    html: Option<&str>,
    attachments: &[String],
) -> Result<()> {
    anyhow::ensure!(!to.is_empty(), "no recipients given");
    let from_email = acct
        .username
        .parse()
        .with_context(|| format!("invalid sender address '{}'", acct.username))?;

    let mut builder = Message::builder()
        .from(Mailbox {
            name: acct.display_name.clone(),
            email: from_email,
        })
        .subject(subject);
    for rcpt in to {
        let mb: Mailbox = rcpt
            .trim()
            .parse()
            .with_context(|| format!("invalid recipient '{rcpt}'"))?;
        builder = builder.to(mb);
    }
    for rcpt in cc {
        let mb: Mailbox = rcpt
            .trim()
            .parse()
            .with_context(|| format!("invalid cc '{rcpt}'"))?;
        builder = builder.cc(mb);
    }
    for rcpt in bcc {
        let mb: Mailbox = rcpt
            .trim()
            .parse()
            .with_context(|| format!("invalid bcc '{rcpt}'"))?;
        builder = builder.bcc(mb);
    }
    // HTML 版本存在 → multipart/alternative(plain, html) 作为正文主体;
    // 否则退回纯文本单段。有附件时正文主体再与附件合成 multipart/mixed。
    let html_body = html.filter(|h| !h.trim().is_empty());
    let body_multipart = html_body.map(|h| MultiPart::alternative_plain_html(body.to_string(), h.to_string()));

    let msg = if attachments.is_empty() {
        match &body_multipart {
            Some(alt) => builder.multipart(alt.clone())?,
            None => builder.header(ContentType::TEXT_PLAIN).body(body.to_string())?,
        }
    } else {
        let mut mixed = match body_multipart {
            Some(alt) => MultiPart::mixed().multipart(alt),
            None => MultiPart::mixed().singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_PLAIN)
                    .body(body.to_string()),
            ),
        };
        for path in attachments {
            let p = Path::new(path.trim());
            // 异步读文件：大附件读取不再阻塞 tokio 工作线程。
            let data = tokio::fs::read(p)
                .await
                .with_context(|| format!("read attachment '{path}'"))?;
            let filename = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .context("attachment path has no file name")?;
            let guessed = mime_guess::from_path(&filename).first_or_octet_stream();
            let content_type: ContentType = guessed
                .to_string()
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid content-type '{guessed}': {e}"))?;
            mixed = mixed.singlepart(Attachment::new(filename).body(data, content_type));
        }
        builder.multipart(mixed)?
    };

    let transport = match acct.smtp_tls {
        TlsMode::Ssl => AsyncSmtpTransport::<Tokio1Executor>::relay(&acct.smtp_host)
            .context("failed to build SMTP transport")?
            .port(acct.smtp_port),
        TlsMode::StartTls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&acct.smtp_host)
                .context("failed to build SMTP transport")?
                .port(acct.smtp_port)
        }
        TlsMode::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&acct.smtp_host)
                .port(acct.smtp_port)
        }
    };
    let transport = transport
        .credentials(Credentials::new(acct.username.clone(), password.to_string()))
        .build();

    transport.send(msg).await.context("SMTP send failed")?;
    Ok(())
}
