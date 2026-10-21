//! 输入模型与防护配置。
//!
//! 对应需求输入：执行时间（超时）、执行命令、命令路径（解释器）、操作类型（同步/异步）、
//! 防护配置（黑白名单）、防护开关（开启正常检测，关闭直接执行不检测）。

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::time::Duration;

/// 操作类型：同步（即时结果）或异步（事件日志流）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecMode {
    /// 同步：阻塞至命令结束，返回完整即时结果
    Sync,
    /// 异步：立即返回事件日志流的消费者，结果在流末尾汇总
    Async,
}

/// 子进程输出解码方式（中文 Windows 的 cmd 管道输出是 OEM/GBK，需自适应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputEncoding {
    /// 自动：优先 UTF-8 严格解码，失败则按系统 OEM 代码页（默认）
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// 强制 UTF-8
    #[serde(rename = "utf8")]
    Utf8,
    /// 强制系统 OEM 代码页（Windows）/ lossy（unix）
    #[serde(rename = "oem")]
    Oem,
}

/// 解释器（命令路径）规格。
#[derive(Debug, Clone)]
pub struct ShellSpec {
    /// 解释器程序路径；为 None 时使用平台默认（Windows: `%COMSPEC%`，macOS: `/bin/sh`）
    pub program: Option<PathBuf>,
    /// true = 直接把命令字符串拆分为参数执行该程序（不走解释器）
    pub direct: bool,
    /// 命令文本原样传递（不自动加引号）；None = 自动判断
    /// （Windows 下解释器为 cmd.exe 且非 direct 时自动启用，规避 cmd 引号剥离规则）
    pub raw_command_arg: Option<bool>,
}

impl Default for ShellSpec {
    fn default() -> Self {
        ShellSpec {
            program: None,
            direct: false,
            raw_command_arg: None,
        }
    }
}

/// 一次沙箱执行的全部输入。
#[derive(Debug, Clone)]
pub struct SandboxInput {
    /// 执行命令（传给解释器的字符串；direct 模式下为其参数文本）
    pub command: String,
    /// 命令路径 / 解释器规格
    pub shell: ShellSpec,
    /// 执行时间（超时上限，超时后强制终止进程树）
    pub timeout: Duration,
    /// 操作类型：同步 / 异步
    pub mode: ExecMode,
    /// 防护配置与开关
    pub protection: ProtectionConfig,
    /// 严格模式：内核强制能力不足时直接失败，而不是降级
    pub strict: bool,
    /// 子进程输出解码方式（默认 auto）
    pub output_encoding: OutputEncoding,
    /// 工作目录（None = 当前进程工作目录）
    pub cwd: Option<PathBuf>,
    /// 会话工作区根（None = 未知）。用途：
    /// - 工作区内 exec 放行（`protection.allow_workspace_exec`）；
    /// - 诊断/事件里说清「边界在哪」。
    pub workspace: Option<PathBuf>,
    /// **持久 HOME**（None = 退化为逐命令临时 HOME）。
    ///
    /// 调用方（工具层）给出一个按工作区稳定的目录，沙箱把它当 `HOME` 注入并放行读写，
    /// 且**命令结束不删除**——工具链与包管理器的缓存/用户级配置因此跨命令复用。
    /// `TMPDIR`/`TEMP` 仍然是逐命令临时目录（用完即删），避免临时文件长期堆积。
    pub session_home: Option<PathBuf>,
    /// 取消信号：置位后 wait 返回 [`WaitOutcome::Cancelled`]，调用方 kill_tree 收尾。
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl SandboxInput {
    /// 便捷构造：默认 shell（平台默认解释器）、默认超时 60s、同步、防护开启。
    pub fn new(command: impl Into<String>) -> Self {
        SandboxInput {
            command: command.into(),
            shell: ShellSpec::default(),
            timeout: Duration::from_secs(60),
            mode: ExecMode::Sync,
            protection: ProtectionConfig::default(),
            strict: false,
            output_encoding: OutputEncoding::Auto,
            cwd: None,
            workspace: None,
            session_home: None,
            cancel: None,
        }
    }
}

/// 资源限制配置（内存 / 磁盘 / CPU / GPU）。所有字段缺省不限。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// 内存上限（MB；macOS=RLIMIT_AS 地址空间，Windows=Job 对象内存限制）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    /// 磁盘写入上限（MB；macOS=RLIMIT_FSIZE 单文件写入上限，Windows 暂不强制）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mb: Option<u64>,
    /// CPU 时间上限（秒；macOS=RLIMIT_CPU，Windows=Job 对象 CPU 时间限制）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<u64>,
    /// CPU 使用率上限（**整机占比**百分比 1-100；macOS=采样节流 STOP/CONT（近似 ±10%），
    /// Windows=Job Object CPU Rate Control 硬上限；None = 不限）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<u8>,
    /// GPU 访问：true = 允许（缺省）；false = 禁止（macOS 通过 Seatbelt 阻断 iokit/mach GPU 服务）
    #[serde(default = "default_true")]
    pub gpu: bool,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        ResourceLimits {
            cpu_percent: None,
            memory_mb: None,
            disk_mb: None,
            cpu_seconds: None,
            gpu: true,
        }
    }
}

