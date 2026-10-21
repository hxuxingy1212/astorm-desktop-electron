//! `mail` 工具：收发与管理多账号邮箱（IMAP 收信 + SMTP 发信）。
//!
//! 经 `App::global().mail()`（[`astorm_mail::MailService`]）访问邮箱：
//! 与前端「邮箱」界面共用同一份账号库、缓存与置顶/分类元数据。
//! 输出为人类可读文本（与 mail-proxy CLI 风格一致）。

use async_trait::async_trait;
use serde_json::{json, Value};

use astorm_mail::service::{
    folder_kind, standard_folders, MessagesQuery, MetaUpdate, SendInput,
};

use crate::app::App;
use crate::tool::{expand_session_vars, resolve_workspace, ToolContext};

/// `mail` 工具。
pub struct MailTool;

#[async_trait]
impl crate::tool::Tool for MailTool {
    fn name(&self) -> &str {
        "mail"
    }

    fn description(&self) -> &str {
        "收发与管理多账号邮箱（IMAP 收信 / SMTP 发信）。支持操作：\n\
         1. accounts — 列出已配置账号与当前活动账号\n\
         2. folders — 列出某账号的文件夹（参数 account 可选，默认活动账号）\n\
         3. list — 列出最近邮件（参数 folder / limit / offset / unread / category / account；category 取 todo|doing|done 可跨文件夹查看待办分类）\n\
         4. read — 按 UID 阅读单封邮件全文（参数 uid 必填，folder / account 可选）\n\
         5. send — 发送邮件（参数 to 必填；cc / bcc / subject / body / html / attachments / account 可选；body 为纯文本、html 为可选 HTML 版本（同时给出则按 multipart/alternative 发送）；attachments 为本地文件路径数组）。**发送前会先弹出写邮件确认界面，由用户确认后才会真正发出**\n\
         6. mark — 标记/置顶/分类：flagged / unflagged / pinned / unpinned / category(todo|doing|done|none)，可组合；参数 uid 必填\n\
         7. move — 移动到指定文件夹（参数 uid、to 必填，folder / account 可选）\n\
         8. trash — 移入废纸篓；9. junk — 移入垃圾邮件（参数 uid 必填）\n\
         10. download_attachment — 下载附件到本地（参数 uid、index 必填；index 来自 read 输出的附件编号；out 为输出目录可选，默认会话工作区）\n\
         注意：发送、移动不可撤销（发送/移动需用户确认）；账号的添加与删除请在应用『设置 → 邮箱』中手动完成，本工具不提供账号增删。"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "accounts", "folders", "list", "read", "send", "mark",
                        "move", "trash", "junk", "download_attachment"
                    ],
                    "description": "操作类型"
                },
                "account": { "type": "string", "description": "账号名（默认当前活动账号）" },
                "folder": { "type": "string", "description": "文件夹名，默认 INBOX" },
                "uid": { "type": "integer", "description": "邮件 UID（来自 list 输出）" },
                "index": { "type": "integer", "description": "附件编号（来自 read 输出的附件列表）" },
                "limit": { "type": "integer", "description": "list 返回条数，默认 20" },
                "offset": { "type": "integer", "description": "list 翻页偏移，默认 0" },
                "unread": { "type": "boolean", "description": "list 只看未读" },
                "category": { "type": "string", "enum": ["todo", "doing", "done", "none"], "description": "list 按分类筛选；mark 设置分类（none 清除）" },
                "to": { "type": "string", "description": "收件人（逗号分隔），move 时为目标文件夹" },
                "cc": { "type": "string", "description": "抄送（逗号分隔）" },
                "bcc": { "type": "string", "description": "密送（逗号分隔）" },
                "subject": { "type": "string", "description": "邮件主题" },
                "body": { "type": "string", "description": "邮件正文（纯文本）" },
                "html": { "type": "string", "description": "send 的正文 HTML 版本（可选；与 body 同时提供时以 multipart/alternative 发送）" },
                "attachments": { "type": "array", "items": { "type": "string" }, "description": "send 的本地附件路径列表" },
                "flagged": { "type": "boolean", "description": "mark：加/取消 IMAP 标记" },
                "unflagged": { "type": "boolean", "description": "mark：取消标记" },
                "pinned": { "type": "boolean", "description": "mark：置顶/取消置顶（本地）" },
                "unpinned": { "type": "boolean", "description": "mark：取消置顶" },
                "out": { "type": "string", "description": "download_attachment 输出目录（默认会话工作区）" }
            },
            "required": ["action"]
        })
    }

    fn interrupt_payload(&self, result: &str) -> Option<serde_json::Value> {
        crate::tool::policy::interrupt_request_from_result(result)
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let app = App::try_global().ok_or_else(|| "App 未初始化，无法访问邮件服务".to_string())?;
        let mail = app.mail();

        match action.as_str() {
            "accounts" => action_accounts(mail).await,
            "folders" => action_folders(mail, opt_str(&params, "account")).await,
            "list" => action_list(mail, &params).await,
            "read" => action_read(mail, &params).await,
            "send" => action_send_gated(mail, &params, ctx).await,
            "mark" => action_mark(mail, &params).await,
            "move" => action_move_gated(mail, &params, None, ctx).await,
            "trash" => action_move_gated(mail, &params, Some("trash"), ctx).await,
            "junk" => action_move_gated(mail, &params, Some("junk"), ctx).await,
            "download_attachment" => action_download(mail, &params, ctx).await,
            other => Err(format!("不支持的邮件操作: {other}")),
        }
    }
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn req_str(v: &Value, key: &str) -> Result<String, String> {
    opt_str(v, key).ok_or_else(|| format!("缺少必填参数: {key}"))
}

