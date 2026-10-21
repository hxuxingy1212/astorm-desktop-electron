//! 内置运行时解析（Python / Node / Git）。
//!
//! 分发时随包携带可移植运行时（`runtime/<name>/…` + `runtime/manifest.json`），
//! 用户无需自行安装。解析器在启动子进程前：
//! - 直接执行模式下改写 argv[0]（如 `python` → 内置绝对路径）
//! - 解释器模式下把内置运行时 bin 目录注入子进程 PATH 前缀
//!
//! `--runtime-prefs system` 时可关闭内置优先，改用系统安装的运行时。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// manifest.json 中单条运行时条目。
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    /// 规范名：python / node / npm / npx / git
    pub name: String,
    /// 入口程序（相对 runtime 根目录）
    pub program: String,
    /// bin 目录（相对 runtime 根目录；缺省为 program 所在目录）
    #[serde(default)]
    pub bin_dir: Option<String>,
    /// 版本号（信息性）
    #[serde(default)]
    pub version: Option<String>,
}

/// manifest.json 根结构。
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeManifestFile {
    #[serde(default)]
    pub runtimes: Vec<ManifestEntry>,
}

/// 已加载的运行时解析器。
#[derive(Debug, Clone)]
pub struct RuntimeResolver {
    /// runtime 根目录（None = 未发现内置运行时）
    root: Option<PathBuf>,
    /// 规范名 → (入口绝对路径, 版本)
    programs: Vec<(String, PathBuf, Option<String>)>,
    /// PATH 注入目录列表
    bin_dirs: Vec<PathBuf>,
    /// true = 内置优先（缺省）；false = 使用系统运行时
    pub prefer_builtin: bool,
    /// 发现失败时的说明（供汇报）
    pub load_notice: Option<String>,
}