impl ResourceLimits {
    /// 是否设置了任一限制（用于能力上报/提示）。
    pub fn any(&self) -> bool {
        self.memory_mb.is_some()
            || self.disk_mb.is_some()
            || self.cpu_seconds.is_some()
            || self.cpu_percent.is_some()
            || !self.gpu
    }
}

/// 防护配置（含开关）。默认：防护开启 + 内置危险命令清单生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectionConfig {
    /// 防护开关：true = 正常检测；false = 直接执行不检测
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 文件读写黑白名单
    #[serde(default)]
    pub file: FilePolicyCfg,
    /// 网络进站出站黑白名单
    #[serde(default)]
    pub net: NetPolicyCfg,
    /// 危险命令黑名单
    #[serde(default)]
    pub danger: DangerPolicyCfg,
    /// 资源限制（内存/磁盘/CPU/GPU）
    #[serde(default)]
    pub resource: ResourceLimits,
    /// 已禁用沙箱自动注入的工具 bin 名（「本地环境」页开关维护；
    /// 会话层注入 exec/读放行时据此过滤。判定集合不受影响）
    #[serde(default)]
    pub toolchain_disabled: Vec<String>,
    /// 是否放行**工作区内**的可执行文件（默认 true）。
    ///
    /// 关掉的话 `.venv/bin/python`、`node_modules/.bin/*`、`cargo build` 的产物、
    /// 自写脚本都必须靠已放行的解释器显式运行（`sh x.sh`）。默认放行的理由：
    /// 解释器（`/bin/sh`、`node`、`python3`）本就放行，`node -e` 已是任意代码执行，
    /// 因此「禁止执行工作区文件」不构成安全边界，只挡住正常开发流程
    /// （会话 2026-10-08：`.venv/bin/python`、`uv sync`、npm postinstall 全被它挡住）。
    #[serde(default = "default_true")]
    pub allow_workspace_exec: bool,
    /// **额外可执行目录**（安全配置 → 「额外可执行目录」，用户显式放行）：
    /// 非常规安装位置的工具链/编译器目录，exec 与只读一并放行。
    /// 与「本地环境」页的 `toolchain_disabled` 相反——那是**减法**（不注入自动探测到的），
    /// 这是**加法**（注入用户指定的）。详见 `toolchain::extra_dirs`。
    #[serde(default)]
    pub exec_white_dirs: Vec<String>,
}

impl Default for ProtectionConfig {
    fn default() -> Self {
        ProtectionConfig {
            enabled: true,
            file: FilePolicyCfg::default(),
            net: NetPolicyCfg::default(),
            danger: DangerPolicyCfg::default(),
            resource: ResourceLimits::default(),
            toolchain_disabled: Vec::new(),
            allow_workspace_exec: true,
            exec_white_dirs: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// 文件读写策略（原始配置，glob 模式，大小写不敏感）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilePolicyCfg {
    #[serde(default)]
    pub read_black: Vec<String>,
    #[serde(default)]
    pub read_white: Vec<String>,
    #[serde(default)]
    pub write_black: Vec<String>,
    #[serde(default)]
    pub write_white: Vec<String>,
}

/// 网络策略（原始配置）。方向区分进站（in）与出站（out）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetPolicyCfg {
    /// 白名单开关：true（默认）= 白名单之外一律拒绝（默认拒绝）；
    /// false = 仅黑名单拦截，白名单无用，其余全部放行
    #[serde(default = "default_true")]
    pub use_whitelist: bool,
    #[serde(default)]
    pub in_black: Vec<String>,
    #[serde(default)]
    pub in_white: Vec<String>,
    #[serde(default)]
    pub out_black: Vec<String>,
    #[serde(default)]
    pub out_white: Vec<String>,
}

impl Default for NetPolicyCfg {
    fn default() -> Self {
        NetPolicyCfg {
            use_whitelist: true,
            in_black: Vec::new(),
            in_white: Vec::new(),
            out_black: Vec::new(),
            out_white: Vec::new(),
        }
    }
}

/// 危险命令策略。条目为正则表达式，匹配整条命令（大小写不敏感）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DangerPolicyCfg {
    #[serde(default)]
    pub black: Vec<String>,
    /// 是否启用内置默认危险命令清单（缺省 true）
    #[serde(default = "default_true")]
    pub use_builtin: bool,
    /// 内置清单的用户编辑副本（Some = 替换代码内置清单；None = 跟随代码内置）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_override: Option<Vec<String>>,
}

