//! 邮件服务：连接池 + 本地 SQLite 缓存 + 后台同步 + 账号/收发/整理操作。
//!
//! 由 `web.rs`（原 mail-proxy 的 axum 层）移植而来：去掉 HTTP/multipart，
//! 数据目录由调用方注入（应用数据目录下的 `mail/`），对外暴露纯异步 Rust API。
//! 所有方法都在 tokio 运行时上下文中调用（napi async / 工具执行），后台同步用
//! `tokio::spawn`。

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::account::{self, AddFields};
use crate::db::{CachedMessage, MailDb};
use crate::error::{MailError, MailResult};
use crate::imap_client::{
    attachment_from_bytes, detail_from_bytes, AttachmentData, ImapClient, MailAttachment,
    MailDetail,
};
use crate::store::{Account, Store, TlsMode};
use crate::smtp_client;

const FOLDERS_TTL: Duration = Duration::from_secs(300);
const COUNTS_TTL: Duration = Duration::from_secs(30);
const SYNC_TTL_SECS: i64 = 60;
const SYNC_PAGE: usize = 50;
const MAX_BACKFILL: usize = 200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const IMAP_TIMEOUT: Duration = Duration::from_secs(30);
/// 凭据缓存里「缺省活动账号」的哨兵键（账号名不含 NUL，不会冲突）。
const ACTIVE_CRED_KEY: &str = "\u{0}active";

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

// ---------------------------------------------------------------------------
// 输出类型（serde；napi 层再做镜像）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub name: String,
    pub display_name: Option<String>,
    pub username: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_tls: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: String,
}

