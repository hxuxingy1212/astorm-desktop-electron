//! 决策引擎。
//!
//! 语义（与需求一致）：
//! - 文件读写：黑名单命中 → 拒绝；未中黑名单但命中白名单 → 允许；都不命中 → 拒绝（默认拒绝）。
//!   优先级恒为 黑名单 > 白名单。
//! - 网络进站/出站：同上。
//! - 危险命令：仅黑名单，命中 → 拒绝，未命中 → 允许。
//!
//! 使用场景：
//! - [`preflight_*`]：静态预检（第一道防线），只对"确定性违规"（黑名单命中）拒绝。
//! - [`decide_*`]：完整决策，供运行期强制层/驱逐监视逐访问判定（黑 > 白 > 默认拒绝）。

use crate::policy::{
    AccessKind, Direction, NetAddr, ResolvedDanger, ResolvedFilePolicy, ResolvedNetPolicy,
};

/// 判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
}

impl Decision {
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny(_))
    }
    pub fn reason(&self) -> String {
        match self {
            Decision::Allow => String::new(),
            Decision::Deny(r) => r.clone(),
        }
    }
}

/// 路径归一化：Windows 反斜杠统一为正斜杠，便于 glob 匹配（与大小写不敏感配合）。
pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

// ---------------------------- 文件读写 ----------------------------

/// 完整决策：黑 > 白 > 默认拒绝。
pub fn decide_file(policy: &ResolvedFilePolicy, kind: AccessKind, path: &str) -> Decision {
    let p = normalize_path(path);
    let (black, white) = match kind {
        AccessKind::Read => (&policy.read_black, &policy.read_white),
        AccessKind::Write => (&policy.write_black, &policy.write_white),
    };
    let kind_name = match kind {
        AccessKind::Read => "读",
        AccessKind::Write => "写",
    };
    if black.is_match(&p) {
        return Decision::Deny(format!("命中文件{kind_name}黑名单"));
    }
    if white.is_match(&p) {
        return Decision::Allow;
    }
    Decision::Deny(format!("默认拒绝：未命中文件{kind_name}白名单"))
}

/// 预检（仅黑名单拦截；未命中黑名单即放行，交由运行期强制层按默认拒绝兜底）。
pub fn preflight_file(policy: &ResolvedFilePolicy, kind: AccessKind, path: &str) -> Decision {
    let p = normalize_path(path);
    let black = match kind {
        AccessKind::Read => &policy.read_black,
        AccessKind::Write => &policy.write_black,
    };
    let kind_name = match kind {
        AccessKind::Read => "读",
        AccessKind::Write => "写",
    };
    if black.is_match(&p) {
        Decision::Deny(format!("命中文件{kind_name}黑名单"))
    } else {
        Decision::Allow
    }
}

// ---------------------------- 网络 ----------------------------

/// 完整决策。
///
/// 白名单开关（`use_whitelist`）：
/// - **开启（默认）**：黑名单命中 → 拒绝；未中黑名单但命中白名单 → 允许；都不命中 → 拒绝（默认拒绝）。
/// - **关闭**：仅黑名单拦截，白名单无用，其余全部放行（默认放行）。
pub fn decide_net(policy: &ResolvedNetPolicy, dir: Direction, addr: &NetAddr) -> Decision {
    let (black, white) = policy.rules_for(dir);
    let dir_name = match dir {
        Direction::Inbound => "进站",
        Direction::Outbound => "出站",
    };
    if black.iter().any(|r| r.matches(addr)) {
        return Decision::Deny(format!("命中网络{dir_name}黑名单"));
    }
    // 回环始终放行（排在白名单默认拒绝**之前**，显式黑名单仍优先）：
    // 本地服务（dev server / 后端 / healthz 自测）不是出站风险，而
    // 「白名单开 + 名单为空」的默认策略会把 127.0.0.1 一起拒掉 —— 编译通过也
    // 起不来、连不上（实测 bind 与 connect 回环都是 EPERM）。
    if addr.is_loopback() {
        return Decision::Allow;
    }
    if !policy.use_whitelist {
        return Decision::Allow;
    }
    if white.iter().any(|r| r.matches(addr)) {
        return Decision::Allow;
    }
    Decision::Deny(format!("默认拒绝：未命中网络{dir_name}白名单"))
}

/// 预检（仅黑名单拦截）。
pub fn preflight_net(policy: &ResolvedNetPolicy, dir: Direction, addr: &NetAddr) -> Decision {
    let (black, _) = policy.rules_for(dir);
    let dir_name = match dir {
        Direction::Inbound => "进站",
        Direction::Outbound => "出站",
    };
    if black.iter().any(|r| r.matches(addr)) {
        Decision::Deny(format!("命中网络{dir_name}黑名单"))
    } else {
        Decision::Allow
    }
}

// ---------------------------- 危险命令 ----------------------------

