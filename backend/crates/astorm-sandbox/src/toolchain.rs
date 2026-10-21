//! 本地工具链解析：把用户机器上实际安装的编程环境目录动态注入沙箱白名单。
//!
//! 单一事实源：探测表 [`TOOLCHAIN_PROBES`] 与 napi「本地环境」设置页（`astorm-napi/src/localenv.rs`）
//! 共用——设置页展示哪些工具、沙箱就放行哪些工具所在目录，两侧不会漂移。
//!
//! 解析全部为**纯 Rust 文件系统扫描**（遍历 `$PATH` + 可执行位检查），不 spawn 子进程，
//! 单次开销微秒级，进程内经 [`OnceLock`] 只解析一次。
//!
//! 安全权衡：放行的只是「exec + 只读」用户自装工具链目录；工具进程仍受
//! 文件写（仅会话空间）与网络（默认拒）沙箱约束——与 Seatbelt 静态放行
//! `/opt/homebrew/bin` 的论证一致（见 `sandbox/seatbelt_builder.rs`）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 平台 home 目录：Unix/macOS 用 HOME；Windows 通常只有 USERPROFILE。
/// （测试会显式设置 HOME，因此仍优先 HOME。）
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// 待探测工具描述：显示名 + 可执行名 + 版本参数（napi localenv 与沙箱白名单共用）。
pub struct ToolchainProbe {
    /// 环境显示名（如 "Node.js"；设置页展示用）。
    pub name: &'static str,
    /// 可执行文件名（按 `$PATH` 解析）。
    pub bin: &'static str,
    /// 版本探测参数（如 `["--version"]`；仅 localenv 用，沙箱不关心）。
    pub version_args: &'static [&'static str],
}

/// 全部待探测项（29 项：语言运行时 / C-C++ 工具链 / 构建与基础设施）。
pub const TOOLCHAIN_PROBES: &[ToolchainProbe] = &[
    // —— 核心语言运行时 ——
    ToolchainProbe { name: "Node.js", bin: "node", version_args: &["--version"] },
    ToolchainProbe { name: "npm", bin: "npm", version_args: &["--version"] },
    ToolchainProbe { name: "pnpm", bin: "pnpm", version_args: &["--version"] },
    ToolchainProbe { name: "yarn", bin: "yarn", version_args: &["--version"] },
    ToolchainProbe { name: "Deno", bin: "deno", version_args: &["--version"] },
    ToolchainProbe { name: "Bun", bin: "bun", version_args: &["--version"] },
    ToolchainProbe { name: "Python", bin: "python3", version_args: &["--version"] },
    ToolchainProbe { name: "pip", bin: "pip3", version_args: &["--version"] },
    ToolchainProbe { name: "uv", bin: "uv", version_args: &["--version"] },
    ToolchainProbe { name: "Go", bin: "go", version_args: &["version"] },
    ToolchainProbe { name: "Cargo", bin: "cargo", version_args: &["--version"] },
    ToolchainProbe { name: "Rustc", bin: "rustc", version_args: &["--version"] },
    ToolchainProbe { name: "Java", bin: "java", version_args: &["-version"] },
    ToolchainProbe { name: ".NET", bin: "dotnet", version_args: &["--version"] },
    ToolchainProbe { name: "Ruby", bin: "ruby", version_args: &["--version"] },
    ToolchainProbe { name: "PHP", bin: "php", version_args: &["--version"] },
    ToolchainProbe { name: "Swift", bin: "swift", version_args: &["--version"] },
    // —— C/C++ 工具链 ——
    ToolchainProbe { name: "GCC", bin: "gcc", version_args: &["--version"] },
    ToolchainProbe { name: "G++", bin: "g++", version_args: &["--version"] },
    ToolchainProbe { name: "Clang", bin: "clang", version_args: &["--version"] },
    ToolchainProbe { name: "CMake", bin: "cmake", version_args: &["--version"] },
    ToolchainProbe { name: "Make", bin: "make", version_args: &["--version"] },
    // —— 构建 / 包管理 / 基础设施 ——
    ToolchainProbe { name: "Git", bin: "git", version_args: &["--version"] },
    ToolchainProbe { name: "Docker", bin: "docker", version_args: &["--version"] },
    ToolchainProbe { name: "Docker Compose", bin: "docker", version_args: &["compose", "version"] },
    ToolchainProbe { name: "Kubernetes", bin: "kubectl", version_args: &["version", "--client=true"] },
    ToolchainProbe { name: "Maven", bin: "mvn", version_args: &["-version"] },
    ToolchainProbe { name: "Gradle", bin: "gradle", version_args: &["--version"] },
    ToolchainProbe { name: "Homebrew", bin: "brew", version_args: &["--version"] },
];

