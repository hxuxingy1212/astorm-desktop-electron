//! 会话级**内存白名单**：审批卡上点「当前会话允许」后，该目标（文件目录 / 网络主机 /
//! 拿不到主机时退化为整条命令）在**本会话内**视为已授权。
//!
//! 生效方式（与持久化的用户安全配置白名单 `path_allowed_by_security` 同一角色）：
//! - 执行前静态预检把它当放行来源 → 命中即**不再弹审批卡**；
//! - 并入沙箱策略（`policy::bash_protection`）→ 命中目录/主机的命令**仍在沙箱内**执行，
//!   其余目录与主机照旧受保护。
//!
//! 生命周期：仅进程内存（挂在 `AppState`），**重启即失效**；删除会话时清理。
//! 危险命令 / `cd` 切换 / 技能目录只读等硬规则**不受**白名单影响。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::tool::interrupt::PendingInterrupt;
use crate::tool::policy;

/// 单个会话的白名单条目。
#[derive(Debug, Default, Clone)]
pub struct SessionAllow {
    /// 允许读的目录（绝对路径；**含子目录**）。
    pub read_dirs: Vec<String>,
    /// 允许写的目录（绝对路径；含子目录）。
    pub write_dirs: Vec<String>,
    /// 允许访问的网络主机（原始 token，如 `example.com` / `example.com:443` / `1.2.3.4`）。
    pub net_hosts: Vec<String>,
    /// 主机识别失败时的退路：本会话内该条命令直接放行（无防护重跑）。
    pub net_commands: Vec<String>,
}

impl SessionAllow {
    pub fn is_empty(&self) -> bool {
        self.read_dirs.is_empty()
            && self.write_dirs.is_empty()
            && self.net_hosts.is_empty()
            && self.net_commands.is_empty()
    }
}

/// 会话白名单注册表（进程级；按 `session_id` 隔离）。
#[derive(Default)]
pub struct SessionAllowRegistry {
    inner: Mutex<HashMap<String, SessionAllow>>,
}

impl SessionAllowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一条**文件**授权：目标是目录取自身、是文件取父目录（不存在时按文件处理）。
    /// 返回是否新增（重复记录返回 false）。
    pub fn allow_path(&self, session_id: &str, path: &str, write: bool) -> bool {
        let dir = dir_of(path);
        if dir.is_empty() {
            return false;
        }
        self.update(session_id, |s| {
            let list = if write {
                &mut s.write_dirs
            } else {
                &mut s.read_dirs
            };
            if list.iter().any(|d| d == &dir) {
                false
            } else {
                list.push(dir.clone());
                true
            }
        })
    }

    /// 记录一条**网络**授权：能识别主机就按主机记，否则退化为「按命令」。
    /// 返回是否新增。
    pub fn allow_net(&self, session_id: &str, host: Option<&str>, command: &str) -> bool {
        match host.map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) => {
                let h = h.to_string();
                self.update(session_id, |s| {
                    if s.net_hosts.iter().any(|x| x.eq_ignore_ascii_case(&h)) {
                        false
                    } else {
                        s.net_hosts.push(h.clone());
                        true
                    }
                })
            }
            None => {
                let cmd = command.trim().to_string();
                if cmd.is_empty() {
                    return false;
                }
                self.update(session_id, |s| {
                    if s.net_commands.iter().any(|x| x == &cmd) {
                        false
                    } else {
                        s.net_commands.push(cmd.clone());
                        true
                    }
                })
            }
        }
    }

    /// 该路径是否已被本会话授权（`write=true` 查写白名单，否则读）。
    pub fn path_allowed(&self, session_id: &str, path: &str, write: bool) -> bool {
        let p = policy::normalize(Path::new(path));
        self.with(session_id, |s| {
            let list = if write { &s.write_dirs } else { &s.read_dirs };
            list.iter().any(|d| under(d, &p))
        })
        .unwrap_or(false)
    }

    /// 该网络主机是否已被本会话授权（存的规则不带端口时不限端口；带了端口需同端口）。
    pub fn host_allowed(&self, session_id: &str, host: &str) -> bool {
        let (cbody, cport) = split_host_port(host);
        if cbody.is_empty() {
            return false;
        }
        self.with(session_id, |s| {
            s.net_hosts.iter().any(|h| {
                let (body, port) = split_host_port(h);
                domain_matches(&body, &cbody) && (port.is_none() || port == cport)
            })
        })
        .unwrap_or(false)
    }

    /// 该命令是否命中「网络按命令放行」的退路。
    pub fn command_allowed(&self, session_id: &str, command: &str) -> bool {
        let c = command.trim();
        if c.is_empty() {
            return false;
        }
        self.with(session_id, |s| s.net_commands.iter().any(|x| x == c))
            .unwrap_or(false)
    }

    /// 会话白名单快照（沙箱策略合并用）。
    pub fn snapshot(&self, session_id: &str) -> Option<SessionAllow> {
        self.inner
            .lock()
            .ok()
            .and_then(|m| m.get(session_id).cloned())
            .filter(|s| !s.is_empty())
    }

    /// 清理某会话的白名单（删除会话时调用）。
    pub fn clear_session(&self, session_id: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(session_id);
        }
    }

    /// 锁内读改写（写回仅在非空时保留会话键）。
    fn update(&self, session_id: &str, f: impl FnOnce(&mut SessionAllow) -> bool) -> bool {
        let Ok(mut m) = self.inner.lock() else {
            return false;
        };
        let entry = m.entry(session_id.to_string()).or_default();
        f(entry)
    }

    fn with<R>(&self, session_id: &str, f: impl FnOnce(&SessionAllow) -> R) -> Option<R> {
        let m = self.inner.lock().ok()?;
        m.get(session_id).map(f)
    }
}