impl Default for DangerPolicyCfg {
    fn default() -> Self {
        DangerPolicyCfg {
            black: Vec::new(),
            use_builtin: true,
            builtin_override: None,
        }
    }
}

/// `rm` **递归删除**的通用正则前缀（下面几条 rm 规则共用）。
///
/// 覆盖 `-rf` / `-fr` / `-r -f` / `--recursive --force` 等写法（开关可拆可拼）。
/// 用 macro 而非 const：`concat!` 只接受字面量，需要宏展开出字面量才能拼接。
macro_rules! rm_recursive_prefix {
    () => {
        r"rm\s+(?:-{1,2}[a-zA-Z]+\s+)*(?:-[a-zA-Z]*[rR][a-zA-Z]*|--recursive)(?:\s+-{1,2}[a-zA-Z]+)*\s+"
    };
}

/// 内置默认危险命令黑名单（正则，大小写不敏感）。
///
/// **取向：只拦真正的不可逆灾难，不拦日常构建/清理写法。**
///
/// 早先的两条 `rm\s+-rf\s+/` 与 `rm\s+-rf\s+\*` 过宽：前者把任何绝对路径删除都当成
/// 「递归删除根目录」，后者把 `rm -rf *.o` 也拦下。后果是 `make clean`、清
/// `node_modules`、构建脚本里的 `rm -rf /abs/target` 一律硬拒；更糟的是**实时轮询**
/// 对子进程命令行用同一组正则，命中即 SIGKILL —— 删除做了一半被杀，留下
/// `Directory not empty` 的半删目录（会话 2026-10-08 实测）。
///
/// 现在改为**精确到路径深度**：
/// - 拦：根 `/`、顶层系统目录本身、`/Users/<用户>`、`~`/`$HOME`、当前目录全部（`*` / `.`）；
/// - 放：`rm -rf *.o`、`rm -rf ./build`、`rm -rf build`、`rm -rf /Users/x/proj/node_modules`。
pub const BUILTIN_DANGER_PATTERNS: &[&str] = &[
    // ---- rm 递归删除：只拦"删到根/顶层/整个家/整层当前目录" ----
    concat!(rm_recursive_prefix!(), r"/(?:\*|\.|/)*(?:\s|$)"), // 根目录本身：/ 、/* 、/. 、//
    concat!(
        rm_recursive_prefix!(),
        // 顶层系统目录本身（含 /usr/、/usr/*；不含 /usr/local）
        r"/(?:bin|boot|dev|etc|home|lib|opt|private|sbin|srv|sys|tmp|usr|var|Applications|Library|System|Volumes|Users)(?:/\*?)?(?:\s|$)"
    ),
    concat!(rm_recursive_prefix!(), r"/Users/[^/\s]+(?:/\*?)?(?:\s|$)"), // 某个用户的家目录整体（不含 /Users/x/proj）
    concat!(rm_recursive_prefix!(), r"(?:~|\$HOME)(?:/\*?)?(?:\s|$)"), // ~ / $HOME 整体
    concat!(rm_recursive_prefix!(), r"(?:\*|\.)(?:\s|$)"), // 当前目录全部（不含 *.o、./build）
    // ---- 其余平台级灾难动作（原样保留）----
    r"format\s+[a-z]:",             // 格式化磁盘（Windows）
    r"del\s+(/[a-z]+\s+)+[a-z]:\\", // 带开关删除系统盘路径（Windows）
    r"rd\s+(/[a-z]+\s+)+[a-z]:\\",  // 删除系统盘根目录（Windows）
    r"rmdir\s+(/[a-z]+\s+)+[a-z]:\\",
    r"mkfs\.\w+",                    // 创建文件系统
    r"shutdown\s+(/[a-z]|-h|-r|-t)", // 关机/重启
    r"reg\s+delete\s+HKLM",          // 删除注册表 HKLM 键
    r"diskpart",                     // 磁盘分区工具
    r"mount\s+-o\s+remount",         // 重挂载文件系统
    r"dd\s+if=/\w+/of=/\w+",         // dd 设备级覆盖
    r"chmod\s+-R\s+777\s+/",         // 根目录放开权限
    r"sc\s+delete\s+",               // 删除 Windows 服务
    r"taskkill\s+/f\s+/im\s+",       // 强杀任意进程
];

/// 文件访问方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessKind {
    Read,
    Write,
}

/// 网络方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Inbound,
    Outbound,
}