/// 在 `$PATH` 中解析可执行文件的真实路径（等价 `command -v` / `where`，纯 Rust 实现）。
///
/// Unix：按可执行位判断；Windows：依次尝试 `PATHEXT` 扩展（.exe/.cmd/…）。
/// 返回未跟随符号链接的命中项（调用方按需 canonicalize）。
pub fn which_path(bin: &str) -> Option<PathBuf> {
    // bin 里已带分隔符 → 视为显式路径，直接检查
    if bin.contains('/') || bin.contains('\\') {
        let p = Path::new(bin);
        return if is_executable(p) { Some(p.to_path_buf()) } else { None };
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        #[cfg(windows)]
        let candidates: Vec<PathBuf> = {
            let bare = dir.join(bin);
            let mut v = vec![bare.clone()];
            if bare.extension().is_none() {
                for ext in pathext_extensions() {
                    v.push(dir.join(format!("{bin}{ext}")));
                }
            }
            v
        };
        #[cfg(unix)]
        let candidates: Vec<PathBuf> = vec![dir.join(bin)];
        for cand in candidates {
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// Windows `PATHEXT` 扩展列表（含前导点；缺省回退 .exe）。
#[cfg(windows)]
fn pathext_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// 路径存在且可执行（Unix 查任意可执行位；Windows 查常规文件）。
fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(p) {
            Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(windows)]
    {
        matches!(std::fs::metadata(p), Ok(m) if m.is_file())
    }
}

/// Seatbelt 已静态放行的目录（`seatbelt_builder.rs` 硬编码）。
/// 动态解析结果剔除这些，避免 profile 重复膨胀。
fn statically_allowed_dirs() -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = home_dir() {
        v.push(home.join(".cargo/bin"));
        v.push(home.join(".local/bin"));
    }
    v
}

/// 单条解析记录：工具 bin → 真实所在目录（含 rustup 附加归属）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainEntry {
    /// 工具 bin 名（对应 `TOOLCHAIN_PROBES[].bin`；rustup 附加目录归属 cargo/rustc）
    pub bin: String,
    /// 真实所在目录（canonicalize 后的父目录）
    pub dir: PathBuf,
}

/// 解析全部探测工具的 bin → 真实目录记录（进程内缓存；机器级不变）。
///
/// 每个 bin 经 [`which_path`] 命中后 canonicalize（跟随符号链接，如
/// `/opt/homebrew/bin/python3` → `…/Cellar/python@3.x/…/bin`）取父目录；
/// 剔除静态放行目录；rustup 特例见 [`rustup_toolchain_bins`]。
/// **禁用过滤不在此做**——`exec_allow_dirs`/`read_allow_globs` 每次调用按
/// 当前 `toolchain_disabled` 配置过滤（配置热更即时生效，无需缓存失效）。
pub fn resolved_entries() -> Vec<ToolchainEntry> {
    static CACHE: OnceLock<Vec<ToolchainEntry>> = OnceLock::new();
    CACHE.get_or_init(compute_entries).clone()
}