/// 审批卡「当前会话允许」：按待决信封的分类把目标记入会话白名单。返回新增条目数。
///
/// - `file_read` / `file_write`：每个 target 取授权目录（相对路径按会话工作区绝对化）；
/// - `network`：targets 里能识别出主机则记主机，否则退化为整条命令；
/// - 其它分类（危险命令 / 资源 / 浏览器）**不记录**（硬规则不受白名单影响）。
pub fn record_from_pending(session_id: &str, pending: &PendingInterrupt) -> usize {
    let Some(app) = crate::app::App::try_global() else {
        return 0;
    };
    let registry = app.session_allow();
    let approval = pending.request.get("approval");
    let category = approval
        .and_then(|a| a.get("category").or_else(|| a.get("kind")))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let targets: Vec<String> = approval
        .and_then(|a| a.get("targets"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let command = pending.command.clone().unwrap_or_default();

    match category {
        "file_read" | "file_write" => {
            let write = category == "file_write";
            let workspace = crate::tool::resolve_workspace(session_id);
            let mut added = 0;
            for t in &targets {
                let abs = if Path::new(t).is_absolute() {
                    t.clone()
                } else {
                    workspace.join(t).to_string_lossy().to_string()
                };
                if registry.allow_path(session_id, &abs, write) {
                    added += 1;
                }
            }
            added
        }
        "network" => {
            let host = targets.iter().find_map(|t| host_token(t));
            usize::from(registry.allow_net(session_id, host.as_deref(), &command))
        }
        _ => 0,
    }
}

/// 目标路径对应的授权目录：目录取自身，文件取父目录（不存在按文件处理）。
/// 非绝对路径 / 取不到父目录（如根）返回空串（调用方跳过）。
pub(crate) fn dir_of(path: &str) -> String {
    let p = Path::new(path.trim());
    if !p.is_absolute() {
        return String::new();
    }
    let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
    policy::normalize(dir)
}

/// 路径分量级前缀：`/a/b` 含 `/a/b/c`，不含 `/a/bc`。两侧均须为 `normalize` 结果。
fn under(dir: &str, path: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    // Windows 允许路径可能来自 `\` 形式，也可能是 `/` 形式；统一后再比较。
    let norm = |s: &str| s.replace('\\', "/");
    let (dir, path) = (norm(dir), norm(path));
    #[cfg(any(windows, target_os = "macos"))]
    let (dir, path) = (dir.to_lowercase(), path.to_lowercase());
    if path == dir {
        return true;
    }
    path.len() > dir.len()
        && path.starts_with(&dir)
        && path.as_bytes().get(dir.len()) == Some(&b'/')
}

/// 允许的目标主体是否覆盖待查主机主体：域名按**子域**匹配（与沙箱域名规则同语义），
/// IP / CIDR / IPv6 只做精确匹配。
fn domain_matches(rule_body: &str, host_body: &str) -> bool {
    if rule_body == host_body {
        return true;
    }
    let is_addr = rule_body.parse::<std::net::Ipv4Addr>().is_ok()
        || rule_body.contains('/')
        || rule_body.starts_with('[');
    if is_addr || rule_body.is_empty() {
        return false;
    }
    host_body.len() > rule_body.len()
        && host_body.ends_with(rule_body)
        && host_body.as_bytes()[host_body.len() - rule_body.len() - 1] == b'.'
}

/// 网络 token 归一为 `(主体, 端口)`：小写、去尾点、拆出末尾可选端口；IPv6 `[..]:port` 保留整体。
fn split_host_port(token: &str) -> (String, Option<String>) {
    let t = token.trim().trim_end_matches('.').to_ascii_lowercase();
    if t.starts_with('[') || t.is_empty() {
        return (t, None);
    }
    match t.rsplit_once(':') {
        Some((body, port)) if !body.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            (body.to_string(), Some(port.to_string()))
        }
        _ => (t, None),
    }
}

/// token 是否像一个主机（含 `.` 或 `:`），且能构造出合法沙箱网络规则；是则返回原 token。
pub(crate) fn host_token(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty() || !(t.contains('.') || t.contains(':')) {
        return None;
    }
    net_rule_literal(t)?;
    Some(t.to_string())
}

/// 域名主体是否**像主机名**：ASCII 字母数字 / `.` / `-` / `_`，无前后点与连续点；
/// 无点则只接受 `localhost`（避免把任意文本当域名记入沙箱白名单）。
fn looks_like_domain(body: &str) -> bool {
    if body.is_empty() || body.starts_with('[') || body.contains('/') {
        return false;
    }
    if !body
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return false;
    }
    if body.starts_with('.') || body.ends_with('.') || body.contains("..") {
        return false;
    }
    body.contains('.') || body.eq_ignore_ascii_case("localhost")
}

