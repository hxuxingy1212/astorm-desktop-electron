//! 策略字段级强制力自曝（结构化报告）。
//!
//! 「某字段到底怎么强制、近似到什么程度」此前只写在平台实现的代码注释里，
//! 上层（工具结果、设置页、CLI）无法消费。本模块把它变成
//! [`ResolvedPolicy::enforcement_report`]：**逐字段**声明
//!
//! - `state`：`Enforced`（内核/强制原语按字段语义硬执行）/ `Approximate`（有强制动作，
//!   但机制只能近似该语义：采样、轮询、单文件/单进程上限…）/ `NotSupported`
//!   （当前平台强制层根本不读该字段，配置被忽略）；
//! - `mechanism`：实际生效的机制（照代码事实写，标注实现文件/函数）；
//! - `caveat`：偏差与前提（`Approximate` / `NotSupported` 必须给出，否则自曝没有意义）。
//!
//! 范围：只描述**平台强制层**的事实（`sandbox/macos.rs`、`sandbox/windows/mod.rs`、
//! `seatbelt_builder.rs`、`decision.rs`），平台差异按 `cfg` 分别声明。
//! 报告是策略级声明（只依赖字段是否被配置），不随单次执行结果变化；单次执行实际落在
//! 哪个后端（passthrough 降级等）由 `runner::ExecReport` 的 backend/capabilities 说明。
//!
//! `protection.enabled = false` 时不做任何检测，报告整体降为 `NotSupported`。
//!
//! 新增 `ProtectionConfig` 字段时：两份平台清单必须同时补齐——`报告覆盖保护配置全部字段`
//! 门禁测试以 serde 叶字段名为独立事实源比对，漏声明即失败。

use serde::Serialize;

use crate::policy::ResolvedPolicy;

/// 字段强制状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementState {
    /// 内核/强制原语按字段语义硬执行。
    Enforced,
    /// 有强制动作，但机制只能近似字段语义（采样/轮询/单文件/单进程上限等）。
    Approximate,
    /// 当前平台强制层不读该字段（配置被忽略）。
    NotSupported,
}

impl EnforcementState {
    /// 机器可读名（snake_case，与 serde / napi JSON 一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            EnforcementState::Enforced => "enforced",
            EnforcementState::Approximate => "approximate",
            EnforcementState::NotSupported => "not_supported",
        }
    }

    /// 人类/模型可读名（与 Rust 变体同名，用于 CLI 与工具结果的一行式输出）。
    pub fn label(self) -> &'static str {
        match self {
            EnforcementState::Enforced => "Enforced",
            EnforcementState::Approximate => "Approximate",
            EnforcementState::NotSupported => "NotSupported",
        }
    }

    /// 是否 Enforced（工具结果只保留非 Enforced 项）。
    pub fn is_enforced(self) -> bool {
        matches!(self, EnforcementState::Enforced)
    }
}

/// 单个策略字段的强制力声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldEnforcement {
    /// 字段路径（`ProtectionConfig` 的 serde 字段名，如 `file.read_black`、`resource.memory_mb`）
    pub field: String,
    pub state: EnforcementState,
    /// 实际生效的强制机制（照代码事实）
    pub mechanism: String,
    /// 偏差/前提（`Approximate`、`NotSupported` 必须给出）
    pub caveat: Option<String>,
}

impl FieldEnforcement {
    fn new(field: &str, state: EnforcementState, mechanism: &str, caveat: Option<&str>) -> Self {
        FieldEnforcement {
            field: field.to_string(),
            state,
            mechanism: mechanism.to_string(),
            caveat: caveat.map(|c| c.to_string()),
        }
    }

    /// 一行式文本：`<field>=<State>: <mechanism>(<caveat>)`（CLI 与工具结果共用）。
    pub fn line(&self) -> String {
        let mut s = format!("{}={}: {}", self.field, self.state.label(), self.mechanism);
        if let Some(c) = &self.caveat {
            s.push_str(&format!("({c})"));
        }
        s
    }
}

