//! 本地编程环境探测：检测系统是否安装了常见编程语言运行时 / 工具链 / 构建工具，
//! 并读取其版本号。供前端「本地环境」设置页展示。
//!
//! 纯函数、无需会话：探测表与沙箱 exec 白名单共用（单一事实源
//! `astorm_sandbox::toolchain::TOOLCHAIN_PROBES`），存在性经纯 Rust `$PATH`
//! 扫描（`which_path`）判定，再用 `<tool> --version` 取版本。每条记录独立成
//! `LocalEnv`，缺失工具标记 `available=false`。

use astorm_sandbox::toolchain::{which_path, ToolchainProbe, TOOLCHAIN_PROBES};
use napi_derive::napi;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Windows 下不创建控制台窗口(静默执行子进程)。
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 版本探测超时(秒)：避免个别 `--version` 挂起(如等待 daemon)。
const VERSION_TIMEOUT_SECS: u64 = 4;

/// 单个本地环境探测结果。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEnv {
    /// 环境显示名(如 "Node.js")。
    pub name: String,
    /// 是否检测到(可执行文件存在)。
    pub available: bool,
    /// 版本号(如 "v22.12.0")；未检测到或取版本失败为 None。
    pub version: Option<String>,
    /// 版本号来源(用于区分系统工具 vs 内建 / 管理工具版本)；
    /// 本模块统一 "system"(按 $PATH 探测)。
    pub version_src: Option<String>,
}

/// 探测全部本地编程环境。
#[napi]
pub fn detect_local_env() -> Vec<LocalEnv> {
    TOOLCHAIN_PROBES.iter().map(detect_one).collect()
}

fn detect_one(p: &ToolchainProbe) -> LocalEnv {
    // 用解析出的绝对路径跑版本探测：Windows 上 npm/pnpm/yarn 是 .cmd，
    // `Command::new("npm")` 无法执行，会出现“available=true 但 version=None”。
    let resolved = which_path(p.bin);
    let available = resolved.is_some();
    let version = resolved
        .as_ref()
        .and_then(|path| run_version(&path.to_string_lossy(), p.version_args));
    LocalEnv {
        name: p.name.to_string(),
        available,
        version,
        version_src: Some("system".to_string()),
    }
}

/// 运行版本命令并抽取版本行；带超时兜底。
fn run_version(bin: &str, args: &[&str]) -> Option<String> {
    let args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let bin = bin.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = {
            let is_batch = std::path::Path::new(&bin)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
                .unwrap_or(false);
            #[allow(unused_mut)]
            let mut cmd = if cfg!(windows) && is_batch {
                // .cmd/.bat 不能直接 CreateProcess，走 cmd /D /S /C。
                let mut line = format!("\"{bin}\"");
                for a in &args {
                    line.push(' ');
                    line.push_str(a);
                }
                let mut c = Command::new("cmd.exe");
                c.args(["/D", "/S", "/C", &line]);
                c
            } else {
                let mut c = Command::new(&bin);
                c.args(&args);
                c
            };
            #[cfg(target_os = "windows")]
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd.output()
        }
        .ok()
        // 很多工具(`go`/`java`/`mvn`/`docker compose`)把版本写到 stderr,合并取首行。
        .map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        });
        let _ = tx.send(out);
    });
    // recv_timeout:超时返回 Err → 视为取版本失败;线程仍会结束,结果丢弃。
    rx.recv_timeout(Duration::from_secs(VERSION_TIMEOUT_SECS))
        .ok()
        .flatten()
        .and_then(|t| version_line(&t))
}

/// 从命令输出里抽取"版本号"所在行。
/// 规则:跳过空行 / 超长介绍行 / 版权行 / 纯元数据行(平台、目录、线程模型等),
/// 取第一条包含数字且形似版本的短行——大多工具的头一版行即真版本。
fn version_line(output: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() || t.chars().count() > 120 {
            continue;
        }
        let lower = t.to_lowercase();
        if lower.contains("copyright") || lower.contains("all rights reserved") {
            continue;
        }
        // 元数据行(平台 / 安装目录 / 线程模型 / 构建目标等)不是版本
        let lower_trim = lower.trim_start();
        if lower_trim.starts_with("target:")
            || lower_trim.starts_with("thread model")
            || lower_trim.starts_with("installeddir")
            || lower_trim.starts_with("this program built")
            || lower_trim.starts_with("kustomize version")
        {
            continue;
        }
        if t.chars().any(|c| c.is_ascii_digit()) && looks_like_version(&t) {
            // 部分工具首行是介绍(如 node 的 "v22.12.0"、go 的 "go version go1.21"),
            // 其含版本关键字,可直接返回;否则记录回退继续找更贴切的行。
            if lower.starts_with("node.js")
                || lower.starts_with("swift-driver")
                || lower.starts_with("this program")
            {
                fallback.get_or_insert_with(|| t.to_string());
                continue;
            }
            return Some(t.to_string());
        }
    }
    fallback
}

/// 形似版本号：含数字且含小数点(如 `1.0` / `v22.12.0` / `go1.21`)或版本关键字。
/// 避免把 `line1`、`Copyright (c) 2024` 这类带数字但非版本的行误判。
fn looks_like_version(t: &str) -> bool {
    let lower = t.to_lowercase();
    if lower.contains("version") {
        return true;
    }
    let b = t.as_bytes();
    let has_dot_near_digit = b.windows(2).any(|w| {
        (w[0].is_ascii_digit() && w[1] == b'.') || (w[0] == b'.' && w[1].is_ascii_digit())
    });
    has_dot_near_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_picks_numeric_short_line() {
        let out = "Copyright (c) 2024 Example\nNode.js v22.12.0\nLicense MIT";
        assert_eq!(version_line(out).as_deref(), Some("Node.js v22.12.0"));
    }

    #[test]
    fn version_line_skips_copyright_and_handles_go() {
        let out = "go version go1.22.1 linux/amd64";
        assert_eq!(version_line(out).as_deref(), Some("go version go1.22.1 linux/amd64"));
    }

    #[test]
    fn version_line_skips_long_lines() {
        let long = "a".repeat(200);
        let out = format!("{long}\nfoo\n");
        assert_eq!(version_line(&out), None);
    }

    #[test]
    fn version_line_takes_first_numeric() {
        let out = "line1\nversion 1.0\nline2\nversion 2.0\n";
        assert_eq!(version_line(out).as_deref(), Some("version 1.0"));
    }

    #[test]
    fn version_line_picks_metadata_target_ignored() {
        // gcc/clang 输出: 首行是 "Apple clang version",后含 Target: 等元数据行
        let out = "Apple clang version 19.1.0 (clang-1911.0.0.1.2)\nTarget: arm64-apple-darwin25.5.0\nThread model: posix\nInstalledDir: /opt/homebrew/bin";
        assert_eq!(version_line(out).as_deref(), Some("Apple clang version 19.1.0 (clang-1911.0.0.1.2)"));
    }

    #[test]
    fn version_line_prefers_first_meaningful_over_metadata() {
        let out = "GNU Make 3.81\nCopyright (C) 2002  Free Software Foundation, Inc.\nThis program built for i386-apple-darwin11.3.0";
        assert_eq!(version_line(out).as_deref(), Some("GNU Make 3.81"));
    }

    #[test]
    fn which_and_run_are_safe_for_missing_tools() {
        // 不存在工具:可用=false,版本=None,不 panic
        assert!(which_path("definitely-not-a-real-tool-xyz").is_none());
        assert_eq!(run_version("definitely-not-a-real-tool-xyz", &["--version"]), None);
    }
}
