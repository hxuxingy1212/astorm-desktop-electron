//! 平台强制层的「静态可读根」单一事实源。
//!
//! 有两个消费者，口径必须一致（否则会出现「内核放行、静态层弹卡」的矛盾）：
//! - macOS 会话：`sandbox::seatbelt_builder` 据此生成 Seatbelt 读白名单；
//! - 工具层静态预检（astorm-core）：判定「空间外读」时豁免内核本已放行的路径——
//!   否则 `which git; /usr/bin/git --version`、`ls /opt/homebrew/bin` 这类命令
//!   会弹无意义的审批卡（内核 profile 明确放行 `/usr`、`/opt/homebrew`）。
//!
//! Windows 的强制层依赖 AppContainer 对系统区的默认可读 ACE
//! （见 `sandbox::windows` 的能力说明），本模块按同一语义给出系统区根，
//! 仅供静态层对齐；Windows 不消费这些根去生成规则。

use std::path::{Path, PathBuf};

/// 一条静态可读根。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRoot {
    /// 整个子树可读（macOS 生成 profile 时会展开 `/tmp` → `/private/tmp` 之类别名）。
    Subtree(String),
    /// 仅该文件可读（profile 里是 `(literal …)`）。
    File(String),
}

/// 平台系统区的可读根（当前平台）。
pub fn system_read_roots() -> Vec<ReadRoot> {
    #[cfg(target_os = "macos")]
    {
        macos_system_read_roots()
    }
    #[cfg(windows)]
    {
        windows_system_read_roots()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_system_read_roots()
    }
    #[cfg(not(any(target_os = "macos", windows, unix)))]
    {
        Vec::new()
    }
}

/// Linux/容器系统区可读根。
///
/// Linux 强制层是 passthrough（无内核限制），静态预检必须与“系统区默认可读”
/// 的事实对齐，否则 `git --version`、读 `/etc/ssl/certs`、`curl` 读
/// `/etc/resolv.conf` 都会弹无意义的“会话空间外读”审批。
#[cfg(all(unix, not(target_os = "macos")))]
pub fn linux_system_read_roots() -> Vec<ReadRoot> {
    [
        "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/dev",
        "/proc",
    ]
    .iter()
    .map(|p| ReadRoot::Subtree((*p).to_string()))
    .collect()
}

/// macOS 系统区的可读根。
///
/// 顺序与 Seatbelt profile 的下发顺序一致，便于和实际 profile 逐条比对；
/// **不带 cfg**：profile 生成器在 Windows 上也生成 macOS profile（可单测），
/// 所以这里必须始终是 macOS 的那一份，不能随宿主平台变化。
pub fn macos_system_read_roots() -> Vec<ReadRoot> {
    vec![
        // dyld/系统运行必要：根目录、系统库、系统框架、可执行目录
        ReadRoot::File("/".to_string()),
        ReadRoot::Subtree("/usr".to_string()),
        ReadRoot::Subtree("/System".to_string()),
        ReadRoot::Subtree("/bin".to_string()),
        ReadRoot::Subtree("/sbin".to_string()),
        ReadRoot::Subtree("/private/var/db/dyld".to_string()),
        ReadRoot::Subtree("/dev".to_string()),
        ReadRoot::Subtree("/private/tmp".to_string()),
        // 系统配置区（`/etc` 在 macOS 上是 `/private/etc` 的符号链接）：TLS 信任库与配置
        // （LibreSSL 启动即读 `/private/etc/ssl/openssl.cnf`、证书 `/private/etc/ssl/cert.pem`）、
        // `/private/etc/hosts`、`/private/etc/services` 等都在此。
        // 缺了它**网络放行也没用**：curl/wget/python-requests 会在 TLS 初始化阶段就以一条
        // 裸 `Operation not permitted` 失败（原文是 `fopen('/private/etc/ssl/openssl.cnf','rb')`），
        // 看起来像「网络被沙箱禁了」，实际出站根本没被限制。
        ReadRoot::Subtree("/private/etc".to_string()),
        // DNS 解析配置（`/etc/resolv.conf` → `/private/var/run/resolv.conf`）：
        // 只放 `/private/etc` 时解析时好时坏（实测同一 profile 两次 curl 一成一败），
        // resolver 配置读不到就只能走 mDNSResponder 兜底。这里是**字面量**，
        // 不整目录放行 `/private/var/run`（那里面是各守护进程的 socket / pid 文件）。
        ReadRoot::File("/private/var/run/resolv.conf".to_string()),
        // Xcode / CLT 许可状态（世界可读的 456 字节 plist）：`cc`/`clang` 启动时会读它，
        // 读不到就报「You have not agreed to the Xcode license agreements」并 exit 69——
        // 在沙箱里表现为「SDK 放行了却仍然编译不了」（实测；沙箱外同一台机器 `cc` 正常）。
        // 只放这一个文件，不放整个 `/Library/Preferences`。
        ReadRoot::File("/Library/Preferences/com.apple.dt.Xcode.plist".to_string()),
    ]
}