/// `ProtectionConfig` 的全部字段路径（报告必须逐个覆盖；门禁测试比对 serde 叶字段名）。
pub const POLICY_FIELDS: &[&str] = &[
    "enabled",
    "file.read_black",
    "file.read_white",
    "file.write_black",
    "file.write_white",
    "net.use_whitelist",
    "net.in_black",
    "net.in_white",
    "net.out_black",
    "net.out_white",
    "danger",
    "resource.memory_mb",
    "resource.disk_mb",
    "resource.cpu_seconds",
    "resource.cpu_percent",
    "resource.gpu",
    "toolchain_disabled",
    "allow_workspace_exec",
    "exec_white_dirs",
];

impl ResolvedPolicy {
    /// 逐字段强制力报告（见模块文档）。
    pub fn enforcement_report(&self) -> Vec<FieldEnforcement> {
        if !self.enabled {
            return disabled_report();
        }
        #[cfg(target_os = "macos")]
        {
            macos_report()
        }
        #[cfg(windows)]
        {
            windows_report()
        }
        #[cfg(target_os = "linux")]
        {
            linux_report()
        }
    }
}

/// 防护总开关关闭：所有字段都不生效（直通执行）。
fn disabled_report() -> Vec<FieldEnforcement> {
    POLICY_FIELDS
        .iter()
        .map(|f| {
            FieldEnforcement::new(
                f,
                EnforcementState::NotSupported,
                "防护总开关关闭（protection.enabled=false）：runner 选 PassthroughBackend 直接执行，不做任何检测",
                Some("该字段配置被忽略；结果 JSON 中 sandbox.protected=false"),
            )
        })
        .collect()
}

/// 总开关本身的强制力（两平台一致：runner 按 `enabled` 选后端）。
fn enabled_entry() -> FieldEnforcement {
    FieldEnforcement::new(
        "enabled",
        EnforcementState::Enforced,
        "总开关（硬）：runner 按 protection.enabled 选后端，false → PassthroughBackend（无内核强制、明确告警）",
        None,
    )
}

// =====================================================================
// macOS：Seatbelt 内核强制 + ps/lsof 监视驱逐（sandbox/macos.rs）
// =====================================================================