/// 网络端点（用于规则匹配与命令分析）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetAddr {
    /// 主机名（可能为空）
    pub host: Option<String>,
    /// IP 地址（可能为空）
    pub ip: Option<IpAddr>,
    /// 端口（可能为空）
    pub port: Option<u16>,
}

impl NetAddr {
    pub fn from_ip(ip: IpAddr) -> Self {
        NetAddr {
            host: None,
            ip: Some(ip),
            port: None,
        }
    }

    pub fn from_host(host: &str) -> Self {
        NetAddr {
            host: Some(host.to_string()),
            ip: None,
            port: None,
        }
    }

    /// 是否为本机回环端点（`127.0.0.0/8`、`::1`，或字面 `localhost` / `*.localhost`）。
    ///
    /// 用途：回环**始终放行**（内核 profile 与运行期驱逐监视都放），因为本地服务
    /// （dev server、后端 8088、healthz 自测）不是出站风险，而「联网白名单开 + 名单为空」
    /// 的默认策略会把 127.0.0.1 一起禁掉 —— 编译通过也起不来、连不上（实测 bind 与
    /// connect 都是 EPERM）。显式黑名单仍然优先：用户主动禁回环时照禁。
    pub fn is_loopback(&self) -> bool {
        if let Some(ip) = self.ip {
            return ip.is_loopback();
        }
        match self.host.as_deref() {
            Some(h) => {
                let h = h.trim_end_matches('.').to_ascii_lowercase();
                h == "localhost"
                    || h.ends_with(".localhost")
                    || h.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
            }
            None => false,
        }
    }

    /// 是否可能命中给定的域名规则（文本匹配，含子域）。
    pub fn matches_domain(&self, domain: &str) -> bool {        let host = match &self.host {
            Some(h) => h,
            None => return false,
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        if host == domain {
            return true;
        }
        // 子域匹配：host 以 ".domain" 结尾
        host.len() > domain.len() && host.ends_with(&format!(".{domain}"))
    }
}

/// 网络规则的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetTarget {
    /// 域名（含子域，如 example.com / *.example.com）
    Domain(String),
    /// 精确 IP
    Ip(IpAddr),
    /// IPv4 网段（CIDR）
    Cidr { addr: Ipv4Addr, prefix: u8 },
    /// 任意地址
    Any,
}

/// 单条网络规则：目标 + 可选端口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRule {
    pub target: NetTarget,
    pub port: Option<u16>,
}

impl NetRule {
    /// 判断端点是否命中本规则（目标 + 端口双重限制）。
    pub fn matches(&self, addr: &NetAddr) -> bool {
        // 端口限制：规则限定端口时，端点必须给出同端口
        if let Some(p) = self.port {
            match addr.port {
                Some(ap) if ap == p => {}
                _ => return false,
            }
        }
        match &self.target {
            NetTarget::Any => true,
            NetTarget::Domain(d) => addr.matches_domain(d),
            NetTarget::Ip(ip) => addr.ip.as_ref() == Some(ip),
            NetTarget::Cidr {
                addr: cidr_addr,
                prefix,
            } => match addr.ip {
                Some(IpAddr::V4(v4)) => {
                    let mask = if *prefix == 0 {
                        0u32
                    } else {
                        u32::MAX << (32 - *prefix as u32)
                    };
                    let a = u32::from(*cidr_addr) & mask;
                    let b = u32::from(v4) & mask;
                    a == b
                }
                _ => false,
            },
        }
    }
}

/// 把配置字符串解析为网络规则。
///
/// 格式：`any` | `domain:<域名>[:端口]` | `ip:<IP>[:端口]` | `ip:<IPv4>/<前缀>`
pub fn parse_net_rule(s: &str) -> Result<NetRule, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("any") {
        return Ok(NetRule {
            target: NetTarget::Any,
            port: None,
        });
    }
    let (kind, rest) = match s.split_once(':') {
        Some((k, r)) if k.eq_ignore_ascii_case("domain") || k.eq_ignore_ascii_case("ip") => {
            (k.to_ascii_lowercase(), r)
        }
        _ => return Err(format!("网络规则格式错误（应为 domain:… 或 ip:…）：{s}")),
    };

    // 拆出可选端口（最后一个冒号后为纯数字时视为端口；IPv6 用 [..]:port 或跳过）
    let (body, port) = split_port(rest);

    match kind.as_str() {
        "domain" => {
            if body.is_empty() {
                return Err(format!("域名规则缺少域名：{s}"));
            }
            Ok(NetRule {
                target: NetTarget::Domain(body.to_string()),
                port,
            })
        }
        "ip" => {
            if let Some((ip, prefix)) = body.split_once('/') {
                let ip: Ipv4Addr = ip
                    .trim()
                    .parse()
                    .map_err(|_| format!("IPv4 地址非法：{body}"))?;
                let prefix: u8 = prefix
                    .trim()
                    .parse()
                    .map_err(|_| format!("CIDR 前缀非法：{body}"))?;
                if prefix > 32 {
                    return Err(format!("CIDR 前缀超出范围：{body}"));
                }
                Ok(NetRule {
                    target: NetTarget::Cidr { addr: ip, prefix },
                    port,
                })
            } else {
                let ip: IpAddr = body.parse().map_err(|_| format!("IP 地址非法：{body}"))?;
                Ok(NetRule {
                    target: NetTarget::Ip(ip),
                    port,
                })
            }
        }
        _ => unreachable!(),
    }
}

