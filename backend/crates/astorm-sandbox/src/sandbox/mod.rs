//! 跨平台抽象层：可移植核心与平台强制层（Windows / macOS；Linux 暂走 Passthrough）之间的契约。
//!
//! 设计要点：
//! - 可移植核心（策略 / 预检 / 执行编排 / 事件流 / CLI）只依赖本模块定义的类型，
//!   不接触任何 Win32 / Darwin API。
//! - 平台差异收敛在 [`SandboxBackend`]（能力探测 + 会话工厂）与
//!   [`SandboxSession`]（一次执行的生命周期）两个 trait 之后。
//! - 平台层通过 [`SandboxCapabilities`] 如实上报"实际强制等级"；能力不足时由
//!   核心决定：`strict` 模式直接失败，非 strict 模式明确告警后继续（绝不静默降级）。

use std::fs::File;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use serde::Serialize;

use crate::policy::{AccessKind, Direction, ResolvedPolicy};

/// 输出缓冲上限（超过即截断并给出事件）。
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// 平台标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Platform {
    #[serde(rename = "windows")]
    Windows,
    #[serde(rename = "macos")]
    MacOs,
    /// Linux：暂无内核强制层，default_backend 返回 Passthrough（诚实上报缺失，
    /// 隔离由容器/宿主边界负责；后续可选补 bubblewrap/Landlock 后端）。
    #[serde(rename = "linux")]
    Linux,
}

/// 网络内核强制模式的粒度（诚实上报实际能力）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum KernelNetMode {
    /// 按规则的完整内核强制（Windows：WFP 防火墙按沙箱 SID + 白名单规则；黑>白>默认拒绝）
    #[serde(rename = "full_per_rule")]
    FullPerRule,
    /// 粗粒度能力开关（例如 AppContainer 网络能力整体开关；无法精确到主机）
    #[serde(rename = "coarse")]
    Coarse,
    /// 仅驱逐监视（轮询连接发现违规后终止进程树；仍属实时阻断结果，但为事件后处置）
    #[serde(rename = "eviction_only")]
    EvictionOnly,
}

/// 资源限制的强制能力（诚实上报：实际可强制哪些维度）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ResourceCapabilities {
    /// 内存上限是否可强制
    pub memory: bool,
    /// 磁盘写入上限是否可强制
    pub disk: bool,
    /// CPU 时间上限是否可强制
    pub cpu: bool,
    /// GPU 访问禁用是否可强制
    pub gpu: bool,
}

impl ResourceCapabilities {
    pub fn none() -> Self {
        ResourceCapabilities {
            memory: false,
            disk: false,
            cpu: false,
            gpu: false,
        }
    }
}

/// 平台强制层的真实能力描述。
#[derive(Debug, Clone, Serialize)]
pub struct SandboxCapabilities {
    pub platform: Platform,
    /// 文件读写是否由内核级强制（Windows：AppContainer DACL；macOS：Seatbelt）
    pub file_runtime_enforced: bool,
    /// 网络进出站强制粒度
    pub net_runtime_enforced: KernelNetMode,
    /// 命令是否具备运行期动态拦截
    pub command_runtime_intercept: bool,
    /// 资源限制强制能力（内存/磁盘/CPU/GPU）
    pub resource: ResourceCapabilities,
    /// 未启用项的明确原因（中文），展示给用户
    pub degraded_reasons: Vec<String>,
    /// 是否以管理员/更高权限运行（影响防火墙规则、系统目录 ACL 等）
    pub admin: bool,
}

impl SandboxCapabilities {
    /// 严格模式要求：文件内核强制 + 命令运行期拦截 + 网络全规则强制。
    pub fn ok_for_strict(&self) -> bool {
        self.file_runtime_enforced
            && self.command_runtime_intercept
            && self.net_runtime_enforced == KernelNetMode::FullPerRule
    }