/// 原始主机 token → 沙箱网络规则字面量（`domain:…` / `ip:…`，含可选端口）。
/// 无法构造合法规则时返回 None（**绝不**产出非法规则：`ResolvedNetPolicy::compile`
/// 对非法规则整体报错，会波及整个防护策略；也绝不把任意文本当域名）。
pub(crate) fn net_rule_literal(token: &str) -> Option<String> {
    let t = token.trim().trim_end_matches('.');
    if t.is_empty() {
        return None;
    }
    let (body, port) = split_host_port(t);
    if body.is_empty() {
        return None;
    }
    let is_v4 = body.parse::<std::net::Ipv4Addr>().is_ok();
    let is_cidr = body
        .split_once('/')
        .map(|(a, p)| {
            a.parse::<std::net::Ipv4Addr>().is_ok()
                && p.parse::<u8>().map(|n| n <= 32).unwrap_or(false)
        })
        .unwrap_or(false);
    let is_v6 = body.starts_with('[') && body.ends_with(']');
    let kind = if is_v4 || is_cidr || is_v6 {
        "ip"
    } else if looks_like_domain(&body) {
        "domain"
    } else {
        return None;
    };
    let cand = match &port {
        Some(p) => format!("{kind}:{body}:{p}"),
        None => format!("{kind}:{body}"),
    };
    astorm_sandbox::policy::parse_net_rule(&cand).ok().map(|_| cand)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录取自身、文件取父目录；不存在按文件处理；相对路径不记。
    #[test]
    fn dir_of_takes_parent_for_files() {
        let tmp = std::env::temp_dir().join("astorm-session-allow-test");
        let sub = tmp.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(dir_of(&sub.to_string_lossy()), policy::normalize(&sub));
        assert_eq!(
            dir_of(&sub.join("a.txt").to_string_lossy()),
            policy::normalize(&sub),
            "文件取父目录"
        );
        assert_eq!(
            dir_of(&sub.join("missing.txt").to_string_lossy()),
            policy::normalize(&sub),
            "不存在按文件处理"
        );
        assert_eq!(dir_of("relative/x.txt"), "", "相对路径不记");
    }

    /// 前缀匹配按路径分量：`/a/b` 含 `/a/b/c`，不含 `/a/bc`。
    #[test]
    fn under_is_component_wise() {
        assert!(under("/a/b", "/a/b"));
        assert!(under("/a/b", "/a/b/c"));
        assert!(!under("/a/b", "/a/bc"));
        assert!(!under("/a/b", "/a"));
        assert!(!under("", "/a"));
    }

    /// 主机 token → 沙箱规则字面量；非法 token 返回 None（不产出非法规则）。
    #[test]
    fn net_rule_literal_is_sandbox_parseable() {
        assert_eq!(net_rule_literal("example.com").as_deref(), Some("domain:example.com"));
        assert_eq!(
            net_rule_literal("example.com:443").as_deref(),
            Some("domain:example.com:443")
        );
        assert_eq!(net_rule_literal("1.2.3.4").as_deref(), Some("ip:1.2.3.4"));
        assert_eq!(net_rule_literal("1.2.3.0/24").as_deref(), Some("ip:1.2.3.0/24"));
        assert!(net_rule_literal("被默认拒绝").is_none());
        assert!(host_token("被默认拒绝").is_none());
        assert!(host_token("api.example.com").is_some());
    }

    /// 域名规则含子域；IP / CIDR 只做精确匹配。
    #[test]
    fn domain_matches_is_suffix_for_domains_only() {
        assert!(domain_matches("example.com", "api.example.com"));
        assert!(!domain_matches("example.com", "notexample.com"));
        assert!(domain_matches("1.2.3.4", "1.2.3.4"));
        assert!(!domain_matches("1.2.3.4", "5.1.2.3.4"));
        assert!(!domain_matches("1.2.3.0/24", "1.2.3.1"));
    }

    /// 会话隔离 + 端口语义（不带端口不限端口；带端口需同端口）。
    #[test]
    fn host_and_path_matching() {
        let r = SessionAllowRegistry::new();
        // 用平台临时目录，避免 Windows 上 `/tmp/...` 不是绝对路径
        let base = std::env::temp_dir().join("astorm-allow-test");
        let a = base.join("a.txt").to_string_lossy().to_string();
        let b = base.join("b.txt").to_string_lossy().to_string();
        let deep = base.join("deep").join("c.txt").to_string_lossy().to_string();
        r.allow_path("s1", &a, true);
        assert!(r.path_allowed("s1", &b, true), "同目录写放行");
        assert!(r.path_allowed("s1", &deep, true), "含子目录");
        assert!(!r.path_allowed("s1", &base.to_string_lossy(), false), "只授权了写");
        assert!(!r.path_allowed("s2", &b, true), "会话隔离");

        r.allow_net("s1", Some("example.com"), "curl https://example.com");
        assert!(r.host_allowed("s1", "example.com"));
        assert!(r.host_allowed("s1", "example.com:443"), "不带端口规则不限端口");
        assert!(r.host_allowed("s1", "api.example.com"));
        assert!(!r.host_allowed("s1", "other.com"));

        r.allow_net("s1", None, "curl https://unknown-host");
        assert!(r.command_allowed("s1", "curl https://unknown-host"));
        assert!(!r.command_allowed("s1", "curl https://other"));
    }
}