/// macOS 清单（纯数据，任何平台都编译/可测；未启用的平台不参与分发）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_report() -> Vec<FieldEnforcement> {
    use EnforcementState::{Approximate, Enforced};

    vec![
        enabled_entry(),
        FieldEnforcement::new(
            "file.read_black",
            Enforced,
            "Seatbelt：profile 以 (deny default) 起底，(deny file-read-data) 黑名单行先于放行下发（首条命中 → 黑 > 白）",
            Some("glob 通配按固定前缀粗化为 literal/subpath（seatbelt_builder::path_rules）；依赖 /usr/lib/libsandbox.dylib 可加载"),
        ),
        FieldEnforcement::new(
            "file.read_white",
            Enforced,
            "Seatbelt：(allow file-read*) 白名单放行，未命中由 (deny default) 内核拒绝（EPERM）",
            Some("系统区/工具链只读根（roots::macos_system_read_roots）总是先放行，用户白名单只是叠加、不能收紧"),
        ),
        FieldEnforcement::new(
            "file.write_black",
            Enforced,
            "Seatbelt：(deny file-write-data) 黑名单行先于放行下发（黑 > 白）",
            Some("glob 通配粗化为固定前缀；依赖 Seatbelt 可用"),
        ),
        FieldEnforcement::new(
            "file.write_white",
            Enforced,
            "Seatbelt：(allow file-write*) 仅会话临时/数据目录、持久 HOME、工作区与写白名单可写",
            Some("未授权写被内核拒绝且不产生结构化事件；/dev/null、/dev/tty 单独放行"),
        ),
        FieldEnforcement::new(
            "net.use_whitelist",
            Approximate,
            "Seatbelt 方向级开关：开且对应方向白名单为空 → (deny network-*)（内核整片拒绝）；否则内核放行整方向，逐主机交 lsof 轮询驱逐",
            Some("内核无逐主机粒度；白名单非空时内核全放行，未命中白名单的主机靠 ~200ms lsof 轮询驱逐，短连接可能先完成；loopback 始终放行"),
        ),
        FieldEnforcement::new(
            "net.in_black",
            Approximate,
            "lsof 轮询 + decision::decide_net 命中即 killpg（事件后处置）；进站白名单为空时内核整方向拒绝已覆盖",
            Some("轮询周期约 200ms（设内存上限时 50ms），短连接可能在驱逐前完成；内核层不按主机拦黑名单"),
        ),
        FieldEnforcement::new(
            "net.in_white",
            Approximate,
            "白名单开且进站名单为空 → 内核 (deny network-inbound)；非空 → 内核放行，lsof 轮询驱逐未命中主机",
            Some("逐主机靠 ~200ms lsof 轮询；loopback 始终放行；域名在会话初始化时解析为 IP，解析失败仅告警（不阻断）"),
        ),
        FieldEnforcement::new(
            "net.out_black",
            Approximate,
            "lsof 轮询 + decision::decide_net 命中即 killpg（事件后处置）；出站白名单为空时内核整方向拒绝已覆盖",
            Some("轮询周期约 200ms（设内存上限时 50ms），短连接可能在驱逐前完成；内核层不按主机拦黑名单"),
        ),
        FieldEnforcement::new(
            "net.out_white",
            Approximate,
            "白名单开且出站名单为空 → 内核 (deny network-outbound)；非空 → 内核放行，lsof 轮询驱逐未命中主机",
            Some("逐主机靠 ~200ms lsof 轮询；loopback 始终放行；域名解析结果不跟随 DNS 变更"),
        ),
        FieldEnforcement::new(
            "danger",
            Enforced,
            "静态预检正则（根命令整条匹配，analyzer::preflight）+ ps 轮询逐子进程命令行，命中即 SIGKILL",
            Some("子进程靠 ~200ms ps 轮询（设内存上限时 50ms），瞬时进程可能先退出；只对 ps 采到的命令行判定"),
        ),
        FieldEnforcement::new(
            "resource.memory_mb",
            Approximate,
            "watch_loop 按 ps rss 合计进程树 RSS 采样，超限 SIGKILL 整树；设内存上限时轮询加速到 ~50ms",
            Some("macOS 无法 setrlimit(RLIMIT_AS/DATA)（EINVAL），只能采样驱逐：瞬时冲高、采样间隙可能漏过"),
        ),
        FieldEnforcement::new(
            "resource.disk_mb",
            Approximate,
            "RLIMIT_FSIZE 单文件写入上限（apply_rlimits），超限触发 SIGXFSZ",
            Some("是单文件上限而非总量配额：多文件/多进程写可累计超过 disk_mb"),
        ),
        FieldEnforcement::new(
            "resource.cpu_seconds",
            Approximate,
            "RLIMIT_CPU 单进程 CPU 时间上限（apply_rlimits），超限触发 SIGXCPU 终止",
            Some("按单进程计，不跨进程累计；每个子进程各有独立额度，进程树总量可超过该值"),
        ),
        FieldEnforcement::new(
            "resource.cpu_percent",
            Approximate,
            "watch_loop 以 cputime 差分算整机占比，超限用 SIGSTOP/SIGCONT 占空比节流（含核数换算）",
            Some("非硬配额：采样周期 ≥0.4s、超 10% 容差才节流，实际占比有 ±10% 级误差；节流周期内跳过一轮网络检查"),
        ),
        FieldEnforcement::new(
            "resource.gpu",
            Enforced,
            "Seatbelt：false 时 (deny iokit-open-user-client) 并阻断 Metal 相关 mach 服务（MTLCompilerService / coreanimation.render-server / MTLRenderCommandEncoder）",
            Some("依赖 Seatbelt 可用；只阻断用户态 GPU 服务，不是硬件级全栈隔离"),
        ),
        FieldEnforcement::new(
            "toolchain_disabled",
            Enforced,
            "从 Seatbelt process-exec 与只读放行集合中剔除被禁用工具所属的自动注入目录（macos.rs 注入过滤）",
            Some("只停止沙箱自动注入，不是执行拦截：bin 位于系统目录/工作区等其它已放行路径时仍可执行"),
        ),
        FieldEnforcement::new(
            "allow_workspace_exec",
            Enforced,
            "true：工作区（canonicalize 后）加入 Seatbelt process-exec 放行；false：不下发该放行（内核拒绝工作区可执行文件）",
            Some("关掉不构成安全边界（已放行解释器仍可用 `sh x.sh` 运行脚本）；工作区读写放行不受该字段影响"),
        ),
        FieldEnforcement::new(
            "exec_white_dirs",
            Enforced,
            "目录加入 Seatbelt process-exec 放行；其自身与父目录（HOME/根除外）叠加只读 glob 放行（toolchain::extra_dirs）",
            Some("目录须存在（不存在/空项忽略）；只放行 exec 与只读，写仍受写白名单约束"),
        ),
    ]
}