/// Windows 系统区的可读根（AppContainer 对系统区的默认可读 ACE 语义）。
pub fn windows_system_read_roots() -> Vec<ReadRoot> {
    let mut roots = Vec::new();
    for key in [
        "SystemRoot",  // C:\Windows（含 System32）
        "ProgramData", // 程序公共数据
        "ProgramFiles",
        "ProgramFiles(x86)",
        "PUBLIC", // 公共用户目录
    ] {
        if let Some(v) = std::env::var_os(key) {
            let p = PathBuf::from(v);
            if !p.as_os_str().is_empty() {
                roots.push(ReadRoot::Subtree(p.to_string_lossy().into_owned()));
            }
        }
    }
    roots
}

/// macOS 根目录符号链接别名（Seatbelt 按解析后的真实路径匹配，静态判定也要两形态都认）。
///
/// macOS 的 `/tmp` `/var` `/etc` 是指向 `/private/*` 的符号链接，用户规则常写前者，
/// 而内核强制在真实路径上判定，因此两者都要下发，否则规则失配。
/// 纯字符串逻辑，跨平台可单测。
pub fn macos_alias_forms(path: &str) -> Vec<String> {
    let mut forms = vec![path.to_string()];
    for (alias, real) in [
        ("/tmp", "/private/tmp"),
        ("/var", "/private/var"),
        ("/etc", "/private/etc"),
    ] {
        if path == alias || path.starts_with(&format!("{alias}/")) {
            forms.push(format!("{real}{}", &path[alias.len()..]));
        }
    }
    forms
}

/// 已登记工具链的只读目录（与 exec 放行同源，见 [`crate::toolchain::exec_allow_dirs`]）。
pub fn toolchain_read_dirs(disabled: &[String]) -> Vec<PathBuf> {
    crate::toolchain::read_allow_dirs(disabled)
}

/// 静态判定：路径是否落在平台强制层默认可读的范围内（系统区 + 已登记工具链目录）。
///
/// 只做**字符串前缀**判定：不解析符号链接、不归一化 `..`。
/// 取向是保守的——判不出来就返回 `false`（静态层于是照常弹卡/走审批），
/// 不会把内核要拒绝的路径当成可读放过去。
pub fn statically_readable(path: &Path, toolchain_disabled: &[String]) -> bool {
    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        macos_alias_forms(&path.to_string_lossy())
            .into_iter()
            .map(PathBuf::from)
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    for root in system_read_roots() {
        match root {
            ReadRoot::Subtree(dir) => {
                let rp = Path::new(&dir);
                if candidates.iter().any(|c| path_starts_with_ci(c, rp)) {
                    return true;
                }
            }
            ReadRoot::File(file) => {
                if candidates
                    .iter()
                    .any(|c| norm_ci(c) == norm_ci(Path::new(&file)))
                {
                    return true;
                }
            }
        }
    }
    toolchain_read_dirs(toolchain_disabled)
        .iter()
        .any(|d| path_starts_with_ci(path, d))
}