/// 从 "主体:端口" 中拆出可选端口；IPv6 裸地址不带端口时不拆分（IPv6 含冒号引导 JSON 时请用 list 形式）。
fn split_port(rest: &str) -> (&str, Option<u16>) {
    // IPv6 地址包含多个冒号，只有形如 xxx:数字 的尾部才视为端口
    if let Some(idx) = rest.rfind(':') {
        let (head, tail) = rest.split_at(idx);
        let tail = &tail[1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(p) = tail.parse::<u16>() {
                if p != 0 {
                    return (head, Some(p));
                }
            }
        }
    }
    (rest, None)
}

// ============================ 编译后的策略 ============================

/// 已编译的文件读写策略（glob 集合，匹配时大小写不敏感；同时保留原始模式供平台层映射）。
#[derive(Debug, Clone)]
pub struct ResolvedFilePolicy {
    pub read_black: globset::GlobSet,
    pub read_white: globset::GlobSet,
    pub write_black: globset::GlobSet,
    pub write_white: globset::GlobSet,
    /// 原始模式（seatbelt profile 生成 / ACL 映射等平台层使用）
    pub read_black_raw: Vec<String>,
    pub read_white_raw: Vec<String>,
    pub write_black_raw: Vec<String>,
    pub write_white_raw: Vec<String>,
}

impl ResolvedFilePolicy {
    /// 空策略：无任何规则（预检时放行全部，运行时按平台强制层默认拒绝）。
    pub fn empty() -> Self {
        let empty = globset::GlobSetBuilder::new().build().unwrap();
        ResolvedFilePolicy {
            read_black: empty.clone(),
            read_white: empty.clone(),
            write_black: empty.clone(),
            write_white: empty,
            read_black_raw: Vec::new(),
            read_white_raw: Vec::new(),
            write_black_raw: Vec::new(),
            write_white_raw: Vec::new(),
        }
    }

    fn build(patterns: &[String]) -> Result<globset::GlobSet, String> {
        let mut builder = globset::GlobSetBuilder::new();
        for p in patterns {
            let glob = globset::GlobBuilder::new(p)
                // Windows/macOS 默认文件系统大小写不敏感；Linux 保持敏感
                .case_insensitive(cfg!(any(windows, target_os = "macos")))
                .literal_separator(false)
                .build()
                .map_err(|e| format!("文件路径规则非法（{p}）：{e}"))?;
            builder.add(glob);
        }
        builder
            .build()
            .map_err(|e| format!("文件路径规则编译失败：{e}"))
    }

    pub fn compile(cfg: &FilePolicyCfg) -> Result<Self, String> {
        Ok(ResolvedFilePolicy {
            read_black: Self::build(&cfg.read_black)?,
            read_white: Self::build(&cfg.read_white)?,
            write_black: Self::build(&cfg.write_black)?,
            write_white: Self::build(&cfg.write_white)?,
            read_black_raw: cfg.read_black.clone(),
            read_white_raw: cfg.read_white.clone(),
            write_black_raw: cfg.write_black.clone(),
            write_white_raw: cfg.write_white.clone(),
        })
    }
}

/// 已编译的网络策略。
#[derive(Debug, Clone, Default)]
pub struct ResolvedNetPolicy {
    /// 白名单开关：false = 仅黑名单拦截，其余放行；true = 白名单之外默认拒绝
    pub use_whitelist: bool,
    pub in_black: Vec<NetRule>,
    pub in_white: Vec<NetRule>,
    pub out_black: Vec<NetRule>,
    pub out_white: Vec<NetRule>,
}

impl ResolvedNetPolicy {
    pub fn compile(cfg: &NetPolicyCfg) -> Result<Self, String> {
        let compile = |list: &Vec<String>| -> Result<Vec<NetRule>, String> {
            list.iter().map(|s| parse_net_rule(s)).collect()
        };
        Ok(ResolvedNetPolicy {
            use_whitelist: cfg.use_whitelist,
            in_black: compile(&cfg.in_black)?,
            in_white: compile(&cfg.in_white)?,
            out_black: compile(&cfg.out_black)?,
            out_white: compile(&cfg.out_white)?,
        })
    }

