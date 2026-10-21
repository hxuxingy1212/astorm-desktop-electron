//! 静态预检分析。
//!
//! 在执行前分析命令字符串，提取候选的：
//! - 文件读写（重定向目标 `<`/`>`/`>>`/`2>` 等；已知读写程序参数启发式）
//! - 网络端点（URL、host:port、IP 字面量、联网程序参数启发式）
//! - 危险命令（整条命令对黑名单正则匹配）
//!
//! 说明：此为启发式分析，可能与真实行为存在偏差（漏报/误报）。
//! 确定性强的黑名单命中由预检直接拒绝；默认拒绝语义由平台强制层在运行期逐访问判定。

use crate::cmdline::split_command_line;
use crate::decision::{preflight_file, preflight_net};
use crate::policy::{AccessKind, Direction, NetAddr, ResolvedPolicy};
use crate::sandbox::Category;
use std::net::IpAddr;

/// 一条文件访问候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub kind: AccessKind,
    pub path: String,
    /// 来源说明（哪个 token/重定向）
    pub source: String,
}

/// 一条网络访问候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRef {
    pub addr: NetAddr,
    pub direction: Direction,
    pub source: String,
}

/// 静态分析结果。
#[derive(Debug, Clone, Default)]
pub struct StaticAnalysis {
    pub file_refs: Vec<FileRef>,
    pub net_refs: Vec<NetRef>,
}

/// 已知的"读文件"程序（取其后首个路径参数）。
///
/// 表内命令的**路径参数**算读引用——这是静态分析判定访问方向的唯一依据：
/// 缺失会导致命令提不出任何文件引用 → 方向判为「未知」→ Plan 模式按写硬拒
/// （`ls -la /Users/…` 曾被拒成「禁止写文件」）。因此**纯只读、且会打开路径参数**的
/// 常用命令都应在此登记（`basename` / `dirname` 只做字符串处理、不碰文件系统，故不收）。
///
/// 注意：本表按**小写**匹配（调用侧 `t.to_ascii_lowercase()`），大写条目永不命中。
const READ_CMDS: &[&str] = &[
    // 查看 / 检索
    "cat", "type", "more", "less", "ls", "stat", "file", "tree", "du", "df", "dir",
    "head", "tail", "grep", "egrep", "rg", "find", "wc", "nl", "tac", "rev", "comm",
    "diff", "cmp", "sort", "uniq", "cut", "awk", "sed", "strings", "xxd", "hexdump", "od",
    "base64", "jq", "zcat", "bzcat", "xzcat", "zgrep", "readlink", "realpath",
    // 校验和（只读）
    "md5", "md5sum", "shasum", "sha1sum", "sha256sum", "cksum",
    // 元数据 / 媒体探测（只读）
    "mdls", "mdfind", "pdfinfo", "ffprobe", "mediainfo",
    // Windows / PowerShell
    "gc", "get-content", "select-string", "get-item", "get-childitem", "where",
];

/// 已知的"写文件"程序（其后路径参数即写目标）。
const WRITE_CMDS: &[&str] = &[
    "rm", "del", "erase", "rd", "rmdir", "mkdir", "md", "touch", "tee", "dd", "truncate",
    "shred", "Set-Content", "sc", "ni", "New-Item", "out-file",
];

/// 已知的"复制"程序：路径参数中**最后一个**是写目标，其余是读来源。
///
/// 方向判错代价很大：`cp /bin/echo x` 曾被整条当成写目标，静态预检报「空间外写」，
/// Auto 模式据此**去掉沙箱重跑整条命令**（会话 2026-10-08 的审计留痕即此）。
const COPY_CMDS: &[&str] = &["cp", "copy", "install"];

/// 已知的"移动/改名"程序：来源既读又写（内容被读走、目录项被改），目标为写。
const MOVE_CMDS: &[&str] = &["mv", "move", "ren", "rename"];

/// 是否为已登记的读/写/复制/移动类命令（其后路径参数按各表语义定方向）。
fn is_file_cmd(lower: &str) -> bool {
    READ_CMDS.contains(&lower)
        || WRITE_CMDS.contains(&lower)
        || COPY_CMDS.contains(&lower)
        || MOVE_CMDS.contains(&lower)
}

/// 已知的联网程序（其后的主机类参数会被提取）。
const NET_CMDS: &[&str] = &[
    "curl", "wget", "ping", "nslookup", "dig", "ssh", "scp", "sftp", "nc", "ncat",
    "netcat", "telnet", "ftp", "git", "Invoke-WebRequest", "iwr", "Invoke-RestMethod",
    "irm", "adb", "Test-NetConnection", "tnc",
];