/// 路径比较用的归一化字符串：Windows 统一 `/`；Windows/macOS 小写化。
/// Linux 保留原始大小写（文件系统区分大小写）。
fn norm_ci(path: &Path) -> String {
    let s = if cfg!(windows) {
        path.to_string_lossy().replace('\\', "/")
    } else {
        path.to_string_lossy().into_owned()
    };
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s
    }
}

fn path_starts_with_ci(path: &Path, root: &Path) -> bool {
    let p = norm_ci(path);
    let r = norm_ci(root);
    let r = r.trim_end_matches('/');
    p == r || p.starts_with(&format!("{r}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statically_readable_case_matching_follows_platform() {
        let root = system_read_roots()
            .into_iter()
            .find_map(|r| match r {
                ReadRoot::Subtree(s) => Some(s),
                _ => None,
            })
            .expect("至少有一个子树根");
        let sep = if cfg!(windows) { '\\' } else { '/' };
        let probe = format!("{}{sep}astorm-case-probe", root.to_uppercase());
        #[cfg(any(windows, target_os = "macos"))]
        assert!(
            statically_readable(Path::new(&probe), &[]),
            "Windows/macOS 系统根应大小写不敏感: {probe}"
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        assert!(
            !statically_readable(Path::new(&probe), &[]),
            "Linux 系统根应保持大小写敏感: {probe}"
        );
        // 原大小写始终命中
        let exact = format!("{root}{sep}astorm-case-probe");
        assert!(statically_readable(Path::new(&exact), &[]));
    }

    #[test]
    fn 别名形态双向匹配() {
        assert_eq!(
            macos_alias_forms("/etc/hosts"),
            vec!["/etc/hosts".to_string(), "/private/etc/hosts".to_string()]
        );
        // 真实路径不再反向展开
        assert_eq!(
            macos_alias_forms("/private/etc/hosts"),
            vec!["/private/etc/hosts".to_string()]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn 系统区可读_用户区不可读() {
        let none: Vec<String> = Vec::new();
        for ok in [
            "/",
            "/usr/bin/git",
            "/bin/echo",
            "/System/Library/x",
            "/etc/hosts",
            "/private/etc/ssl/cert.pem",
            "/tmp/x.txt",
            "/private/tmp/x.txt",
            "/dev/null",
            "/var/db/dyld/x",
        ] {
            assert!(statically_readable(Path::new(ok), &none), "{ok} 应判为可读");
        }
        for no in [
            "/Applications/Xcode.app/x",
            "/Users/x/.ssh/id_rsa",
            "/Users/x/Library/Keychains/login.keychain-db",
            "/var/run/other.sock",
            "/usrx/evil",
        ] {
            assert!(!statically_readable(Path::new(no), &none), "{no} 不应判为可读");
        }
        // 开发工具链目录在 P1 之后按设计可读（原生编译前提）：本机装了才断言
        if Path::new("/Library/Developer/CommandLineTools/SDKs").exists() {
            assert!(
                statically_readable(
                    Path::new("/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/x"),
                    &none
                ),
                "CLT SDK 应判为可读（原生编译前提）"
            );
        }
        if Path::new("/Applications/Xcode.app/Contents/Developer").exists() {
            assert!(
                statically_readable(
                    Path::new("/Applications/Xcode.app/Contents/Developer/usr/lib/libxcrun.dylib"),
                    &none
                ),
                "Xcode Developer 目录应判为可读（xcrun 前提）"
            );
        }
    }

    #[test]
    fn 前缀判定不跨路径分量() {
        let none: Vec<String> = Vec::new();
        // `/usr` 放行不得把 `/usrx` 也放进去（Path::starts_with 按分量匹配）
        assert!(!statically_readable(Path::new("/usrlocal/x"), &none));
    }
}