fn opt_u32(v: &Value, key: &str) -> Option<u32> {
    v.get(key).and_then(|x| x.as_u64()).map(|n| n as u32)
}

fn opt_bool(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|x| x.as_bool())
}

fn split_list(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| x.as_str())
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

async fn action_accounts(mail: &std::sync::Arc<astorm_mail::MailService>) -> Result<String, String> {
    let state = mail.state().map_err(|e| e.to_string())?;
    if state.accounts.is_empty() {
        return Ok("尚未配置邮箱账号。请在应用「设置 → 邮箱」中手动添加账号（本工具不支持账号增删）。".into());
    }
    let mut out = Vec::new();
    out.push(format!(
        "{:<16} {:<32} {:<28} {}",
        "NAME", "IMAP", "SMTP", "ACTIVE"
    ));
    for a in &state.accounts {
        let active = if state.active.as_deref() == Some(a.name.as_str()) {
            "*"
        } else {
            ""
        };
        out.push(format!(
            "{:<16} {}:{} {:<5} {}:{} {:<5} {}",
            a.name, a.imap_host, a.imap_port, a.imap_tls, a.smtp_host, a.smtp_port, a.smtp_tls, active
        ));
    }
    Ok(out.join("\n"))
}

async fn action_folders(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    account: Option<String>,
) -> Result<String, String> {
    let folders = mail.folders(account).await.map_err(|e| e.to_string())?;
    if folders.is_empty() {
        return Ok("未获取到任何文件夹".into());
    }
    Ok(folders.join("\n"))
}

async fn action_list(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    params: &Value,
) -> Result<String, String> {
    let q = MessagesQuery {
        folder: opt_str(params, "folder"),
        limit: params.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize),
        offset: params.get("offset").and_then(|v| v.as_u64()).map(|n| n as usize),
        unread: opt_bool(params, "unread"),
        category: opt_str(params, "category"),
        refresh: None,
        account: opt_str(params, "account"),
    };
    let page = mail.messages(q).await.map_err(|e| e.to_string())?;
    if page.messages.is_empty() {
        return Ok("没有邮件".into());
    }
    let mut out = Vec::new();
    out.push(format!(
        "文件夹: {}  总数: {}  未读: {}{}",
        if page.folder.is_empty() { "(分类视图)" } else { &page.folder },
        page.total,
        page.unread,
        if page.has_more { "  （还有更多）" } else { "" }
    ));
    out.push(format!("{:<10} {:<3} {:<6} {:<28} {:<20} {}", "UID", "FL", "MK", "FROM", "DATE", "SUBJECT"));
    for m in &page.messages {
        let mut marks = String::new();
        if m.flagged {
            marks.push('F');
        }
        if m.pinned {
            marks.push('P');
        }
        match m.category.as_deref() {
            Some("todo") => marks.push('T'),
            Some("doing") => marks.push('I'),
            Some("done") => marks.push('D'),
            _ => {}
        }
        out.push(format!(
            "{:<10} {:<3} {:<6} {:<28} {:<20} {}",
            m.uid,
            if m.seen { "" } else { "N" },
            if marks.is_empty() { "-" } else { &marks },
            trunc(&m.from, 28),
            trunc(&m.date, 20),
            trunc(&m.subject, 60),
        ));
    }
    Ok(out.join("\n"))
}