/// 分析整条命令。
pub fn analyze_command(command: &str) -> StaticAnalysis {
    let tokens = split_command_line(command);
    let mut out = StaticAnalysis::default();

    // 先处理重定向（最可靠的信号）
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].as_str();

        // 重定向：`>file` `>>file` `2>file` `1>file` `&>file` 等（可带空格）
        if let Some((kind, rest)) = parse_redirect(t) {
            let separated = rest.is_empty();
            let raw_target = if separated {
                // 分离式：`> file`
                tokens.get(i + 1).cloned().unwrap_or_default()
            } else {
                rest
            };
            if !raw_target.is_empty() {
                push_file_ref(&mut out, kind, &raw_target, format!("重定向 {t}"));
            }
            if separated {
                i += 1; // 跳过已被消费的目标 token（无论是否产生引用）
            }
        }
        i += 1;
    }

    // 联网程序与读写程序：按已知命令表 + 其后参数启发式
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].as_str();
        let lower = t.to_ascii_lowercase();

        if is_redirection_token(t) {
            // 已经在上面处理，跳过其后首个参数
            let rest_owned = t.trim_start_matches(|c: char| c == '>' || c == '<' || c == '&' || c.is_ascii_digit());
            let standalone = rest_owned.is_empty();
            i += 1;
            if standalone {
                i += 1; // 分离式重定向
            }
        } else if parse_stdin_redirect(t).is_some() {
            // `< file`：file 为读取目标
            if let Some(p) = tokens.get(i + 1) {
                push_file_ref(&mut out, AccessKind::Read, p, format!("重定向 {t}"));
                i += 1;
            }
        } else if is_file_cmd(&lower) {
            // 先收集路径参数（最多 4 个，够 `cp a b c dir/`），再按命令语义定方向
            let mut paths: Vec<&String> = Vec::new();
            let mut j = i + 1;
            while j < tokens.len() && paths.len() < 4 {
                let arg = &tokens[j];
                if arg == ">" || arg == ">>" || arg == "<" || arg.starts_with("2>") || arg.starts_with("1>") {
                    break;
                }
                if looks_like_flag(arg) {
                    j += 1;
                    continue;
                }
                if looks_like_path(arg) {
                    paths.push(arg);
                }
                j += 1;
            }
            let cmd = lower.as_str();
            let last = paths.len().saturating_sub(1);
            for (idx, p) in paths.iter().enumerate() {
                let source = format!("{} 的参数", t);
                if READ_CMDS.contains(&cmd) {
                    push_file_ref(&mut out, AccessKind::Read, p.as_str(), source);
                } else if MOVE_CMDS.contains(&cmd) {
                    // 来源要被读出并删除 → 两个方向都算
                    push_file_ref(&mut out, AccessKind::Read, p.as_str(), source.clone());
                    push_file_ref(&mut out, AccessKind::Write, p.as_str(), source);
                } else if COPY_CMDS.contains(&cmd) {
                    // 复制的来源只读；只有**最后一个**路径参数是写目标。
                    // （只剩一个参数的不完整命令按读处理，宁可漏一次写，不凭空造写目标）
                    if paths.len() >= 2 && idx == last {
                        push_file_ref(&mut out, AccessKind::Write, p.as_str(), source);
                    } else {
                        push_file_ref(&mut out, AccessKind::Read, p.as_str(), source);
                    }
                } else {
                    push_file_ref(&mut out, AccessKind::Write, p.as_str(), source);
                }
            }
            i = j;
            continue;
        } else if NET_CMDS.contains(&lower.as_str()) {
            let mut j = i + 1;
            let mut consumed = 0usize;
            let netcmd = t.to_string();
            while j < tokens.len() && consumed < 3 {
                let arg = &tokens[j];
                if arg == ">" || arg == ">>" || arg.starts_with("2>") || arg.starts_with("1>") {
                    break;
                }
                if looks_like_flag(arg) {
                    // 跳过常见取值 flag（-H host:port 等取一个参数）
                    if is_value_flag(arg) {
                        j += 1;
                        if let Some(v) = tokens.get(j) {
                            if let Some(a) = extract_net_addr(v) {
                                out.net_refs.push(NetRef {
                                    addr: a,
                                    direction: Direction::Outbound,
                                    source: format!("{netcmd} 参数"),
                                });
                                consumed += 1;
                            }
                        }
                    }
                    j += 1;
                    continue;
                }
                if let Some(a) = extract_net_addr(arg) {
                    out.net_refs.push(NetRef {
                        addr: a,
                        direction: if lower == "nc" || lower == "ncat" || lower == "netcat"
                            || tokens.get(j.saturating_sub(1)).map(|s| s == "-l").unwrap_or(false)
                        {
                            Direction::Inbound
                        } else {
                            Direction::Outbound
                        },
                        source: format!("{netcmd} 参数"),
                    });
                    consumed += 1;
                }
                j += 1;
            }
            i = j;
            continue;
        }
        i += 1;
    }

    // 兜底扫描：裸 URL / IP 字面量（任何位置）
    for t in &tokens {
        if let Some(a) = extract_net_addr(t) {
            if !out
                .net_refs
                .iter()
                .any(|r| r.addr == a && r.direction == Direction::Outbound)
            {
                out.net_refs.push(NetRef {
                    addr: a,
                    direction: Direction::Outbound,
                    source: "URL/IP 字面量".to_string(),
                });
            }
        }
    }

    out
}

