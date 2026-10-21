//! 静态「联网意图」判定：命令是否有联网需求。
//!
//! **只在「内核层会拦下所有出站」时才用得上**（白名单开启且名单为空 → profile 里
//! `(deny network-outbound)`，见 `seatbelt_builder`）：那种情况下连接在 connect/bind 阶段
//! 就被 EPERM，**根本建不起来**，于是 lsof 驱逐监视看不到任何连接、产生不了结构化网络事件，
//! `has_network_violation` 恒 false —— Auto 没有信号可重跑、Manual 没有依据弹卡，联网命令
//! 只能以一条裸 `Operation not permitted` 失败（2026_09_14_00_22_47 会话里 curl/git/brew/pip
//! 全部如此，模型据此得出「沙箱 shell 无外网」并用掉大量轮次）。这里用静态意图补位。
//!
//! **宁可漏判，不可误判**：命中 = Auto 关沙箱重跑 / Manual 弹审批卡，代价不对称。
//! 未命中只是退回「按普通失败返回」（与今天一致）。因此只认三类高置信信号：
//!   ① 网络客户端程序本身（`curl` / `ssh` / `uvx`…，自省参数如 `--version` 除外）
//!   ② `程序 联网子命令`（`git clone` / `npm install` / `pip install`…，子命令表只列需要网络的）
//!   ③ 关注程序 + 明确的网络目标（URL / IP / host:port，取自 `analyzer` 的 net_refs）
//!
//! 已知残余（不误判的代价）：解释器内联的下载（`python3 script.py` 里请求网络）、
//! 冷缓存下的依赖解析型构建（`cargo build` 拉 crate）、`pacman -Syu` 这类纯旗标式包管理
//! —— 这些静态判不出来，仍按普通失败返回；要覆盖它们需把目标写进安全白名单
//! （`security.net.outWhite`）或关掉网络白名单（黑名单模式）。

use astorm_sandbox::analyzer::analyze_command;
use astorm_sandbox::cmdline::split_command_line;
use astorm_sandbox::policy::NetAddr;

/// 网络客户端程序：命令本身就意味着网络访问（少数自省参数除外）。
const NET_CLIENT_CMDS: &[&str] = &[
    "curl", "wget", "ping", "ping6", "traceroute", "nslookup", "dig", "host", "ssh", "scp",
    "sftp", "rsync", "nc", "ncat", "netcat", "telnet", "ftp", "uvx", "npx", "gh", "kubectl",
];

/// 包管理 / 工具链 / 云 CLI：既有本地用法也有联网用法 → 必须按子命令判定。
const PKG_CMDS: &[&str] = &[
    "git", "npm", "pnpm", "yarn", "bun", "pip", "pip3", "uv", "cargo", "go", "gem", "brew",
    "rustup", "docker", "podman", "helm", "apt", "apt-get", "yum", "dnf", "composer", "bundle",
    "mix", "deno",
];

/// 联网子命令表：**只列需要网络的**。本地子命令一律不列（`git status`、`cargo build`、
/// `pip list`、`npm run` 都不在此），漏判只是退回普通失败，误判会关掉沙箱。
const NET_SUBCMDS: &[(&str, &[&str])] = &[
    ("git", &["clone", "fetch", "pull", "push", "ls-remote", "submodule", "lfs"]),
    (
        "npm",
        &["install", "i", "ci", "add", "update", "upgrade", "create", "publish", "view", "info",
          "search", "audit", "outdated", "exec", "x", "deprecate", "owner", "dist-tag"],
    ),
    (
        "pnpm",
        &["install", "i", "add", "update", "up", "create", "dlx", "publish", "view", "info",
          "audit", "outdated", "fetch", "import"],
    ),
    (
        "yarn",
        &["install", "add", "upgrade", "create", "dlx", "publish", "info", "audit", "outdated",
          "npm"],
    ),
    ("bun", &["install", "add", "update", "create", "publish", "x", "link"]),
    ("pip", &["install", "download", "search", "index"]),
    ("pip3", &["install", "download", "search", "index"]),
    ("uv", &["pip", "add", "sync", "lock", "tool", "publish", "self"]),
    (
        "cargo",
        &["install", "add", "update", "search", "publish", "login", "logout", "owner", "yank",
          "fetch"],
    ),
    ("go", &["get", "install", "mod"]),
    ("gem", &["install", "update", "fetch", "search"]),
    ("brew", &["install", "update", "upgrade", "tap", "untap", "reinstall", "fetch", "bundle"]),
    ("rustup", &["install", "update", "toolchain", "component", "target", "self"]),
    ("docker", &["pull", "push", "build", "search", "login", "logout", "compose"]),
    ("podman", &["pull", "push", "build", "search", "login", "logout"]),
    ("helm", &["install", "upgrade", "repo", "pull", "search", "dependency"]),
    ("apt", &["install", "update", "upgrade", "dist-upgrade", "download", "source"]),
    ("apt-get", &["install", "update", "upgrade", "dist-upgrade", "download", "source"]),
    ("yum", &["install", "update", "upgrade", "search", "download", "check-update"]),
    ("dnf", &["install", "update", "upgrade", "search", "download", "makecache", "check-update"]),
    ("composer", &["install", "update", "require", "create-project", "search"]),
    ("bundle", &["install", "update", "add"]),
    ("mix", &["deps.get", "deps.update", "hex"]),
    ("deno", &["install", "cache", "add"]),
];