async fn action_read(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    params: &Value,
) -> Result<String, String> {
    let uid = opt_u32(params, "uid").ok_or_else(|| "缺少必填参数: uid".to_string())?;
    let folder = opt_str(params, "folder");
    let account = opt_str(params, "account");
    let detail = mail
        .message(uid, folder.clone(), account.clone(), false)
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    out.push(format!("Account: {}", account.unwrap_or_else(|| "(活动账号)".into())));
    out.push(format!("Folder: {}", folder.unwrap_or_else(|| "INBOX".into())));
    out.push(format!("From: {}", detail.from));
    out.push(format!("To: {}", detail.to));
    if !detail.cc.is_empty() {
        out.push(format!("Cc: {}", detail.cc));
    }
    out.push(format!("Subject: {}", detail.subject));
    out.push(format!("Date: {}", detail.date));
    out.push(format!("Status: {}", if detail.seen { "已读" } else { "未读" }));
    if !detail.attachments.is_empty() {
        out.push("Attachments:".into());
        for (i, a) in detail.attachments.iter().enumerate() {
            out.push(format!("  [{}] {} ({} bytes)", i, a.filename, a.size));
        }
    }
    out.push(String::new());
    match detail.text.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(text) => out.push(trunc(text, 20000)),
        None => {
            if detail.html.is_some() {
                out.push("[text/html message]".into());
            } else {
                out.push("(无正文)".into());
            }
        }
    }
    Ok(out.join("\n"))
}

/// 从工具参数构造发送输入（校验 to 必填）。供信封与最终发送共用。
fn send_input_of(params: &Value) -> Result<SendInput, String> {
    let input = SendInput {
        account: opt_str(params, "account"),
        to: split_list(params, "to"),
        cc: split_list(params, "cc"),
        bcc: split_list(params, "bcc"),
        subject: opt_str(params, "subject").unwrap_or_default(),
        body: params
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        html: params
            .get("html")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        attachments: split_list(params, "attachments"),
    };
    if input.to.is_empty() {
        return Err("缺少必填参数: to".into());
    }
    Ok(input)
}

/// 发送邮件的稳定命令 key（重放闸门 / 去重 / 审计；不含正文，避免过长与泄漏）。
fn send_key(input: &SendInput) -> String {
    format!(
        "mail send to={} cc={} bcc={} subject={} attachments={}",
        input.to.join(","),
        input.cc.join(","),
        input.bcc.join(","),
        input.subject,
        input.attachments.len()
    )
}

/// `send` 的门禁：Plan 硬拒 → 已拒短路 → 否则一律构造**写邮件确认中断信封**。
///
/// 发送**总是**需要用户通过写邮件 UI 确认（不受 Auto 影响：这不是可审批项，
/// 而是发信的必过人机确认）。用户点发送后把最终内容作为决议 `answer` 交回，
/// 由 `resume_interrupt` 真正调用 `mail.send`。
async fn action_send_gated(
    _mail: &std::sync::Arc<astorm_mail::MailService>,
    params: &Value,
    ctx: &ToolContext,
) -> Result<String, String> {
    let input = send_input_of(params)?;
    let key = send_key(&input);

    // Plan 只读是硬规则：先于一切判定，直接拒绝。
    if crate::tool::policy::plan_read_only(&ctx.session_id) {
        let d = crate::tool::denial::Denial::new(
            crate::tool::denial::DenialCategory::Mail,
            crate::tool::denial::DenialSource::StaticPrecheck,
            format!("当前模式为 Plan，不允许发送邮件（to={}，subject={}）。请切换到 Execute 或 Auto 模式后重试。", input.to.join("、"), input.subject),
        )
        .with_targets(input.to.clone())
        .with_command(key.clone());
        crate::tool::policy::audit(
            &ctx.session_id,
            crate::tool::policy::audit_kind_of(crate::tool::denial::DenialCategory::Mail),
            &d.detail,
            crate::tool::audit::AuditApproval::None,
            Some(key.as_str()),
            Some(crate::tool::denial::DenialCategory::Mail),
            Some(crate::tool::denial::DenialSource::StaticPrecheck),
        );
        return Err(crate::tool::denial::denial_error(&d));
    }
    // 已拒绝过同一封 → 短路，避免重复弹卡。
    if crate::tool::policy::is_denied(&ctx.session_id, &key) {
        return Err(format!(
            "该邮件刚被用户拒绝：{key}\n请勿重复发送同一封，可换内容或换思路。"
        ));
    }

    // 其余一律弹写邮件确认 UI。
    Ok(send_envelope(params, &key, ctx))
}