// =====================================================================
// Windows：AppContainer/DACL + Job Object + 防火墙（sandbox/windows/mod.rs）
// =====================================================================

/// Windows 清单（纯数据，任何平台都编译/可测；未启用的平台不参与分发）。
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_report() -> Vec<FieldEnforcement> {
    use EnforcementState::{Enforced, NotSupported};

    vec![
        enabled_entry(),
        FieldEnforcement::new(
            "file.read_black",
            Enforced,
            "AppContainer(lowbox) 主令牌 + icacls Deny(R) ACE；Deny 优先于 Allow（内核实现黑 > 白），未授权由内核拒绝",
            Some("glob 只取固定前缀（glob_fixed_prefix，通配截断）；系统目录的 Deny ACE 需管理员；icacls 失败仅 Note 告警（该条可能不生效）"),
        ),
        FieldEnforcement::new(
            "file.read_white",
            Enforced,
            "icacls Grant(RX) ACE（读+执行），叠加在 AppContainer 默认只读基线上",
            Some("系统区（C:\\Windows 等）默认可读，白名单是额外授权；glob 只取固定前缀"),
        ),
        FieldEnforcement::new(
            "file.write_black",
            Enforced,
            "icacls Deny(W,DC) ACE（写+删子项）；Deny 优先于 Allow",
            Some("glob 只取固定前缀；系统目录 Deny 需管理员；icacls 失败仅 Note 告警（该条可能不生效）"),
        ),
        FieldEnforcement::new(
            "file.write_white",
            Enforced,
            "icacls Grant(M) ACE；AppContainer 默认拒绝写任何未授权路径（含系统区），写白名单是唯一写入来源",
            Some("glob 只取固定前缀；cwd 只授予 RX，工作区写入需显式在白名单中"),
        ),
        FieldEnforcement::new(
            "net.use_whitelist",
            Enforced,
            "管理员：WFP 防火墙按沙箱 SID 下发方向级默认拒绝 + 白名单 Allow/黑名单 Block 规则；白名单关闭时只下发黑名单 Block",
            Some("需管理员：非管理员降级为 netstat 轮询（约 1s）驱逐（连接先建立后终止）；含 any 的规则防火墙层无法表达，初始化直接报错"),
        ),
        FieldEnforcement::new(
            "net.in_black",
            Enforced,
            "防火墙 Block 规则（RemoteAddress = IP/CIDR/域名解析结果，按沙箱 SID 生效）",
            Some("域名在规则下发时解析为 IP、不跟随 DNS 变更；需管理员（非管理员走 netstat 驱逐，短连接可能漏）"),
        ),
        FieldEnforcement::new(
            "net.in_white",
            Enforced,
            "防火墙 Allow 规则（白名单开启时先下发方向级默认拒绝）",
            Some("域名解析为 IP、需管理员；loopback 始终放行（decision::decide_net 豁免，显式黑名单除外）"),
        ),
        FieldEnforcement::new(
            "net.out_black",
            Enforced,
            "防火墙 Block 规则（RemoteAddress = IP/CIDR/域名解析结果，按沙箱 SID 生效）",
            Some("域名在规则下发时解析为 IP、不跟随 DNS 变更；需管理员（非管理员走 netstat 驱逐，短连接可能漏）"),
        ),
        FieldEnforcement::new(
            "net.out_white",
            Enforced,
            "防火墙 Allow 规则（白名单开启时先下发方向级默认拒绝）",
            Some("域名解析为 IP、需管理员；loopback 始终放行；DNS 变更后规则不自动跟随"),
        ),
        FieldEnforcement::new(
            "danger",
            Enforced,
            "静态预检正则（根命令）+ DEBUG_PROCESS 调试循环：CREATE_PROCESS 事件读子进程 PEB 命令行，命中即 TerminateProcess",
            Some("调试循环只判新建子进程（根命令由预检覆盖）；PEB 读取失败按空命令行处理，可能漏判"),
        ),
        FieldEnforcement::new(
            "resource.memory_mb",
            Enforced,
            "Job Object JobMemoryLimit（JOB_OBJECT_LIMIT_JOB_MEMORY）：整 Job 提交内存上限",
            Some("按提交内存（commit）计量：超限表现为分配失败而非终止进程；Job 与整树一起回收"),
        ),
        FieldEnforcement::new(
            "resource.disk_mb",
            NotSupported,
            "未实现：windows/mod.rs 的 Job 与 ACL 均未设置磁盘写入上限，resource.disk_mb 不被读取",
            Some("capabilities.resource.disk=false：磁盘写入不受 disk_mb 约束"),
        ),
        FieldEnforcement::new(
            "resource.cpu_seconds",
            Enforced,
            "Job Object PerJobUserTimeLimit（JOB_OBJECT_LIMIT_JOB_TIME）：整 Job 用户态 CPU 时间累计上限",
            Some("跨进程累计；由 Job 语义在超限时终止整个 Job（非仅超限进程）"),
        ),
        FieldEnforcement::new(
            "resource.cpu_percent",
            Enforced,
            "Job Object CPU Rate Control 硬上限（HARD_CAP，CpuRate=百分比×100，整机占比口径）",
            Some("按调度间隔配额限速（不终止进程），极短窗口内可能瞬时超标；设置失败即会话初始化报错"),
        ),
        FieldEnforcement::new(
            "resource.gpu",
            NotSupported,
            "未实现：windows/mod.rs 不读取 resource.gpu，无 GPU 阻断路径",
            Some("capabilities.resource.gpu=false：AppContainer 不影响 GPU 访问"),
        ),
        FieldEnforcement::new(
            "toolchain_disabled",
            NotSupported,
            "Windows 强制层未读取该字段：无工具链 exec/只读注入过滤（ACL 授权不区分工具链来源）",
            Some("该配置只对 macOS 的 Seatbelt 注入生效"),
        ),
        FieldEnforcement::new(
            "allow_workspace_exec",
            NotSupported,
            "Windows 强制层未读取该字段：apply_acl 无条件 icacls Grant(cwd, RX)，工作区可执行文件始终可运行",
            Some("关掉 allow_workspace_exec 不改变 Windows 行为（与 macOS 不同，此处为如实上报的未实现项）"),
        ),
        FieldEnforcement::new(
            "exec_white_dirs",
            Enforced,
            "每个目录 icacls Grant(RX) ACE（读+执行，不授予写）",
            Some("目录须存在；授权失败仅 Note 告警（该目录对沙箱不可用）；按目录整前缀授权，不解析 glob"),
        ),
    ]
}