/// 自省参数：程序后只跟这些参数时不联网（`curl --version` / `git --help`）。
const INTROSPECT_ARGS: &[&str] = &["--version", "-v", "--help", "-h", "--usage", "--build-info"];

/// 脚本宿主（解释器 / shell）：命令里**同时**出现明确网络目标时才算联网意图。
/// 覆盖 `python3 -c "…requests.get('https://…')"` 这类内联脚本；单独出现 URL 不算。
const SCRIPT_HOSTS: &[&str] = &[
    "python", "python3", "node", "deno", "ruby", "perl", "php", "sh", "bash", "zsh", "env",
];

/// 判定命令的联网意图；命中返回人类可读证据（进审计 / 审批卡明细），未命中返回 `None`。
pub fn network_intent(command: &str) -> Option<String> {
    let tokens = split_command_line(command);
    if tokens.is_empty() {
        return None;
    }
    let programs: Vec<&str> = tokens.iter().map(|t| basename(t)).collect();
    let addresses = network_targets(command);

    // ⓪ **只有回环目标**（`curl http://127.0.0.1:8088/healthz`、`npm install --registry
    //    http://127.0.0.1:4873`）不算联网意图：沙箱对回环始终放行（profile 与驱逐监视
    //    同源，见 `NetAddr::is_loopback`），弹卡只会让用户白点一次。
    //    注意只在"有目标且全部是回环"时豁免——`curl localhost:1 && curl https://x` 仍算意图。
    if !addresses.is_empty() && addresses.iter().all(|a| target_is_loopback(a)) {
        return None;
    }

    // ① 网络客户端：自身即联网
    for (i, prog) in programs.iter().enumerate() {
        if NET_CLIENT_CMDS.contains(prog) && !is_introspection_only(&tokens[i + 1..]) {
            return Some(format!("{prog}（网络客户端）"));
        }
    }

    // ② `程序 联网子命令`（子命令取程序后前几个裸词——`git -C /path clone` 这类带参形式也覆盖）
    for (i, prog) in programs.iter().enumerate() {
        let Some((_, subs)) = NET_SUBCMDS.iter().find(|(p, _)| p == prog) else {
            continue;
        };
        if let Some(sub) = match_subcommand(&tokens[i + 1..], subs) {
            return Some(match addresses.first() {
                Some(addr) => format!("{prog} {sub} + {addr}（联网子命令 + 网络目标）"),
                None => format!("{prog} {sub}（联网子命令）"),
            });
        }
    }

    // ③ 关注程序 + 明确网络目标：`python3 -c "…requests.get('https://…')"`
    //    单独出现 URL 不算（`echo "https://…" > doc.md` 是常见误判源）
    if let Some(addr) = addresses.first() {
        if let Some(prog) = programs
            .iter()
            .find(|p| PKG_CMDS.contains(p) || SCRIPT_HOSTS.contains(p))
        {
            return Some(format!("{prog} + {addr}（程序 + 网络目标）"));
        }
    }
    None
}