/// 构造 `mail_compose` 中断信封（工具结果 = 中断请求；引擎据此打断并透传前端）。
///
/// **信封刻意保持极小**：不塞收件人/主题/正文——它们已在本次 tool 调用的参数里
/// （前端按 `tool_call_id` 取回即可渲染预览；决议时后端也从那里取回参数发送）。
/// 只额外附带 `attachment_meta`（附件文件名 + 大小），因为大小需后端 stat、前端算不出。
/// 这样既避免 tool result 体积膨胀（长正文不再重复进历史），又保留附件体积展示。
fn send_envelope(params: &Value, key: &str, ctx: &ToolContext) -> String {
    let request_id = uuid::Uuid::new_v4().to_string();
    // 附件元数据：路径 + 文件名 + 大小（前端预览用文件树图标 + 体积；读不到 size 时为 0）
    let attachment_meta: Vec<Value> = split_list(params, "attachments")
        .iter()
        .map(|p| {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.clone());
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            json!({ "path": p, "name": name, "size": size })
        })
        .collect();
    let request = json!({
        "interrupt": true,
        "interrupt_type": "mail_compose",
        "request_id": request_id,
        "tool_call_id": ctx.tool_call_id,
        "message": "请确认并发送邮件",
        "mail_compose": { "attachment_meta": attachment_meta },
    });
    crate::tool::policy::register_pending_interrupt(
        &ctx.session_id,
        crate::tool::interrupt::PendingInterrupt {
            request_id: request_id.clone(),
            interrupt_type: astorm_common::session::InterruptType::MailCompose,
            tool_call_id: ctx.tool_call_id.clone(),
            tool_name: "mail".to_string(),
            request: request.clone(),
            command: Some(key.to_string()),
            requested_at: crate::agent::session::unix_now(),
        },
    );
    request.to_string()
}

/// 真实发送（由 `resume_interrupt` 在用户确认后调用）。返回模型可读结果。
pub async fn send_confirmed(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    payload: &Value,
) -> Result<String, String> {
    let input = SendInput {
        account: opt_str(payload, "account"),
        to: split_list(payload, "to"),
        cc: split_list(payload, "cc"),
        bcc: split_list(payload, "bcc"),
        subject: opt_str(payload, "subject").unwrap_or_default(),
        body: payload
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        html: payload
            .get("html")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        attachments: split_list(payload, "attachments"),
    };
    if input.to.is_empty() {
        return Err("缺少必填参数: to".into());
    }
    let sent_as = mail.send(input).await.map_err(|e| e.to_string())?;
    Ok(format!("邮件已发送（发件账号: {sent_as}）"))
}

async fn action_mark(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    params: &Value,
) -> Result<String, String> {
    let uid = opt_u32(params, "uid").ok_or_else(|| "缺少必填参数: uid".to_string())?;
    let flagged = match (opt_bool(params, "flagged"), opt_bool(params, "unflagged")) {
        (Some(true), _) => Some(true),
        (_, Some(true)) => Some(false),
        _ => None,
    };
    let pinned = match (opt_bool(params, "pinned"), opt_bool(params, "unpinned")) {
        (Some(true), _) => Some(true),
        (_, Some(true)) => Some(false),
        _ => None,
    };
    let category = match opt_str(params, "category").as_deref() {
        Some("none") | Some("clear") | Some("-") => Some(String::new()),
        Some(c @ ("todo" | "doing" | "done")) => Some(c.to_string()),
        Some(other) => return Err(format!("不支持的分类: {other}（可用 todo / doing / done / none）")),
        None => None,
    };
    if flagged.is_none() && pinned.is_none() && category.is_none() {
        return Err("mark 至少需要 flagged / pinned / category 之一".into());
    }
    let body = MetaUpdate {
        flagged,
        pinned,
        category,
        seen: None,
    };
    mail.set_meta(uid, opt_str(params, "folder"), opt_str(params, "account"), body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("UID {uid} 已更新"))
}