impl RuntimeResolver {
    /// 自动探测内置运行时：先取可执行文件旁的 `runtime/`，再取当前工作目录的 `runtime/`。
    pub fn detect(prefer_builtin: bool) -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("runtime"));
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("runtime"));
        }
        for c in candidates {
            if let Ok(r) = Self::from_dir(&c, prefer_builtin) {
                return r;
            }
        }
        RuntimeResolver {
            root: None,
            programs: Vec::new(),
            bin_dirs: Vec::new(),
            prefer_builtin,
            load_notice: Some("未发现内置运行时目录（runtime/），使用系统路径".to_string()),
        }
    }

    /// 从指定 runtime 根目录加载。
    pub fn from_dir(dir: &Path, prefer_builtin: bool) -> Result<Self, String> {
        let manifest_path = dir.join("manifest.json");
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            format!("读取运行时清单 {} 失败：{e}", manifest_path.display())
        })?;
        let mf: RuntimeManifestFile =
            serde_json::from_str(&text).map_err(|e| format!("解析运行时清单失败：{e}"))?;

        let mut programs = Vec::new();
        let mut bin_dirs = Vec::new();
        for e in &mf.runtimes {
            let program = dir.join(&e.program);
            if !program.is_file() {
                return Err(format!(
                    "运行时入口不存在：{}（清单声明 {}）",
                    program.display(),
                    e.program
                ));
            }
            let bin = e
                .bin_dir
                .as_ref()
                .map(|b| dir.join(b))
                .unwrap_or_else(|| {
                    program
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| dir.to_path_buf())
                });
            programs.push((e.name.clone(), program, e.version.clone()));
            if !bin_dirs.contains(&bin) {
                bin_dirs.push(bin);
            }
        }
        Ok(RuntimeResolver {
            root: Some(dir.to_path_buf()),
            programs,
            bin_dirs,
            prefer_builtin,
            load_notice: None,
        })
    }

    /// 未加载任何内置运行时的解析器（等价于仅使用系统运行时）。
    pub fn none(prefer_builtin: bool) -> Self {
        RuntimeResolver {
            root: None,
            programs: Vec::new(),
            bin_dirs: Vec::new(),
            prefer_builtin,
            load_notice: None,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn is_loaded(&self) -> bool {
        self.root.is_some()
    }

    /// 规范化命令名（python3 → python 等）。
    fn canonical(name: &str) -> Option<&'static str> {
        let n = name.to_ascii_lowercase();
        if n == "python" || n.starts_with("python3") || n.starts_with("python2") {
            Some("python")
        } else if n == "node" {
            Some("node")
        } else if n == "npm" {
            Some("npm")
        } else if n == "npx" {
            Some("npx")
        } else if n == "git" {
            Some("git")
        } else {
            None
        }
    }

    /// 把 argv[0] 解析为内置运行时的绝对路径；未知或未启用内置时返回 None（走系统）。
    pub fn resolve_argv0(&self, argv0: &str) -> Option<PathBuf> {
        if !self.prefer_builtin {
            return None;
        }
        if argv0.contains('/') || argv0.contains('\\') {
            return None; // 已是路径，不做改写
        }
        let canonical = Self::canonical(argv0)?;
        self.programs
            .iter()
            .find(|(name, _, _)| name == canonical)
            .map(|(_, p, _)| p.clone())
    }

    /// PATH 注入目录前缀列表（内置优先时）。
    pub fn path_entries(&self) -> Vec<PathBuf> {
        if !self.prefer_builtin {
            return Vec::new();
        }
        self.bin_dirs.clone()
    }

    /// 列出全部内置运行时（CLI `sbox runtimes list` 用）。
    pub fn list(&self) -> Vec<(String, PathBuf, Option<String>)> {
        self.programs.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, entries: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        for (_name, program) in entries {
            let full = dir.join(program);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, "stub").unwrap();
        }
        let runtimes: Vec<_> = entries
            .iter()
            .map(|(name, program)| {
                serde_json::json!({ "name": name, "program": program })
            })
            .collect();
        let manifest = serde_json::json!({ "runtimes": runtimes });
        std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    }

    #[test]
    fn 加载清单并解析argv0() {
        let dir = std::env::temp_dir().join(format!("astorm-test-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_manifest(
            &dir,
            &[
                ("python", "python/python.exe"),
                ("node", "node/node.exe"),
                ("git", "git/cmd/git.exe"),
            ],
        );
        let r = RuntimeResolver::from_dir(&dir, true).unwrap();
        let p = r.resolve_argv0("python").unwrap();
        assert_eq!(p, dir.join("python/python.exe"));
        assert_eq!(r.resolve_argv0("python3"), Some(dir.join("python/python.exe")));
        assert_eq!(r.resolve_argv0("node").unwrap(), dir.join("node/node.exe"));
        assert_eq!(r.resolve_argv0("git").unwrap(), dir.join("git/cmd/git.exe"));
        // 未知命令 → None
        assert!(r.resolve_argv0("ls").is_none());
        // 已是路径 → None
        assert!(r.resolve_argv0("C:\\tools\\python.exe").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 内置优先开关() {
        let dir = std::env::temp_dir().join(format!("astorm-test-rt2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_manifest(&dir, &[("node", "node/node.exe")]);
        let sys = RuntimeResolver::from_dir(&dir, false).unwrap();
        assert!(sys.resolve_argv0("node").is_none());
        assert!(sys.path_entries().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path注入目录() {
        let dir = std::env::temp_dir().join(format!("astorm-test-rt3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_manifest(
            &dir,
            &[
                ("python", "python/python.exe"),
                ("node", "node/node.exe"),
            ],
        );
        let r = RuntimeResolver::from_dir(&dir, true).unwrap();
        let entries = r.path_entries();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e == &dir.join("python")));
        assert!(entries.iter().any(|e| e == &dir.join("node")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 缺失入口报错() {
        let dir = std::env::temp_dir().join(format!("astorm-test-rt4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = serde_json::json!({
            "runtimes": [{ "name": "python", "program": "python/python.exe" }]
        });
        std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(RuntimeResolver::from_dir(&dir, true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}