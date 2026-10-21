//! 本机浏览器数据导入：定位 Chrome Profile → 读取 Cookie 库 → 按平台解密 → 交给主进程。
//!
//! 设计边界：
//! - **只读**本机 Chrome 数据，不修改 Chrome 任何文件（Cookie 库先复制到临时目录再读）；
//! - 本模块只负责「把 cookie 明文取出来」。真正写入内置浏览器是 Electron 主进程的事
//!   （`session.defaultSession.cookies.set`）——Rust 侧没有 Electron 的 cookie jar；
//! - 解不开的条目按原因分别计数（[`CookieImportOutcome::skipped`] / `failed`），
//!   不静默丢弃，让设置页能如实告诉用户「哪些没导入成功、为什么」。

pub mod cookie_db;
pub mod decrypt;
pub mod profile;

use decrypt::DecryptError;
use serde::Serialize;

/// 导入过程中的错误（整体性失败，非单条 cookie）。
#[derive(Debug, thiserror::Error)]
pub enum BrowserDataError {
    #[error("未找到本机 Chrome 数据目录（可能未安装 Chrome）")]
    ProfileNotFound,
    #[error("Profile `{0}` 下未找到 Cookie 数据库")]
    CookieDbMissing(String),
    #[error("读取 Cookie 数据库失败: {0}")]
    CookieDb(String),
    #[error("读取解密密钥失败: {0}")]
    KeyUnavailable(String),
    #[error("文件操作失败: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, BrowserDataError>;

/// 一条解密后的 Cookie（字段对齐 Electron `cookies.set`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedCookie {
    /// 原始 `host_key`（可能带前导 `.`，表示域 cookie）。由主进程决定是否传 `domain`。
    pub host_key: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// `unspecified` / `no_restriction` / `lax` / `strict`
    pub same_site: String,
    /// Unix 秒；None = 会话 cookie。
    pub expires: Option<f64>,
}

/// 一次导入的结果统计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieImportOutcome {
    /// 使用的 Profile 名（如 `Default`）。
    pub profile: String,
    /// Profile 目录绝对路径（便于用户核对来源）。
    pub profile_dir: String,
    /// Cookie 库里的总条数（含解不开的）。
    pub total: u32,
    /// 加密方式不支持而跳过的条数（目前只有 Windows 的 app-bound v20）。
    pub skipped: u32,
    /// 解密出错的条数。
    pub failed: u32,
    pub cookies: Vec<ImportedCookie>,
}