// =====================================================================
// Linux：暂无内核强制层（Passthrough；隔离由容器/宿主边界负责）
// =====================================================================

/// Linux 清单（纯数据，任何平台都编译/可测）。当前无平台强制层：
/// 静态预检（analyzer::preflight）仍然生效，黑名单类按 Approximate 如实上报；
/// 白名单/资源/工具链类无任何强制动作，按 NotSupported 如实上报。
/// 后续补 bubblewrap/Landlock 后端时按字段重写。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_report() -> Vec<FieldEnforcement> {
    use EnforcementState::{Approximate, Enforced, NotSupported};

    let passthrough = |field: &str, state, mechanism: &str, caveat: &str| {
        FieldEnforcement::new(field, state, mechanism, Some(caveat))
    };

    vec![
        passthrough(
            "enabled",
            Enforced,
            "总开关（硬）：runner 按 protection.enabled 决定是否编译策略与静态预检；Linux 平台后端恒为 Passthrough",
            "内核级文件/网络强制缺失，进程隔离由容器/宿主边界负责；超时、输出流与进程树终止仍生效",
        ),
        passthrough(
            "file.read_black",
            Approximate,
            "静态预检：命令文本命中黑名单路径即拒绝执行（analyzer::preflight）",
            "仅命令文本级：间接读写、子进程、编码变形不检测；无内核 deny 默认拒绝",
        ),
        passthrough(
            "file.read_white",
            NotSupported,
            "无内核强制层：白名单授权不产生 deny 默认边界，该字段不被读取",
            "任何路径均可读（容器文件系统范围内）",
        ),
        passthrough(
            "file.write_black",
            Approximate,
            "静态预检：命令文本命中黑名单路径即拒绝执行（analyzer::preflight）",
            "仅命令文本级：间接写入、子进程、编码变形不检测；无内核 deny 默认拒绝",
        ),
        passthrough(
            "file.write_white",
            NotSupported,
            "无内核强制层：白名单授权不产生 deny 默认边界，该字段不被读取",
            "任何路径均可写（容器文件系统范围内）",
        ),
        passthrough(
            "net.use_whitelist",
            NotSupported,
            "无内核网络强制与连接监视：方向级开关不被读取",
            "网络不受策略约束（容器网络边界除外）",
        ),
        passthrough(
            "net.in_black",
            Approximate,
            "静态预检：命令文本命中黑名单主机/网段即拒绝执行（analyzer::preflight）",
            "仅命令文本级：运行期建立的连接不监视、不驱逐",
        ),
        passthrough(
            "net.in_white",
            NotSupported,
            "无内核网络强制与连接监视：进站白名单不被读取",
            "网络不受策略约束（容器网络边界除外）",
        ),
        passthrough(
            "net.out_black",
            Approximate,
            "静态预检：命令文本命中黑名单主机/网段即拒绝执行（analyzer::preflight）",
            "仅命令文本级：运行期建立的连接不监视、不驱逐",
        ),
        passthrough(
            "net.out_white",
            NotSupported,
            "无内核网络强制与连接监视：出站白名单不被读取",
            "网络不受策略约束（容器网络边界除外）",
        ),
        passthrough(
            "danger",
            Approximate,
            "静态预检正则（根命令整条匹配，analyzer::preflight），命中拒绝执行",
            "无 ps 轮询：子进程命令行不检测；只对命令文本判定",
        ),
        passthrough(
            "resource.memory_mb",
            NotSupported,
            "Passthrough 会话不设置 rlimits，无内存采样驱逐",
            "进程内存不受 memory_mb 约束（容器 cgroup 限额除外）",
        ),
        passthrough(
            "resource.disk_mb",
            NotSupported,
            "Passthrough 会话不设置 RLIMIT_FSIZE",
            "单文件写入不受 disk_mb 约束",
        ),
        passthrough(
            "resource.cpu_seconds",
            NotSupported,
            "Passthrough 会话不设置 RLIMIT_CPU",
            "CPU 时间不受 cpu_seconds 约束",
        ),
        passthrough(
            "resource.cpu_percent",
            NotSupported,
            "无占空比节流实现",
            "CPU 占比不受 cpu_percent 约束",
        ),
        passthrough(
            "resource.gpu",
            NotSupported,
            "无 GPU 阻断路径",
            "GPU 访问不受 resource.gpu 约束",
        ),
        passthrough(
            "toolchain_disabled",
            NotSupported,
            "无工具链注入机制：该字段不被读取",
            "沙箱不自动注入工具链，禁用清单无效果",
        ),
        passthrough(
            "allow_workspace_exec",
            NotSupported,
            "无 process-exec 内核放行机制：该字段不被读取",
            "可执行性由文件系统权限与容器配置决定",
        ),
        passthrough(
            "exec_white_dirs",
            NotSupported,
            "无 process-exec 内核放行机制：该字段不被读取",
            "目录授权无效果",
        ),
    ]
}