fn compute_entries() -> Vec<ToolchainEntry> {
    let skip = statically_allowed_dirs();
    let mut entries: Vec<ToolchainEntry> = Vec::new();
    let push = |bin: &str,
                dir: PathBuf,
                entries: &mut Vec<ToolchainEntry>,
                skip: &[PathBuf]| {
        if !skip.contains(&dir) && !entries.iter().any(|e| e.bin == bin && e.dir == dir) {
            entries.push(ToolchainEntry {
                bin: bin.to_string(),
                dir,
            });
        }
    };

    for probe in TOOLCHAIN_PROBES {
        if let Some(p) = which_path(probe.bin) {
            // canonicalize：符号链接落在真实安装目录（Homebrew Cellar / .nvm versions 等）
            let real = std::fs::canonicalize(&p).unwrap_or(p);
            if let Some(parent) = real.parent() {
                push(probe.bin, parent.to_path_buf(), &mut entries, &skip);
            }
        }
    }

    // rustup 特例：~/.cargo/bin/cargo 是 shim，运行期 exec ~/.rustup/toolchains/*/bin
    // 里的真实工具链——shim 目录静态放行了，真实目录必须一并放行，否则
    // `cargo build` 在沙箱内必失败 → Auto 无防护二跑，沙箱形同虚设。
    // 归属 cargo 与 rustc 两个 bin：任一未禁用即保留（禁用其一不影响另一工具）。
    for d in rustup_toolchain_bins() {
        push("cargo", d.clone(), &mut entries, &skip);
        push("rustc", d, &mut entries, &skip);
    }

    // uv 托管 Python：不在 PATH 上（PATH 上是 Homebrew 的 python3），
    // 只有 venv 符号链接指过来 → 显式登记，归属 python3。
    for d in uv_python_bins() {
        push("python3", d, &mut entries, &skip);
    }

    // Xcode / CLT 开发目录：SDK 与 libxcrun 只读放行、clang/ld 可执行（原生编译前提），
    // 归属 clang（rustc 经 `cc` 驱动它）；「本地环境」页禁用 clang 即不注入。
    for d in developer_roots() {
        push("clang", d, &mut entries, &skip);
    }

    entries
}

/// bin 是否被禁用（`toolchain_disabled` 列表包含该 bin 名）。
fn bin_disabled(disabled: &[String], bin: &str) -> bool {
    disabled.iter().any(|d| d == bin)
}

/// 未禁用工具贡献的目录（去重保序；多工具共享目录任一启用即保留）。
fn enabled_dirs(disabled: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for e in resolved_entries() {
        if !bin_disabled(disabled, &e.bin) && !out.contains(&e.dir) {
            out.push(e.dir);
        }
    }
    out
}

/// 全部解析目录（去重，**未过滤禁用**；展示/测试用）。
pub fn resolve_tool_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for e in resolved_entries() {
        if !out.contains(&e.dir) {
            out.push(e.dir);
        }
    }
    out
}

/// `~/.rustup/toolchains/*/bin` 目录列表（rustup 未安装或无工具链 → 空）。
fn rustup_toolchain_bins() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let tc_root = home.join(".rustup").join("toolchains");
    let Ok(entries) = std::fs::read_dir(&tc_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let bin = e.path().join("bin");
        if bin.is_dir() {
            out.push(std::fs::canonicalize(&bin).unwrap_or(bin));
        }
    }
    out.sort();
    out
}

/// 是否经 rustup 安装（cargo/rustc 解析为 `~/.cargo/bin` shim；真实工具链在 ~/.rustup）。
pub fn rustup_installed() -> bool {
    which_path("cargo")
        .map(|p| {
            let p = p.to_string_lossy().to_string();
            p.contains(".cargo")
        })
        .unwrap_or(false)
}

/// uv 托管的 Python 解释器 bin 目录（`uv python install` / `uv sync` 自动拉取）。
///
/// PATH 上的 `python3` 通常是 Homebrew 口径，**uv 托管的解释器不在 PATH 上**：
/// 只有 venv 里的符号链接指过来（`.venv/bin/python` → `…/uv/python/<cpython-…>/bin/python3.11`）。
/// 不显式放行时 `.venv/bin/python` exec 被拒 → `uv sync` / `uv run` 必失败，
/// 而 `uv sync` 又拿不到解释器版本 ⇒ 整个 Python 工作流在沙箱内走不通
/// （会话 2026-10-08 实测：`Failed to query Python interpreter … Operation not permitted`）。
///
/// 目录源：`UV_PYTHON_INSTALL_DIR`（uv 官方覆盖变量）→ 否则平台默认
/// （macOS/Linux `~/.local/share/uv/python`，Windows `%APPDATA%\uv\python`）。
fn uv_python_bins() -> Vec<PathBuf> {
    let root = std::env::var_os("UV_PYTHON_INSTALL_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(windows)]
            {
                std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("uv").join("python"))
            }
            #[cfg(not(windows))]
            {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".local").join("share").join("uv").join("python"))
            }
        });
    let Some(root) = root else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let bin = e.path().join("bin");
        // Windows 布局把解释器放在安装根（`<…>/python.exe`），没有 bin/ 子目录
        let dir = if bin.is_dir() { bin } else { e.path() };
        if dir.is_dir() {
            out.push(std::fs::canonicalize(&dir).unwrap_or(dir));
        }
    }
    out.sort();
    out
}