    /// 取指定方向的规则列表。
    pub fn rules_for(&self, dir: Direction) -> (&[NetRule], &[NetRule]) {
        match dir {
            Direction::Inbound => (&self.in_black, &self.in_white),
            Direction::Outbound => (&self.out_black, &self.out_white),
        }
    }

    /// 运行期策略：把域名规则解析为 IP 规则并追加。
    ///
    /// 运行期监视（netstat/lsof）只能拿到对端 IP，域名规则若不预解析将永远匹配不到，
    /// 导致"白名单写了域名仍被默认拒绝驱逐"。解析失败的域名返回在 failed 列表中，
    /// 由调用方以 Note 事件明示（绝不静默）。原策略不变（预检仍按文本匹配域名）。
    pub fn with_domains_resolved(&self) -> (ResolvedNetPolicy, Vec<String>) {
        use std::net::ToSocketAddrs;
        let mut p = self.clone();
        let mut failed: Vec<String> = Vec::new();
        let mut expand = |src: &[NetRule], dst: &mut Vec<NetRule>| {
            for r in src {
                if let NetTarget::Domain(d) = &r.target {
                    match (d.as_str(), 0u16).to_socket_addrs() {
                        Ok(addrs) => {
                            for a in addrs {
                                let rule = NetRule {
                                    target: NetTarget::Ip(a.ip()),
                                    port: r.port,
                                };
                                if !dst.contains(&rule) {
                                    dst.push(rule);
                                }
                            }
                        }
                        Err(_) => failed.push(d.clone()),
                    }
                }
            }
        };
        expand(&self.in_black, &mut p.in_black);
        expand(&self.in_white, &mut p.in_white);
        expand(&self.out_black, &mut p.out_black);
        expand(&self.out_white, &mut p.out_white);
        (p, failed)
    }
}

/// 已编译的危险命令策略（正则集合，大小写不敏感）。
#[derive(Debug, Clone)]
pub struct ResolvedDanger {
    pub patterns: Vec<String>,
    set: regex::RegexSet,
}

impl ResolvedDanger {
    pub fn compile(cfg: &DangerPolicyCfg) -> Result<Self, String> {
        let mut patterns: Vec<String> = Vec::new();
        if cfg.use_builtin {
            match &cfg.builtin_override {
                // 用户编辑过的内置清单副本优先（可增删改条目）
                Some(overrides) => patterns.extend(overrides.iter().cloned()),
                None => patterns.extend(BUILTIN_DANGER_PATTERNS.iter().map(|s| s.to_string())),
            }
        }
        patterns.extend(cfg.black.iter().cloned());
        let set = regex::RegexSetBuilder::new(&patterns)
            .case_insensitive(true)
            .build()
            .map_err(|e| format!("危险命令正则编译失败：{e}"))?;
        Ok(ResolvedDanger { patterns, set })
    }

    /// 返回命中的全部模式（空 = 未命中）。
    pub fn matches(&self, cmdline: &str) -> Vec<String> {
        self.set
            .matches(cmdline)
            .iter()
            .map(|i| self.patterns[i].clone())
            .collect()
    }
}

/// 汇总的已编译防护策略；`enabled=false` 表示直接执行不检测。
#[derive(Debug, Clone)]
pub struct ResolvedPolicy {
    pub enabled: bool,
    pub strict: bool,
    pub file: ResolvedFilePolicy,
    pub net: ResolvedNetPolicy,
    pub danger: ResolvedDanger,
    pub resource: ResourceLimits,
    /// 已禁用沙箱自动注入的工具 bin 名（判定不使用；透传给平台会话层过滤注入）
    pub toolchain_disabled: Vec<String>,
    /// 工作区内 exec 放行（透传给平台会话层）
    pub allow_workspace_exec: bool,
    /// 额外可执行目录（透传 [`ProtectionConfig::exec_white_dirs`]）
    pub exec_white_dirs: Vec<String>,
    /// 会话工作区（透传 [`SandboxInput::workspace`]；None = 未知）
    pub workspace: Option<PathBuf>,
    /// 持久 HOME（透传 [`SandboxInput::session_home`]；None = 逐命令临时 HOME）
    pub session_home: Option<PathBuf>,
}