/// 危险命令判定：仅黑名单，命中拒绝，未命中允许。
pub fn decide_danger(policy: &ResolvedDanger, cmdline: &str) -> Decision {
    let hits = policy.matches(cmdline);
    if hits.is_empty() {
        Decision::Allow
    } else {
        Decision::Deny(format!("命中危险命令黑名单：{}", hits.join("、")))
    }
}

// ============================ 测试 ============================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg};

    fn fp(read_black: &[&str], read_white: &[&str], write_black: &[&str], write_white: &[&str]) -> ResolvedFilePolicy {
        ResolvedFilePolicy::compile(&FilePolicyCfg {
            read_black: read_black.iter().map(|s| s.to_string()).collect(),
            read_white: read_white.iter().map(|s| s.to_string()).collect(),
            write_black: write_black.iter().map(|s| s.to_string()).collect(),
            write_white: write_white.iter().map(|s| s.to_string()).collect(),
        })
        .unwrap()
    }

    fn np(in_black: &[&str], in_white: &[&str], out_black: &[&str], out_white: &[&str]) -> ResolvedNetPolicy {
        np_opt(in_black, in_white, out_black, out_white, true)
    }

    fn np_opt(
        in_black: &[&str],
        in_white: &[&str],
        out_black: &[&str],
        out_white: &[&str],
        use_whitelist: bool,
    ) -> ResolvedNetPolicy {
        ResolvedNetPolicy::compile(&NetPolicyCfg {
            use_whitelist,
            in_black: in_black.iter().map(|s| s.to_string()).collect(),
            in_white: in_white.iter().map(|s| s.to_string()).collect(),
            out_black: out_black.iter().map(|s| s.to_string()).collect(),
            out_white: out_white.iter().map(|s| s.to_string()).collect(),
        })
        .unwrap()
    }

    #[test]
    fn 文件_黑名单优先于白名单() {
        let p = fp(&["D:/secret/**"], &["D:/secret/ok.txt"], &[], &[]);
        // 同一路径同时命中黑名单与白名单 → 黑名单优先拒绝
        assert!(decide_file(&p, AccessKind::Read, "D:/secret/ok.txt").is_deny());
    }

    #[test]
    fn 文件_白名单命中允许() {
        let p = fp(&[], &["C:/work/**"], &[], &["C:/work/out/**"]);
        assert_eq!(decide_file(&p, AccessKind::Read, "C:/work/a.txt"), Decision::Allow);
        assert_eq!(decide_file(&p, AccessKind::Write, "C:/work/out/x.txt"), Decision::Allow);
        // 写未在白名单（虽在白名单读目录下）→ 默认拒绝
        assert!(decide_file(&p, AccessKind::Write, "C:/work/other.txt").is_deny());
    }

    #[test]
    fn 文件_都不命中默认拒绝() {
        let p = fp(&[], &["C:/work/**"], &[], &[]);
        let d = decide_file(&p, AccessKind::Read, "C:/elsewhere/x.txt");
        assert!(d.is_deny());
        assert!(d.reason().contains("默认拒绝"));
    }

    #[test]
    fn 文件_反斜杠归一化() {
        let p = fp(&[], &["D:/work/**"], &[], &[]);
        assert_eq!(decide_file(&p, AccessKind::Read, "D:\\work\\a.txt"), Decision::Allow);
    }

    #[test]
    fn 文件_windows大小写不敏感() {
        let p = fp(&[], &["D:/Work/**"], &[], &[]);
        assert_eq!(decide_file(&p, AccessKind::Read, "d:/WORK/a.txt"), Decision::Allow);
    }

    #[test]
    fn 文件_预检仅黑名单() {
        let p = fp(&["C:/secret/**"], &["C:/secret/shown.txt"], &[], &[]);
        // 黑名单命中 → 拒绝
        assert!(preflight_file(&p, AccessKind::Read, "C:/secret/deep/a.txt").is_deny());
        // 未命中黑名单（即使不在白名单）→ 放行，交给运行期默认拒绝
        assert_eq!(preflight_file(&p, AccessKind::Read, "C:/work/a.txt"), Decision::Allow);
    }

    #[test]
    fn 网络_白名单开关关闭_仅黑名单拦截其余放行() {
        // use_whitelist=false：白名单无用；黑名单仍拦截；其余全部放行
        let p = np_opt(&["ip:8.8.8.8"], &["domain:example.com"], &["ip:1.1.1.1"], &["ip:2.2.2.2"], false);
        // 黑名单仍拦截（进/出都查各自方向）
        assert!(decide_net(&p, Direction::Inbound, &NetAddr::from_ip("8.8.8.8".parse().unwrap())).is_deny());
        assert!(decide_net(&p, Direction::Outbound, &NetAddr::from_ip("1.1.1.1".parse().unwrap())).is_deny());
        // 命中白名单 → 放行（白名单模式本来也放行）
        assert_eq!(
            decide_net(&p, Direction::Outbound, &NetAddr::from_host("www.example.com")),
            Decision::Allow
        );
        // 都不命中 → 放行（与白名单模式的关键区别）
        assert_eq!(
            decide_net(&p, Direction::Outbound, &NetAddr::from_ip("9.9.9.9".parse().unwrap())),
            Decision::Allow
        );
        assert_eq!(
            decide_net(&p, Direction::Inbound, &NetAddr::from_ip("9.9.9.9".parse().unwrap())),
            Decision::Allow
        );
        // 白名单列表完全不影响判定（off 模式下列表为空也一样放行）
        let p2 = np_opt(&[], &[], &[], &[], false);
        assert_eq!(
            decide_net(&p2, Direction::Outbound, &NetAddr::from_ip("9.9.9.9".parse().unwrap())),
            Decision::Allow
        );
    }

    #[test]
    fn 网络_主流程() {
        let p = np(&[], &[], &["ip:8.8.8.8"], &["domain:example.com"]);
        // 黑名单优先
        assert!(decide_net(&p, Direction::Outbound, &NetAddr::from_ip("8.8.8.8".parse().unwrap())).is_deny());
        // 白名单命中允许
        assert_eq!(
            decide_net(&p, Direction::Outbound, &NetAddr::from_host("www.example.com")),
            Decision::Allow
        );
        // 都不命中默认拒绝
        let d = decide_net(&p, Direction::Outbound, &NetAddr::from_ip("1.1.1.1".parse().unwrap()));
        assert!(d.is_deny());
        assert!(d.reason().contains("默认拒绝"));
    }

    #[test]
    fn 网络_方向区分() {
        let p = np(&["ip:10.0.0.1"], &["ip:10.0.0.2"], &["ip:10.0.0.2"], &["ip:10.0.0.1"]);
        // 进站黑名单优先
        assert!(decide_net(&p, Direction::Inbound, &NetAddr::from_ip("10.0.0.1".parse().unwrap())).is_deny());
        // 进站白名单命中允许
        assert_eq!(
            decide_net(&p, Direction::Inbound, &NetAddr::from_ip("10.0.0.2".parse().unwrap())),
            Decision::Allow
        );
        // 出站视角：10.0.0.2 在出站黑名单 → 拒绝
        assert!(decide_net(&p, Direction::Outbound, &NetAddr::from_ip("10.0.0.2".parse().unwrap())).is_deny());
    }

    #[test]
    fn 网络_预检仅黑名单() {
        let p = np(&[], &["domain:example.com"], &[], &[]);
        assert_eq!(
            preflight_net(&p, Direction::Outbound, &NetAddr::from_ip("1.2.3.4".parse().unwrap())),
            Decision::Allow
        );
    }

    #[test]
    fn 危险命令_命中拒绝未命中允许() {
        let d = ResolvedDanger::compile(&DangerPolicyCfg {
            black: vec![r"rm\s+-rf".to_string()],
            use_builtin: false,
            builtin_override: None,
        })
        .unwrap();
        assert!(decide_danger(&d, "rm -rf /tmp/x").is_deny());
        assert_eq!(decide_danger(&d, "ls -la"), Decision::Allow);
    }

    #[test]
    fn cidr网段命中() {
        let p = np(&["ip:10.0.0.0/8"], &[], &[], &[]);
        assert!(decide_net(&p, Direction::Outbound, &NetAddr::from_ip("10.9.9.9".parse().unwrap())).is_deny());
        assert_eq!(
            decide_net(&p, Direction::Outbound, &NetAddr::from_ip("11.0.0.1".parse().unwrap())),
            Decision::Deny("默认拒绝：未命中网络出站白名单".to_string())
        );
    }

    /// 回环始终放行（白名单开 + 名单空也不拦），外网与显式黑名单照旧。
    #[test]
    fn 回环放行而外网与黑名单照拦() {
        let empty = np(&[], &[], &[], &[]); // 白名单开 + 四个名单全空（默认策略）
        for addr in [
            NetAddr::from_ip("127.0.0.1".parse().unwrap()),
            NetAddr::from_ip("::1".parse().unwrap()),
            NetAddr::from_host("localhost"),
            NetAddr::from_host("api.localhost"),
        ] {
            for dir in [Direction::Outbound, Direction::Inbound] {
                assert_eq!(
                    decide_net(&empty, dir, &addr),
                    Decision::Allow,
                    "回环应放行: {addr:?} {dir:?}"
                );
            }
        }
        // 外网仍默认拒绝
        assert!(decide_net(&empty, Direction::Outbound, &NetAddr::from_ip("1.1.1.1".parse().unwrap())).is_deny());
        assert!(decide_net(&empty, Direction::Inbound, &NetAddr::from_host("example.com")).is_deny());
        // 显式黑名单优先于回环豁免（用户主动禁回环时照禁）
        let black = np(&[], &[], &["ip:127.0.0.1"], &[]);
        assert!(decide_net(&black, Direction::Outbound, &NetAddr::from_ip("127.0.0.1".parse().unwrap())).is_deny());
    }
}