/// 解析出来的开发工具链环境（macOS）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevToolchainEnv {
    /// `DEVELOPER_DIR` 取值（沙箱内固定用它，见 [`dev_toolchain_env`] 的说明）
    pub developer_dir: PathBuf,
    /// `SDKROOT` 取值（存在时给出；直接调 clang 时省掉 `-isysroot`）
    pub sdk_root: Option<PathBuf>,
}

/// 沙箱内可用的开发工具链环境（`DEVELOPER_DIR` / `SDKROOT`）。
///
/// 为什么需要（会话 2026-10-08 的硬墙，逐条实测）：
/// - **Xcode 选中时 `/usr/bin/cc` 在沙箱内必失败**：shim 会跑
///   `xcodebuild -sdk … -find clang`，而 xcodebuild 启动即要加载
///   `CoreDevice → /System/Library/PrivateFrameworks/MobileDevice.framework/…`——
///   该文件带 SIP `restricted` 标记，**Seatbelt 无法放行**（连 literal 白名单也照拒，
///   实测 `file system sandbox blocked open()`）；
/// - 把 `DEVELOPER_DIR` 指向 **Command Line Tools** 后，`cc` / `clang` / `xcrun` 全部走
///   `xcrun` 直连（不碰 xcodebuild），实测 `cc` 编译运行、`rustc -O` 链接运行全部通过；
/// - 仅装了 Xcode（无 CLT）时保留 Xcode 目录并给出 `SDKROOT`：直调
///   `<dev>/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang` 可用，`cc` shim 在沙箱内
///   仍不可用（无解，报错里会点明 license / xcodebuild）。
///
/// 归属 bin `clang`：在「本地环境」页禁用 clang 即不注入（与 exec/读放行同一口径）。
pub fn dev_toolchain_env(disabled: &[String]) -> Option<DevToolchainEnv> {
    if bin_disabled(disabled, "clang") {
        return None;
    }
    // 机器级不变：解析一次缓存（xcode-select 子进程只跑一次）
    static CACHE: OnceLock<Option<DevToolchainEnv>> = OnceLock::new();
    CACHE.get_or_init(compute_dev_toolchain).clone()
}

fn compute_dev_toolchain() -> Option<DevToolchainEnv> {
    let selected = selected_developer_dir()?;
    let selected_text = selected.to_string_lossy().to_string();
    let clt = PathBuf::from("/Library/Developer/CommandLineTools");
    let clt_clang = clt.join("usr").join("bin").join("clang");

    // Xcode 选中时 `cc` shim 依赖 xcodebuild（SIP 受限框架，沙箱内无解）→ 改指 CLT。
    // 该事实写进 bash 工具描述与 docs/backend-contract.md（沙箱层无日志桥接，不从这儿上报）。
    if selected_text.contains("Xcode") && clt_clang.exists() {
        return Some(DevToolchainEnv {
            sdk_root: first_sdk(&clt.join("SDKs")),
            developer_dir: clt,
        });
    }

    let sdk = if selected_text.contains("CommandLineTools") {
        first_sdk(&selected.join("SDKs"))
    } else {
        first_sdk(
            &selected
                .join("Platforms")
                .join("MacOSX.platform")
                .join("Developer")
                .join("SDKs"),
        )
    };
    Some(DevToolchainEnv {
        developer_dir: selected,
        sdk_root: sdk,
    })
}

