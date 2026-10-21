//! 定位本机 Chrome 的 User Data 根目录与「最近使用」Profile。
//!
//! 只做路径解析与只读探测，不触碰 Chrome 任何文件。Profile 的挑选依据
//! `Local State` 里的 `profile.last_used` / `profile.last_active_profiles`
//! （Chrome 自己记录的最近使用顺序），取不到时依次回退 `Default`、任意含
//! Cookie 库的目录，保证「有 Chrome 就尽量能导入」。

use std::path::{Path, PathBuf};

use super::{BrowserDataError, Result};

/// 一个可供导入的 Chrome Profile。
#[derive(Debug, Clone)]
pub struct ChromeProfile {
    /// Profile 目录名（Chrome 内部标识，如 `Default` / `Profile 1`）。
    pub name: String,
    /// Profile 目录绝对路径。
    pub dir: PathBuf,
}

/// 本机 Chrome 的 User Data 根目录（不存在则 None）。
pub fn chrome_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let root = dirs::home_dir()?.join("Library/Application Support/Google/Chrome");
    #[cfg(target_os = "windows")]
    let root = dirs::data_local_dir()?.join("Google").join("Chrome").join("User Data");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let root = dirs::config_dir()?.join("google-chrome");
    // 目录名更准确：Chrome 不在时 data_local_dir 仍存在（Windows 上尤其如此）
    root.is_dir().then_some(root)
}

/// Cookie 数据库路径：Chrome 96+ 放在 `Network/` 子目录，旧版直接放在 Profile 根。
pub fn cookies_db_path(profile_dir: &Path) -> PathBuf {
    let nested = profile_dir.join("Network").join("Cookies");
    if nested.is_file() { nested } else { profile_dir.join("Cookies") }
}

/// 该目录是否像一个可用 Profile（含 Cookie 库或 Local Storage）。
fn has_profile_data(dir: &Path) -> bool {
    cookies_db_path(dir).is_file() || dir.join("Local Storage").is_dir()
}

/// 选出「最近使用」的 Profile。
pub fn find_latest_profile() -> Result<ChromeProfile> {
    let root = chrome_root().ok_or(BrowserDataError::ProfileNotFound)?;
    for name in preferred_order(&root) {
        let dir = root.join(&name);
        if has_profile_data(&dir) {
            return Ok(ChromeProfile { name, dir });
        }
    }
    // 兜底:扫目录挑第一个像 Profile 的（`Default` 之外还有 `Profile N`）
    let mut candidates = profile_dirs(&root);
    candidates.sort_by_key(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        // Default 优先,其余按名字
        (name != "Default", name)
    });
    if let Some(dir) = candidates.into_iter().find(|p| has_profile_data(p)) {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        return Ok(ChromeProfile { name, dir });
    }
    Err(BrowserDataError::ProfileNotFound)
}

/// 根目录下所有像 Profile 的子目录（排除 `System Profile` 等 Chrome 内部目录）。
pub fn profile_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            name == "Default" || name.starts_with("Profile ") || name == "Guest Profile"
        })
        .collect()
}

/// 依据 `Local State` 的最近使用记录给出选取顺序（去重）。
fn preferred_order(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(state) = read_local_state(root) {
        let profile = state.get("profile");
        let mut push = |v: Option<&str>| {
            if let Some(name) = v {
                if !name.is_empty() && !out.iter().any(|n| n == name) {
                    out.push(name.to_string());
                }
            }
        };
        push(profile.and_then(|p| p.get("last_used")).and_then(|v| v.as_str()));
        if let Some(list) = profile
            .and_then(|p| p.get("last_active_profiles"))
            .and_then(|v| v.as_array())
        {
            for item in list {
                push(item.as_str());
            }
        }
    }
    if !out.iter().any(|n| n == "Default") {
        out.push("Default".to_string());
    }
    out
}

/// 读取并解析 `Local State`（JSON；缺失或损坏返回 None，不影响兜底逻辑）。
pub fn read_local_state(root: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(root.join("Local State")).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_db_path_prefers_network_subdir() {
        let tmp = std::env::temp_dir().join(format!("astorm-cookies-{}", uuid::Uuid::new_v4()));
        let profile = tmp.join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        // 旧布局:Profile 根下的 Cookies
        std::fs::write(profile.join("Cookies"), b"x").unwrap();
        assert_eq!(cookies_db_path(&profile), profile.join("Cookies"));
        // 新布局:Network/Cookies 优先
        std::fs::create_dir_all(profile.join("Network")).unwrap();
        std::fs::write(profile.join("Network").join("Cookies"), b"x").unwrap();
        assert_eq!(cookies_db_path(&profile), profile.join("Network").join("Cookies"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn profile_dirs_filters_chrome_internal_dirs() {
        let tmp = std::env::temp_dir().join(format!("astorm-profiles-{}", uuid::Uuid::new_v4()));
        for name in ["Default", "Profile 1", "System Profile", "Crashpad", "ShaderCache"] {
            std::fs::create_dir_all(tmp.join(name)).unwrap();
        }
        let mut names: Vec<String> = profile_dirs(&tmp)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["Default".to_string(), "Profile 1".to_string()]);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