impl From<&Account> for AccountView {
    fn from(a: &Account) -> Self {
        AccountView {
            name: a.name.clone(),
            display_name: a.display_name.clone(),
            username: a.username.clone(),
            imap_host: a.imap_host.clone(),
            imap_port: a.imap_port,
            imap_tls: a.imap_tls.as_str().to_string(),
            smtp_host: a.smtp_host.clone(),
            smtp_port: a.smtp_port,
            smtp_tls: a.smtp_tls.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateView {
    pub active: Option<String>,
    pub presets: Vec<String>,
    pub accounts: Vec<AccountView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCountView {
    pub folder: String,
    pub total: u32,
    pub unread: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MsgListView {
    pub folder: String,
    pub total: u32,
    pub unread: u32,
    pub has_more: bool,
    pub synced_at: i64,
    pub stale: bool,
    pub messages: Vec<CachedMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentBlob {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl From<AttachmentData> for AttachmentBlob {
    fn from(a: AttachmentData) -> Self {
        Self {
            filename: a.filename,
            content_type: a.content_type,
            data: a.data,
        }
    }
}

// ---------------------------------------------------------------------------
// 输入类型（serde camelCase；前端 / 工具直接 JSON）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountInput {
    pub preset: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub display_name: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub imap_tls: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_tls: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAccountInput {
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub imap_tls: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_tls: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagesQuery {
    pub folder: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub unread: Option<bool>,
    pub category: Option<String>,
    pub refresh: Option<bool>,
    pub account: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaUpdate {
    pub flagged: Option<bool>,
    pub pinned: Option<bool>,
    pub category: Option<String>,
    /// 已读/未读:true = 标为已读,false = 标为未读。
    pub seen: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendInput {
    pub account: Option<String>,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    /// 正文的 HTML 版本（富文本编辑产出）。为空时只发 `body` 的纯文本；
    /// 非空时以 multipart/alternative 同时携带 HTML 与纯文本回退。
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
}

// ---------------------------------------------------------------------------
// 服务
// ---------------------------------------------------------------------------

/// 原始报文内存缓存：同一封邮件的正文解析与附件解码复用一份 `BODY[]`，
/// 避免每次点附件都重新全量下载整封（这是「点附件要等 1~2 秒」的根因）。
/// 按「最近使用」淘汰，容量受条数 + 总字节双重约束。
const RAW_CACHE_MAX_ITEMS: usize = 8;
const RAW_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct RawCache {
    map: HashMap<String, (Arc<Vec<u8>>, bool)>,
    order: VecDeque<String>,
    bytes: usize,
}

impl RawCache {
    /// 命中即把该键移到队尾（真正的 LRU：淘汰最久未用，而非最久插入）。
    fn get(&mut self, key: &str) -> Option<(Arc<Vec<u8>>, bool)> {
        let hit = self.map.get(key).cloned();
        if hit.is_some() {
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_string());
        }
        hit
    }
    fn put(&mut self, key: String, raw: Vec<u8>, seen: bool) -> (Arc<Vec<u8>>, bool) {
        if let Some(existing) = self.map.get(&key) {
            return existing.clone();
        }
        let arc = Arc::new(raw);
        self.bytes += arc.len();
        self.map.insert(key.clone(), (Arc::clone(&arc), seen));
        self.order.push_back(key);
        while self.order.len() > RAW_CACHE_MAX_ITEMS || self.bytes > RAW_CACHE_MAX_BYTES {
            let Some(old) = self.order.pop_front() else { break };
            if let Some((evicted, _)) = self.map.remove(&old) {
                self.bytes = self.bytes.saturating_sub(evicted.len());
            }
        }
        (arc, seen)
    }
}

/// 每账号 IMAP 连接池：并发上限 `POOL_MAX`，空闲连接复用（免去反复 TCP+TLS+LOGIN）。
///
/// 相比「单会话 + 一把互斥锁」：并发操作（打开正文 / 拉摘要 / 取附件）各占一条连接，
/// 一条慢邮件的全量下载不再串行阻塞该账号的其它操作。
struct AccountPool {
    idle: Mutex<Vec<ImapClient>>,
    permits: tokio::sync::Semaphore,
}

impl AccountPool {
    fn new() -> Self {
        Self {
            idle: Mutex::new(Vec::new()),
            permits: tokio::sync::Semaphore::new(POOL_MAX),
        }
    }
    fn take_idle(&self) -> Option<ImapClient> {
        self.idle.lock().unwrap().pop()
    }
    fn put_idle(&self, client: ImapClient) {
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < POOL_MAX_IDLE {
            idle.push(client);
        }
    }
}

/// 每账号并发连接上限 / 空闲连接保留上限。
const POOL_MAX: usize = 3;
const POOL_MAX_IDLE: usize = 2;

/// 共享状态：每账号一个 IMAP 连接池（免去反复 TCP+TLS+LOGIN），后台同步去重，
/// SQLite 缓存即时供目录/列表/正文/本地元数据。
#[derive(Clone)]
pub struct MailService {
    dir: Arc<PathBuf>,
    pool: Arc<Mutex<HashMap<String, Arc<AccountPool>>>>,
    /// 后台同步专用的连接池（与前台 `pool` 隔离，避免同步占用前台连接；但**复用连接**
    /// 免去每次同步都 TCP+TLS+LOGIN）。
    sync_pool: Arc<Mutex<HashMap<String, Arc<AccountPool>>>>,
    syncing: Arc<Mutex<HashSet<String>>>,
    db: Arc<MailDb>,
    /// 原始报文缓存（`account|folder|uid` → 原始字节 + 已读标记）
    raw_cache: Arc<Mutex<RawCache>>,
    /// 已解析账号凭据缓存（账号名 → (Account, password)）：免去每次 API 调用都
    /// 读盘 + AES 解密。账号增删改 / 切换活动账号时整体失效。
    creds: Arc<Mutex<HashMap<String, (Account, String)>>>,
    /// 上次清理过期缓存的时间（unix 秒）；用于把 prune 节流到「至多每小时一次」。
    last_prune: Arc<std::sync::atomic::AtomicU64>,
}

impl MailService {
    /// 打开数据目录（不存在则创建），初始化账号库与缓存库。
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let db = MailDb::open(dir.join("mail.db")).context("open mail cache")?;
        // 启动时清理过期正文 / 摘要缓存（30 天 TTL；保留摘要行）。
        let _ = db.prune_expired_bodies();
        Ok(Self {
            dir: Arc::new(dir),
            pool: Arc::default(),
            sync_pool: Arc::default(),
            syncing: Arc::default(),
            db: Arc::new(db),
            raw_cache: Arc::default(),
            creds: Arc::default(),
            last_prune: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// 清理过期缓存，但**至多每小时一次**（启动时 last_prune=0，必然执行一次）。
    fn prune_if_due(&self) {
        use std::sync::atomic::Ordering;
        let now = now_secs().max(0) as u64;
        let last = self.last_prune.load(Ordering::Relaxed);
        if now.saturating_sub(last) < 3600 {
            return;
        }
        self.last_prune.store(now, Ordering::Relaxed);
        let _ = self.db.prune_expired_bodies();
    }

    fn raw_key(account: &str, folder: &str, uid: u32) -> String {
        format!("{account}|{folder}|{uid}")
    }

    /// 取整封原始报文：命中内存缓存直接返回；否则经池化会话 `BODY.PEEK[]` 拉取并缓存。
    async fn raw_of(
        &self,
        acct: &Account,
        password: &str,
        folder: &str,
        uid: u32,
    ) -> MailResult<(Arc<Vec<u8>>, bool)> {
        let key = Self::raw_key(&acct.name, folder, uid);
        if let Some(hit) = self.raw_cache.lock().unwrap().get(&key) {
            return Ok(hit);
        }
        let (bytes, seen) = self
            .read(acct, password, |c| {
                let folder = folder.to_string();
                Box::pin(async move { c.fetch_raw(&folder, uid).await })
            })
            .await?;
        Ok(self.raw_cache.lock().unwrap().put(key, bytes, seen))
    }

    /// 仅查内存缓存（不触发网络）。
    fn raw_cached(&self, account: &str, folder: &str, uid: u32) -> Option<Arc<Vec<u8>>> {
        self.raw_cache
            .lock()
            .unwrap()
            .get(&Self::raw_key(account, folder, uid))
            .map(|(raw, _)| raw)
    }

    /// 已缓存的该附件文件名（用于分片拉取后校验；无缓存返回 None）。
    fn expected_attachment_name(
        &self,
        account: &str,
        folder: &str,
        uid: u32,
        index: usize,
    ) -> Option<String> {
        let body = self.db.get_body(account, folder, uid).ok().flatten()?;
        let list: Vec<MailAttachment> = body
            .attachments
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())?;
        list.get(index).map(|a| a.filename.clone())
    }

    fn store(&self) -> Result<Store> {
        Store::open(self.dir.as_ref().clone())
    }

    fn cell(&self, account: &str) -> Arc<AccountPool> {
        self.pool
            .lock()
            .unwrap()
            .entry(account.to_string())
            .or_insert_with(|| Arc::new(AccountPool::new()))
            .clone()
    }

    /// 后台同步连接池（与前台隔离）。
    fn sync_cell(&self, account: &str) -> Arc<AccountPool> {
        self.sync_pool
            .lock()
            .unwrap()
            .entry(account.to_string())
            .or_insert_with(|| Arc::new(AccountPool::new()))
            .clone()
    }

    fn begin_sync(&self, key: &str) -> bool {
        self.syncing.lock().unwrap().insert(key.to_string())
    }

    fn end_sync(&self, key: &str) {
        self.syncing.lock().unwrap().remove(key);
    }

    fn drop_session(&self, account: &str) {
        self.pool.lock().unwrap().remove(account);
        self.sync_pool.lock().unwrap().remove(account);
    }

    fn state_view(&self) -> Result<StateView> {
        let st = self.store()?;
        let active = st.active_name()?;
        let accounts = st.list_accounts()?;
        Ok(StateView {
            active,
            presets: crate::preset::names().into_iter().map(|s| s.to_string()).collect(),
            accounts: accounts.iter().map(AccountView::from).collect(),
        })
    }

    fn resolve_account(&self, name: Option<String>) -> Result<(Account, String)> {
        // 命中缓存（按账号名；缺省活动账号用哨兵键）→ 免去读盘 + AES 解密。
        let key = name.clone().unwrap_or_else(|| ACTIVE_CRED_KEY.to_string());
        if let Some(hit) = self.creds.lock().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        let st = self.store()?;
        let acct = match name {
            Some(n) => st.get_account(&n)?,
            None => st.active_account()?.context("no active account configured")?,
        };
        let password = st.decrypt_password(&acct)?;
        let entry = (acct.clone(), password);
        let mut c = self.creds.lock().unwrap();
        c.insert(acct.name.clone(), entry.clone());
        c.insert(key, entry.clone());
        Ok(entry)
    }

    /// 账号库变更后清空凭据缓存（增删改 / 切换活动账号）。
    fn clear_creds(&self) {
        self.creds.lock().unwrap().clear();
    }

    fn spawn_account_sync(&self, acct: Account, password: String) {
        let key = format!("{}|#meta", acct.name);
        if !self.begin_sync(&key) {
            return;
        }
        let st = self.clone();
        let key2 = key.clone();
        tokio::spawn(async move {
            let name = acct.name.clone();
            // 复用**常驻同步会话**（不再每次 TCP+TLS+LOGIN）。
            let res = st
                .read_sync(&acct, &password, |c| {
                    Box::pin(async move {
                        let list = c.list_folders().await;
                        let mut stats = Vec::new();
                        if let Ok(folders) = &list {
                            for f in standard_folders(folders) {
                                if let Ok(s) = c.folder_stats(&f).await {
                                    stats.push((f, s.total, s.unread));
                                }
                            }
                        }
                        Ok::<_, anyhow::Error>((list, stats))
                    })
                })
                .await;
            if let Ok((list, stats)) = res {
                if let Ok(folders) = list {
                    let _ = st.db.save_folders(&name, &folders);
                    for (f, total, unread) in stats {
                        let _ = st.db.update_folder_totals(&name, &f, total, unread);
                    }
                    let _ = st.db.mark_synced(&name, "#folders", folders.len() as u32, 0);
                }
            }
            st.end_sync(&key2);
        });
    }

    fn spawn_folder_sync(&self, acct: Account, password: String, folder: String) {
        let key = format!("{}|{}", acct.name, folder);
        if !self.begin_sync(&key) {
            return;
        }
        let st = self.clone();
        let key2 = key.clone();
        tokio::spawn(async move {
            let name = acct.name.clone();
            let folder_for_f = folder.clone();
            // 复用**常驻同步会话**（不再每次 TCP+TLS+LOGIN）。
            let res = st
                .read_sync(&acct, &password, |c| {
                    let folder = folder_for_f.clone();
                    Box::pin(async move {
                        let page = c.list_messages(&folder, SYNC_PAGE, 0, false).await;
                        let stats = c.folder_stats(&folder).await;
                        Ok::<_, anyhow::Error>((page, stats))
                    })
                })
                .await;
            if let Ok((Ok(page), Ok(stats))) = res {
                let _ = st.db.upsert_page(&name, &folder, &page.messages);
                let _ = st.db.mark_synced(&name, &folder, stats.total, stats.unread);
            }
            // 清理过期缓存（30 天 TTL）——节流到至多每小时一次，避免每次同步全表 UPDATE。
            st.prune_if_due();
            st.end_sync(&key2);
        });
    }

    /// 对池化会话执行只读 `f`：失败/超时则丢弃并重连重试一次。
    async fn read<T, F>(&self, acct: &Account, password: &str, f: F) -> MailResult<T>
    where
        F: for<'a> Fn(&'a mut ImapClient) -> BoxFuture<'a, T>,
    {
        Self::with_session(self.cell(&acct.name), acct, password, f).await
    }

    /// 对**后台同步**常驻会话执行 `f`（与前台隔离，但复用连接）。
    async fn read_sync<T, F>(&self, acct: &Account, password: &str, f: F) -> MailResult<T>
    where
        F: for<'a> Fn(&'a mut ImapClient) -> BoxFuture<'a, T>,
    {
        Self::with_session(self.sync_cell(&acct.name), acct, password, f).await
    }

    /// 从连接池借一条连接执行 `f`，用完归还；失败/超时丢弃该连接并（复用连接时）重连一次。
    async fn with_session<T, F>(
        pool: Arc<AccountPool>,
        acct: &Account,
        password: &str,
        f: F,
    ) -> MailResult<T>
    where
        F: for<'a> Fn(&'a mut ImapClient) -> BoxFuture<'a, T>,
    {
        // 并发上限：最多 POOL_MAX 条连接同时在用（超出则排队，而非互相阻塞在同一把锁上）。
        let _permit = pool
            .permits
            .acquire()
            .await
            .map_err(|_| MailError::new("mail 连接池已关闭"))?;

        let reused = pool.take_idle();
        let is_reused = reused.is_some();
        let mut client = match reused {
            Some(c) => c,
            None => Self::connect_imap(acct, password).await?,
        };

        match tokio::time::timeout(IMAP_TIMEOUT, f(&mut client)).await {
            Ok(Ok(v)) => {
                pool.put_idle(client);
                Ok(v)
            }
            // 复用连接失败/超时 → 丢弃并重连重试一次（全新连接多半是瞬时故障）。
            _ if is_reused => {
                drop(client);
                let mut fresh = Self::connect_imap(acct, password).await?;
                let v = tokio::time::timeout(IMAP_TIMEOUT, f(&mut fresh))
                    .await
                    .map_err(|_| MailError::new("IMAP 响应超时"))?
                    .map_err(MailError::from)?;
                pool.put_idle(fresh);
                Ok(v)
            }
            _ => Err(MailError::new("IMAP 响应超时")),
        }
    }

    /// 新建一条 IMAP 连接（带连接超时）。
    async fn connect_imap(acct: &Account, password: &str) -> MailResult<ImapClient> {
        tokio::time::timeout(CONNECT_TIMEOUT, ImapClient::connect(acct, password))
            .await
            .map_err(|_| MailError::new(format!("连接 {} 超时", acct.imap_host)))?
            .map_err(MailError::from)
    }

    // -- 账号 -------------------------------------------------------------

    pub fn state(&self) -> MailResult<StateView> {
        self.state_view().map_err(MailError::from)
    }

    pub fn add_account(&self, input: AddAccountInput) -> MailResult<StateView> {
        let parse = |s: Option<&String>| -> MailResult<Option<TlsMode>> {
            s.map(|v| account::parse_tls(v).map_err(MailError::from))
                .transpose()
        };
        let (acct, password) = account::build_account(AddFields {
            preset: input.preset,
            name: input.name,
            username: input.username,
            password: input.password,
            display_name: input.display_name,
            imap_host: input.imap_host,
            imap_port: input.imap_port,
            imap_tls: parse(input.imap_tls.as_ref())?,
            smtp_host: input.smtp_host,
            smtp_port: input.smtp_port,
            smtp_tls: parse(input.smtp_tls.as_ref())?,
        })
        .map_err(MailError::from)?;
        self.store()
            .map_err(MailError::from)?
            .add_account(acct, &password)
            .map_err(MailError::from)?;
        self.clear_creds();
        self.state_view().map_err(MailError::from)
    }

    pub fn update_account(
        &self,
        name: &str,
        input: UpdateAccountInput,
    ) -> MailResult<StateView> {
        let st = self.store().map_err(MailError::from)?;
        let mut acct = st.get_account(name).map_err(MailError::from)?;
        if let Some(v) = input.username {
            acct.username = v;
        }
        if let Some(v) = input.display_name {
            let v = v.trim().to_string();
            acct.display_name = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = input.imap_host {
            acct.imap_host = v;
        }
        if let Some(v) = input.imap_port {
            acct.imap_port = v;
        }
        if let Some(v) = input.imap_tls {
            acct.imap_tls = account::parse_tls(&v).map_err(MailError::from)?;
        }
        if let Some(v) = input.smtp_host {
            acct.smtp_host = v;
        }
        if let Some(v) = input.smtp_port {
            acct.smtp_port = v;
        }
        if let Some(v) = input.smtp_tls {
            acct.smtp_tls = account::parse_tls(&v).map_err(MailError::from)?;
        }
        let new_password = input.password.filter(|p| !p.trim().is_empty());
        st.update_account(acct, new_password.as_deref())
            .map_err(MailError::from)?;
        self.drop_session(name);
        self.clear_creds();
        self.state_view().map_err(MailError::from)
    }

    pub fn remove_account(&self, name: &str) -> MailResult<StateView> {
        let st = self.store().map_err(MailError::from)?;
        if !st.remove_account(name).map_err(MailError::from)? {
            return Err(MailError::new(format!("account '{name}' not found")));
        }
        self.drop_session(name);
        self.db.delete_account(name).map_err(MailError::from)?;
        self.clear_creds();
        self.state_view().map_err(MailError::from)
    }

    pub fn activate(&self, name: &str) -> MailResult<StateView> {
        self.store()
            .map_err(MailError::from)?
            .set_active(name)
            .map_err(MailError::from)?;
        self.clear_creds();
        self.state_view().map_err(MailError::from)
    }

    // -- 读取 -------------------------------------------------------------

    pub async fn folders(&self, account: Option<String>) -> MailResult<Vec<String>> {
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let cached = self.db.get_folders(&acct.name).map_err(MailError::from)?;
        let stale = cached.is_empty()
            || self
                .db
                .synced_at(&acct.name, "#folders")
                .map_err(MailError::from)?
                + FOLDERS_TTL.as_secs() as i64
                <= now_secs();
        if !cached.is_empty() {
            if stale {
                self.spawn_account_sync(acct, password);
            }
            return Ok(cached);
        }
        let list = self
            .read(&acct, &password, |c| Box::pin(async move { c.list_folders().await }))
            .await?;
        self.db.save_folders(&acct.name, &list).map_err(MailError::from)?;
        self.db
            .mark_synced(&acct.name, "#folders", list.len() as u32, 0)
            .map_err(MailError::from)?;
        Ok(list)
    }

    pub async fn folder_counts(&self, account: Option<String>) -> MailResult<Vec<FolderCountView>> {
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let mut names = self.db.get_folders(&acct.name).map_err(MailError::from)?;
        let meta_synced = self.db.synced_at(&acct.name, "#folders").map_err(MailError::from)?;

        if names.is_empty() {
            let list = self
                .read(&acct, &password, |c| Box::pin(async move { c.list_folders().await }))
                .await?;
            self.db.save_folders(&acct.name, &list).map_err(MailError::from)?;
            self.db
                .mark_synced(&acct.name, "#folders", list.len() as u32, 0)
                .map_err(MailError::from)?;
            names = list;
        } else if meta_synced + COUNTS_TTL.as_secs() as i64 <= now_secs() {
            self.spawn_account_sync(acct.clone(), password.clone());
        }

        let stats = self.db.folder_stats_cached(&acct.name).map_err(MailError::from)?;
        let mut out: Vec<FolderCountView> = standard_folders(&names)
            .into_iter()
            .map(|f| {
                let s = stats.iter().find(|(n, _, _)| *n == f);
                FolderCountView {
                    folder: f,
                    total: s.map(|x| x.1).unwrap_or(0),
                    unread: s.map(|x| x.2).unwrap_or(0),
                }
            })
            .collect();
        for (cat, total) in self.db.category_counts(&acct.name).map_err(MailError::from)? {
            out.push(FolderCountView {
                folder: format!("#{cat}"),
                total,
                unread: 0,
            });
        }
        Ok(out)
    }

    pub async fn messages(&self, q: MessagesQuery) -> MailResult<MsgListView> {
        let (acct, password) = self.resolve_account(q.account).map_err(MailError::from)?;
        let limit = q.limit.unwrap_or(30).clamp(1, 100);
        let offset = q.offset.unwrap_or(0);
        let unread = q.unread.unwrap_or(false);
        let category = q.category.as_deref().filter(|c| !c.is_empty());
        let folder = match q.folder.filter(|f| !f.is_empty()) {
            Some(f) => Some(f),
            None if category.is_some() => None,
            None => Some("INBOX".to_string()),
        };

        let mut stale = false;
        if let Some(folder_name) = folder.as_deref() {
            let synced_at = self.db.synced_at(&acct.name, folder_name).map_err(MailError::from)?;
            let cached_rows = self
                .db
                .filtered_count(&acct.name, Some(folder_name), unread, None)
                .map_err(MailError::from)? as usize;
            stale = cached_rows == 0 || synced_at + SYNC_TTL_SECS <= now_secs();
            if q.refresh.unwrap_or(false) || cached_rows == 0 {
                // 缓存优先：立即返回现有缓存（冷启动时可能为空），后台同步，前端轮询补齐差异。
                // 注意：这里**不阻塞**——切文件夹不再等一次完整的 IMAP 同步。
                self.spawn_folder_sync(acct.clone(), password.clone(), folder_name.to_string());
                stale = true;
            } else if offset + limit > cached_rows {
                let want_to = offset + limit;
                let missing = want_to.saturating_sub(cached_rows).min(MAX_BACKFILL);
                // list_messages 已返回 total/unread（内部一次 STATUS），无需再单独 folder_stats。
                let page = self
                    .read(&acct, &password, |c| {
                        let folder = folder_name.to_string();
                        Box::pin(async move { c.list_messages(&folder, missing, cached_rows, false).await })
                    })
                    .await?;
                self.db
                    .upsert_page(&acct.name, folder_name, &page.messages)
                    .map_err(MailError::from)?;
                self.db
                    .update_folder_totals(&acct.name, folder_name, page.total, page.unread)
                    .map_err(MailError::from)?;
            } else if stale {
                self.spawn_folder_sync(acct.clone(), password.clone(), folder_name.to_string());
            }
        }

        let page = self
            .db
            .page(&acct.name, folder.as_deref(), limit, offset, unread, category)
            .map_err(MailError::from)?;
        let synced_at = match folder.as_deref() {
            Some(f) => self.db.synced_at(&acct.name, f).map_err(MailError::from)?,
            None => now_secs(),
        };
        Ok(MsgListView {
            folder: folder.unwrap_or_default(),
            total: page.total,
            unread: page.unread,
            has_more: page.has_more,
            synced_at,
            stale,
            messages: page.messages,
        })
    }

    pub async fn previews(
        &self,
        folder: Option<String>,
        account: Option<String>,
        uids: Vec<u32>,
    ) -> MailResult<HashMap<u32, String>> {
        if uids.is_empty() {
            return Ok(HashMap::new());
        }
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let folder = folder.unwrap_or_else(|| "INBOX".to_string());
        // 先读本地缓存（30 天内），只对缺失的 uid 走 IMAP。
        let mut map = self
            .db
            .get_previews(&acct.name, &folder, &uids)
            .map_err(MailError::from)?;
        let missing: Vec<u32> = uids.iter().copied().filter(|u| !map.contains_key(u)).collect();
        if !missing.is_empty() {
            let fetched = self
                .read(&acct, &password, |c| {
                    let folder = folder.clone();
                    let uids = missing.clone();
                    Box::pin(async move { c.fetch_previews(&folder, &uids).await })
                })
                .await?;
            self.db
                .save_previews(&acct.name, &folder, &fetched)
                .map_err(MailError::from)?;
            map.extend(fetched);
        }
        Ok(map)
    }

    pub async fn message(
        &self,
        uid: u32,
        folder: Option<String>,
        account: Option<String>,
        refresh: bool,
    ) -> MailResult<MailDetail> {
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let folder = folder.unwrap_or_else(|| "INBOX".to_string());

        if !refresh {
            if let (Ok(Some(body)), Ok(Some(row))) = (
                self.db.get_body(&acct.name, &folder, uid),
                self.db.get_message(&acct.name, &folder, uid),
            ) {
                let detail = MailDetail {
                    uid,
                    seen: row.seen,
                    from: row.from.clone(),
                    to: row.to.clone(),
                    cc: String::new(),
                    subject: row.subject.clone(),
                    date: row.date.clone(),
                    text: (!body.text.is_empty()).then(|| body.text.clone()),
                    html: (!body.html.is_empty()).then(|| body.html.clone()),
                    attachments: body
                        .attachments
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default(),
                    truncated: body.truncated,
                };
                return Ok(detail);
            }
        }

        // 拉取原始报文（顺带进内存缓存，后续点附件直接复用），再本地解析。
        // 超限正文按缓存上限截断后再返回/落库，保证「首次打开 == 再次打开」一致。
        let (raw, seen) = self.raw_of(&acct, &password, &folder, uid).await?;
        let mut detail = detail_from_bytes(uid, &raw, seen).map_err(MailError::from)?;
        let (text, t_trunc) = crate::db::clamp_body(detail.text.as_deref());
        let (html, h_trunc) = crate::db::clamp_body(detail.html.as_deref());
        detail.text = text;
        detail.html = html;
        detail.truncated = t_trunc || h_trunc;
        let att_json = serde_json::to_string(&detail.attachments).ok();
        self.db
            .save_body(
                &acct.name,
                &folder,
                uid,
                detail.text.as_deref(),
                detail.html.as_deref(),
                att_json.as_deref(),
            )
            .map_err(MailError::from)?;
        Ok(detail)
    }

    pub async fn set_meta(
        &self,
        uid: u32,
        folder: Option<String>,
        account: Option<String>,
        body: MetaUpdate,
    ) -> MailResult<()> {
        let category = body
            .category
            .as_deref()
            .map(str::trim)
            .filter(|c| matches!(*c, "" | "todo" | "doing" | "done"));
        if body.flagged.is_none() && body.pinned.is_none() && body.category.is_none() && body.seen.is_none() {
            return Err(MailError::new("nothing to update"));
        }
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let folder = folder.unwrap_or_else(|| "INBOX".to_string());

        if let Some(on) = body.flagged {
            let mut client = ImapClient::connect(&acct, &password)
                .await
                .map_err(MailError::from)?;
            let r = client.set_flagged(&folder, uid, on).await;
            let _ = client.logout().await;
            r.map_err(MailError::from)?;
        }

        if let Some(on) = body.seen {
            let mut client = ImapClient::connect(&acct, &password)
                .await
                .map_err(MailError::from)?;
            let r = client.set_seen(&folder, uid, on).await;
            let _ = client.logout().await;
            r.map_err(MailError::from)?;
        }

        self.db
            .set_meta(&acct.name, &folder, uid, body.flagged, body.pinned, category, body.seen)
            .map_err(MailError::from)?;
        Ok(())
    }

    pub async fn move_message(
        &self,
        uid: u32,
        folder: Option<String>,
        account: Option<String>,
        dest: String,
    ) -> MailResult<()> {
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let folder = folder.unwrap_or_else(|| "INBOX".to_string());
        let mut client = ImapClient::connect(&acct, &password)
            .await
            .map_err(MailError::from)?;
        let r = client.move_message(&folder, uid, &dest).await;
        let _ = client.logout().await;
        r.map_err(MailError::from)?;
        self.db
            .delete_row(&acct.name, &folder, uid)
            .map_err(MailError::from)?;
        Ok(())
    }

    pub async fn attachment(
        &self,
        uid: u32,
        index: usize,
        folder: Option<String>,
        account: Option<String>,
    ) -> MailResult<AttachmentBlob> {
        Ok(self
            .fetch_attachment_data(uid, index, folder, account)
            .await?
            .into())
    }

    /// 取附件字节：内存原始报文缓存 → 仅取 MIME 分片 → 全量拉取（逐级回退）。
    async fn fetch_attachment_data(
        &self,
        uid: u32,
        index: usize,
        folder: Option<String>,
        account: Option<String>,
    ) -> MailResult<AttachmentData> {
        let (acct, password) = self.resolve_account(account).map_err(MailError::from)?;
        let folder = folder.unwrap_or_else(|| "INBOX".to_string());

        // 1) 内存缓存命中（打开过该邮件）→ 本地解码，零网络
        if let Some(raw) = self.raw_cached(&acct.name, &folder, uid) {
            return attachment_from_bytes(&raw, index).map_err(MailError::from);
        }

        // 2) 首次点附件：用 BODYSTRUCTURE 只取目标 MIME 分片（校验文件名，避免错配）
        let expected = self.expected_attachment_name(&acct.name, &folder, uid, index);
        if let Ok(Some(a)) = self
            .read(&acct, &password, |c| {
                let folder = folder.clone();
                let exp = expected.clone();
                Box::pin(async move {
                    c.fetch_attachment_fast(&folder, uid, index, exp.as_deref())
                        .await
                })
            })
            .await
        {
            return Ok(a);
        }

        // 3) 回退：全量拉取整封并进内存缓存
        let (raw, _) = self.raw_of(&acct, &password, &folder, uid).await?;
        attachment_from_bytes(&raw, index).map_err(MailError::from)
    }

    /// 把附件解码后写入进程临时目录并返回路径；前端文件预览据此直连渲染，
    /// 省去「字节 → base64 → IPC → 再落盘 → 再读回」的多趟开销。
    pub async fn attachment_to_temp(
        &self,
        uid: u32,
        index: usize,
        folder: Option<String>,
        account: Option<String>,
    ) -> MailResult<String> {
        let a = self
            .fetch_attachment_data(uid, index, folder, account)
            .await?;
        let dir = std::env::temp_dir().join("astorm-mail-preview");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| MailError::new(format!("创建临时目录失败: {e}")))?;
        // 文件名前缀 uid-index：避免不同邮件里同名附件互相覆盖；保留原文件名（含扩展名）
        // 以便前端按扩展名选图标 / 预览器。
        let safe = a.filename.replace(['/', '\\'], "_");
        let name = if safe.is_empty() {
            format!("{uid}-{index}")
        } else {
            format!("{uid}-{index}-{safe}")
        };
        let path = dir.join(name);
        // 已存在且大小一致 → 直接复用（省去重复写盘）。
        let reuse = tokio::fs::metadata(&path)
            .await
            .map(|m| m.len() == a.data.len() as u64)
            .unwrap_or(false);
        if !reuse {
            // 异步写盘：大附件不再阻塞 tokio 工作线程。
            tokio::fs::write(&path, &a.data)
                .await
                .map_err(|e| MailError::new(format!("写入附件失败: {e}")))?;
        }
        Ok(path.to_string_lossy().to_string())
    }

    pub async fn send(&self, input: SendInput) -> MailResult<String> {
        let clean = |v: &[String]| -> Vec<String> {
            v.iter()
                .flat_map(|s| s.split(','))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let to = clean(&input.to);
        if to.is_empty() {
            return Err(MailError::new("`to` is required"));
        }
        let cc = clean(&input.cc);
        let bcc = clean(&input.bcc);
        let (acct, password) = self.resolve_account(input.account).map_err(MailError::from)?;
        smtp_client::send_mail(
            &acct,
            &password,
            &to,
            &cc,
            &bcc,
            &input.subject,
            &input.body,
            input.html.as_deref(),
            &input.attachments,
        )
        .await
        .map_err(MailError::from)?;
        let sent = standard_folders(&self.db.get_folders(&acct.name).map_err(MailError::from)?)
            .into_iter()
            .find(|f| folder_kind(f) == "sent");
        if let Some(folder) = sent {
            self.spawn_folder_sync(acct.clone(), password.clone(), folder);
        }
        Ok(acct.username)
    }

    /// 启动预热：登录当前账号并刷新文件夹/计数与收件箱首页（后台）。
    pub fn warmup(&self) {
        let warm = self.clone();
        let Ok((acct, password)) = warm.resolve_account(None) else {
            return;
        };
        tokio::spawn(async move {
            let _ = warm
                .read(&acct, &password, |c| {
                    Box::pin(async move { c.list_folders().await })
                })
                .await;
            warm.spawn_account_sync(acct.clone(), password.clone());
            warm.spawn_folder_sync(acct, password, "INBOX".to_string());
        });
    }
}

/// 文件夹名 → 标准类别（与前端 `folderMeta` 一致）。
pub fn folder_kind(name: &str) -> &'static str {
    let f = name.to_lowercase();
    if f == "inbox" || f.contains("收件箱") {
        "inbox"
    } else if f.contains("draft") || f.contains("草稿") {
        "draft"
    } else if f.contains("sent") || f.contains("已发送") {
        "sent"
    } else if f.contains("junk") || f.contains("spam") || f.contains("垃圾") {
        "junk"
    } else if f.contains("trash") || f.contains("deleted") || f.contains("废纸篓") {
        "trash"
    } else {
        "other"
    }
}

/// 侧栏展示的文件夹：每个标准类别取一个。
pub fn standard_folders(folders: &[String]) -> Vec<String> {
    ["inbox", "draft", "sent", "junk", "trash"]
        .iter()
        .filter_map(|k| folders.iter().find(|f| folder_kind(f) == *k).cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_kind_matches_ui() {
        assert_eq!(folder_kind("INBOX"), "inbox");
        assert_eq!(folder_kind("Sent Messages"), "sent");
        assert_eq!(folder_kind("已发送"), "sent");
        assert_eq!(folder_kind("Deleted Messages"), "trash");
        assert_eq!(folder_kind("Junk"), "junk");
        assert_eq!(folder_kind("Drafts"), "draft");
        assert_eq!(folder_kind("其他文件夹"), "other");
    }

    #[test]
    fn standard_folders_keeps_one_per_kind_and_skips_others() {
        let folders: Vec<String> = [
            "Archive",
            "Deleted Messages",
            "Drafts",
            "INBOX",
            "Junk",
            "Sent Messages",
            "其他文件夹",
            "其他文件夹/QQ邮件订阅",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let picked = standard_folders(&folders);
        assert_eq!(
            picked,
            vec!["INBOX", "Drafts", "Sent Messages", "Junk", "Deleted Messages"]
        );
    }

    #[test]
    fn opens_with_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let svc = MailService::open(dir.path().join("mail")).unwrap();
        let state = svc.state().unwrap();
        assert!(state.accounts.is_empty());
        assert!(!state.presets.is_empty());
    }
}