/// 当前选中的开发目录：`xcode-select -p` → `/var/db/xcode_select_link` → 标准位置。
fn selected_developer_dir() -> Option<PathBuf> {
    if let Ok(out) = std::process::Command::new("xcode-select").arg("-p").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && Path::new(&p).is_dir() {
                return Some(PathBuf::from(p));
            }
        }
    }
    if let Ok(target) = std::fs::read_link("/var/db/xcode_select_link") {
        let abs = if target.is_absolute() {
            target
        } else {
            Path::new("/var/db").join(target)
        };
        if abs.is_dir() {
            return Some(abs);
        }
    }
    developer_roots().into_iter().next()
}

/// 目录下的 SDK：优先 `MacOSX.sdk`（当前版本的规范符号链接），否则取任一条目。
fn first_sdk(dir: &Path) -> Option<PathBuf> {
    let canonical = dir.join("MacOSX.sdk");
    if canonical.exists() {
        return Some(canonical);
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("MacOSX"))
                .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next_back()
}

/// 用户显式配置的「额外可执行目录」解析结果（安全配置 → 额外可执行目录）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtraToolDirs {
    /// exec 放行目录（Seatbelt `process-exec` / Windows ACL RX）
    pub exec: Vec<PathBuf>,
    /// 只读放行目录（exec 只是让进程能启动；dyld 还要读安装树里的 dylib/资源，
    /// 否则进程在 dyld 阶段 abort 且错误不含 "not permitted"，归因失效）
    pub read: Vec<PathBuf>,
}

/// 解析「额外可执行目录」配置项。
///
/// 用途：放行**非常规安装位置**的工具链——手动解压的编译器、独立工具链
/// （如 `/Library/Developer/Toolchains/swift-*.xctoolchain`）、自定义 SDK。
/// 自动探测只覆盖 PATH 上能解析到的工具（见 [`resolved_entries`]），
/// 这类目录进不了白名单，实测会以 EPERM 静默失败。
///
/// 规则（与工具链探测同构）：
/// - 只登记**真实存在**的目录（写错路径不报错，只是不生效）；
/// - canonicalize（Seatbelt 按解析后路径匹配，配置里可能是符号链接）；
/// - 读放行额外包含**父目录**（安装前缀，覆盖 bin 旁的 lib/Frameworks），
///   但父目录是 `/` 或 `$HOME` 时**不**放行——否则一条配置就把整个家目录或根读开了。
pub fn extra_dirs(configured: &[String]) -> ExtraToolDirs {
    let home = home_dir();
    let mut out = ExtraToolDirs::default();
    for raw in configured {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let p = PathBuf::from(raw);
        if !p.is_dir() {
            continue;
        }
        let real = std::fs::canonicalize(&p).unwrap_or(p);
        if !out.exec.contains(&real) {
            out.exec.push(real.clone());
        }
        if !out.read.contains(&real) {
            out.read.push(real.clone());
        }
        if let Some(parent) = real.parent() {
            let parent_is_root = parent.parent().is_none();
            let parent_is_home = home.as_deref() == Some(parent);
            if !parent_is_root && !parent_is_home && !out.read.contains(&parent.to_path_buf()) {
                out.read.push(parent.to_path_buf());
            }
        }
    }
    out
}

/// Xcode / Command Line Tools 的开发目录（SDK、libxcrun、clang/ld 都在这里）。
///
/// 不放行则**原生编译在沙箱内不可能完成**：`rustc` 链接时 `xcrun` 要
/// `dlopen(<dev>/usr/lib/libxcrun.dylib)`，链接器要读 `<dev>/…/SDKs/MacOSX.sdk`
/// （会话 2026-10-08 的硬墙：`file system sandbox blocked open()` 与
/// `Operation not permitted` 读 SDK 目录，报错里带着 `/Applications/Xcode.app/...`）。
/// 这里是**只读**放行（写仍只限会话工作区），并且只登记本机真实存在的目录。
///
/// 目录源：`xcode-select` 的选择结果（`/var/db/xcode_select_link`）→
/// `/Applications/Xcode*.app/Contents/Developer`（含 Xcode-beta）→
/// `/Library/Developer/CommandLineTools`。
fn developer_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if p.is_dir() {
            let real = std::fs::canonicalize(&p).unwrap_or(p);
            if !out.contains(&real) {
                out.push(real);
            }
        }
    };
    if let Ok(target) = std::fs::read_link("/var/db/xcode_select_link") {
        let abs = if target.is_absolute() {
            target
        } else {
            Path::new("/var/db").join(target)
        };
        push(abs, &mut out);
    }
    if let Ok(entries) = std::fs::read_dir("/Applications") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("Xcode") {
                push(e.path().join("Contents").join("Developer"), &mut out);
            }
        }
    }
    push(
        PathBuf::from("/Library/Developer/CommandLineTools"),
        &mut out,
    );
    out.sort();
    out
}

