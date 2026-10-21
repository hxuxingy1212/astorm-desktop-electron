//! 原子文件写入：同目录临时文件 + rename。
//!
//! 配置文件（settings / providers / MCP / OAuth / KB manifest / mail accounts）
//! 直接 `fs::write` 在进程崩溃或断电时会留下截断 JSON，下次启动全部回退默认值。
//! 这里统一写入同目录临时文件，`fsync` 后 rename 覆盖目标；rename 在同一文件
//! 系统内是原子的（Windows 上同样替换已有文件）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 原子写入 `path`（普通权限）。
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    write_atomic_mode(path, data, None)
}

/// 原子写入 `path`；`mode` 仅 Unix 生效（例如 OAuth token / mail 账户用 0o600）。
///
/// 临时文件创建时即设置权限，避免“先宽后窄”的窗口；rename 后目标继承该权限。
pub fn write_atomic_mode(path: &Path, data: &[u8], _mode: Option<u32>) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let tmp = parent.join(temp_name(name));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = _mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn temp_name(name: &std::ffi::OsStr) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!(
        ".{}.tmp-{}-{nanos}-{seq}",
        name.to_string_lossy(),
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astorm-fsx-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn replaces_existing_and_leaves_no_temp() {
        let dir = temp_dir("replace");
        let path = dir.join("config.json");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, br#"{"ok":true}"#).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"ok":true}"#);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp leftovers: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_parent_and_writes() {
        let dir = temp_dir("parents");
        let path = dir.join("a").join("b").join("config.json");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failure_keeps_existing_target_and_cleans_temp() {
        let dir = temp_dir("failure");
        // 目标是目录：rename(file, dir) 必然失败
        let target = dir.join("config-dir");
        std::fs::create_dir(&target).unwrap();
        let err = write_atomic(&target, b"x").unwrap_err();
        assert!(target.is_dir(), "err={err}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp leftovers: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn private_mode_applied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let path = dir.join("oauth.json");
        write_atomic_mode(&path, b"{}", Some(0o600)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