/// 移动到目标文件夹。`kind` 为 `Some` 时按标准类别（trash/junk）自动解析目标。
///
/// 门禁（与 browser 一致）：Plan 硬拒 → 已拒短路 → 审批/自动；通过后就地重放执行。
async fn action_move_gated(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    params: &Value,
    kind: Option<&str>,
    ctx: &ToolContext,
) -> Result<String, String> {
    let uid = opt_u32(params, "uid").ok_or_else(|| "缺少必填参数: uid".to_string())?;
    let account = opt_str(params, "account");
    let folder = opt_str(params, "folder");
    let label = match kind {
        Some("trash") => "移入废纸篓",
        Some("junk") => "移入垃圾邮件",
        _ => "移动",
    };
    let key = format!(
        "mail {} uid={} folder={} account={}",
        kind.unwrap_or("move"),
        uid,
        folder.as_deref().unwrap_or("INBOX"),
        account.as_deref().unwrap_or("")
    );

    // Plan 只读硬拒。
    if crate::tool::policy::plan_read_only(&ctx.session_id) {
        let d = crate::tool::denial::Denial::new(
            crate::tool::denial::DenialCategory::Mail,
            crate::tool::denial::DenialSource::StaticPrecheck,
            format!("当前模式为 Plan，不允许改邮箱（{label}，UID {uid}）。请切换到 Execute 或 Auto 后重试。"),
        )
        .with_command(key.clone());
        crate::tool::policy::audit(
            &ctx.session_id,
            crate::tool::policy::audit_kind_of(crate::tool::denial::DenialCategory::Mail),
            &d.detail,
            crate::tool::audit::AuditApproval::None,
            Some(key.as_str()),
            Some(crate::tool::denial::DenialCategory::Mail),
            Some(crate::tool::denial::DenialSource::StaticPrecheck),
        );
        return Err(crate::tool::denial::denial_error(&d));
    }
    // 已拒短路。
    if crate::tool::policy::is_denied(&ctx.session_id, &key) {
        return Err(format!(
            "该邮箱操作刚被用户拒绝：{key}\n请勿重复，换思路完成目标。"
        ));
    }
    // 审批 / 自动放行。
    let approved = crate::tool::policy::consume_allowed_rerun(&ctx.session_id, &key);
    if !approved && !crate::tool::policy::auto_guard(&ctx.session_id) {
        return Ok(crate::tool::policy::needs_approval(crate::tool::policy::ApprovalReq {
            denial: crate::tool::denial::Denial::new(
                crate::tool::denial::DenialCategory::Mail,
                crate::tool::denial::DenialSource::StaticPrecheck,
                format!("{label}：UID {uid}（folder={}）", folder.as_deref().unwrap_or("INBOX")),
            )
            .with_command(key.clone()),
            command: &key,
            session_id: &ctx.session_id,
            tool_name: "mail",
            tool_call_id: &ctx.tool_call_id,
        }));
    }

    let dest = match kind {
        Some(k) => {
            let folders = mail.folders(account.clone()).await.map_err(|e| e.to_string())?;
            standard_folders(&folders)
                .into_iter()
                .find(|f| folder_kind(f) == k)
                .ok_or_else(|| format!("未找到{}文件夹", if k == "trash" { "废纸篓" } else { "垃圾邮件" }))?
        }
        None => req_str(params, "to")?,
    };
    mail.move_message(uid, folder, account, dest.clone())
        .await
        .map_err(|e| e.to_string())?;
    crate::tool::policy::audit(
        &ctx.session_id,
        crate::tool::policy::audit_kind_of(crate::tool::denial::DenialCategory::Mail),
        &format!("{label}：UID {uid} → {dest}"),
        if approved {
            crate::tool::audit::AuditApproval::Agree
        } else {
            crate::tool::audit::AuditApproval::Auto
        },
        Some(key.as_str()),
        Some(crate::tool::denial::DenialCategory::Mail),
        Some(crate::tool::denial::DenialSource::StaticPrecheck),
    );
    Ok(format!("UID {uid} 已移动到「{dest}」（目标文件夹中 UID 会变化）"))
}