/// 归一化后追加一条文件访问候选。
///
/// 非文件目标（句柄复制 `&1` 等）与「写向零副作用设备」的目标直接丢弃，不产生引用：
/// `2>/dev/null` 这类丢弃输出的惯用法若被当成真文件写，Plan 模式会被硬拒
/// （会话空间外目标），Execute 模式会弹多余审批。
fn push_file_ref(out: &mut StaticAnalysis, kind: AccessKind, raw_target: &str, source: String) {
    let Some(path) = normalize_target(raw_target) else {
        return;
    };
    if kind == AccessKind::Write && is_null_device(&path) {
        return;
    }
    out.file_refs.push(FileRef { kind, path, source });
}

/// 零副作用输出设备：写向这些目标不落文件（仅终端/空设备）。
fn is_null_device(path: &str) -> bool {
    matches!(path, "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty")
        // Windows 空设备（`> NUL`）
        || path.eq_ignore_ascii_case("nul")
}

/// 未加引号时会终止一个词、但分词器不处理的 shell 元字符。
const SHELL_WORD_BREAKERS: &[char] = &[';', '&', '|', ')', '(', '`', '\n'];

/// 归一化重定向/路径 token：取第一个 shell 元字符之前的片段，剥掉成对引号。
///
/// 分词器只按空白切分（`cmdline::split_command_line`），元字符会粘在路径上：
/// `2>/dev/null;`、`>/tmp/a.txt&&ls`、`cat /tmp/x.txt|less` 得到的 token 分别是
/// `/dev/null;`、`/tmp/a.txt&&ls`、`/tmp/x.txt|less`，导致：
/// - 零副作用设备豁免失效（`/dev/null;` ≠ `/dev/null`）→ 误判为会话空间外写；
/// - 安全配置白名单 glob 匹配不上（`/tmp/ok.txt;` ≠ `/tmp/ok.txt`）。
///
/// 裁剪是保守的：只丢掉元字符之后的片段，最坏情况是少识别一个引用（该引用仍由运行期
/// 平台强制层逐访问拦截），不会把空间外写误判成空间内写。
///
/// 返回 `None` 表示不是文件目标（裁剪后为空 / 句柄复制 `&1` `&2` `&-`）。
fn normalize_target(raw: &str) -> Option<String> {
    let path = raw
        .trim()
        .split(SHELL_WORD_BREAKERS)
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '"' || c == '\'')
        .trim();
    // 句柄复制 `&1` / `&2` / `&-` / `&`：目标已打开的 fd，不是文件
    if let Some(fd) = path.strip_prefix('&') {
        if fd.is_empty() || fd == "-" || fd.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// 判断 token 是否为重定向目标标记（仅用于跳过逻辑）。
fn is_redirection_token(t: &str) -> bool {
    t == ">" || t == ">>" || t == "2>" || t == "2>>" || t == "1>" || t == "1>>" || t == "&>" || t == "&>>"
}

/// 解析重定向：返回 (访问方向, 内联目标路径)。`>file` / `>>file` / `2>file` / `&>file` / `1>>file`。
fn parse_redirect(t: &str) -> Option<(AccessKind, String)> {
    let (kind, prefix) = if t.starts_with("<") {
        (AccessKind::Read, "<")
    } else if t.starts_with(">>") {
        (AccessKind::Write, ">>")
    } else if t.starts_with(">") {
        (AccessKind::Write, ">")
    } else if t.starts_with("2>>") {
        (AccessKind::Write, "2>>")
    } else if t.starts_with("2>") {
        (AccessKind::Write, "2>")
    } else if t.starts_with("1>>") {
        (AccessKind::Write, "1>>")
    } else if t.starts_with("1>") {
        (AccessKind::Write, "1>")
    } else if t.starts_with("&>>") {
        (AccessKind::Write, "&>>")
    } else if t.starts_with("&>") {
        (AccessKind::Write, "&>")
    } else {
        return None;
    };
    let rest = &t[prefix.len()..];
    // 空目标：分离式重定向（目标在下一个 token）
    if rest.is_empty() {
        return Some((kind, String::new()));
    }
    // 句柄重定向不算文件：`2>&1` / `>&2` / `2>file` 的 fd 复制形态（rest 为 `&` 或 `&<数字>`）
    if rest == "&"
        || rest.chars().all(|c| c.is_ascii_digit())
        || (rest.starts_with('&') && rest[1..].chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some((kind, rest.to_string()))
}

/// 解析 `<file` 形式的输入重定向。
fn parse_stdin_redirect(t: &str) -> Option<AccessKind> {
    if t == "<" || (t.starts_with('<') && t.len() > 1 && !t[1..].chars().all(|c| c.is_ascii_digit())) {
        Some(AccessKind::Read)
    } else {
        None
    }
}

fn looks_like_flag(arg: &str) -> bool {
    arg.starts_with('-') && arg.len() > 1
}

fn is_value_flag(arg: &str) -> bool {
    matches!(
        arg.to_ascii_lowercase().as_str(),
        "-h" | "--header" | "-H" | "--host" | "-o" | "--output" | "-r" | "--remote" | "-p" | "--port"
            | "-u" | "--url" | "connect" | "--connect-to"
    )
}

/// 粗略路径判定：含路径分隔符 / 盘符 / 相对前缀 / 常见扩展名。
fn looks_like_path(arg: &str) -> bool {
    if arg.is_empty() {
        return false;
    }
    let lower = arg.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return false;
    }
    arg.contains('/')
        || arg.contains('\\')
        || arg.starts_with("./")
        || arg.starts_with("../")
        || arg.starts_with("~/")
        || looks_like_drive_path(arg)
        || (arg.contains('.') && arg.contains(":") == false && arg.len() > 4)
}

fn looks_like_drive_path(arg: &str) -> bool {
    let b = arg.as_bytes();
    b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// 从 token 提取网络地址（URL / host:port / IP）。
fn extract_net_addr(token: &str) -> Option<NetAddr> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    // URL：scheme://host[:port]
    if let Some(idx) = t.find("://") {
        let scheme = &t[..idx];
        if scheme.len() <= 10 && scheme.chars().all(|c| c.is_ascii_alphanumeric()) {
            let rest = &t[idx + 3..];
            let (host_port, _path) = match rest.find('/') {
                Some(p) => (&rest[..p], &rest[p..]),
                None => (rest, ""),
            };
            return parse_host_port(host_port);
        }
    }
    // IP:port（用括号包裹的形式兼容 IPv6 略过）
    if let Some(a) = parse_host_port(t) {
        return Some(a);
    }
    // 裸 IPv4 / IPv6
    if let Ok(ip) = t.parse::<IpAddr>() {
        return Some(NetAddr::from_ip(ip));
    }
    None
}

/// 解析 "host[:port]"；user@host 形式取 host 部分；IPv6 字面量需带括号则直接按 IP 处理。
fn parse_host_port(s: &str) -> Option<NetAddr> {
    let s = s.trim_start_matches('[').trim_end_matches(']');
    // 裸 `host:port/path`（`curl localhost:8080/health`）：先剪掉路径，与 `scheme://` 分支同口径。
    // 不剪的话整串会被当成一个「主机名」（含字母即通过旧判定），白/黑名单 glob 永远匹配不上。
    let s = match s.find('/') {
        Some(idx) => &s[..idx],
        None => s,
    };
    if s.is_empty() {
        return None;
    }
    // 去掉 user@ 前缀
    let s = match s.rfind('@') {
        Some(idx) => &s[idx + 1..],
        None => s,
    };
    if s.starts_with('[') {
        // [::1]:443
        if let Some(end) = s.find(']') {
            let ip_str = &s[1..end];
            let port = s[end + 1..]
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok());
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                return Some(NetAddr {
                    host: None,
                    ip: Some(ip),
                    port,
                });
            }
        }
        return None;
    }
    // host:port
    let mut host_name = s;
    let mut port = None;
    if let Some(idx) = s.rfind(':') {
        let tail = &s[idx + 1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(p) = tail.parse::<u16>() {
                port = Some(p);
                host_name = &s[..idx];
            }
        }
    }
    if host_name.is_empty() {
        return None;
    }
    // 仍可能是 IP（不带端口）
    if let Ok(ip) = host_name.parse::<IpAddr>() {
        return Some(NetAddr {
            host: None,
            ip: Some(ip),
            port,
        });
    }
    if is_plausible_host(host_name) {
        Some(NetAddr {
            host: Some(host_name.to_string()),
            ip: None,
            port,
        })
    } else {
        None
    }
}