    /// 简要描述文本。
    pub fn summary(&self) -> String {
        let net = match self.net_runtime_enforced {
            KernelNetMode::FullPerRule => "按规则内核强制",
            KernelNetMode::Coarse => "粗粒度能力开关",
            KernelNetMode::EvictionOnly => "驱逐监视",
        };
        format!(
            "平台={} 文件内核强制={} 网络={} 命令动态拦截={} 资源(内存={} 磁盘={} CPU={} GPU={}) 管理员={}",
            match self.platform {
                Platform::Windows => "Windows",
                Platform::MacOs => "macOS",
                Platform::Linux => "Linux",
            },
            if self.file_runtime_enforced { "开" } else { "关" },
            net,
            if self.command_runtime_intercept { "开" } else { "关" },
            if self.resource.memory { "可限" } else { "无" },
            if self.resource.disk { "可限" } else { "无" },
            if self.resource.cpu { "可限" } else { "无" },
            if self.resource.gpu { "可禁" } else { "无" },
            if self.admin { "是" } else { "否" },
        )
    }
}

/// 强制层初始化失败。
#[derive(Debug, Clone)]
pub struct BackendError {
    pub message: String,
}

impl BackendError {
    pub(crate) fn from_message(message: String) -> Self {
        BackendError { message }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BackendError {}

/// 可移植的启动描述：核心算好"用什么程序、什么参数、在哪个目录、什么环境、标准流接到哪"，
/// 平台强制层只负责在沙箱内落实。
#[derive(Debug)]
pub struct ChildSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// 完整环境（键值，由核心从当前环境构建并叠加 PATH 注入等）
    pub env: Vec<(String, String)>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
    pub stdin: Option<File>,
    /// true = 最后一个参数是"命令文本"（cmd.exe /C 场景），平台层不得对它再加引号，
    /// 否则 cmd 的引号剥离规则会产生伪引号（Windows 专用语义）
    pub raw_command_arg: bool,
}

/// 平台无关的进程引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessRef {
    pub pid: u32,
    /// 会话内的唯一跟踪号
    pub trace_id: u64,
}

/// 等待结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// 正常结束，带退出码
    Exited { code: i32 },
    /// 超时（需调用方 kill_tree）
    Timeout,
    /// 因运行时驱逐（网络/命令违规）被强制终止
    Evicted,
    /// 外部取消信号（需调用方 kill_tree）
    Cancelled,
}

/// 输出流类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StreamKind {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
}

/// 违规类别。
///
/// 平台/策略内部枚举：上层（astorm-core）映射为面向应用的拒绝分类——
/// 本枚举覆盖其中 5 个（`net_in`/`net_out` 合并为「网络」）；
/// 第 6 个 `browser` 来自 browser 工具自身策略，不经沙箱，故不在此枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Category {
    #[serde(rename = "file_write")]
    FileWrite,
    #[serde(rename = "file_read")]
    FileRead,
    #[serde(rename = "net_in")]
    NetIn,
    #[serde(rename = "net_out")]
    NetOut,
    #[serde(rename = "danger_command")]
    DangerCommand,
    /// 资源限制（内存 / 磁盘 / CPU 时间 / 超时）
    #[serde(rename = "resource")]
    Resource,
}

impl Category {
    pub fn from_access(kind: AccessKind) -> Category {
        match kind {
            AccessKind::Read => Category::FileRead,
            AccessKind::Write => Category::FileWrite,
        }
    }
    pub fn from_direction(dir: Direction) -> Category {
        match dir {
            Direction::Inbound => Category::NetIn,
            Direction::Outbound => Category::NetOut,
        }
    }
    /// 是否网络类违规（进/出站合并判定）。
    pub fn is_network(&self) -> bool {
        matches!(self, Category::NetIn | Category::NetOut)
    }
    /// 中文名（用于人类可读输出）。
    pub fn zh(&self) -> &'static str {
        match self {
            Category::FileWrite => "文件写",
            Category::FileRead => "文件读",
            Category::NetIn => "网络进站",
            Category::NetOut => "网络出站",
            Category::DangerCommand => "危险命令",
            Category::Resource => "资源限制",
        }
    }
}

/// 运行期事件（同步模式进入 report.events；异步模式作为日志流逐条发出）。
#[derive(Debug, Clone, Serialize)]
pub enum Event {
    /// 能力上报（每次执行的第一条事件）
    Capabilities(SandboxCapabilities),
    /// 说明性消息（降级原因、边界提示等）
    Note(String),
    /// 输出增量
    Chunk { stream: StreamKind, data: String },
    /// 进程产生（调试器/监视器捕获）
    Spawn { pid: u32, argv: String },
    /// 违规被拦截
    Denied { category: Category, detail: String },
    /// 运行时驱逐（发现违规连接/命令/资源超限后终止进程树）
    Evicted { category: Category, reason: String },
    /// 因超时强制终止
    TimeoutKilled,
    /// 输出达到上限被截断
    OutputTruncated,
    /// 进程退出
    Exit { pid: u32, code: i32 },
    /// 执行完成（异步流末尾发送）
    Finished,
    /// 执行层错误
    Error(String),
}