/// 导入本机 Chrome「最近使用」Profile 的全部 Cookie。
pub fn import_chrome_cookies() -> Result<CookieImportOutcome> {
    let profile = profile::find_latest_profile()?;
    let raw = cookie_db::read_cookies(&profile.dir)?;
    let decryptor = decrypt::PlatformDecryptor::for_profile(&profile.dir)
        .map_err(|e| BrowserDataError::KeyUnavailable(e.to_string()))?;

    let total = raw.len() as u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    let mut cookies = Vec::with_capacity(raw.len());

    for item in raw {
        // 未加密的条（值直接在 value 列）
        let value = if item.encrypted_value.is_empty() {
            item.value.clone()
        } else {
            match decryptor.decrypt(&item.host_key, &item.encrypted_value) {
                Ok(value) => value,
                Err(DecryptError::Unsupported(reason)) => {
                    skipped += 1;
                    tracing::debug!(host = %item.host_key, name = %item.name, %reason, "跳过不支持的 cookie");
                    continue;
                }
                Err(DecryptError::Failed(reason)) => {
                    failed += 1;
                    tracing::debug!(host = %item.host_key, name = %item.name, %reason, "cookie 解密失败");
                    continue;
                }
            }
        };
        cookies.push(ImportedCookie {
            host_key: item.host_key,
            name: item.name,
            value,
            path: item.path,
            secure: item.secure,
            http_only: item.http_only,
            same_site: cookie_db::same_site_name(item.same_site).to_string(),
            expires: cookie_db::chrome_epoch_to_unix(item.expires_utc),
        });
    }

    Ok(CookieImportOutcome {
        profile: profile.name,
        profile_dir: profile.dir.display().to_string(),
        total,
        skipped,
        failed,
        cookies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn outcome_serializes_to_camel_case_for_frontend() {
        let outcome = CookieImportOutcome {
            profile: "Default".into(),
            profile_dir: "/tmp/Default".into(),
            total: 2,
            skipped: 1,
            failed: 0,
            cookies: vec![ImportedCookie {
                host_key: ".example.com".into(),
                name: "sid".into(),
                value: "v".into(),
                path: "/".into(),
                secure: true,
                http_only: true,
                same_site: "lax".into(),
                expires: Some(1_704_067_200.0),
            }],
        };
        let json = serde_json::to_value(&outcome).unwrap();
        // 前端(napi object / 渲染层)按 camelCase 读
        assert_eq!(json["profileDir"], "/tmp/Default");
        assert_eq!(json["cookies"][0]["hostKey"], ".example.com");
        assert_eq!(json["cookies"][0]["httpOnly"], true);
        assert_eq!(json["cookies"][0]["sameSite"], "lax");
        assert_eq!(json["cookies"][0]["expires"], 1_704_067_200.0);
    }

    #[test]
    fn profile_not_found_message_mentions_chrome() {
        let msg = BrowserDataError::ProfileNotFound.to_string();
        assert!(msg.contains("Chrome"));
    }

    #[test]
    fn domain_hash_prefix_matches_chromium_convention() {
        // 回归保护:strip 逻辑依赖 SHA256(host_key),这里把约定钉住
        let host = ".xiaohongshu.com";
        let digest = Sha256::digest(host.as_bytes());
        let mut plain = digest.to_vec();
        plain.extend_from_slice(b"web_session=abc");
        assert_eq!(decrypt::strip_domain_prefix(host, &plain), b"web_session=abc");
    }

    /// 真实环境冒烟验收（需本机装有 Chrome 且登录过站点）：
    /// `cargo test -p astorm-core --lib import_real_chrome -- --ignored --nocapture`
    ///
    /// macOS 首次运行会弹出钥匙串授权框（授权给测试进程）。**只打印条数与值长度，
    /// 不打印 cookie 值本身**。
    ///
    /// 关键回归位：明文前的 32 字节域名哈希若没剥掉，值会变成
    /// 「32 个替换字符 + 真实值」（`from_utf8_lossy`），肉眼即可从 char 数看出异常
    /// ——真实 cookie 值的长度是有特征短串（时间戳 10 位、十六进制 32 位等），
    /// 不是 32+ 的乱码。这里据此设一条兜底断言。
    #[test]
    #[ignore = "需要本机 Chrome 真实数据与钥匙串授权"]
    fn import_real_chrome_profile_smoke() {
        let outcome = import_chrome_cookies().expect("导入本机 Chrome cookie 失败");
        println!(
            "profile={} total={} 成功={} 跳过={} 失败={}",
            outcome.profile,
            outcome.total,
            outcome.cookies.len(),
            outcome.skipped,
            outcome.failed
        );
        for c in outcome.cookies.iter().take(5) {
            println!("  {} {} ({} 字符)", c.host_key, c.name, c.value.chars().count());
        }
        assert!(!outcome.cookies.is_empty(), "应至少导入若干条 cookie");

        // 域名哈希未剥离的条会含 U+FFFD(替换字符)。允许极少数本身是二进制值的 cookie,
        // 但不允许成规模出现——那就是解密/剥离逻辑坏了。
        let non_empty: Vec<&str> = outcome
            .cookies
            .iter()
            .map(|c| c.value.as_str())
            .filter(|v| !v.is_empty())
            .collect();
        let corrupted = non_empty.iter().filter(|v| v.contains('\u{FFFD}')).count();
        println!("非空值 {} 条，其中含替换字符 {corrupted} 条", non_empty.len());
        assert!(
            corrupted * 4 <= non_empty.len(),
            "含替换字符的值过多（{corrupted}/{}）：疑似域名哈希前缀未剥离",
            non_empty.len()
        );
    }
}
