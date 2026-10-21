//! 读取 Chrome 的 Cookie 数据库（SQLite）。
//!
//! Chrome 运行中会持有该库的锁，且值可能还在 WAL 里未落盘，因此**先把
//! `Cookies` / `-wal` / `-shm` 复制到临时目录**再只读打开——既不阻塞 Chrome，
//! 也不修改 Chrome 任何文件（与设置页「不修改 Chrome 数据」的承诺一致）。
//!
//! 本模块只负责「取到密文」，密钥派生与解密见 [`super::decrypt`]。

use std::path::{Path, PathBuf};

use super::{profile, BrowserDataError, Result};

/// Chrome 时间戳纪元（1601-01-01）相对 Unix 纪元的秒差。
const CHROME_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;

/// Cookie 库里的一行原始记录（值仍为密文）。
#[derive(Debug, Clone)]
pub struct RawCookie {
    /// 所属域名，可能带前导 `.`（域 cookie）；host-only cookie 不带。
    pub host_key: String,
    pub name: String,
    /// 明文值列（绝大多数行为空，值在 [`Self::encrypted_value`]）。
    pub value: String,
    /// 密文（含 `v10` / `v20` 版本前缀）；可能为空 = 该 cookie 未加密。
    pub encrypted_value: Vec<u8>,
    pub path: String,
    /// Chrome 纪元微秒；0 = 会话 cookie。
    pub expires_utc: i64,
    pub secure: bool,
    pub http_only: bool,
    /// 0=unspecified 1=no_restriction 2=lax 3=strict。
    pub same_site: i64,
}

/// 读取指定 Profile 的全部 Cookie（跳过 Chrome 内部空域名记录）。
pub fn read_cookies(profile_dir: &Path) -> Result<Vec<RawCookie>> {
    let db = profile::cookies_db_path(profile_dir);
    if !db.is_file() {
        return Err(BrowserDataError::CookieDbMissing(profile_dir.display().to_string()));
    }
    let staging = StagingDir::copy_cookies(&db)?;
    let conn = rusqlite::Connection::open_with_flags(
        staging.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| BrowserDataError::CookieDb(e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite \
             FROM cookies WHERE host_key <> ''",
        )
        .map_err(|e| BrowserDataError::CookieDb(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RawCookie {
                host_key: row.get(0)?,
                name: row.get(1)?,
                value: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                encrypted_value: row.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
                path: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                expires_utc: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                secure: row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                http_only: row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
                same_site: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
            })
        })
        .map_err(|e| BrowserDataError::CookieDb(e.to_string()))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| BrowserDataError::CookieDb(e.to_string()))?);
    }
    Ok(out)
}

/// Chrome 纪元微秒 → Unix 秒；0（会话 cookie）或非法值返回 None。
pub fn chrome_epoch_to_unix(expires_utc: i64) -> Option<f64> {
    if expires_utc <= 0 {
        return None;
    }
    let secs = expires_utc / 1_000_000 - CHROME_EPOCH_OFFSET_SECS;
    (secs > 0).then_some(secs as f64)
}

/// Chrome 的 samesite 数值 → Electron `cookies.set` 的取值。
pub fn same_site_name(value: i64) -> &'static str {
    match value {
        1 => "no_restriction",
        2 => "lax",
        3 => "strict",
        _ => "unspecified",
    }
}

/// Cookie 库的临时副本；Drop 时清理。
struct StagingDir(PathBuf);

impl StagingDir {
    /// 复制主库 + WAL/SHM 边车文件（缺一个就可能读到旧数据）。
    fn copy_cookies(db: &Path) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("astorm-chrome-cookies-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(|e| BrowserDataError::Io(e.to_string()))?;
        let staging = StagingDir(dir);
        let file_name = db.file_name().unwrap_or_default().to_string_lossy().into_owned();
        std::fs::copy(db, staging.0.join(&file_name))
            .map_err(|e| BrowserDataError::Io(format!("复制 Cookie 库失败: {e}")))?;
        for suffix in ["-wal", "-shm"] {
            let side = db.with_file_name(format!("{file_name}{suffix}"));
            if side.is_file() {
                // 边车文件可能刚好在复制瞬间被 Chrome 删除/换名,失败不算致命
                let _ = std::fs::copy(&side, staging.0.join(format!("{file_name}{suffix}")));
            }
        }
        Ok(staging)
    }

    fn path(&self) -> PathBuf {
        let mut entries = std::fs::read_dir(&self.0).into_iter().flatten().flatten();
        entries
            .find(|e| e.file_name() == "Cookies")
            .map(|e| e.path())
            .unwrap_or_else(|| self.0.join("Cookies"))
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_epoch_matches_unix_conversion() {
        // 2024-01-01T00:00:00Z = 1704067200;Chrome 纪元 = 13348540800000000
        assert_eq!(chrome_epoch_to_unix(13_348_540_800_000_000), Some(1_704_067_200.0));
        // 会话 cookie(0)与脏数据都为 None
        assert_eq!(chrome_epoch_to_unix(0), None);
        assert_eq!(chrome_epoch_to_unix(-1), None);
    }

    #[test]
    fn same_site_maps_to_electron_names() {
        assert_eq!(same_site_name(0), "unspecified");
        assert_eq!(same_site_name(1), "no_restriction");
        assert_eq!(same_site_name(2), "lax");
        assert_eq!(same_site_name(3), "strict");
        // 未知取值按 unspecified,不 panic
        assert_eq!(same_site_name(9), "unspecified");
    }

    #[test]
    fn read_cookies_reads_staged_db_and_cleans_up() {
        let tmp = std::env::temp_dir().join(format!("astorm-cookiedb-{}", uuid::Uuid::new_v4()));
        let profile = tmp.join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        let db = profile.join("Cookies");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB, \
             path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, samesite INTEGER);\
             INSERT INTO cookies VALUES ('.example.com','sid','',X'763130616263','/', 13348540800000000, 1, 1, 2);\
             INSERT INTO cookies VALUES ('','__unique','x','', '/', 0, 0, 0, 0);",
        )
        .unwrap();
        drop(conn);

        let cookies = read_cookies(&profile).unwrap();
        // 空域名行被 SQL 过滤
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].host_key, ".example.com");
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].encrypted_value, b"v10abc");
        assert!(cookies[0].secure && cookies[0].http_only);
        assert_eq!(chrome_epoch_to_unix(cookies[0].expires_utc), Some(1_704_067_200.0));

        // 暂存目录已清理
        let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("astorm-chrome-cookies-"))
            .collect();
        assert!(leftovers.is_empty(), "暂存目录未清理: {leftovers:?}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn missing_cookie_db_is_reported() {
        let tmp = std::env::temp_dir().join(format!("astorm-nocookie-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(matches!(
            read_cookies(&tmp),
            Err(BrowserDataError::CookieDbMissing(_))
        ));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
