//! SQLite-backed mail cache: message summaries, bodies, and local metadata
//! (pin / category) persisted in `~/.mail-proxy/mail.db`.
//!
//! `flagged` mirrors the IMAP `\Flagged` flag (kept in sync by the caller);
//! `pinned` and `category` are local-only. Bodies are cached for instant
//! reopen and offline reading; attachments are always streamed from IMAP.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;

use crate::imap_client::MailSummary;

/// 单个正文字段（text / html）缓存上限：超过则**截断**保存（不再直接丢弃，避免
/// 「点开过的邮件永远命中不了缓存 → 每次重下」）。
const BODY_CACHE_LIMIT: usize = 2 * 1024 * 1024;

/// 正文 / 摘要缓存的**有效期**：30 天。过期即视为 miss（下次点开重新拉取），
/// 并由 [`MailDb::prune_expired_bodies`] 自动清理。
const BODY_TTL_SECS: i64 = 30 * 24 * 60 * 60;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A cached message row (summary + local metadata).
#[derive(Debug, Clone, Serialize)]
pub struct CachedMessage {
    pub uid: u32,
    /// 所在文件夹（分类视图跨文件夹时需要）
    pub folder: String,
    pub seen: bool,
    pub flagged: bool,
    pub pinned: bool,
    pub category: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub size: u64,
    pub ts: i64,
    pub preview: Option<String>,
}

/// One page of cached messages plus folder totals.
#[derive(Debug, Clone, Serialize)]
pub struct CachedPage {
    pub messages: Vec<CachedMessage>,
    pub total: u32,
    pub unread: u32,
    /// Rows matching the current filter (category/unread), used for paging.
    pub filtered_total: u32,
    pub has_more: bool,
}

/// Cached body + attachment metadata for one message.
#[derive(Debug, Clone)]
pub struct CachedBody {
    pub text: String,
    pub html: String,
    /// JSON array of `{filename, size}` as stored by the web layer.
    pub attachments: Option<String>,
    /// 正文是否因超过 [`BODY_CACHE_LIMIT`] 被截断保存。
    pub truncated: bool,
}

pub struct MailDb {
    conn: Mutex<Connection>,
}