impl ResolvedPolicy {
    /// 把原始配置编译为可直接判定与映射到平台强制层的策略。
    pub fn compile(input: &SandboxInput) -> Result<Self, String> {
        let cfg = &input.protection;
        Ok(ResolvedPolicy {
            enabled: cfg.enabled,
            strict: input.strict,
            file: ResolvedFilePolicy::compile(&cfg.file)?,
            net: ResolvedNetPolicy::compile(&cfg.net)?,
            danger: ResolvedDanger::compile(&cfg.danger)?,
            resource: cfg.resource.clone(),
            toolchain_disabled: cfg.toolchain_disabled.clone(),
            allow_workspace_exec: cfg.allow_workspace_exec,
            exec_white_dirs: cfg.exec_white_dirs.clone(),
            workspace: input.workspace.clone(),
            session_home: input.session_home.clone(),
        })
    }

    /// 防护关闭时的空策略（仍然持有空集合，判定函数永远放行/或不被调用）。
    pub fn disabled() -> Self {
        ResolvedPolicy {
            enabled: false,
            strict: false,
            file: ResolvedFilePolicy::empty(),
            net: ResolvedNetPolicy::default(),
            danger: ResolvedDanger {
                patterns: Vec::new(),
                set: regex::RegexSet::empty(),
            },
            resource: ResourceLimits::default(),
            toolchain_disabled: Vec::new(),
            allow_workspace_exec: false,
            exec_white_dirs: Vec::new(),
            workspace: None,
            session_home: None,
        }
    }

    /// 输出报告附带的能力信息（由执行层填充）。
    #[allow(dead_code)]
    pub fn describe(&self) -> String {
        format!(
            "防护={} strict={} 文件规则(读黑{} 读白{} 写黑{} 写白{}) 网络规则(进黑{} 进白{} 出黑{} 出白{}) 危险命令{}条 资源(内存{:?}MB 磁盘{:?}MB CPU{:?}s GPU{})",
            if self.enabled { "开" } else { "关" },
            self.strict,
            pattern_count(&self.file.read_black),
            pattern_count(&self.file.read_white),
            pattern_count(&self.file.write_black),
            pattern_count(&self.file.write_white),
            self.net.in_black.len(),
            self.net.in_white.len(),
            self.net.out_black.len(),
            self.net.out_white.len(),
            self.danger.patterns.len(),
            self.resource.memory_mb,
            self.resource.disk_mb,
            self.resource.cpu_seconds,
            if self.resource.gpu { "允许" } else { "禁止" },
        )
    }

    /// 报告输出时携带的防护开关状态。
    pub fn protection_on(&self) -> bool {
        self.enabled
    }
}

fn pattern_count(set: &globset::GlobSet) -> usize {
    // GlobSet 不提供计数，这里以空集特征判断即可
    let _ = set;
    0
}