/// 后端：能力探测 + 会话工厂。
pub trait SandboxBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> SandboxCapabilities;
    fn init(
        &self,
        policy: &ResolvedPolicy,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Box<dyn SandboxSession>, BackendError>;
}

/// 会话：一次沙箱执行的生命周期。
pub trait SandboxSession: Send {
    fn spawn(&mut self, spec: ChildSpec) -> Result<ProcessRef, BackendError>;
    /// 事件接收端（内部线程投递 spawn/denied/exit/evicted 等）
    fn event_receiver(&mut self) -> Receiver<Event>;
    /// 等待结束；超时返回 Timeout（调用方负责 kill_tree）
    fn wait(&mut self, timeout: Duration) -> Result<WaitOutcome, BackendError>;
    fn kill_tree(&mut self) -> Result<(), BackendError>;
    /// 会话级环境注入（如沙箱临时目录 TEMP/TMP、容器数据目录等），默认无
    fn extra_env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// 回收平台对象（防火墙规则、ACL、句柄等；Drop 兜底）
    fn teardown(self: Box<Self>) -> Result<(), BackendError>;
}

/// 当前平台的默认后端（cfg 分发）。
pub fn default_backend() -> Box<dyn SandboxBackend> {
    #[cfg(windows)]
    {
        Box::new(crate::sandbox::windows::WindowsBackend::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(crate::sandbox::macos::MacosBackend::new())
    }
    #[cfg(target_os = "linux")]
    {
        // Linux 暂无内核强制层：直通执行并诚实上报缺失（隔离由容器/宿主边界负责）。
        // strict 模式下 runner 会因 capabilities 不满足 ok_for_strict() 而明确失败，
        // 绝不静默降级；后续补 bubblewrap/Landlock 后端时替换此处。
        Box::new(PassthroughBackend::new(
            "Linux 平台暂无内核强制层（依赖容器/宿主边界隔离），仅保留超时、输出流与进程树终止",
        ))
    }
}

/// 防护关闭（或强制层不可用且非严格模式）时使用的直通执行后端。
pub struct PassthroughBackend {
    capabilities: SandboxCapabilities,
}

impl PassthroughBackend {
    pub fn new(reason: &str) -> Self {
        let mut degraded = vec![reason.to_string()];
        degraded.push("当前为直通执行：无内核强制，仅保留超时与输出流".to_string());
        PassthroughBackend {
            capabilities: SandboxCapabilities {
                platform: current_platform(),
                file_runtime_enforced: false,
                net_runtime_enforced: KernelNetMode::EvictionOnly,
                command_runtime_intercept: false,
                resource: ResourceCapabilities::none(),
                degraded_reasons: degraded,
                admin: is_admin(),
            },
        }
    }
}

impl SandboxBackend for PassthroughBackend {
    fn name(&self) -> &'static str {
        "passthrough"
    }
    fn capabilities(&self) -> SandboxCapabilities {
        self.capabilities.clone()
    }
    fn init(
        &self,
        _policy: &ResolvedPolicy,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Box<dyn SandboxSession>, BackendError> {
        Ok(Box::new(PassthroughSession::new(cancel)))
    }
}

fn current_platform() -> Platform {
    #[cfg(windows)]
    {
        Platform::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Platform::MacOs
    }
    #[cfg(target_os = "linux")]
    {
        Platform::Linux
    }
}

fn is_admin() -> bool {
    #[cfg(windows)]
    {
        crate::sandbox::windows::is_admin()
    }
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
}

/// 直通会话：标准进程执行（防护关闭时使用），保留超时、输出流与进程树终止。
pub struct PassthroughSession {
    child: Option<std::process::Child>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl PassthroughSession {
    pub fn new(cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> Self {
        PassthroughSession { child: None, cancel }
    }
}

impl SandboxSession for PassthroughSession {
    fn spawn(&mut self, spec: ChildSpec) -> Result<ProcessRef, BackendError> {
        let mut cmd = std::process::Command::new(&spec.program);
        // cmd.exe /C 的命令文本必须原样传递（std 的自动加引号会触发 cmd 的
        // "保留首引号、丢弃尾引号" 规则，产生伪引号输出）
        #[cfg(windows)]
        let raw_last = spec.raw_command_arg && !spec.args.is_empty();
        #[cfg(not(windows))]
        let raw_last = false;
        if raw_last {
            cmd.args(&spec.args[..spec.args.len() - 1]);
        } else {
            cmd.args(&spec.args);
        }
        #[cfg(windows)]
        if raw_last {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(spec.args.last().unwrap().clone());
        }
        // GUI 进程(Electron)拉起控制台程序会闪 cmd 窗口,必须静默
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(crate::CREATE_NO_WINDOW);
        }
        cmd.current_dir(&spec.cwd)
            .env_clear()
            .envs(spec.env.iter().cloned());
        if let Some(f) = spec.stdout {
            cmd.stdout(std::process::Stdio::from(f));
        } else {
            cmd.stdout(std::process::Stdio::null());
        }
        if let Some(f) = spec.stderr {
            cmd.stderr(std::process::Stdio::from(f));
        } else {
            cmd.stderr(std::process::Stdio::null());
        }
        if let Some(f) = spec.stdin {
            cmd.stdin(std::process::Stdio::from(f));
        } else {
            cmd.stdin(std::process::Stdio::null());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // 子进程自成一进程组，便于 killpg 整树终止
            cmd.process_group(0);
        }
        let child = cmd.spawn().map_err(|e| BackendError {
            message: format!("进程启动失败：{e}"),
        })?;
        let pid = child.id();
        self.child = Some(child);
        Ok(ProcessRef { pid, trace_id: 0 })
    }

    fn event_receiver(&mut self) -> Receiver<Event> {
        // 直通会话内部不产生事件；返回一个立即关闭的通道
        let (tx, rx) = std::sync::mpsc::channel::<Event>();
        drop(tx);
        rx
    }

    fn wait(&mut self, timeout: Duration) -> Result<WaitOutcome, BackendError> {
        let child = self.child.as_mut().ok_or_else(|| BackendError {
            message: "尚未启动进程".to_string(),
        })?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(cancel) = &self.cancel {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(WaitOutcome::Cancelled);
                }
            }
            if let Some(status) = child.try_wait().map_err(|e| BackendError {
                message: format!("等待进程失败：{e}"),
            })? {
                let code = status.code().unwrap_or(-1);
                return Ok(WaitOutcome::Exited { code });
            }
            if std::time::Instant::now() >= deadline {
                return Ok(WaitOutcome::Timeout);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill_tree(&mut self) -> Result<(), BackendError> {
        if let Some(child) = self.child.as_mut() {
            let pid = child.id();
            // 先 taskkill /T /F 整树（趁进程树还活着），再杀直接子进程
            let _ = kill_tree_external(pid);
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    fn teardown(self: Box<Self>) -> Result<(), BackendError> {
        Ok(())
    }
}

impl Default for PassthroughSession {
    fn default() -> Self {
        Self::new(None)
    }
}

/// 独立于会话的进程树终止辅助（taskkill / killpg 兜底）。
pub(crate) fn kill_tree_external(pid: u32) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(crate::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        Ok(())
    }
    #[cfg(unix)]
    {
        // 子进程自成一进程组（process_group(0)），组 id == 进程 id
        let pgid = pid as i32;
        let _ = unsafe { libc::kill(-pgid, libc::SIGKILL) };
        Ok(())
    }
}

#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

/// macOS 监视解析器（ps / lsof；平台无关纯函数，Windows 上也可单测）。
pub mod macos_support;

/// macOS Seatbelt profile 生成器（平台无关纯函数，Windows 上也可单测；macOS 运行层使用）。
pub mod seatbelt_builder;

/// 静态预检结果（供 runner 汇报"被预检拦截"的事件）。
#[derive(Debug, Clone)]
pub struct PreflightBlock {
    pub denies: Vec<String>,
}

/// 控制台输出切换为 UTF-8（仅 Windows；中文默认 GBK，避免 CLI 输出乱码）。
#[cfg(windows)]
pub fn enable_utf8_console() {
    windows::ffi::enable_utf8_console();
}

/// 非 Windows 平台的空实现（保持调用方代码一致）。
#[cfg(not(windows))]
pub fn enable_utf8_console() {}