impl MailDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).context("open mail.db")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().context("open in-memory db")?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // WAL 下 NORMAL 已足够安全，写入更快
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch("PRAGMA optimize;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                account    TEXT NOT NULL,
                folder     TEXT NOT NULL,
                uid        INTEGER NOT NULL,
                message_id TEXT,
                seen       INTEGER NOT NULL DEFAULT 0,
                flagged    INTEGER NOT NULL DEFAULT 0,
                pinned     INTEGER NOT NULL DEFAULT 0,
                category   TEXT,
                from_addr  TEXT,
                to_addr    TEXT,
                subject    TEXT,
                date       TEXT,
                ts         INTEGER NOT NULL DEFAULT 0,
                size       INTEGER NOT NULL DEFAULT 0,
                preview    TEXT,
                body_text  TEXT,
                body_html  TEXT,
                attachments TEXT,
                body_fetched_at INTEGER,
                body_truncated INTEGER NOT NULL DEFAULT 0,
                preview_fetched_at INTEGER,
                updated_at INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account, folder, uid)
            );
            CREATE INDEX IF NOT EXISTS idx_msg_category ON messages(account, folder, category);
            -- 列表分页排序（pinned 置顶 → ts 倒序）用的复合索引，避免每次 page() 全表建临时 B 树
            CREATE INDEX IF NOT EXISTS idx_msg_sort ON messages(account, folder, pinned DESC, ts DESC, uid DESC);
            CREATE INDEX IF NOT EXISTS idx_msg_sort_all ON messages(account, pinned DESC, ts DESC, uid DESC);
            CREATE TABLE IF NOT EXISTS folders (
                account TEXT NOT NULL,
                name    TEXT NOT NULL,
                total   INTEGER NOT NULL DEFAULT 0,
                unread  INTEGER NOT NULL DEFAULT 0,
                synced_at INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account, name)
            );",
        )?;
        // 迁移（幂等）：老库补两列。SQLite 无 IF NOT EXISTS 的 ADD COLUMN，故查表后补。
        Self::ensure_column(&conn, "messages", "body_truncated", "INTEGER NOT NULL DEFAULT 0")?;
        Self::ensure_column(&conn, "messages", "preview_fetched_at", "INTEGER")?;
        // 过期清理用的索引（body/preview 的 fetched_at）：必须在补列之后建（老库此时才有该列）。
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_msg_body_fetched ON messages(body_fetched_at);
             CREATE INDEX IF NOT EXISTS idx_msg_preview_fetched ON messages(preview_fetched_at);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 若列不存在则 `ALTER TABLE ADD COLUMN`（幂等迁移）。
    fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
        let exists = {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let mut found = false;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == column {
                    found = true;
                    break;
                }
            }
            found
        };
        if !exists {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
        }
        Ok(())
    }

    /// Insert or refresh summaries. Existing pinned/category/body/preview
    /// columns are preserved.
    pub fn upsert_page(&self, account: &str, folder: &str, msgs: &[MailSummary]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // 单事务 + 复用 prepared statement：整页一次提交（而非每行一次 WAL fsync）。
        let tx = conn.unchecked_transaction()?;
        let ts = now();
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO messages (account, folder, uid, seen, flagged, from_addr, to_addr,
                                       subject, date, ts, size, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(account, folder, uid) DO UPDATE SET
                    seen = excluded.seen,
                    flagged = excluded.flagged,
                    from_addr = excluded.from_addr,
                    to_addr = excluded.to_addr,
                    subject = excluded.subject,
                    date = excluded.date,
                    ts = excluded.ts,
                    size = excluded.size,
                    updated_at = excluded.updated_at",
            )?;
            for m in msgs {
                stmt.execute(params![
                    account,
                    folder,
                    m.uid,
                    m.seen,
                    m.flagged,
                    m.from,
                    m.to,
                    m.subject,
                    m.date,
                    m.ts,
                    m.size as i64,
                    ts
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// One page of cached messages (newest first, pinned on top) plus totals.
    /// `folder = None` means "all folders" (used by the category views).
    pub fn page(
        &self,
        account: &str,
        folder: Option<&str>,
        limit: usize,
        offset: usize,
        unread_only: bool,
        category: Option<&str>,
    ) -> Result<CachedPage> {
        let conn = self.conn.lock().unwrap();

        // 动态 WHERE：用无编号占位符，避免 folder 缺省时编号错位
        let mut where_sql = String::from("account = ?");
        if folder.is_some() {
            where_sql.push_str(" AND folder = ?");
        }
        if unread_only {
            where_sql.push_str(" AND seen = 0");
        }
        if category.is_some() {
            where_sql.push_str(" AND category = ?");
        }
        let bind = |params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>| {
            params_vec.push(Box::new(account.to_string()));
            if let Some(f) = folder {
                params_vec.push(Box::new(f.to_string()));
            }
            if let Some(c) = category {
                params_vec.push(Box::new(c.to_string()));
            }
        };
        let mut pv = Vec::new();
        bind(&mut pv);
        let filtered_total: u32 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM messages WHERE {where_sql}"),
                params_from_iter(pv.iter().map(|b| b.as_ref())),
                |r| r.get(0),
            )
            .context("count filtered")?;

        // 文件夹视图下 total/unread 优先用 folders 表里服务端 STATUS 的结果；
        // 跨文件夹视图（None）退回过滤计数。
        let (total, unread) = match folder {
            Some(f) => {
                let row = conn
                    .query_row(
                        "SELECT total, unread FROM folders WHERE account = ?1 AND name = ?2",
                        params![account, f],
                        |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)),
                    )
                    .optional()?;
                match row {
                    Some((t, u)) if t > 0 => (t, u),
                    _ => (filtered_total, 0),
                }
            }
            None => (filtered_total, 0),
        };

        let mut pv = Vec::new();
        bind(&mut pv);
        let sql = format!(
            "SELECT uid, folder, seen, flagged, pinned, category, from_addr, to_addr, subject, date, size, ts, preview
             FROM messages WHERE {where_sql}
             ORDER BY pinned DESC, ts DESC, uid DESC
             LIMIT {limit} OFFSET {offset}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let messages = stmt
            .query_map(params_from_iter(pv.iter().map(|b| b.as_ref())), row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("read page")?;

        // 分页余量按「服务端总数」判断：本地缓存未覆盖的页由调用方回源补齐。
        let paging_total = total.max(filtered_total);
        // 注意：`as u32 <` 会被解析成泛型参数，必须给类型转换加括号
        let has_more = ((offset + messages.len()) as u32) < paging_total;
        Ok(CachedPage {
            has_more,
            messages,
            total,
            unread,
            filtered_total,
        })
    }

    /// 只统计过滤后的行数（`page()` 的轻量版）——供调用方判断「缓存是否为空 / 是否够翻页」，
    /// 避免为了拿 `filtered_total` 而跑一次带排序的分页查询。
    pub fn filtered_count(
        &self,
        account: &str,
        folder: Option<&str>,
        unread_only: bool,
        category: Option<&str>,
    ) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        let mut where_sql = String::from("account = ?");
        if folder.is_some() {
            where_sql.push_str(" AND folder = ?");
        }
        if unread_only {
            where_sql.push_str(" AND seen = 0");
        }
        if category.is_some() {
            where_sql.push_str(" AND category = ?");
        }
        let mut pv: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(account.to_string())];
        if let Some(f) = folder {
            pv.push(Box::new(f.to_string()));
        }
        if let Some(c) = category {
            pv.push(Box::new(c.to_string()));
        }
        let n = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM messages WHERE {where_sql}"),
                params_from_iter(pv.iter().map(|b| b.as_ref())),
                |r| r.get(0),
            )
            .context("count filtered")?;
        Ok(n)
    }

    /// Cached body + attachment metadata, if **fetched and not expired**.
    ///
    /// 命中条件用 `body_fetched_at`（「已拉取过」标记，且未超 30 天）而非
    /// 「text/html 非空」——这样纯附件 / 无正文 / 超限截断的邮件点过一次后
    /// 也不会永远 miss、反复重下。过期 → None（下次点开重新拉取）。
    pub fn get_body(&self, account: &str, folder: &str, uid: u32) -> Result<Option<CachedBody>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now() - BODY_TTL_SECS;
        let row = conn
            .query_row(
                "SELECT body_text, body_html, attachments, body_truncated FROM messages
                 WHERE account = ?1 AND folder = ?2 AND uid = ?3
                   AND body_fetched_at IS NOT NULL AND body_fetched_at >= ?4",
                params![account, folder, uid, cutoff],
                |r| {
                    Ok(CachedBody {
                        text: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        html: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        attachments: r.get(2)?,
                        truncated: r.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// The cached summary row for one message, if present.
    pub fn get_message(&self, account: &str, folder: &str, uid: u32) -> Result<Option<CachedMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT uid, folder, seen, flagged, pinned, category, from_addr, to_addr, subject, date, size, ts, preview
             FROM messages WHERE account = ?1 AND folder = ?2 AND uid = ?3",
        )?;
        let row = stmt
            .query_row(params![account, folder, uid], row_to_message)
            .optional()?;
        Ok(row)
    }

    /// Message counts per category across all folders of an account.
    pub fn category_counts(&self, account: &str) -> Result<Vec<(String, u32)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT category, COUNT(*) FROM messages
             WHERE account = ?1 AND category IS NOT NULL
             GROUP BY category",
        )?;
        let rows = stmt
            .query_map(params![account], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("count categories")?;
        Ok(rows)
    }

    /// Cache a message body + attachment metadata.
    ///
    /// 超过 [`BODY_CACHE_LIMIT`] 的字段**截断保存**（不丢弃），保证点开过的邮件
    /// 必定缓存、不再重复下载。返回是否发生截断。`body_fetched_at` 记当前时刻（TTL 基准）。
    pub fn save_body(
        &self,
        account: &str,
        folder: &str,
        uid: u32,
        text: Option<&str>,
        html: Option<&str>,
        attachments: Option<&str>,
    ) -> Result<bool> {
        self.ensure_row(account, folder, uid)?;
        let conn = self.conn.lock().unwrap();
        let (text, t_trunc) = clamp_body(text);
        let (html, h_trunc) = clamp_body(html);
        let truncated = t_trunc || h_trunc;
        conn.execute(
            "UPDATE messages SET body_text = ?4, body_html = ?5, attachments = ?6,
                    body_fetched_at = ?7, body_truncated = ?8
             WHERE account = ?1 AND folder = ?2 AND uid = ?3",
            params![
                account,
                folder,
                uid,
                text,
                html,
                attachments,
                now(),
                truncated as i64
            ],
        )?;
        Ok(truncated)
    }

    /// Persist lazily fetched previews (records `preview_fetched_at` for TTL).
    pub fn save_previews(
        &self,
        account: &str,
        folder: &str,
        previews: &HashMap<u32, String>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let at = now();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE messages SET preview = ?4, preview_fetched_at = ?5
                 WHERE account = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for (uid, text) in previews {
                stmt.execute(params![account, folder, uid, text, at])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 已缓存且未过期的摘要（`uid → preview`）；供 `previews()` 先读缓存、只补缺失。
    pub fn get_previews(
        &self,
        account: &str,
        folder: &str,
        uids: &[u32],
    ) -> Result<HashMap<u32, String>> {
        if uids.is_empty() {
            return Ok(HashMap::new());
        }
        let conn = self.conn.lock().unwrap();
        let cutoff = now() - BODY_TTL_SECS;
        // 只读请求的这 ~30 个 uid（SQL `IN`），不做整文件夹扫描。
        let placeholders = uids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT uid, preview FROM messages
             WHERE account = ? AND folder = ? AND uid IN ({placeholders})
               AND preview IS NOT NULL AND preview_fetched_at IS NOT NULL AND preview_fetched_at >= ?"
        );
        let mut pv: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(account.to_string()),
            Box::new(folder.to_string()),
        ];
        for u in uids {
            pv.push(Box::new(*u));
        }
        pv.push(Box::new(cutoff));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(pv.iter().map(|b| b.as_ref())), |r| {
                Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<HashMap<u32, String>, _>>()
            .context("read previews")?;
        Ok(rows)
    }

    /// 清理超过 [`BODY_TTL_SECS`] 的正文 / 摘要缓存（**保留摘要行**：列表 / 置顶 /
    /// 分类不受影响）。返回清理的行数。
    pub fn prune_expired_bodies(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now() - BODY_TTL_SECS;
        let n = conn.execute(
            "UPDATE messages SET
                body_text = NULL, body_html = NULL, attachments = NULL,
                body_fetched_at = NULL, body_truncated = 0
             WHERE body_fetched_at IS NOT NULL AND body_fetched_at < ?1",
            params![cutoff],
        )?;
        conn.execute(
            "UPDATE messages SET preview = NULL, preview_fetched_at = NULL
             WHERE preview_fetched_at IS NOT NULL AND preview_fetched_at < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    /// Update local metadata. `category = Some("")` clears it. Creates a stub
    /// row when the message is not cached yet.
    pub fn set_meta(
        &self,
        account: &str,
        folder: &str,
        uid: u32,
        flagged: Option<bool>,
        pinned: Option<bool>,
        category: Option<&str>,
        seen: Option<bool>,
    ) -> Result<()> {
        if flagged.is_none() && pinned.is_none() && category.is_none() && seen.is_none() {
            return Ok(());
        }
        self.ensure_row(account, folder, uid)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET
                flagged = COALESCE(?4, flagged),
                pinned = COALESCE(?5, pinned),
                category = CASE
                    WHEN ?6 = '' THEN NULL
                    WHEN ?6 IS NOT NULL THEN ?6
                    ELSE category END,
                seen = COALESCE(?7, seen)
             WHERE account = ?1 AND folder = ?2 AND uid = ?3",
            params![account, folder, uid, flagged, pinned, category, seen],
        )?;
        Ok(())
    }

    /// Remove a row (after the message was moved/deleted on the server).
    pub fn delete_row(&self, account: &str, folder: &str, uid: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM messages WHERE account = ?1 AND folder = ?2 AND uid = ?3",
            params![account, folder, uid],
        )?;
        Ok(())
    }

    /// Record the folder list returned by IMAP LIST (keeps existing stats).
    pub fn save_folders(&self, account: &str, names: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO folders (account, name, total, unread, synced_at)
                 VALUES (?1, ?2, 0, 0, 0)",
            )?;
            for name in names {
                stmt.execute(params![account, name])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Cached folder names for an account (instant sidebar render).
    pub fn get_folders(&self, account: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT name FROM folders WHERE account = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(params![account], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("read folders")?;
        Ok(rows)
    }

    /// Cached (name, total, unread) per folder, for instant sidebar badges.
    pub fn folder_stats_cached(&self, account: &str) -> Result<Vec<(String, u32, u32)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, total, unread FROM folders WHERE account = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map(params![account], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("read folder stats")?;
        Ok(rows)
    }

    /// Drop everything cached for an account (after it was removed).
    pub fn delete_account(&self, account: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE account = ?1", params![account])?;
        conn.execute("DELETE FROM folders WHERE account = ?1", params![account])?;
        Ok(())
    }

    /// Update folder totals without touching `synced_at` — used when paging
    /// back-fills older pages, which must not make the newest page look fresh.
    pub fn update_folder_totals(
        &self,
        account: &str,
        folder: &str,
        total: u32,
        unread: u32,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE folders SET total = ?3, unread = ?4 WHERE account = ?1 AND name = ?2",
            params![account, folder, total, unread],
        )?;
        Ok(())
    }

    /// Record folder totals and the sync timestamp.
    pub fn mark_synced(&self, account: &str, folder: &str, total: u32, unread: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO folders (account, name, total, unread, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account, name) DO UPDATE SET
                total = excluded.total,
                unread = excluded.unread,
                synced_at = excluded.synced_at",
            params![account, folder, total, unread, now()],
        )?;
        Ok(())
    }

    pub fn synced_at(&self, account: &str, folder: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let ts: i64 = conn
            .query_row(
                "SELECT synced_at FROM folders WHERE account = ?1 AND name = ?2",
                params![account, folder],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(ts)
    }

    /// Make sure a (mostly empty) row exists so metadata can be written even
    /// for messages that were never synced.
    fn ensure_row(&self, account: &str, folder: &str, uid: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO messages (account, folder, uid, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![account, folder, uid, now()],
        )?;
        Ok(())
    }
}

/// 正文字段按 [`BODY_CACHE_LIMIT`] 截断（UTF-8 边界安全）。返回 `(值, 是否截断)`。
pub(crate) fn clamp_body(v: Option<&str>) -> (Option<String>, bool) {
    match v {
        None => (None, false),
        Some(s) if s.len() <= BODY_CACHE_LIMIT => (Some(s.to_string()), false),
        Some(s) => {
            let mut end = BODY_CACHE_LIMIT;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            (Some(s[..end].to_string()), true)
        }
    }
}

fn row_to_message(r: &rusqlite::Row) -> rusqlite::Result<CachedMessage> {
    // 元数据存根行（尚未同步正文/摘要）的文本列可能为 NULL，统一按空串处理
    let text = |i: usize| -> rusqlite::Result<String> {
        Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default())
    };
    Ok(CachedMessage {
        uid: r.get(0)?,
        folder: text(1)?,
        seen: r.get::<_, i64>(2)? != 0,
        flagged: r.get::<_, i64>(3)? != 0,
        pinned: r.get::<_, i64>(4)? != 0,
        category: r.get(5)?,
        from: text(6)?,
        to: text(7)?,
        subject: text(8)?,
        date: text(9)?,
        size: r.get::<_, i64>(10)?.max(0) as u64,
        ts: r.get::<_, i64>(11)?,
        preview: r.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(uid: u32, ts: i64, subject: &str) -> MailSummary {
        MailSummary {
            uid,
            seen: false,
            flagged: false,
            from: format!("sender{uid}@x.com"),
            to: "me@x.com".into(),
            subject: subject.into(),
            date: format!("2026-01-0{}T10:00:00+08:00", uid % 9 + 1),
            size: 1000,
            ts,
        }
    }

    fn db() -> MailDb {
        MailDb::open_in_memory().unwrap()
    }

    #[test]
    fn upsert_preserves_meta_and_body() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 100, "first")]).unwrap();
        d.set_meta("a", "INBOX", 1, None, Some(true), Some("todo"), None).unwrap();
        d.save_body("a", "INBOX", 1, Some("hello"), None, None).unwrap();

        d.upsert_page("a", "INBOX", &[summary(1, 200, "updated")]).unwrap();
        let page = d.page("a", Some("INBOX"), 10, 0, false, None).unwrap();
        let m = &page.messages[0];
        assert!(m.pinned && m.category.as_deref() == Some("todo"));
        assert_eq!(m.ts, 200, "summary fields should refresh");
        let body = d.get_body("a", "INBOX", 1).unwrap().unwrap();
        assert_eq!(body.text, "hello");
    }

    #[test]
    fn page_orders_pinned_first_and_filters_category() {
        let d = db();
        d.upsert_page(
            "a",
            "INBOX",
            &[
                summary(1, 100, "a"),
                summary(2, 300, "b"),
                summary(3, 200, "c"),
            ],
        )
        .unwrap();
        d.set_meta("a", "INBOX", 1, None, Some(true), None, None).unwrap();
        let page = d.page("a", Some("INBOX"), 10, 0, false, None).unwrap();
        let uids: Vec<u32> = page.messages.iter().map(|m| m.uid).collect();
        assert_eq!(uids, vec![1, 2, 3], "pinned first, then ts desc");

        d.set_meta("a", "INBOX", 2, None, None, Some("todo"), None).unwrap();
        let todo = d.page("a", Some("INBOX"), 10, 0, false, Some("todo")).unwrap();
        assert_eq!(todo.messages.len(), 1);
        assert_eq!(todo.messages[0].uid, 2);
        assert_eq!(todo.filtered_total, 1);
    }

    #[test]
    fn meta_on_uncached_message_creates_stub() {
        let d = db();
        d.set_meta("a", "INBOX", 42, Some(true), None, None, None).unwrap();
        let page = d.page("a", Some("INBOX"), 10, 0, false, None).unwrap();
        assert!(page.messages[0].flagged);
    }

    #[test]
    fn body_over_limit_is_truncated_not_dropped() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 1, "s")]).unwrap();
        let big = "x".repeat(3 * 1024 * 1024);
        let truncated = d.save_body("a", "INBOX", 1, Some(&big), None, None).unwrap();
        assert!(truncated, "超限应标记为截断");
        // 关键：点开过的邮件不再 miss（否则每次重下）；返回截断后的内容。
        let body = d.get_body("a", "INBOX", 1).unwrap().expect("应命中缓存");
        assert!(body.truncated);
        assert_eq!(body.text.len(), BODY_CACHE_LIMIT);
    }

    #[test]
    fn body_without_text_or_html_still_hits_cache() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 1, "s")]).unwrap();
        // 纯附件 / 无正文邮件：text/html 均空，但已拉取过 → 应命中（空正文），不重下。
        d.save_body("a", "INBOX", 1, None, None, Some("[]")).unwrap();
        let body = d.get_body("a", "INBOX", 1).unwrap().expect("应命中缓存");
        assert!(body.text.is_empty() && body.html.is_empty());
    }

    #[test]
    fn expired_body_is_a_miss_and_pruned() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 1, "s")]).unwrap();
        d.save_body("a", "INBOX", 1, Some("hi"), None, None).unwrap();
        assert!(d.get_body("a", "INBOX", 1).unwrap().is_some());
        // 手动把拉取时间改到 31 天前 → 过期即 miss。
        {
            let conn = d.conn.lock().unwrap();
            conn.execute(
                "UPDATE messages SET body_fetched_at = ?1 WHERE uid = 1",
                params![now() - BODY_TTL_SECS - 10],
            )
            .unwrap();
        }
        assert!(d.get_body("a", "INBOX", 1).unwrap().is_none(), "过期应为 miss");
        // 清理：正文被清空，但摘要行（列表）保留。
        assert_eq!(d.prune_expired_bodies().unwrap(), 1);
        assert!(d.get_body("a", "INBOX", 1).unwrap().is_none());
        let page = d.page("a", Some("INBOX"), 10, 0, false, None).unwrap();
        assert_eq!(page.messages.len(), 1, "摘要行必须保留");
    }

    #[test]
    fn previews_cache_roundtrip_and_missing() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 1, "s"), summary(2, 2, "t")]).unwrap();
        let mut m = HashMap::new();
        m.insert(1u32, "hello".to_string());
        d.save_previews("a", "INBOX", &m).unwrap();
        let got = d.get_previews("a", "INBOX", &[1, 2]).unwrap();
        assert_eq!(got.get(&1).map(String::as_str), Some("hello"));
        assert!(!got.contains_key(&2), "未缓存的 uid 不应返回");
    }

    #[test]
    fn clear_category_and_delete_row() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 1, "s")]).unwrap();
        d.set_meta("a", "INBOX", 1, None, None, Some("done"), None).unwrap();
        d.set_meta("a", "INBOX", 1, None, None, Some(""), None).unwrap();
        let page = d.page("a", Some("INBOX"), 10, 0, false, None).unwrap();
        assert_eq!(page.messages[0].category, None);
        d.delete_row("a", "INBOX", 1).unwrap();
        assert_eq!(d.page("a", Some("INBOX"), 10, 0, false, None).unwrap().messages.len(), 0);
    }

    #[test]
    fn folders_cache_roundtrip() {
        let d = db();
        d.save_folders("a", &["INBOX".into(), "Sent Messages".into()]).unwrap();
        // 再次保存不应清掉已有统计
        d.mark_synced("a", "INBOX", 42, 7).unwrap();
        d.save_folders("a", &["INBOX".into(), "Sent Messages".into(), "Junk".into()])
            .unwrap();
        assert_eq!(
            d.get_folders("a").unwrap(),
            vec!["INBOX", "Junk", "Sent Messages"]
        );
        let stats = d.folder_stats_cached("a").unwrap();
        let inbox = stats.iter().find(|(n, _, _)| n == "INBOX").unwrap();
        assert_eq!((inbox.1, inbox.2), (42, 7), "已有计数应保留");

        d.delete_account("a").unwrap();
        assert!(d.get_folders("a").unwrap().is_empty());
        assert!(d.folder_stats_cached("a").unwrap().is_empty());
    }
    #[test]
    fn category_view_spans_folders() {
        let d = db();
        d.upsert_page("a", "INBOX", &[summary(1, 100, "inbox")]).unwrap();
        d.upsert_page("a", "Sent Messages", &[summary(2, 200, "sent")]).unwrap();
        d.set_meta("a", "INBOX", 1, None, None, Some("todo"), None).unwrap();
        d.set_meta("a", "Sent Messages", 2, None, None, Some("todo"), None).unwrap();
        d.set_meta("a", "Sent Messages", 2, None, None, Some("done"), None).unwrap();

        // folder = None → 跨文件夹；只返回分类匹配的行，并带出所在文件夹
        let todo = d.page("a", None, 10, 0, false, Some("todo")).unwrap();
        assert_eq!(todo.messages.len(), 1);
        assert_eq!(todo.messages[0].folder, "INBOX");

        let done = d.page("a", None, 10, 0, false, Some("done")).unwrap();
        assert_eq!(done.messages[0].folder, "Sent Messages");

        let counts = d.category_counts("a").unwrap();
        assert_eq!(counts, vec![("done".to_string(), 1), ("todo".to_string(), 1)]);
    }
}
