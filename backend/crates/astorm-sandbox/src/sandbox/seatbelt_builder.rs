//! macOS Seatbelt profile 生成器（平台无关纯函数）。
//!
//! macOS 运行时强制层（`sandbox/macos.rs`）把策略编译为 Seatbelt 描述文件，
//! 通过 `/usr/lib/libsandbox.dylib` 的 `sandbox_init_with_parameters` 应用到子进程
//! （内核强制托管）。本模块在 Windows 上也可编译、可单测。
//!
//! 生成规则（顺序敏感：Seatbelt 按"首条命中"求值，黑名单规则必须排在放行之前）：
//! - `(deny default)` 默认拒绝
//! - 黑名单文件规则（读/写）先行
//! - 白名单文件规则与系统必要路径放行
//! - 网络按方向粗粒度（白名单非空→allow，否则→deny；主机级粒度由驱逐监视兜底）
//! - 执行白名单（系统目录 + 会话程序目录）
//!
//! 注意：Seatbelt 为 Apple 私有 API，具体规则集需要在真实 macOS 上验证微调。

use crate::policy::ResolvedPolicy;
use crate::roots::{macos_alias_forms, macos_system_read_roots, ReadRoot};

/// 生成 profile 所需的会话上下文（由 macOS 后端在 init 时填充）。
#[derive(Debug, Clone, Default)]
pub struct SeatbeltContext {
    /// 允许执行的程序目录（内置运行时目录、系统目录）
    pub program_dirs: Vec<String>,
    /// 工作目录（允许读）
    pub cwd: String,
    /// 会话临时目录（允许读写，逐命令删除）
    pub session_temp: String,
    /// 会话数据目录（允许读写）
    pub session_data: String,
    /// 持久 HOME（允许读写，命令结束保留；None = 未启用持久 HOME）
    pub home: Option<String>,
}

/// 目录路径 → Seatbelt (subpath …) 片段（含符号链接别名展开）。
fn dir_fragments(path: &str) -> Vec<String> {
    macos_alias_forms(path)
        .iter()
        .map(|p| format!("(subpath \"{}\")", escape(p)))
        .collect()
}

/// 把 glob 模式转成 Seatbelt 的 (literal …)/(subpath …) 片段。
///
/// 粗略策略：以目录结尾或含通配符的按前缀子路径放行（subpath），否则按字面量（literal）。
/// 每条模式同时下发符号链接别名（`/tmp`/`/var`/`/etc` → `/private/*`）。
fn path_rules(patterns: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in patterns {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        // 去掉尾部通配符成为固定前缀
        let fixed = p.trim_end_matches('*').trim_end_matches('/');
        if fixed.is_empty() {
            continue;
        }
        let is_subpath = p.ends_with('/') || p.contains('*') && !p.contains('?');
        for form in macos_alias_forms(fixed) {
            if is_subpath {
                out.push(format!("(subpath \"{}\")", escape(&form)));
            } else {
                out.push(format!("(literal \"{}\")", escape(&form)));
            }
        }
    }
    out
}