// =====================================================================
// 门禁测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg, ProtectionConfig, ResourceLimits, SandboxInput};

    /// 收集 serde JSON 的叶字段路径（对象递归；数组/标量按叶计）。
    fn leaf_paths(value: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
        let serde_json::Value::Object(map) = value else {
            return;
        };
        for (k, v) in map {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                serde_json::Value::Object(_) => leaf_paths(v, &path, out),
                _ => out.push(path),
            }
        }
    }

    /// 全部字段都给值的配置：让 `skip_serializing_if` 的可选字段也出现在 JSON 里，
    /// 从而 serde 叶字段名可以当作「ProtectionConfig 字段清单」的独立事实源。
    fn full_config() -> ProtectionConfig {
        ProtectionConfig {
            enabled: true,
            file: FilePolicyCfg {
                read_black: vec![],
                read_white: vec![],
                write_black: vec![],
                write_white: vec![],
            },
            net: NetPolicyCfg {
                use_whitelist: true,
                in_black: vec![],
                in_white: vec![],
                out_black: vec![],
                out_white: vec![],
            },
            danger: DangerPolicyCfg {
                black: vec![],
                use_builtin: true,
                builtin_override: Some(vec![]),
            },
            resource: ResourceLimits {
                memory_mb: Some(1),
                disk_mb: Some(1),
                cpu_seconds: Some(1),
                cpu_percent: Some(1),
                gpu: false,
            },
            toolchain_disabled: vec![],
            allow_workspace_exec: false,
            exec_white_dirs: vec![],
        }
    }

    fn report_fields(r: &[FieldEnforcement]) -> Vec<String> {
        r.iter().map(|f| f.field.clone()).collect()
    }

    /// 门禁：报告必须覆盖 ProtectionConfig 的**每个**字段（新增字段漏声明 → 失败）。
    #[test]
    fn 报告覆盖保护配置全部字段() {
        // 1) 独立事实源：serde 叶字段名（可选字段已被 full_config 填值带出）
        let mut leaves = Vec::new();
        leaf_paths(&serde_json::to_value(full_config()).unwrap(), "", &mut leaves);
        assert!(leaves.len() >= 19, "字段清单异常: {leaves:?}");

        // 2) 三个平台的报告都必须逐个覆盖（父对象条目覆盖其子字段，如 danger.* → danger）
        for (platform, report) in [
            ("macos", macos_report()),
            ("windows", windows_report()),
            ("linux", linux_report()),
        ] {
            let fields = report_fields(&report);
            for leaf in &leaves {
                assert!(
                    fields
                        .iter()
                        .any(|f| f == leaf || leaf.starts_with(&format!("{f}."))),
                    "[{platform}] ProtectionConfig 字段 {leaf} 未在 enforcement_report 声明"
                );
            }
            // 反向：报告不得出现游离字段名
            for f in &fields {
                assert!(
                    leaves
                        .iter()
                        .any(|l| l == f || l.starts_with(&format!("{f}."))),
                    "[{platform}] 报告字段 {f} 不在 ProtectionConfig 中"
                );
            }
            assert_eq!(
                fields,
                POLICY_FIELDS.to_vec(),
                "[{platform}] 字段清单/顺序不一致"
            );
        }

        // 3) 真实入口：防护开启 → 当前平台报告（platform_report 分发）
        let mut input = SandboxInput::new("true");
        input.protection = full_config();
        let policy = ResolvedPolicy::compile(&input).unwrap();
        assert_eq!(report_fields(&policy.enforcement_report()), POLICY_FIELDS.to_vec());
    }

    /// Approximate / NotSupported 必须给出偏差说明，机制不得为空（自曝必须可消费）。
    #[test]
    fn 近似与不支持项必须带说明() {
        for (platform, report) in [
            ("macos", macos_report()),
            ("windows", windows_report()),
            ("linux", linux_report()),
        ] {
            assert!(!report.is_empty());
            for f in &report {
                assert!(!f.mechanism.trim().is_empty(), "{platform}/{} 缺机制", f.field);
                if !f.state.is_enforced() {
                    assert!(
                        f.caveat.as_deref().is_some_and(|c| !c.trim().is_empty()),
                        "{platform}/{} 为 {} 却没有 caveat",
                        f.field,
                        f.state.label()
                    );
                }
            }
        }
        // 至少各有一个 NotSupported：如实上报未实现字段是本模块的重点
        assert!(windows_report()
            .iter()
            .any(|f| f.state == EnforcementState::NotSupported));
    }

    /// 防护关闭：全部字段 NotSupported（配置被忽略），一行式输出形状稳定。
    #[test]
    fn 防护关闭时全部字段不生效() {
        let report = ResolvedPolicy::disabled().enforcement_report();
        assert_eq!(report_fields(&report), POLICY_FIELDS.to_vec());
        assert!(report
            .iter()
            .all(|f| f.state == EnforcementState::NotSupported));
        let line = report[0].line();
        assert!(line.starts_with("enabled=NotSupported: "), "行格式: {line}");
        assert!(line.contains("该字段配置被忽略"));
    }

    /// 一行式文本形状：`field=State: mechanism(caveat)`。
    #[test]
    fn 一行式文本形状() {
        let f = FieldEnforcement::new(
            "resource.disk_mb",
            EnforcementState::Approximate,
            "机制",
            Some("偏差"),
        );
        assert_eq!(f.line(), "resource.disk_mb=Approximate: 机制(偏差)");
        assert_eq!(EnforcementState::Approximate.as_str(), "approximate");
        assert_eq!(EnforcementState::NotSupported.as_str(), "not_supported");
        assert_eq!(EnforcementState::Enforced.label(), "Enforced");
    }
}