async fn action_download(
    mail: &std::sync::Arc<astorm_mail::MailService>,
    params: &Value,
    ctx: &ToolContext,
) -> Result<String, String> {
    let uid = opt_u32(params, "uid").ok_or_else(|| "缺少必填参数: uid".to_string())?;
    let index = opt_u32(params, "index").ok_or_else(|| "缺少必填参数: index".to_string())? as usize;
    let blob = mail
        .attachment(uid, index, opt_str(params, "folder"), opt_str(params, "account"))
        .await
        .map_err(|e| e.to_string())?;
    let workspace = resolve_workspace(&ctx.session_id);
    let dir = match opt_str(params, "out") {
        Some(raw) => {
            let expanded = expand_session_vars(&raw, &ctx.session_id, &workspace);
            let p = std::path::Path::new(&expanded);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                workspace.join(p)
            }
        }
        None => workspace.clone(),
    };
    // 附件落盘目录必须在会话工作区内：`out` 可为绝对路径，不能让模型
    // （或恶意邮件）把附件写到启动目录等任意位置。
    if !crate::tool::policy::path_in_workspace(&workspace, &dir) {
        return Err(format!(
            "附件保存目录必须位于会话工作区内: {}",
            dir.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))?;
    let mut safe_name = blob
        .filename
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    if safe_name.trim().is_empty() || is_windows_reserved_name(&safe_name) {
        safe_name = format!("attachment-{uid}-{index}");
    }
    let path = dir.join(safe_name);
    std::fs::write(&path, &blob.data).map_err(|e| format!("写入附件失败: {e}"))?;
    Ok(format!("附件已保存到 {}", path.display()))
}

/// Windows 保留设备名（大小写不敏感，带扩展名同样命中）。
fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name
        .trim_end_matches(['.', ' '])
        .split('.')
        .next()
        .unwrap_or(name)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    )
}

fn trunc(s: &str, max: usize) -> String {
    let cleaned: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= max {
        return cleaned;
    }
    let mut out: String = cleaned.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_list_handles_array_and_string() {
        let v = json!({ "a": ["x, y", "z"], "b": "p,q" });
        assert_eq!(split_list(&v, "a"), vec!["x", "y", "z"]);
        assert_eq!(split_list(&v, "b"), vec!["p", "q"]);
        assert!(split_list(&v, "missing").is_empty());
    }

    #[test]
    fn trunc_collapses_and_ellipsizes() {
        assert_eq!(trunc("a  b", 10), "a b");
        assert!(trunc(&"x".repeat(50), 10).ends_with('…'));
    }

    #[test]
    fn send_input_requires_to() {
        assert!(send_input_of(&json!({"subject": "x"})).is_err());
        let ok = send_input_of(&json!({"to": "a@x.com, b@x.com", "subject": "hi"})).unwrap();
        assert_eq!(ok.to, vec!["a@x.com", "b@x.com"]);
    }

    #[test]
    fn send_key_is_stable_and_excludes_body() {
        let a = send_input_of(&json!({"to": "a@x.com", "subject": "hi", "body": "secret-1"})).unwrap();
        let b = send_input_of(&json!({"to": "a@x.com", "subject": "hi", "body": "secret-2"})).unwrap();
        // 正文不同但收件人/主题相同 → key 一致（不含正文）
        assert_eq!(send_key(&a), send_key(&b));
        assert!(send_key(&a).contains("to=a@x.com"));
    }

    #[test]
    fn compose_envelope_is_minimal() {
        let params = json!({
            "to": "a@x.com",
            "cc": "c@x.com",
            "subject": "你好",
            "body": "很长很长的正文……",
            "html": "<b>富文本</b>",
            "attachments": ["/tmp/a.pdf"]
        });
        // send_envelope 会登记待决（App 未初始化时静默），故可直接调用并解析。
        let ctx = ToolContext::new(tokio_util::sync::CancellationToken::new());
        let text = send_envelope(&params, "mail send to=a@x.com subject=你好", &ctx);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["interrupt"], true);
        assert_eq!(v["interrupt_type"], "mail_compose");
        assert!(v["request_id"].as_str().unwrap().len() >= 32);
        // 信封**不含**收件人/主题/正文（避免重复进历史；它们已在 tool 调用参数里）
        assert!(v.get("mail_params").is_none(), "信封不得再带 mail_params");
        let mc = &v["mail_compose"];
        assert!(mc.get("body").is_none(), "信封不得带正文");
        assert!(mc.get("to").is_none());
        assert!(mc.get("subject").is_none());
        // 只带附件元数据（文件名 + 大小；路径不存在时 size=0，但结构必须在）
        assert_eq!(mc["attachment_meta"][0]["name"], "a.pdf");
        assert_eq!(mc["attachment_meta"][0]["size"], 0);
    }
}