/// 生成 Seatbelt profile 文本。
pub fn build_profile(policy: &ResolvedPolicy, ctx: &SeatbeltContext) -> String {
    let mut out = String::new();
    out.push_str("(version 1)\n");
    out.push_str("(deny default)\n\n");

    // ---- 黑名单文件规则（先于放行：首条命中求值保证黑 > 白）----
    // 用 file-read-data / file-write-data（而非 file-read*/file-write*）精确拦截内容访问，
    // 否则全局 (allow file-read-metadata) 会覆盖宽泛的 file-read* 黑名单。
    for r in path_rules(&policy.file.read_black_raw) {
        out.push_str(&format!("(deny file-read-data {r})\n"));
    }
    for r in path_rules(&policy.file.write_black_raw) {
        out.push_str(&format!("(deny file-write-data {r})\n"));
    }

    // ---- 网络（方向级粗粒度；主机级由驱逐监视兜底）----
    // **loopback 始终放行，且必须排在方向级 deny 之前**（Seatbelt 首条命中生效）：
    // 本地服务（vite dev / 后端 8088 / curl healthz 自测）不构成出站风险，而
    // 「白名单开 + 名单为空」的默认策略原本会把 127.0.0.1 一起禁掉——实测 bind 与
    // connect 回环都是 EPERM，于是"编译通过却起不来、自测不了"。外网仍按下面的
    // 方向级规则 + lsof 驱逐监视处置；内核层与运行期监视两侧同源（见 decision::decide_net）。
    out.push_str("(allow network-outbound (remote ip \"localhost:*\"))\n");
    out.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
    // 白名单开关关闭（仅黑名单模式）→ 内核层不限制，黑名单交由驱逐监视拦截
    if policy.net.use_whitelist && policy.net.out_white.is_empty() {
        out.push_str("(deny network-outbound)\n");
    } else {
        out.push_str("(allow network-outbound)\n");
    }
    if policy.net.use_whitelist && policy.net.in_white.is_empty() {
        out.push_str("(deny network-inbound)\n");
    } else {
        out.push_str("(allow network-inbound)\n");
    }
    out.push('\n');

    // ---- GPU 禁用（--gpu off）：阻断 GPU 设备访问与 Metal 服务 ----
    if !policy.resource.gpu {
        out.push_str("(deny iokit-open-user-client)\n");
        out.push_str("(deny mach-lookup (global-name \"com.apple.MTLCompilerService\")\n");
        out.push_str("              (global-name \"com.apple.coreanimation.render-server\")\n");
        out.push_str("              (global-name \"com.apple.MTLRenderCommandEncoder\"))\n");
        out.push('\n');
    }

    // ---- 进程与系统通信基础能力 ----
    out.push_str("(allow process-fork)\n");
    out.push_str("(allow process-exec (subpath \"/bin\"))\n");
    out.push_str("(allow process-exec (subpath \"/sbin\"))\n");
    out.push_str("(allow process-exec (subpath \"/usr/bin\"))\n");
    out.push_str("(allow process-exec (subpath \"/usr/lib\"))\n");
    // 用户自装工具链（Homebrew / rustup cargo）：不放行则 python3/cargo 等 exec 被拒，
    // Auto 模式会走无防护二跑使沙箱失效；放行后工具进程仍受文件/网络沙箱约束
    out.push_str("(allow process-exec (subpath \"/opt/homebrew/bin\"))\n");
    out.push_str("(allow process-exec (subpath \"/usr/local/bin\"))\n");
    for d in &ctx.program_dirs {
        // 别名形态一并下发：Seatbelt 按**解析后**的真实路径匹配，而调用方给的可能是
        // `/tmp/x`、`/var/…` 这类别名（实测：工作区落在 `/var/folders` 下时，
        // 只下发别名路径 → 工作区脚本 exec 仍被拒 `bad interpreter: Operation not
        // permitted`）。真实路径与别名形态都放行，语义与读写规则的 dir_fragments 一致。
        for form in macos_alias_forms(d) {
            out.push_str(&format!(
                "(allow process-exec (subpath \"{}\"))\n",
                escape(&form)
            ));
        }
    }
    // 用户自装工具链（Homebrew / rustup cargo）：不放行则 python3/cargo 等 exec 被拒，
    // Auto 模式会走无防护二跑使沙箱失效；放行后工具进程仍受文件/网络沙箱约束
    if let Ok(home) = std::env::var("HOME") {
        for extra in [format!("{home}/.cargo/bin"), format!("{home}/.local/bin")] {
            out.push_str(&format!(
                "(allow process-exec (subpath \"{}\"))\n",
                escape(&extra)
            ));
        }
    }
    out.push_str("(allow signal)\n");
    out.push_str("(allow sysctl-read)\n");
    out.push_str("(allow mach-lookup (global-name \"com.apple.system.opendirectoryd\")\n");
    out.push_str("              (global-name \"com.apple.system.logger\")\n");
    out.push_str("              (global-name \"com.apple.system.notification_center\")\n");
    out.push_str("              (global-name \"com.apple.cfprefsd.daemon\")\n");
    // 证书信任评估（Security.framework / native-tls）：不开放 trustd 通信时，
    // 走系统信任库的客户端在 TLS 握手阶段就以 `invalid peer certificate:
    // OSStatus -26276` 失败，看起来像「网络被禁」（会话 2026-10-08：rustup 默认
    // native-tls 路径全程如此，只有 `RUSTUP_USE_CURL=1` 能绕过）。
    // 只放行**信任评估**；用户钥匙串仍不可达（SecurityServer 未放行、
    // ~/Library/Keychains 读写均被拒），因此不会暴露用户密钥。
    out.push_str("              (global-name \"com.apple.trustd.agent\")\n");
    out.push_str("              (global-name \"com.apple.trustd\"))\n");
    out.push_str("(allow file-read-metadata)\n");
    out.push('\n');

    // ---- 文件系统放行 ----
    // 系统区与运行所需路径默认可读（与 Windows「系统区默认可读」语义一致）；
    // **根清单的唯一事实源是 `crate::roots::macos_system_read_roots()`**——
    // 工具层静态预检复用同一份，避免「内核放行、静态层却弹空间外读审批卡」的口径漂移。
    // 用户自定义读白名单在此叠加；其余路径由 (deny default) 默认拒绝。
    let mut reads: Vec<String> = macos_system_read_roots()
        .iter()
        .flat_map(|root| match root {
            ReadRoot::Subtree(dir) => dir_fragments(dir.as_str()),
            ReadRoot::File(file) => vec![format!("(literal \"{}\")", escape(file.as_str()))],
        })
        .collect();
    reads.extend(dir_fragments(&ctx.cwd));
    reads.extend(dir_fragments(&ctx.session_temp));
    reads.extend(dir_fragments(&ctx.session_data));
    // 持久 HOME（按工作区）：必须读写双放行，否则 cargo/npm/uv 等连自己的
    // 缓存目录都写不进去（HOME 指向它，见 macos.rs 的 extra_env）。
    if let Some(home) = &ctx.home {
        reads.extend(dir_fragments(home));
    }
    reads.extend(path_rules(&policy.file.read_white_raw));
    out.push_str(&format!("(allow file-read* {})\n", reads.join(" ")));

    let mut writes = dir_fragments(&ctx.session_temp);
    writes.extend(dir_fragments(&ctx.session_data));
    if let Some(home) = &ctx.home {
        writes.extend(dir_fragments(home));
    }
    writes.extend(path_rules(&policy.file.write_white_raw));
    out.push_str(&format!("(allow file-write* {})\n", writes.join(" ")));
    // 零副作用输出设备：`2>/dev/null` 是 shell 惯用法，静态分析也把它从写引用里豁免
    // （见 analyzer::is_null_device）。两侧必须一致——内核若仍拒绝，命令会以一条裸
    // `Operation not permitted` 失败，且**不产生任何结构化事件**（Auto 没有信号可重跑、
    // Manual 没有依据弹卡，只能按普通失败返回），用户/模型只会以为命令写错了。
    // 只放设备节点本身：/dev/stdout|stderr 在 macOS 是 `fd/1|2` 的符号链接，写管道
    // 本来就不受 file-write* 约束（实测无需放行）。
    out.push_str(
        "(allow file-write-data (literal \"/dev/null\") (literal \"/dev/tty\"))\n",
    );
    out.push('\n');

    out
}