/// 动态注入的 **exec 放行目录**（macOS 会话注入 Seatbelt process-exec 白名单；
/// `disabled` 为「本地环境」页禁用的工具 bin 名，命中即不注入其目录）。
///
/// Homebrew 特例：路径含 `/Cellar/` 段时放行**整个 brew 前缀**（`/opt/homebrew`
/// 或 `/usr/local`）——`bin/` 下均为符号链接，Seatbelt `subpath` 按解析后的真实
/// 路径匹配，仅放行 bin 目录放不住；且部分工具是「启动器」模式（python3 经
/// posix_spawn 再 exec `Frameworks/…/Python.app/Contents/MacOS/Python`），
/// 真实二进制散布在 Cellar 各处。共享前缀：任一未禁用的 brew 工具都会保住前缀。
pub fn exec_allow_dirs(disabled: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in enabled_dirs(disabled) {
        let s = d.to_string_lossy().into_owned();
        let entry = match s.find("/Cellar/") {
            Some(idx) => PathBuf::from(&s[..idx]),
            None => d,
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// 动态工具链的**只读放行目录**（macOS 会话注入 Seatbelt 读白名单；
/// `disabled` 同 [`exec_allow_dirs`]，命中即不注入其目录）。
///
/// exec 放行只是让进程能启动；dyld 还需读安装树里的 dylib/资源（bin 旁边的
/// `../Frameworks`、`../lib`），否则进程在 dyld 阶段 abort——比 exec 拒绝更糟
/// （错误不含 "not permitted"，stderr 归因失效）。规则：
/// - Homebrew（路径含 `/Cellar/` 段）：整前缀放行——工具经 `opt/` 互相链接共享库
///   （icu4c/openssl…），逐目录放行必漏；
/// - 其余目录：目录本身 + 父目录（安装前缀，覆盖 bin 旁的 lib/Frameworks）；
///   父目录恰为 `$HOME` 时不放行父目录（避免整 HOME 可读）；
/// - rustup：`~/.rustup` + `~/.cargo`（registry/工具链；离线构建可用），
///   cargo/rustc 任一未禁用即注入。
///
/// 这是**唯一事实源**：Seatbelt profile 与工具层静态预检（`crate::roots`）都从这里取，
/// 避免「内核放行工具链目录、静态层却按空间外读弹卡」的口径漂移。
pub fn read_allow_dirs(disabled: &[String]) -> Vec<PathBuf> {
    let home = home_dir();
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for d in enabled_dirs(disabled) {
        let s = d.to_string_lossy().into_owned();
        if let Some(idx) = s.find("/Cellar/") {
            // Homebrew 前缀 = /Cellar 段之前（/opt/homebrew 或 /usr/local）
            push(PathBuf::from(&s[..idx]), &mut out);
            continue;
        }
        push(d.clone(), &mut out);
        if let Some(parent) = d.parent() {
            if home.as_deref() != Some(parent) {
                push(parent.to_path_buf(), &mut out);
            }
        }
    }
    if rustup_installed()
        && (!bin_disabled(disabled, "cargo") || !bin_disabled(disabled, "rustc"))
    {
        if let Some(h) = &home {
            push(h.join(".rustup"), &mut out);
            push(h.join(".cargo"), &mut out);
        }
    }
    out
}

/// 同 [`read_allow_dirs`]，但输出 Seatbelt 使用的 `dir/**` 形态。
pub fn read_allow_globs(disabled: &[String]) -> Vec<String> {
    read_allow_dirs(disabled)
        .iter()
        .map(|d| format!("{}/**", d.to_string_lossy()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_path_命中本机工具() {
        // 通用工具：make/git 几乎必在（/usr/bin）；homebrew 机另含 brew
        assert!(which_path("make").is_some() || which_path("git").is_some());
    }

    #[test]
    fn which_path_未命中不存在的工具() {
        assert!(which_path("definitely-not-a-real-tool-xyz").is_none());
    }

    #[test]
    fn 探测表覆盖声明数量() {
        // 29 项：与 localenv 设置页展示一致；增删项时此断言提醒同步
        assert_eq!(TOOLCHAIN_PROBES.len(), 29);
    }

    #[test]
    fn 解析目录不含静态放行目录() {
        let skip = statically_allowed_dirs();
        for d in resolve_tool_dirs() {
            assert!(!skip.contains(&d), "不应重复注入静态目录: {d:?}");
        }
    }

    #[test]
    fn 解析目录存在且去重() {
        let dirs = resolve_tool_dirs();
        let mut seen = std::collections::HashSet::new();
        for d in &dirs {
            assert!(d.is_dir(), "解析结果应为存在目录: {d:?}");
            assert!(seen.insert(d.clone()), "应去重: {d:?}");
        }
    }

    #[test]
    fn rustup_存在时注入真实工具链目录() {
        // 条件式：本机装了 rustup（cargo 在 ~/.cargo/bin）才断言
        if !rustup_installed() {
            return;
        }
        let dirs = resolve_tool_dirs();
        let any_tc = dirs.iter().any(|d| {
            d.to_string_lossy()
                .replace('\\', "/")
                .contains(".rustup/toolchains")
        });
        assert!(any_tc, "rustup 已安装但未注入 toolchains/*/bin: {dirs:?}");
    }

    #[test]
    fn rustup_存在时读放行含_rustup与cargo() {
        if !rustup_installed() {
            return;
        }
        let globs = read_allow_globs(&[]);
        assert!(
            globs.iter().any(|g| g.ends_with(".rustup/**")),
            "应含 ~/.rustup/**: {globs:?}"
        );
        assert!(
            globs.iter().any(|g| g.ends_with(".cargo/**")),
            "应含 ~/.cargo/**: {globs:?}"
        );
    }

    /// 额外可执行目录：只登记真实存在的目录；读放行含父目录，但父目录是 `/` 或 `$HOME` 时
    /// 不放行（否则一条配置就把整个家目录或根读开了）。
    #[test]
    fn 额外可执行目录解析() {
        // 不存在的路径不生效（不报错——配置写错只是没用）
        assert_eq!(
            extra_dirs(&["/definitely/not/here-xyz".to_string()]),
            ExtraToolDirs::default()
        );
        assert_eq!(extra_dirs(&["   ".to_string()]), ExtraToolDirs::default());

        let base = std::env::temp_dir().join(format!("astorm-extra-{}", std::process::id()));
        let dir = base.join("toolchain").join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let out = extra_dirs(&[dir.to_string_lossy().to_string()]);
        let real = std::fs::canonicalize(&dir).unwrap();
        assert!(out.exec.contains(&real), "目录本身应进 exec 放行: {out:?}");
        assert!(out.read.contains(&real), "目录本身应进只读放行: {out:?}");
        assert!(
            out.read.contains(&real.parent().unwrap().to_path_buf()),
            "父目录应只读放行（覆盖 bin 旁的 lib/Frameworks）: {out:?}"
        );
        let _ = std::fs::remove_dir_all(&base);

        // 根目录 / 家目录不被当作父目录放行
        if Path::new("/usr").is_dir() {
            let out = extra_dirs(&["/usr".to_string()]);
            assert!(!out.read.contains(&PathBuf::from("/")), "不得因父目录是 / 而放行根");
        }
        if let Some(home) = std::env::var_os("HOME") {
            let local = PathBuf::from(&home).join(".local");
            if local.is_dir() {
                let out = extra_dirs(&[local.to_string_lossy().to_string()]);
                assert!(
                    !out.read.contains(&PathBuf::from(&home)),
                    "不得因父目录是 $HOME 而放行整个家目录"
                );
            }
        }
    }

    #[test]
    fn 读放行不包含整个home目录() {
        // 父目录恰为 $HOME 时不放行父目录（避免整 HOME 可读）
        if let Some(home) = std::env::var_os("HOME") {
            let home_glob = format!("{}/**", PathBuf::from(&home).to_string_lossy());
            assert!(
                !read_allow_globs(&[]).contains(&home_glob),
                "不应放行整个 $HOME"
            );
        }
    }

    #[test]
    fn homebrew工具应整前缀只读放行() {
        // 条件式：本机有 homebrew 工具（解析到 /Cellar/）→ 应出现整前缀 glob
        let has_brew = resolve_tool_dirs()
            .iter()
            .any(|d| d.to_string_lossy().contains("/Cellar/"));
        if !has_brew {
            return;
        }
        let globs = read_allow_globs(&[]);
        assert!(
            globs.iter().any(|g| g == "/opt/homebrew/**" || g == "/usr/local/**"),
            "Homebrew 前缀应整树只读放行: {globs:?}"
        );
        let dirs = exec_allow_dirs(&[]);
        assert!(
            dirs.iter().any(|d| d == Path::new("/opt/homebrew") || d == Path::new("/usr/local")),
            "Homebrew 前缀应整树 exec 放行: {dirs:?}"
        );
    }

    #[test]
    fn 禁用工具会移除其独占目录() {
        // 找一个目录仅由单个 bin 贡献的工具 → 禁用后 exec/read 注入均不再含该目录
        let entries = resolved_entries();
        let Some(e) = entries
            .iter()
            .find(|e| entries.iter().filter(|x| x.dir == e.dir).count() == 1)
        else {
            return; // 本机无独占目录工具（少见），跳过
        };
        let disabled = vec![e.bin.clone()];
        let dirs = exec_allow_dirs(&disabled);
        assert!(
            !dirs.contains(&e.dir),
            "禁用 {} 后不应再注入 {:?}: {dirs:?}",
            e.bin,
            e.dir
        );
        let globs = read_allow_globs(&disabled);
        let dir_glob = format!("{}/**", e.dir.to_string_lossy());
        assert!(!globs.contains(&dir_glob), "读放行同步移除: {globs:?}");
    }

    #[test]
    fn 禁用部分brew工具_其余brew工具保住前缀() {
        // 条件式：本机有 brew 工具且不止一个 → 禁用一个，前缀仍在
        let brew_bins: Vec<String> = resolved_entries()
            .iter()
            .filter(|e| e.dir.to_string_lossy().contains("/Cellar/"))
            .map(|e| e.bin.clone())
            .collect();
        if brew_bins.len() < 2 {
            return;
        }
        let disabled = vec![brew_bins[0].clone()];
        let dirs = exec_allow_dirs(&disabled);
        assert!(
            dirs.iter().any(|d| d == Path::new("/opt/homebrew") || d == Path::new("/usr/local")),
            "其余 brew 工具应保住前缀: {dirs:?}"
        );
    }

    #[test]
    fn 禁用全部brew工具_前缀消失() {
        // 条件式：本机有 brew 工具 → 全部禁用后前缀不再注入
        let brew_bins: Vec<String> = resolved_entries()
            .iter()
            .filter(|e| e.dir.to_string_lossy().contains("/Cellar/"))
            .map(|e| e.bin.clone())
            .collect();
        if brew_bins.is_empty() {
            return;
        }
        let dirs = exec_allow_dirs(&brew_bins);
        assert!(
            !dirs.iter().any(|d| d == Path::new("/opt/homebrew") || d == Path::new("/usr/local")),
            "全部 brew 工具禁用后不应注入前缀: {dirs:?}"
        );
    }

    #[test]
    fn 禁用rustup双工具_生态读放行移除() {
        // 条件式：rustup 机 → cargo+rustc 全禁用 → ~/.rustup/** ~/.cargo/** 不再注入
        if !rustup_installed() {
            return;
        }
        let disabled = vec!["cargo".to_string(), "rustc".to_string()];
        let globs = read_allow_globs(&disabled);
        assert!(
            !globs.iter().any(|g| g.ends_with(".rustup/**")),
            "双工具禁用后应移除 ~/.rustup/**: {globs:?}"
        );
        assert!(
            !globs.iter().any(|g| g.ends_with(".cargo/**")),
            "双工具禁用后应移除 ~/.cargo/**: {globs:?}"
        );
    }
}