/// 是否像主机名。
///
/// **收紧到「带点/是 localhost」**：宽松版（任何含字母的词就算主机）会把 `git status` 的
/// `status`、`npm config get registry` 的 `config` / `registry` 都当成主机名 —— 白名单模式下
/// 这些**纯本地命令**会被 `preflight` 判成网络违规：Auto 关沙箱重跑（安全倒退）、
/// Manual 弹出莫名其妙的审批卡（2026_09_14_00_22_47 会话里「多次沙箱被拒」的一类来源）。
/// 真正的「命令有没有联网意图」由 `astorm-core::tool::net_intent` 按命令/子命令表判定，
/// 不依赖这里；这里的地址只用于白/黑名单比对与审批卡 targets，收紧后只少报不算多报。
fn is_plausible_host(s: &str) -> bool {
    if s.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // 点分域名：至少一个点、各段非空且只由字母数字/-/_ 组成，且整体含字母
    // （排除 "1.2.3" 这类版本号；合法 IP 在上游 parse::<IpAddr> 已返回）
    s.contains('.')
        && s.chars().any(|c| c.is_ascii_alphabetic())
        && s.split('.').all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// 一条预检拒绝：违规类别 + 人类可读明细。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PreflightDeny {
    pub category: Category,
    pub detail: String,
}

