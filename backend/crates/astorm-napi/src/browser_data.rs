//! 本机 Chrome Cookie 导入（设置页「自动化控制 → 浏览器数据」）。
//!
//! 只做「取明文并交出去」：读本机 Chrome 的 Cookie 库并解密，返回明文列表与统计；
//! 真正写入内置浏览器的 cookie jar 由 Electron 主进程完成（Rust 侧没有 session）。
//! 业务实现见 `astorm_core::browser`，本层只做 napi 形状与线程切换。

use napi_derive::napi;

/// 一条已解密的 Cookie（字段与 `astorm_core::browser::ImportedCookie` 对齐）。
#[napi(object)]
pub struct ChromeCookie {
    /// 原始 host_key，可能带前导 `.`（域 cookie）。
    pub host_key: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// `unspecified` / `no_restriction` / `lax` / `strict`
    pub same_site: String,
    /// Unix 秒；`null` = 会话 cookie。
    pub expires: Option<f64>,
}

/// 一次导入的结果（统计 + 明文 cookie 列表）。
#[napi(object)]
pub struct ChromeCookieImport {
    /// 使用的 Chrome Profile 名（如 `Default`）。
    pub profile: String,
    /// Profile 目录绝对路径（便于用户核对来源）。
    pub profile_dir: String,
    /// Cookie 库总条数（含解不开的）。
    pub total: u32,
    /// 因加密方式不支持而跳过的条数（目前只有 Windows 的 app-bound v20）。
    pub skipped: u32,
    /// 解密出错的条数。
    pub failed: u32,
    pub cookies: Vec<ChromeCookie>,
}

impl From<astorm_core::browser::ImportedCookie> for ChromeCookie {
    fn from(c: astorm_core::browser::ImportedCookie) -> Self {
        Self {
            host_key: c.host_key,
            name: c.name,
            value: c.value,
            path: c.path,
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site,
            expires: c.expires,
        }
    }
}

impl From<astorm_core::browser::CookieImportOutcome> for ChromeCookieImport {
    fn from(o: astorm_core::browser::CookieImportOutcome) -> Self {
        Self {
            profile: o.profile,
            profile_dir: o.profile_dir,
            total: o.total,
            skipped: o.skipped,
            failed: o.failed,
            cookies: o.cookies.into_iter().map(Into::into).collect(),
        }
    }
}

/// 导入本机 Chrome「最近使用」Profile 的全部 Cookie（明文）。
///
/// 走 async + `spawn_blocking`：读钥匙串/DPAPI 与解 SQLite 都是阻塞操作，
/// 且 macOS 首次会弹系统授权框；若同步执行会卡死 Electron 主进程。
#[napi]
pub async fn import_chrome_cookies() -> napi::Result<ChromeCookieImport> {
    let outcome = tokio::task::spawn_blocking(astorm_core::browser::import_chrome_cookies)
        .await
        .map_err(|e| napi::Error::from_reason(format!("导入任务执行失败: {e}")))?
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(outcome.into())
}
