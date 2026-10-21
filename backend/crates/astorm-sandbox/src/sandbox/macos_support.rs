//! macOS 监视相关纯函数解析器（进程树 ps / 网络 lsof）。
//!
//! 不做 cfg 门控：即使在本模块于 Windows 上不运行 macOS 后端，
//! 这些解析逻辑仍可在 Windows 上编译并单测。

use crate::policy::{Direction, NetAddr};
use std::net::IpAddr;

/// ps 输出中的一行（`ps -axwo pid=,ppid=,rss=,command=`；rss 可选，缺省 0）。
#[derive(Debug, Clone, PartialEq)]
pub struct PsRow {
    pub pid: i32,
    pub ppid: i32,
    /// 常驻内存（KB；未请求 rss 列时为 0）
    pub rss: i64,
    /// 累计 CPU 时间（秒；未请求 cputime 列时为 0）
    pub cputime: f64,
    pub command: String,
}

/// 解析 `ps -axwo pid=,ppid=,rss=,cputime=,command=`（或 `pid=,ppid=,command=`）输出。
///
/// rss / cputime 列为可选：rss 为数字列；cputime 恒为 `[[HH:]MM:]SS` 冒号格式
/// （不含冒号的 token 视为 command 的一部分，避免误吞命令首词）。
pub fn parse_ps_rows(output: &str) -> Vec<PsRow> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        let mut parts = t.split_whitespace();
        let pid_s = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let pid: i32 = match pid_s.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let ppid_s = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let ppid: i32 = match ppid_s.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let rest: Vec<&str> = parts.collect();
        let mut rss = 0i64;
        let mut cputime = 0f64;
        let mut command = String::new();
        if !rest.is_empty() {
            let mut idx = 0usize;
            if let Ok(r) = rest[0].parse::<i64>() {
                // 有 rss 列：第 3 列是数字
                rss = r;
                idx = 1;
            }
            if rest.len() > idx {
                if let Some(ct) = parse_cputime(rest[idx]) {
                    cputime = ct;
                    idx += 1;
                }
            }
            command = rest[idx.min(rest.len())..].join(" ");
        }
        if pid <= 0 {
            continue;
        }
        rows.push(PsRow {
            pid,
            ppid,
            rss,
            cputime,
            command: command.to_string(),
        });
    }
    rows
}