/// 对命令执行预检：危险命令 + 文件黑名单 + 网络黑名单，返回拒绝项（带类别）。
pub fn preflight(policy: &ResolvedPolicy, command: &str) -> Vec<PreflightDeny> {
    let mut denies: Vec<PreflightDeny> = Vec::new();

    // 危险命令（整条命令，大小写不敏感）
    if !policy.danger.patterns.is_empty() {
        let d = crate::decision::decide_danger(&policy.danger, command);
        if let crate::decision::Decision::Deny(r) = d {
            denies.push(PreflightDeny { category: Category::DangerCommand, detail: r });
        }
    }

    let analysis = analyze_command(command);
    for f in &analysis.file_refs {
        if let crate::decision::Decision::Deny(r) = preflight_file(&policy.file, f.kind, &f.path) {
            denies.push(PreflightDeny {
                category: Category::from_access(f.kind),
                detail: format!("{}（{}）", r, f.source),
            });
        }
    }
    for n in &analysis.net_refs {
        if let crate::decision::Decision::Deny(r) = preflight_net(&policy.net, n.direction, &n.addr) {
            denies.push(PreflightDeny {
                category: Category::from_direction(n.direction),
                detail: format!("{}（{}）", r, n.source),
            });
        }
    }
    denies
}

// ============================ 测试 ============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只读命令的路径参数算读引用() {
        // `ls -la /Users/… 2>&1`：曾被漏判——ls 不在只读命令表里，静态分析提不出任何
        // 文件引用 → 方向判为「未知」→ Plan 模式按写硬拒（「禁止写文件」），
        // 实际只是列目录。
        let a = analyze_command("ls -la /Users/x/Desktop 2>&1");
        assert!(
            a.file_refs
                .iter()
                .any(|f| f.kind == AccessKind::Read && f.path == "/Users/x/Desktop"),
            "ls 的路径参数应产生读引用: {:?}",
            a.file_refs
        );
        assert_eq!(
            a.file_refs.iter().filter(|f| f.kind == AccessKind::Write).count(),
            0,
            "只读命令不应产生写引用: {:?}",
            a.file_refs
        );
        // 多个路径参数都应被捕获
        let a = analyze_command("stat /etc/hosts /etc/passwd");
        assert!(
            a.file_refs
                .iter()
                .any(|f| f.kind == AccessKind::Read && f.path == "/etc/hosts"),
            "stat 的路径参数应产生读引用: {:?}",
            a.file_refs
        );
        // 写重定向仍然压过读（`ls` 也可能被用来写）
        let a = analyze_command("ls /etc > /tmp/x.txt");
        assert!(
            a.file_refs
                .iter()
                .any(|f| f.kind == AccessKind::Write && f.path == "/tmp/x.txt"),
            "重定向写仍应捕获: {:?}",
            a.file_refs
        );
    }

    #[test]
    fn fd复制与dev_null不产生写引用() {
        // 2>&1（fd 复制）不算文件写
        let a = analyze_command("ls -la /Users/x 2>&1 | head -20");
        assert!(
            a.file_refs.iter().all(|f| f.path != "&1"),
            "2>&1 不应产生写引用: {:?}",
            a.file_refs
        );
        // >/dev/null 丢弃输出零副作用：不应产生写引用
        let a = analyze_command("find /Users/x -name y 2>/dev/null | head");
        assert_eq!(
            a.file_refs.iter().filter(|f| f.kind == AccessKind::Write).count(),
            0,
            "2>/dev/null 不应产生写引用: {:?}",
            a.file_refs
        );
        let a = analyze_command("cargo --version >/dev/null 2>&1");
        assert_eq!(
            a.file_refs.iter().filter(|f| f.kind == AccessKind::Write).count(),
            0,
            ">/dev/null 2>&1 不应产生写引用: {:?}",
            a.file_refs
        );
        // 真文件写仍被捕获
        let a = analyze_command("echo hi > out.txt 2>/dev/null");
        assert!(
            a.file_refs.iter().any(|f| f.kind == AccessKind::Write && f.path == "out.txt"),
            "out.txt 写仍应捕获: {:?}",
            a.file_refs
        );
    }

    #[test]
    fn 粘着元字符的重定向目标被裁剪() {
        // 丢弃输出的惯用法 + 尾随 `;` / `&&` / `|`：不应产生写引用
        // （曾因 `/dev/null;` 与 `/dev/null` 不等而被误判为「会话空间外写」→ Plan 硬拒）
        for cmd in [
            "ls /Users 2>/dev/null;",
            "ls /Users 2>/dev/null&&echo ok",
            "ls /Users 2>/dev/null|head -3",
            "ls /Users >/dev/null 2>&1;",
            "ls /Users 2>/dev/stderr",
            "x 1>/dev/null;",
        ] {
            let a = analyze_command(cmd);
            assert_eq!(
                a.file_refs.iter().filter(|f| f.kind == AccessKind::Write).count(),
                0,
                "丢弃输出不应产生写引用 ({cmd}): {:?}",
                a.file_refs
            );
        }

        // 裁剪不能过头：真文件写即便粘着元字符 / 引号也必须识别
        let a = analyze_command("echo hi > out.txt; ls");
        assert!(
            a.file_refs.iter().any(|f| f.kind == AccessKind::Write && f.path == "out.txt"),
            "out.txt 写仍应捕获: {:?}",
            a.file_refs
        );
        let a = analyze_command("echo hi >/tmp/out.txt&&ls");
        assert!(
            a.file_refs.iter().any(|f| f.kind == AccessKind::Write && f.path == "/tmp/out.txt"),
            "绝对路径写应裁掉 &&: {:?}",
            a.file_refs
        );
        let a = analyze_command("echo hi > \"out 2.txt\"|cat");
        assert!(
            a.file_refs.iter().any(|f| f.kind == AccessKind::Write && f.path == "out 2.txt"),
            "带空格引号路径写应捕获: {:?}",
            a.file_refs
        );
        // 读侧同样裁剪（白名单 glob 匹配依赖路径干净）
        let a = analyze_command("cat /tmp/in.txt; echo done");
        assert!(
            a.file_refs.iter().any(|f| f.kind == AccessKind::Read && f.path == "/tmp/in.txt"),
            "读目标应裁掉尾随分号: {:?}",
            a.file_refs
        );
    }

    #[test]
    fn 重定向提取写与读() {
        let a = analyze_command("cmd /C echo hello > D:\\out.txt");
        assert!(a
            .file_refs
            .iter()
            .any(|f| f.kind == AccessKind::Write && f.path == "D:\\out.txt"));

        let a = analyze_command("cmd /C sort < in.txt");
        assert!(a.file_refs.iter().any(|f| f.kind == AccessKind::Read && f.path == "in.txt"));

        let a = analyze_command("echo hi >> log.log 2> err.log");
        assert_eq!(a.file_refs.len(), 2);
    }

    #[test]
    fn 读写程序启发式() {
        let a = analyze_command("type C:\\Windows\\win.ini");
        assert!(a.file_refs.iter().any(|f| f.kind == AccessKind::Read && f.path == "C:\\Windows\\win.ini"));

        // 复制：来源是读、目标是写（曾经两个都判成写，Auto 模式会因此误关沙箱）
        let a = analyze_command("cp src.txt dst.txt");
        assert!(a
            .file_refs
            .iter()
            .any(|f| f.kind == AccessKind::Read && f.path == "src.txt"));
        assert!(a
            .file_refs
            .iter()
            .any(|f| f.kind == AccessKind::Write && f.path == "dst.txt"));
    }

    #[test]
    fn url与ip提取() {
        let a = analyze_command("curl https://example.com/download?s=1 -o /tmp/x");
        assert!(a.net_refs.iter().any(|n| {
            n.direction == Direction::Outbound && n.addr.host.as_deref() == Some("example.com")
        }));

        let a = analyze_command("ping 8.8.8.8");
        assert!(a
            .net_refs
            .iter()
            .any(|n| n.addr.ip == Some("8.8.8.8".parse().unwrap())));
    }

    #[test]
    fn ssh主机带用户() {
        let a = analyze_command("ssh root@192.168.1.1 -p 22");
        assert!(a.net_refs.iter().any(|n| n.addr.ip == Some("192.168.1.1".parse().unwrap())));
    }

    #[test]
    fn 非主机名参数不产生网络引用() {
        // 收紧主机判定：NET_CMD 的普通单词参数（子命令 / 配置项）不得被当成主机，
        // 否则白名单模式下 `git status` 会被静态判成网络违规。
        for cmd in [
            "git status",
            "git log --oneline -20",
            "npm config get registry",
            "npm run build",
            "docker ps",
            "adb devices",
            "git commit -m \"fix 1.2.3\"",
        ] {
            let a = analyze_command(cmd);
            assert!(a.net_refs.is_empty(), "{cmd} 不应产生网络引用: {:?}", a.net_refs);
        }
        // 真正的主机仍要识别（点分域名 / localhost / host:port）
        for (cmd, host) in [
            ("curl example.com", "example.com"),
            ("curl -s localhost:8080/health", "localhost"),
            ("ssh api.example.co.uk", "api.example.co.uk"),
        ] {
            let a = analyze_command(cmd);
            assert!(
                a.net_refs.iter().any(|n| n.addr.host.as_deref() == Some(host)),
                "{cmd} 应识别出主机 {host}: {:?}",
                a.net_refs
            );
        }
    }

    #[test]
    fn 引号内路径() {
        let a = analyze_command("type \"C:\\Program Files\\App\\x.txt\"");
        assert!(a
            .file_refs
            .iter()
            .any(|f| f.path == "C:\\Program Files\\App\\x.txt"));
    }

    #[test]
    fn 危险命令可由预检捕获() {
        use crate::policy::{DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg, ProtectionConfig, ResolvedPolicy, SandboxInput};
        let input = SandboxInput::new("cmd /C del /F /S /Q C:\\Windows\\x");
        let mut p = ProtectionConfig::default();
        p.danger = DangerPolicyCfg {
            black: vec![r"del\s+(/[a-z]+\s+)+[a-z]:\\".to_string()],
            use_builtin: false,
            builtin_override: None,
        };
        // 注入自定义危险清单，避免内置清单干扰
        let mut input2 = input.clone();
        input2.protection = p;
        let rp = ResolvedPolicy::compile(&input2).unwrap();
        let denies = preflight(&rp, &input2.command);
        assert!(!denies.is_empty());

        let _ = FilePolicyCfg::default();
        let _ = NetPolicyCfg::default();
    }

    #[test]
    fn 无违规时预检通过() {
        use crate::policy::{ResolvedPolicy, SandboxInput};
        let input = SandboxInput::new("cmd /C echo hello");
        let rp = ResolvedPolicy::compile(&input).unwrap();
        let denies = preflight(&rp, &input.command);
        assert!(denies.is_empty());
    }

    #[test]
    fn 文件黑名单预检拒绝() {
        use crate::policy::{FilePolicyCfg, ProtectionConfig, ResolvedPolicy, SandboxInput};
        let mut input = SandboxInput::new("cmd /C echo x > C:\\Work\\secret.txt");
        let mut p = ProtectionConfig::default();
        p.file = FilePolicyCfg {
            write_black: vec!["C:/Work/secret.txt".to_string()],
            ..Default::default()
        };
        input.protection = p;
        let rp = ResolvedPolicy::compile(&input).unwrap();
        let denies = preflight(&rp, &input.command);
        assert!(denies.iter().any(|d| d.detail.contains("黑名单")));
        // 类别不得被错标：文件黑名单 = 文件写（而非危险命令）
        assert!(denies.iter().any(|d| d.category == Category::FileWrite));
    }

    #[test]
    fn 网络黑名单预检拒绝() {
        use crate::policy::{NetPolicyCfg, ProtectionConfig, ResolvedPolicy, SandboxInput};
        let mut input = SandboxInput::new("cmd /C ping 8.8.8.8");
        let mut p = ProtectionConfig::default();
        p.net = NetPolicyCfg {
            out_black: vec!["ip:8.8.8.8".to_string()],
            ..Default::default()
        };
        input.protection = p;
        let rp = ResolvedPolicy::compile(&input).unwrap();
        let denies = preflight(&rp, &input.command);
        assert!(denies.iter().any(|d| d.detail.contains("网络出站黑名单") || d.detail.contains("黑名单")));
        // 类别不得被错标：网络黑名单 = 网络出站（而非危险命令）
        assert!(denies.iter().any(|d| d.category == Category::NetOut));
    }

    /// `cp` 的来源是读、只有最后一个路径参数是写。
    ///
    /// 回归自 2026-10-08 会话审计：`cp /bin/echo .toolchain/echo` 曾把 `/bin/echo`
    /// 判成「空间外写」→ Auto 模式据此关掉沙箱重跑整条命令。
    #[test]
    fn 复制的来源是读_目标是写() {
        let a = analyze_command("cp /bin/echo .toolchain/echo");
        assert!(
            a.file_refs
                .iter()
                .any(|f| f.kind == AccessKind::Read && f.path == "/bin/echo"),
            "cp 的来源应为读: {:?}",
            a.file_refs
        );
        assert_eq!(
            a.file_refs
                .iter()
                .filter(|f| f.kind == AccessKind::Write)
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec![".toolchain/echo"],
            "cp 只有最后一个路径参数是写目标: {:?}",
            a.file_refs
        );
        // 多来源形态：cp -r a b dir/
        let a = analyze_command("cp -r src/a.txt src/b.txt /tmp/dst/");
        assert_eq!(
            a.file_refs
                .iter()
                .filter(|f| f.kind == AccessKind::Write)
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/tmp/dst/"],
            "cp 多来源时仅末位是写: {:?}",
            a.file_refs
        );
        // 单参数（不完整命令）不该凭空产生写引用
        let a = analyze_command("cp /bin/echo");
        assert!(
            a.file_refs.iter().all(|f| f.kind == AccessKind::Read),
            "cp 只有一个参数时按读处理: {:?}",
            a.file_refs
        );
    }

    /// `mv` 的来源与目标都要写（来源被删除），且来源含读。
    #[test]
    fn 移动的来源与目标都算写() {
        let a = analyze_command("mv /Users/x/a.txt /Users/y/b.txt");
        let writes: Vec<&str> = a
            .file_refs
            .iter()
            .filter(|f| f.kind == AccessKind::Write)
            .map(|f| f.path.as_str())
            .collect();
        assert!(writes.contains(&"/Users/x/a.txt") && writes.contains(&"/Users/y/b.txt"), "{:?}", a.file_refs);
        assert!(
            a.file_refs
                .iter()
                .any(|f| f.kind == AccessKind::Read && f.path == "/Users/x/a.txt"),
            "mv 的来源也应算读: {:?}",
            a.file_refs
        );
    }

    /// 纯写命令语义不变（Plan 只读闸门依赖它）。
    #[test]
    fn 纯写命令仍是写引用() {
        // 注：`looks_like_path` 只认「像路径」的参数（含分隔符/扩展名等），
        // 裸相对目录名（`rm -rf build` 里的 `build`）本来就不产生引用——沿用既有口径。
        let a = analyze_command("rm -rf ./build /tmp/junk");
        assert!(
            a.file_refs
                .iter()
                .filter(|f| f.kind == AccessKind::Write)
                .count()
                >= 2,
            "rm 的路径参数应为写: {:?}",
            a.file_refs
        );
        let a = analyze_command("mkdir -p /tmp/a/b");
        assert!(
            a.file_refs
                .iter()
                .any(|f| f.kind == AccessKind::Write && f.path == "/tmp/a/b"),
            "mkdir 的路径参数应为写: {:?}",
            a.file_refs
        );
    }
}