// 供 cli / runner 使用的小助手
#[allow(dead_code)]
pub(crate) fn cmdline_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.chars().any(|c| c.is_whitespace() || c == '"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================ 测试 ============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_policy_case_matching_follows_platform() {
        let cfg = FilePolicyCfg {
            read_white: vec!["/tmp/Docs/**".to_string()],
            ..Default::default()
        };
        let p = ResolvedFilePolicy::compile(&cfg).unwrap();
        #[cfg(any(windows, target_os = "macos"))]
        assert!(
            p.read_white.is_match("/tmp/docs/a.txt"),
            "Windows/macOS 应大小写不敏感"
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        assert!(
            !p.read_white.is_match("/tmp/docs/a.txt"),
            "Linux 应保持大小写敏感"
        );
        assert!(p.read_white.is_match("/tmp/Docs/a.txt"));
    }

    #[test]
    fn 网络规则解析() {
        let r = parse_net_rule("domain:example.com").unwrap();
        assert!(matches!(r.target, NetTarget::Domain(_)));
        assert_eq!(r.port, None);

        let r = parse_net_rule("domain:example.com:443").unwrap();
        assert_eq!(r.port, Some(443));

        let r = parse_net_rule("ip:8.8.8.8").unwrap();
        assert_eq!(r.target, NetTarget::Ip("8.8.8.8".parse().unwrap()));

        let r = parse_net_rule("ip:10.0.0.0/8").unwrap();
        assert!(matches!(r.target, NetTarget::Cidr { prefix: 8, .. }));

        let r = parse_net_rule("ip:192.168.1.1:8080").unwrap();
        assert_eq!(r.port, Some(8080));

        assert!(parse_net_rule("wat").is_err());
        assert!(parse_net_rule("ip:zzz").is_err());
        assert!(parse_net_rule("ip:1.2.3.4/33").is_err());
    }

    #[test]
    fn 域名规则运行期解析为_ip() {
        let np = ResolvedNetPolicy::compile(&NetPolicyCfg {
            out_white: vec!["domain:localhost".to_string()],
            out_black: vec!["domain:nonexistent.invalid.example".to_string()],
            ..Default::default()
        })
        .unwrap();
        let (runtime, failed) = np.with_domains_resolved();
        // localhost 可解析 → 追加 IP 规则；不存在的域名 → 进 failed 列表（明示）
        assert_eq!(failed, vec!["nonexistent.invalid.example".to_string()]);
        assert!(runtime
            .out_white
            .iter()
            .any(|r| matches!(r.target, NetTarget::Ip(_))));
        // 白名单里的 localhost 解析结果应能命中环回地址
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(runtime.out_white.iter().any(|r| r.matches(&NetAddr {
            host: None,
            ip: Some(loopback),
            port: Some(8080),
        })));
        // 原策略不变（预检仍按文本匹配域名）
        assert!(np
            .out_white
            .iter()
            .all(|r| !matches!(r.target, NetTarget::Ip(_))));
    }

    #[test]
    fn 域名规则匹配含子域() {
        let addr = NetAddr {
            host: Some("a.b.example.com".to_string()),
            ip: None,
            port: Some(443),
        };
        let rule = NetRule {
            target: NetTarget::Domain("example.com".to_string()),
            port: Some(443),
        };
        assert!(rule.matches(&addr));

        let other = NetRule {
            target: NetTarget::Domain("notexample.com".to_string()),
            port: None,
        };
        assert!(!other.matches(&addr));
    }

    #[test]
    fn cidr匹配() {
        let rule = NetRule {
            target: NetTarget::Cidr {
                addr: "10.0.0.0".parse().unwrap(),
                prefix: 8,
            },
            port: None,
        };
        assert!(rule.matches(&NetAddr::from_ip("10.1.2.3".parse().unwrap())));
        assert!(!rule.matches(&NetAddr::from_ip("11.0.0.1".parse().unwrap())));
    }

    #[test]
    fn 端口限制() {
        let rule = NetRule {
            target: NetTarget::Ip("1.2.3.4".parse().unwrap()),
            port: Some(443),
        };
        assert!(rule.matches(&NetAddr {
            host: None,
            ip: Some("1.2.3.4".parse().unwrap()),
            port: Some(443),
        }));
        assert!(!rule.matches(&NetAddr::from_ip("1.2.3.4".parse().unwrap())));
    }

    #[test]
    fn 危险命令编译与匹配() {
        let cfg = DangerPolicyCfg {
            black: vec![r"rm\s+-rf\s+/".to_string()],
            use_builtin: false,
        
            builtin_override: None,
};
        let d = ResolvedDanger::compile(&cfg).unwrap();
        assert_eq!(d.matches("apt-get install x").len(), 0);
        assert!(d.matches("rm -rf /").len() > 0);
        assert!(d.matches("RM -RF /").len() > 0); // 大小写不敏感
    }

    #[test]
    fn 内置危险清单存在() {
        assert!(!BUILTIN_DANGER_PATTERNS.is_empty());
    }

    /// 内置 rm 规则的收敛矩阵：**只拦删根/顶层/整个家/整层当前目录**，
    /// 日常构建清理写法一律放行（会话 2026-10-08：`rm -rf /abs/path` 与
    /// `rm -rf *.o` 被误判，且实时轮询会边删边杀）。
    #[test]
    fn 内置rm规则只拦灾难_不误伤构建清理() {
        let cfg = DangerPolicyCfg::default(); // use_builtin = true
        let d = ResolvedDanger::compile(&cfg).unwrap();
        let blocked = [
            "rm -rf /",
            "rm -rf /*",
            "rm -rf /.",
            "rm -fr /",
            "rm -r -f /",
            "rm --recursive --force /",
            "sudo rm -rf /",
            "rm -rf /usr",
            "rm -rf /usr/",
            "rm -rf /usr/*",
            "rm -rf /etc",
            "rm -rf /Users",
            "rm -rf /Users/x",
            "rm -rf /Users/x/",
            "rm -rf /Users/x/*",
            "rm -rf ~",
            "rm -rf ~/*",
            "rm -rf $HOME",
            "rm -rf *",
            "rm -rf .",
        ];
        for c in blocked {
            assert!(d.matches(c).len() > 0, "应判为危险命令: {c}");
        }
        let allowed = [
            "rm -rf node_modules",
            "rm -rf build",
            "rm -rf ./build",
            "rm -rf ../build",
            "rm -rf *.o",
            "rm -rf target/release",
            "rm -rf /Users/x/workspace/my-project/console/node_modules",
            "rm -rf /tmp/zzbuild",
            "rm -rf /var/folders/j_/T/astorm-1",
            "rm -rf ~/proj/target",
            "rm -f a.txt",
            "rm a.txt",
            "npm ci --prefix console",
        ];
        for c in allowed {
            assert_eq!(d.matches(c).len(), 0, "不应判为危险命令: {c}");
        }
    }
}