/// 转义 profile 字符串中的引号与反斜杠。
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{
        DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg, ProtectionConfig, ResolvedPolicy,
        SandboxInput,
    };

    fn ctx() -> SeatbeltContext {
        SeatbeltContext {
            program_dirs: vec!["/opt/astorm/runtime/python".to_string()],
            cwd: "/Users/test/work".to_string(),
            session_temp: "/private/var/folders/x/T/astorm-1".to_string(),
            session_data: "/private/var/folders/x/D/astorm-1".to_string(),
            home: None,
        }
    }

    fn policy(file: FilePolicyCfg, net: NetPolicyCfg) -> ResolvedPolicy {
        let input = SandboxInput {
            protection: ProtectionConfig {
                enabled: true,
                file,
                net,
                danger: DangerPolicyCfg::default(),
                resource: crate::policy::ResourceLimits::default(),
                ..Default::default()
            },
            ..SandboxInput::new("test")
        };
        ResolvedPolicy::compile(&input).unwrap()
    }

    #[test]
    fn 生成包含默认拒绝与方向网络规则() {
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &ctx());
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(deny network-outbound)"));
        assert!(profile.contains("(deny network-inbound)"));
        assert!(profile.contains("process-fork"));
        assert!(profile.contains("/opt/astorm/runtime/python"));
        assert!(profile.contains("/Users/test/work"));
    }

    #[test]
    fn 零副作用设备可写与静态豁免一致() {
        // analyzer::is_null_device 把 /dev/null 等从写引用里豁免（不触发审批 / 不拦 Plan），
        // 内核层必须同样放行，否则 `2>/dev/null` 会以裸 EPERM 失败且不产生任何事件。
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &ctx());
        assert!(
            profile.contains("(allow file-write-data (literal \"/dev/null\")"),
            "写放行必须包含 /dev/null: {profile}"
        );
        assert!(profile.contains("(literal \"/dev/tty\")"));
        // 不能顺手把整个 /dev 写开
        assert!(!profile.contains("(allow file-write* (subpath \"/dev\")"));
    }

    #[test]
    fn 系统配置区可读_网络工具才有可用前提() {
        // /etc（= /private/etc）与 /private/var/run/resolv.conf 必须读放行，否则即使
        // 出站已放行，curl/wget 也会在 TLS 初始化阶段以裸 EPERM 崩掉
        // （fopen('/private/etc/ssl/openssl.cnf','rb')），DNS 解析时好时坏——
        // 现场看起来就是"网络被沙箱禁了"（会话 2026_09_14_00_22_47）。
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &ctx());
        assert!(
            profile.contains("(subpath \"/private/etc\")"),
            "必须放行系统配置区（TLS 配置 / 证书 / hosts）: {profile}"
        );
        assert!(
            profile.contains("(literal \"/private/var/run/resolv.conf\")"),
            "必须放行 DNS 解析配置: {profile}"
        );
        // 只给 resolv.conf 这一个字面量：不整目录放行 /private/var/run
        // （里面还有各守护进程的 socket 与 pid 文件）
        assert!(!profile.contains("(subpath \"/private/var/run\")"));
    }

    #[test]
    fn 黑名单先行于放行() {
        let p = policy(
            FilePolicyCfg {
                read_black: vec!["/etc/**".to_string()],
                read_white: vec!["/Users/test/work/**".to_string()],
                ..Default::default()
            },
            NetPolicyCfg::default(),
        );
        let profile = build_profile(&p, &ctx());
        let deny_pos = profile
            .find("(deny file-read-data (subpath \"/etc\")")
            .expect("黑名单读规则应存在");
        let allow_pos = profile.find("(allow file-read*").expect("放行规则应存在");
        assert!(deny_pos < allow_pos, "黑名单规则必须排在放行规则之前");
        assert!(profile.contains("(subpath \"/Users/test/work\")"));
    }

    #[test]
    fn 白名单网络方向放行() {
        let p = policy(
            FilePolicyCfg::default(),
            NetPolicyCfg {
                out_white: vec!["domain:example.com".to_string()],
                ..Default::default()
            },
        );
        let profile = build_profile(&p, &ctx());
        assert!(profile.contains("(allow network-outbound)"));
        assert!(profile.contains("(deny network-inbound)"));
    }

    #[test]
    fn 白名单开关关闭_内核网络不限制() {
        // use_whitelist=false：即使白名单为空，网络方向也不 deny（黑名单交由驱逐监视）
        let p = policy(
            FilePolicyCfg::default(),
            NetPolicyCfg {
                use_whitelist: false,
                out_black: vec!["ip:1.1.1.1".to_string()],
                ..Default::default()
            },
        );
        let profile = build_profile(&p, &ctx());
        assert!(profile.contains("(allow network-outbound)"));
        assert!(profile.contains("(allow network-inbound)"));
        assert!(!profile.contains("(deny network-outbound)"));
        assert!(!profile.contains("(deny network-inbound)"));
    }

    #[test]
    fn 写白名单放行路径() {
        let p = policy(
            FilePolicyCfg {
                write_white: vec!["/Users/test/out/**".to_string()],
                ..Default::default()
            },
            NetPolicyCfg::default(),
        );
        let profile = build_profile(&p, &ctx());
        assert!(profile.contains("(subpath \"/Users/test/out\")"));
    }

    #[test]
    fn 生态缓存目录经读白名单进入profile() {
        // 动态注入(rustup 等)扩 read_white_raw → Seatbelt 读放行;仅文本层,判定 GlobSet 不变
        let p = policy(
            FilePolicyCfg {
                read_white: vec![
                    "/Users/test/.rustup/**".to_string(),
                    "/Users/test/.cargo/**".to_string(),
                ],
                ..Default::default()
            },
            NetPolicyCfg::default(),
        );
        let profile = build_profile(&p, &ctx());
        assert!(profile.contains("(subpath \"/Users/test/.rustup\")"));
        assert!(profile.contains("(subpath \"/Users/test/.cargo\")"));
    }

    #[test]
    fn 符号链接别名展开() {
        // /tmp 与 /var 指向 /private/*；Seatbelt 按真实路径匹配，两者都要下发
        let p = policy(
            FilePolicyCfg {
                write_white: vec!["/tmp/out/**".to_string()],
                read_black: vec!["/etc/secret/**".to_string()],
                ..Default::default()
            },
            NetPolicyCfg::default(),
        );
        let profile = build_profile(&p, &ctx());
        assert!(
            profile.contains("(subpath \"/tmp/out\")"),
            "应保留用户原路径"
        );
        assert!(
            profile.contains("(subpath \"/private/tmp/out\")"),
            "应下发 /private 真实路径"
        );
        assert!(
            profile.contains("(deny file-read-data (subpath \"/etc/secret\")"),
            "黑名单应保留 /etc"
        );
        assert!(
            profile.contains("(deny file-read-data (subpath \"/private/etc/secret\")"),
            "黑名单应下发 /private/etc"
        );
    }

    #[test]
    fn 会话目录别名展开() {
        // std::env::temp_dir() 返回 /var/folders/...，真实路径为 /private/var/folders/...
        let c = SeatbeltContext {
            program_dirs: vec![],
            cwd: "/Users/test/work".to_string(),
            session_temp: "/var/folders/j_/T/astorm-1".to_string(),
            session_data: "/var/folders/j_/D/astorm-1".to_string(),
            home: None,
        };
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &c);
        assert!(profile.contains("(subpath \"/var/folders/j_/T/astorm-1\")"));
        assert!(profile.contains("(subpath \"/private/var/folders/j_/T/astorm-1\")"));
    }

    /// 持久 HOME 必须读写双放行（HOME 指向它）；程序目录的别名形态同样下发
    /// （Seatbelt 按解析后路径匹配——工作区落在 /var、/tmp 下时，只发别名会漏）。
    #[test]
    fn 持久home双放行与程序目录别名展开() {
        let c = SeatbeltContext {
            program_dirs: vec!["/var/folders/j_/T/ws".to_string()],
            cwd: "/Users/test/work".to_string(),
            session_temp: "/var/folders/j_/T/astorm-1".to_string(),
            session_data: "/var/folders/j_/D/astorm-1".to_string(),
            home: Some("/var/folders/j_/H/home-proj-1234".to_string()),
        };
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &c);
        // HOME：读、写两侧都要有（含 /private 别名）
        let read_line = profile
            .lines()
            .find(|l| l.starts_with("(allow file-read*"))
            .unwrap();
        let write_line = profile
            .lines()
            .find(|l| l.starts_with("(allow file-write*"))
            .unwrap();
        for line in [read_line, write_line] {
            assert!(
                line.contains("(subpath \"/var/folders/j_/H/home-proj-1234\")"),
                "缺少 HOME 别名形态：{line}"
            );
            assert!(
                line.contains("(subpath \"/private/var/folders/j_/H/home-proj-1234\")"),
                "缺少 HOME 真实路径：{line}"
            );
        }
        // 程序目录（工作区）也要有别名与真实两种形态
        assert!(profile.contains("(allow process-exec (subpath \"/var/folders/j_/T/ws\"))"));
        assert!(profile.contains("(allow process-exec (subpath \"/private/var/folders/j_/T/ws\"))"));
    }

    /// trustd 通信必须放行：否则走系统信任库的 TLS 客户端在握手阶段就失败
    /// （会话 2026-10-08：rustup 默认 native-tls 路径 `OSStatus -26276`）。
    #[test]
    fn 放行trustd通信() {
        let c = SeatbeltContext {
            program_dirs: vec![],
            cwd: "/Users/test/work".to_string(),
            session_temp: "/tmp/astorm-1".to_string(),
            session_data: "/tmp/astorm-1/data".to_string(),
            home: None,
        };
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &c);
        assert!(profile.contains("com.apple.trustd.agent"), "{profile}");
        assert!(
            profile.contains("/Library/Preferences/com.apple.dt.Xcode.plist"),
            "Xcode 许可状态需可读，否则 cc/clang 报未同意许可（exit 69）"
        );
    }

    /// 回环放行必须排在方向级 deny **之前**（Seatbelt 首条命中生效）：
    /// 否则默认策略（白名单开 + 名单空）下 `bind` 与连回环仍被拒，
    /// "编译通过却起不来、自测不了"（会话 2026-10-08 实测 bind/connect 都是 EPERM）。
    #[test]
    fn 回环放行先于方向级拒绝() {
        // 默认策略：白名单开 + 四个名单全空 → 方向级 deny 会下发
        let p = policy(FilePolicyCfg::default(), NetPolicyCfg::default());
        let profile = build_profile(&p, &ctx());
        let loop_out = profile
            .find("(allow network-outbound (remote ip \"localhost:*\"))")
            .expect("缺回环出站放行");
        let loop_in = profile
            .find("(allow network-inbound (local ip \"localhost:*\"))")
            .expect("缺回环进站放行");
        let deny_out = profile.find("(deny network-outbound)").expect("缺出站默认拒绝");
        let deny_in = profile.find("(deny network-inbound)").expect("缺进站默认拒绝");
        assert!(loop_out < deny_out, "回环出站放行必须排在 (deny network-outbound) 之前");
        assert!(loop_in < deny_in, "回环进站放行必须排在 (deny network-inbound) 之前");
    }
}