/// 从 ps 全表中收集 root 的全部后代 pid（含自身）。
pub fn descendants_of(rows: &[PsRow], root: i32) -> Vec<i32> {
    let mut set: Vec<i32> = vec![root];
    loop {
        let mut grew = false;
        for r in rows {
            if !set.contains(&r.pid) && set.contains(&r.ppid) {
                set.push(r.pid);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    set
}

/// 计算 root 进程树的总常驻内存（字节）。不在表内的进程按 0 计。
pub fn total_rss_bytes(rows: &[PsRow], root: i32) -> u64 {
    let tree = descendants_of(rows, root);
    tree.iter()
        .filter_map(|pid| rows.iter().find(|r| r.pid == *pid).map(|r| r.rss.max(0)))
        .map(|kb| kb as u64 * 1024)
        .sum()
}

/// 计算 root 进程树的总累计 CPU 时间（秒）。不在表内的进程按 0 计。
pub fn total_cputime_secs(rows: &[PsRow], root: i32) -> f64 {
    let tree = descendants_of(rows, root);
    tree.iter()
        .filter_map(|pid| rows.iter().find(|r| r.pid == *pid))
        .map(|r| r.cputime.max(0.0))
        .sum()
}

/// 解析 ps 的 TIME 列（`[[HH:]MM:]SS`，恒含冒号；无冒号返回 None 以免误吞命令首词）。
fn parse_cputime(s: &str) -> Option<f64> {
    if !s.contains(':') {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty() || p.parse::<f64>().is_err()) {
        return None;
    }
    let nums: Vec<f64> = parts.iter().map(|p| p.parse::<f64>().unwrap()).collect();
    let n = nums.len();
    Some((0..n).map(|i| nums[i] * 60f64.powi((n - 1 - i) as i32)).sum())
}

/// lsof 网络行解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LsofNet {
    pub pid: i32,
    pub direction: Direction,
    pub addr: NetAddr,
}

/// 解析 `lsof -nP -i` 输出中的 TCP/UDP 行。
///
/// 形如：`node 1234 user 20u IPv4 0xtcp 0t0 TCP 1.2.3.4:443->5.6.7.8:5555 (ESTABLISHED)`
/// 或 `node 1234 user 21u IPv6 0xtcp 0t0 TCP *:8080 (LISTEN)`
pub fn parse_lsof_rows(output: &str) -> Vec<LsofNet> {
    let mut out = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        // COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME [(STATE)]
        if parts.len() < 9 {
            continue;
        }
        let pid: i32 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let node = parts[7];
        if node != "TCP" && node != "UDP" {
            continue;
        }
        let name = parts[8];
        let state = parts.get(9).copied().unwrap_or("");
        if name.contains("->") {
            // 出站：取 -> 右侧远端
            if let Some((_local, remote)) = name.split_once("->") {
                if let Some(addr) = parse_lsof_addr(remote) {
                    out.push(LsofNet {
                        pid,
                        direction: Direction::Outbound,
                        addr,
                    });
                }
            }
        } else if state.contains("LISTEN") {
            // 进站监听：本地地址
            if let Some(addr) = parse_lsof_addr(name) {
                out.push(LsofNet {
                    pid,
                    direction: Direction::Inbound,
                    addr,
                });
            }
        }
    }
    out
}

/// lsof 地址形如 `1.2.3.4:443`、`[::1]:80`、`*:8080`。
///
/// 通配主机（`*`）返回仅含端口的端点（用于进站监听判定；无端口则跳过）。
fn parse_lsof_addr(s: &str) -> Option<NetAddr> {
    if s.starts_with('[') {
        let end = s.find(']')?;
        let ip: IpAddr = s[1..end].parse().ok()?;
        let port = s[end + 1..].strip_prefix(':').and_then(|p| p.parse().ok());
        return Some(NetAddr {
            host: None,
            ip: Some(ip),
            port,
        });
    }
    let (host, port) = s.rsplit_once(':')?;
    let wildcard = host == "*" || host == "0.0.0.0";
    let port: Option<u16> = port.parse().ok();
    if wildcard {
        if matches!(port, Some(0) | None) {
            return None; // `*:*` 之类无意义端点
        }
        return Some(NetAddr {
            host: None,
            ip: None,
            port,
        });
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(NetAddr {
            host: None,
            ip: Some(ip),
            port,
        });
    }
    if host.contains('.') || host.contains(':') {
        return Some(NetAddr {
            host: Some(host.to_string()),
            ip: None,
            port,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps输出解析与后代收集() {
        let sample = concat!(
            "  100     1 /sbin/launchd\n",
            "  200   100 /bin/sh -c echo hi\n",
            "  300   200 ping -n 2 127.0.0.1\n",
            "  400     1 Finder.app\n",
        );
        let rows = parse_ps_rows(sample);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].ppid, 100);
        let descendants = descendants_of(&rows, 100);
        assert!(descendants.contains(&200));
        assert!(descendants.contains(&300));
        assert!(!descendants.contains(&400));
    }

    #[test]
    fn ps_rss列解析与内存统计() {
        // pid=,ppid=,rss=,command=（rss 单位为 KB）
        let sample = concat!(
            "100     1  4096 /sbin/launchd\n",
            "200   100  8192 /bin/sh -c echo hi\n",
            "300   200 12288 ping -n 2 127.0.0.1\n",
            "400     1  2048 Finder.app\n",
        );
        let rows = parse_ps_rows(sample);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].rss, 4096);
        assert_eq!(rows[2].rss, 12288);
        // 进程树 100 → 200 → 300：RSS 总和 = (4096+8192+12288) KB
        let bytes = total_rss_bytes(&rows, 100);
        assert_eq!(bytes, (4096 + 8192 + 12288) * 1024);
    }

    #[test]
    fn lsof出站与监听() {
        let sample = concat!(
            "COMMAND   PID USER   FD   TYPE  DEVICE SIZE/OFF NODE NAME\n",
            "node    1234  test   20u  IPv4  0xa1b1      0t0  TCP 127.0.0.1:5000->8.8.8.8:443 (ESTABLISHED)\n",
            "node    1234  test   21u  IPv6  0xa1b2      0t0  TCP *:8080 (LISTEN)\n",
        );
        let rows = parse_lsof_rows(sample);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].direction, Direction::Outbound);
        assert_eq!(rows[0].addr.ip, Some("8.8.8.8".parse().unwrap()));
        assert_eq!(rows[0].addr.port, Some(443));
        assert_eq!(rows[1].direction, Direction::Inbound);
    }

    #[test]
    fn lsof通配跳过() {
        // UDP *:* 无意义端点 → 跳过；TCP *:8080 监听 → 保留（仅端口）
        let sample = "node 9 user 20u IPv4 0xa 0t0 UDP *:*";
        assert!(parse_lsof_rows(sample).is_empty());
        let listen = "node 9 user 21u IPv4 0xa 0t0 TCP *:8080 (LISTEN)";
        let rows = parse_lsof_rows(listen);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].direction, Direction::Inbound);
        assert_eq!(rows[0].addr.port, Some(8080));
        assert_eq!(rows[0].addr.ip, None);
    }
}