/// 审批卡 targets 里的单条目标是否为回环（格式见 [`fmt_addr`] / [`url_literals`]：
/// 可能是 `host:port`、`ip:port`、`http://host` 或裸主机名）。
fn target_is_loopback(target: &str) -> bool {
    let host = target
        .split("://")
        .last()
        .unwrap_or(target)
        .trim_end_matches('/');
    // 去掉端口（IPv6 形如 `[::1]:8080`）
    let host = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    host.trim_end_matches('.')
        .eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// 命令里可识别的网络目标（URL / IP / host:port），用于审批卡 targets 与审计明细。
///
/// 两个来源：① 分析器按 token 提取的地址（主机判定已收紧，普通单词参数不再算主机）；
/// ② 原始文本里的 URL 字面量——分词器保留引号，`python3 -c "…'https://x'…"` 这种
/// 内联脚本里的 URL 不是独立 token，提不出来。
pub fn network_targets(command: &str) -> Vec<String> {
    let mut out: Vec<String> = url_literals(command);
    for r in analyze_command(command).net_refs {
        let text = fmt_addr(&r.addr);
        if !text.is_empty() && !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// 原始文本里的 URL 字面量（`scheme://host[:port]`，忽略路径与查询串）。
fn url_literals(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = command[from..].find("://") {
        let idx = from + rel;
        // 向前找 scheme 起始（scheme = 字母开头的 字母数字/+/-/. 串）
        let start = command[..idx]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let scheme = &command[start..idx];
        let scheme_ok = !scheme.is_empty()
            && scheme.len() <= 10
            && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if !scheme_ok {
            from = idx + 3;
            continue;
        }
        // 向后取 host[:port]，遇到空白 / 引号 / shell 元字符即止
        let rest = &command[idx + 3..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | '(' | ';' | '&' | '|' | '>' | '<')
            })
            .unwrap_or(rest.len());
        let host_port = rest[..end].split('/').next().unwrap_or("").trim();
        if !host_port.is_empty() {
            let text = host_port.to_string();
            if !out.contains(&text) {
                out.push(text);
            }
        }
        from = idx + 3 + end;
    }
    out
}

/// 子命令匹配：程序后前 4 个「裸词」（跳过旗标与带引号的整段文本）中命中联网子命令即算。
///
/// 只取前几个词并排除带引号的 token（分词器保留引号），避免把 `git commit -m "…fetch…"`
/// 这类参数文本误判成子命令；`git -C /path clone` 这种带前面的选项也能覆盖到。
fn match_subcommand(rest: &[String], subs: &[&str]) -> Option<String> {
    let mut seen = 0usize;
    for tok in rest {
        if tok.starts_with('-') || tok.contains('"') || tok.contains('\'') {
            continue;
        }
        seen += 1;
        let word = tok.to_ascii_lowercase();
        if subs.contains(&word.as_str()) {
            return Some(word);
        }
        if seen >= 4 {
            return None;
        }
    }
    None
}

/// 程序后是否只有自省参数（无参数同样按自省处理：`curl` 空跑只打印用法）。
fn is_introspection_only(rest: &[String]) -> bool {
    rest.is_empty()
        || rest
            .iter()
            .all(|t| INTROSPECT_ARGS.contains(&t.trim().to_ascii_lowercase().as_str()))
}

/// 取 token 的程序名（剥引号、去目录前缀、去 Windows `.exe`）。
fn basename(tok: &str) -> &str {
    let t = tok.trim().trim_matches(|c| c == '"' || c == '\'');
    let base = t.rsplit(['/', '\\']).next().unwrap_or(t);
    base.strip_suffix(".exe").unwrap_or(base)
}

/// 供审计/审批明细复用：把 `NetAddr` 渲染成 `host:port` 文本（仅测试与内部格式化用）。
#[allow(dead_code)]
fn fmt_addr(a: &NetAddr) -> String {
    let body = a
        .host
        .clone()
        .or_else(|| a.ip.map(|ip| ip.to_string()))
        .unwrap_or_default();
    match a.port {
        Some(p) => format!("{body}:{p}"),
        None => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(cmd: &str) -> String {
        network_intent(cmd).unwrap_or_else(|| panic!("应判定为联网意图: {cmd}"))
    }
    fn miss(cmd: &str) {
        assert!(network_intent(cmd).is_none(), "不应判定为联网意图: {cmd}");
    }

    #[test]
    fn 网络客户端命中() {
        assert!(hit("curl -fsSL https://example.com/a.txt").contains("网络客户端"));
        assert!(hit("wget -O f.tar.gz https://example.com/f.tar.gz").contains("网络客户端"));
        assert!(hit("git clone git@github.com:ahujasid/blender-mcp.git").contains("网络客户端") == false);
        assert!(hit("ssh user@host").contains("网络客户端"));
        assert!(hit("uvx blender-mcp").contains("网络客户端"));
        assert!(hit("nc -z example.com 443").contains("网络客户端"));
    }

    #[test]
    fn 自省与空跑不算联网() {
        miss("curl --version");
        miss("curl --help");
        miss("wget -V");
        miss("curl");
        miss("ssh --version");
    }

    #[test]
    fn 联网子命令命中而本地子命令不命中() {
        assert!(hit("git clone https://github.com/x/y.git").contains("clone"));
        assert!(hit("git push origin main").contains("push"));
        assert!(hit("git -C /Users/x/proj fetch --all").contains("fetch"));
        assert!(hit("npm install").contains("install"));
        assert!(hit("pnpm install --frozen-lockfile").contains("install"));
        assert!(hit("pip3 install uv").contains("install"));
        assert!(hit("uv pip install blender-mcp").contains("pip"));
        assert!(hit("python3 -m pip install uv").contains("install"));
        assert!(hit("brew install --cask blender").contains("install"));
        assert!(hit("rustup toolchain install stable").contains("toolchain"));
        assert!(hit("cargo install cargo-watch").contains("install"));
        assert!(hit("docker pull ubuntu:24.04").contains("pull"));
        assert!(hit("helm repo update").contains("repo"));
        // kubectl 除自省外都是在访问集群 API
        assert!(hit("kubectl get pods").contains("网络客户端"));
        assert!(hit("kubectl apply -f deploy.yaml").contains("网络客户端"));

        // 本地子命令：必须不命中（误判会让 Auto 关掉沙箱）
        miss("git status");
        miss("git log --oneline -20");
        miss("git diff HEAD");
        miss("git add -A && git commit -m x");
        miss("git remote -v");
        miss("npm run build");
        miss("npm test");
        miss("pnpm ls");
        miss("cargo build --release");
        miss("pip list");
        miss("uv --version");
        miss("docker ps");
        miss("helm template ./chart");
        miss("brew --version");
        miss("kubectl --help");
    }

    #[test]
    fn 引用文本里的子命令不算数() {
        // 参数文本（带引号保留为单个 token）不得被当成子命令
        miss("git commit -m \"fix npm install flow\"");
        miss("echo \"git pull 之后要重启\"");
    }

    #[test]
    fn 程序加明确网络目标才算联网() {
        assert!(hit("python3 -c \"import requests; requests.get('https://example.com')\"").contains("网络目标"));
        assert!(hit("node -e \"fetch('https://api.example.com/data')\"").contains("网络目标"));
        // 仅出现 URL、没有联网程序参与 → 不算（写文档里带链接是常见形态）
        miss("echo \"https://example.com\" > notes.md");
        miss("cat readme.md");
    }

    /// 只有回环目标不算联网意图：沙箱对回环始终放行，弹卡只会白点一次。
    /// （混合目标仍算——只要有一个外部地址就需要审批/放行。）
    #[test]
    fn 仅回环目标不算联网意图() {
        miss("curl -sS http://127.0.0.1:8088/healthz");
        miss("curl http://localhost:5173/");
        miss("curl http://[::1]:8080/x");
        miss("npm install --registry http://127.0.0.1:4873");
        miss("nc -z 127.0.0.1 8080");
        miss("python3 -c \"import urllib.request; urllib.request.urlopen('http://localhost:9000')\"");
        // 混合：回环 + 外部 → 仍是联网意图（不能因为是回环就整体豁免）
        assert!(network_intent("curl http://127.0.0.1:1/x && curl https://api.example.com/y").is_some());
    }

    #[test]
    fn 目标提取() {
        let t = network_targets("curl https://example.com:8443/a?b=1");
        assert!(t.iter().any(|x| x.contains("example.com")), "{t:?}");
        assert!(network_targets("git status").is_empty());
    }
}
