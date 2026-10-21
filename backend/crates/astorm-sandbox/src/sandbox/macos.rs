//! macOS 强制层实现（`cfg(target_os = "macos")` 门控；需在真实 macOS 上验证）。
//!
//! 落实 [`SandboxBackend`] 契约：
//! - **文件读写内核强制**：Seatbelt（`/usr/lib/libsandbox.dylib` 的
//!   `sandbox_init_with_parameters`，Apple 私有 API、用户态调用、内核强制）。
//!   策略由平台无关的 [`crate::sandbox::seatbelt_builder`] 编译为 profile；
//!   子进程 fork 后 exec 前调用 `sandbox_init`。
//! - **网络**：Seatbelt 方向级粗粒度（allow/deny network-inbound/outbound）；
//!   主机级由 lsof 驱逐监视兜底（黑 > 白 > 默认拒绝，违规即 killpg）。
//! - **命令动态拦截**：ps 轮询进程树，新后代命令行命中危险命令黑名单立即 SIGKILL。
//! - **超时/进程树终止**：子进程独立进程组，killpg(SIGKILL)。
//!
//! 说明：Seatbelt 为 Apple 私有 API，dlopen 失败时能力上报明确降级（驱逐兜底）。

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::macos_support::{descendants_of, parse_lsof_rows, parse_ps_rows, total_rss_bytes};
use super::seatbelt_builder::{build_profile, SeatbeltContext};
use super::{
    BackendError, Category, ChildSpec, Event, KernelNetMode, Platform, ProcessRef,
    ResourceCapabilities, SandboxBackend, SandboxCapabilities, SandboxSession, WaitOutcome,
};
use crate::policy::{Direction, ResolvedPolicy};

pub struct MacosBackend;

impl MacosBackend {
    pub fn new() -> Self {
        MacosBackend
    }
}

impl Default for MacosBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for MacosBackend {
    fn name(&self) -> &'static str {
        "macos-seatbelt"
    }

    fn capabilities(&self) -> SandboxCapabilities {
        let seatbelt_ok = libsandbox_available();
        let mut degraded = Vec::new();
        if !seatbelt_ok {
            degraded.push(
                "无法加载 /usr/lib/libsandbox.dylib（Seatbelt 私有 API），\
                 文件/网络内核强制不可用，降级为驱逐监视"
                    .to_string(),
            );
        }
        degraded.push(
            "网络主机级白名单在内核层不可表达（需 root 的 PF 规则），Seatbelt 仅方向级；\
             主机级由 lsof 驱逐监视兜底"
                .to_string(),
        );
        degraded.push(
            "Seatbelt 为 Apple 私有 API，规则集需在目标 macOS 版本验证（应用沙箱正当用途）"
                .to_string(),
        );
        SandboxCapabilities {
            platform: Platform::MacOs,
            file_runtime_enforced: seatbelt_ok,
            net_runtime_enforced: KernelNetMode::Coarse,
            command_runtime_intercept: true,
            // 内存/磁盘/CPU 通过 POSIX setrlimit 强制；GPU 通过 Seatbelt 阻断（依赖 seatbelt）
            resource: ResourceCapabilities {
                memory: true,
                disk: true,
                cpu: true,
                gpu: seatbelt_ok,
            },
            degraded_reasons: degraded,
            admin: unsafe { libc::geteuid() == 0 },
        }
    }

    fn init(
        &self,
        policy: &ResolvedPolicy,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Box<dyn SandboxSession>, BackendError> {
        MacosSession::new(Arc::new(policy.clone()), cancel)
            .map(|s| Box::new(s) as Box<dyn SandboxSession>)
    }
}

// =====================================================================
// libsandbox 动态加载
// =====================================================================

type SandboxInitFn = unsafe extern "C" fn(
    profile: *const c_char,
    flags: u64,
    parameters: *const *const c_char,
    errorbuf: *mut *mut c_char,
) -> c_int;

fn libsandbox_available() -> bool {
    unsafe {
        let path = CString::new("/usr/lib/libsandbox.dylib").unwrap();
        let handle = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return false;
        }
        let sym = CString::new("sandbox_init_with_parameters").unwrap();
        let f = libc::dlsym(handle, sym.as_ptr());
        libc::dlclose(handle);
        !f.is_null()
    }
}

fn load_sandbox_init() -> Result<SandboxInitFn, String> {
    unsafe {
        let path = CString::new("/usr/lib/libsandbox.dylib").unwrap();
        let handle = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return Err("dlopen libsandbox 失败".to_string());
        }
        let sym = CString::new("sandbox_init_with_parameters").unwrap();
        let f = libc::dlsym(handle, sym.as_ptr());
        if f.is_null() {
            libc::dlclose(handle);
            return Err("dlsym sandbox_init_with_parameters 失败".to_string());
        }
        // 注意：句柄常驻（函数符号使用期间不能 dlclose）
        Ok(std::mem::transmute::<*mut c_void, SandboxInitFn>(f))
    }
}

// =====================================================================
// 会话
// =====================================================================

struct Shared {
    root_pid: Mutex<i32>,
    tree: Mutex<Vec<i32>>,
    over: AtomicBool,
    exit_code: Mutex<Option<i32>>,
    evicted: AtomicBool,
    stop: AtomicBool,
    policy: Arc<ResolvedPolicy>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// CPU 使用率节流状态：上次采样（进程树累计 CPU 秒, 采样时刻, 是否已发节流说明）
    cpu_sample: Mutex<Option<(f64, std::time::Instant, bool)>>,
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

pub struct MacosSession {
    shared: Arc<Shared>,
    profile_cstring: CString,
    event_tx: Sender<Event>,
    event_rx: Mutex<Option<Receiver<Event>>>,
    session_temp: PathBuf,
    /// 持久 HOME（按工作区；None = 本会话用临时目录当 HOME）
    home: Option<PathBuf>,
    /// 开发工具链环境（DEVELOPER_DIR / SDKROOT；None = 无可用开发目录）
    dev_env: Option<crate::toolchain::DevToolchainEnv>,
    seatbelt_enabled: bool,
}

impl MacosSession {
    fn new(
        policy: Arc<ResolvedPolicy>,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self, BackendError> {
        let err = |m: String| BackendError { message: m };

        // 会话临时目录
        let short = format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64 + d.as_secs())
                .unwrap_or(0)
        );
        let session_temp = std::env::temp_dir().join(format!("astorm-{short}"));
        std::fs::create_dir_all(&session_temp)
            .map_err(|e| err(format!("创建会话临时目录失败：{e}")))?;
        let session_data = session_temp.join("data");
        std::fs::create_dir_all(&session_data)
            .map_err(|e| err(format!("创建会话数据目录失败：{e}")))?;

        // 持久 HOME（工具层按工作区给出的目录）：创建后**命令结束不删除**，
        // cargo/rustup/uv/npm/pip 的缓存与用户级配置因此跨命令复用。
        // 创建失败（权限/磁盘）时退化为逐命令临时 HOME：只是缓存不复用，不阻断执行。
        let persistent_home: Option<PathBuf> = policy.session_home.as_ref().and_then(|p| {
            std::fs::create_dir_all(p)
                .ok()
                .map(|_| p.clone())
        });
        // 工作区内 exec 放行用（见下）
        let session_workspace = policy.workspace.clone();

        // Seatbelt profile（纯函数生成；失败即能力降级）
        let (profile, seatbelt_enabled) = match load_sandbox_init() {
            Ok(_) => {
                let resolver = crate::runtime::RuntimeResolver::detect(true);
                // 动态注入：探测到的用户工具链目录（rustup 真实工具链 / go 官方包 /
                // deno / bun / nvm…）加入 exec 白名单，避免受保护执行必失败 →
                // Auto 无防护二跑。工具进程仍受文件写/网络沙箱约束。
                // Homebrew 经 exec_allow_dirs 整前缀放行（bin 均为符号链接，
                // subpath 按真实路径匹配；python3 等启动器还会二次 exec）。
                // 「本地环境」页禁用的工具（policy.toolchain_disabled）不注入。
                let disabled = &policy.toolchain_disabled;
                let mut program_dirs: Vec<String> = resolver
                    .path_entries()
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                program_dirs.extend(
                    crate::toolchain::exec_allow_dirs(disabled)
                        .iter()
                        .map(|p| p.to_string_lossy().to_string()),
                );
                // 工作区内可执行文件（.venv/bin、node_modules/.bin、cargo 产物、自写脚本）：
                // 默认放行。理由见 ProtectionConfig::allow_workspace_exec 的文档——
                // 解释器本就放行，禁止工作区 exec 只挡正常开发流程；子进程仍受
                // 文件读写与网络沙箱约束（沙箱对整棵进程树生效）。
                if policy.allow_workspace_exec {
                    if let Some(ws) = &session_workspace {
                        // canonicalize：Seatbelt 按解析后路径匹配，工作区可能是符号链接
                        // 或 /var、/tmp 别名（别名形态由 builder 另行展开下发）
                        let real = std::fs::canonicalize(ws).unwrap_or_else(|_| ws.clone());
                        let ws_text = real.to_string_lossy().to_string();
                        if !program_dirs.contains(&ws_text) {
                            program_dirs.push(ws_text);
                        }
                    }
                }
                // 「安全配置 → 额外可执行目录」：用户显式放行的工具链/编译器目录
                // （非常规安装位置——自动探测只覆盖 PATH 上能解析到的工具）。
                let extra_dirs = crate::toolchain::extra_dirs(&policy.exec_white_dirs);
                for d in &extra_dirs.exec {
                    let s = d.to_string_lossy().to_string();
                    if !program_dirs.contains(&s) {
                        program_dirs.push(s);
                    }
                }
                let ctx = SeatbeltContext {
                    program_dirs,
                    // 只放行会话工作区；没有 workspace 的调用方（CLI/测试）退回
                    // 命令默认 cwd（GUI 启动的 cwd 通常是 "/"，但真实会话一定有 workspace）。
                    cwd: session_workspace
                        .clone()
                        .unwrap_or_else(|| {
                            std::env::current_dir().unwrap_or_else(|_| session_temp.clone())
                        })
                        .to_string_lossy()
                        .to_string(),
                    session_temp: session_temp.to_string_lossy().to_string(),
                    session_data: session_data.to_string_lossy().to_string(),
                    home: persistent_home.as_ref().map(|p| p.to_string_lossy().to_string()),
                };
                // 工具链只读放行：exec 只是让进程能启动，dyld 还需读安装树的
                // dylib/资源（Cellar 前缀 / rustup registry 等），否则进程在 dyld
                // 阶段 abort 且错误不含 "not permitted"，stderr 归因失效。
                // 只扩 read_white_raw（Seatbelt 文本专用），判定用 GlobSet 保持原样；
                // 禁用工具（本地环境页开关）同样不注入。
                let mut policy_for_profile = (*policy).clone();
                policy_for_profile
                    .file
                    .read_white_raw
                    .extend(crate::toolchain::read_allow_globs(disabled));
                // 额外可执行目录的只读放行（含父目录，覆盖 bin 旁的 lib/Frameworks）
                policy_for_profile.file.read_white_raw.extend(
                    extra_dirs
                        .read
                        .iter()
                        .map(|d| format!("{}/**", d.to_string_lossy())),
                );
                (build_profile(&policy_for_profile, &ctx), true)
            }
            Err(_) => (String::new(), false),
        };

        let (tx, rx) = channel::<Event>();
        if !seatbelt_enabled {
            let _ = tx.send(Event::Note(
                "Seatbelt 不可用：文件/网络无内核强制，仅预检 + 驱逐监视".to_string(),
            ));
        }

        // 运行期策略：域名规则预解析为 IP（lsof -n 只给 IP），失败以 Note 明示
        let shared_policy: Arc<ResolvedPolicy> = {
            let (net, failed) = policy.net.with_domains_resolved();
            for d in &failed {
                let _ = tx.send(Event::Note(format!(
                    "域名 {d} 解析失败：运行期驱逐监视无法匹配该域名（预检仍按文本生效）"
                )));
            }
            let mut p = (*policy).clone();
            p.net = net;
            Arc::new(p)
        };

        let shared = Arc::new(Shared {
            root_pid: Mutex::new(0),
            tree: Mutex::new(Vec::new()),
            over: AtomicBool::new(false),
            exit_code: Mutex::new(None),
            evicted: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            cpu_sample: Mutex::new(None),
            policy: shared_policy,
            cancel,
        });

        Ok(MacosSession {
            shared,
            profile_cstring: CString::new(profile)
                .map_err(|e| err(format!("profile 编码失败：{e}")))?,
            event_tx: tx,
            event_rx: Mutex::new(Some(rx)),
            session_temp,
            home: persistent_home,
            dev_env: crate::toolchain::dev_toolchain_env(&policy.toolchain_disabled),
            seatbelt_enabled,
        })
    }

    fn kill_tree_internal(&self) {
        let root = *self.shared.root_pid.lock().unwrap();
        if root > 0 {
            unsafe {
                // 整个进程组 SIGKILL
                libc::kill(-root, libc::SIGKILL);
                libc::kill(root, libc::SIGKILL);
            }
        }
    }
}

impl SandboxSession for MacosSession {
    fn spawn(&mut self, spec: ChildSpec) -> Result<ProcessRef, BackendError> {
        let err = |m: String| BackendError { message: m };

        // 预先构造 C 字符串（fork 后仅执行 async-signal-safe 操作）。
        // 用 execve 显式传入自定义环境（PATH 注入 / TMPDIR-HOME 重定向 / 会话注入），
        // 否则 execvp 会使用父进程 environ，导致自定义环境全部失效。
        let program_path = resolve_program_path(&spec.program, &spec.env);
        let program = CString::new(program_path.to_string_lossy().as_ref())
            .map_err(|e| err(format!("程序路径编码失败：{e}")))?;
        let mut argv: Vec<CString> = vec![program.clone()];
        for a in &spec.args {
            argv.push(CString::new(a.as_str()).map_err(|e| err(format!("参数编码失败：{e}")))?);
        }
        let mut argv_ptrs: Vec<*const c_char> = argv.iter().map(|s| s.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        let mut envp_ptrs: Vec<*const c_char> = Vec::new();
        let mut env_cstrings: Vec<CString> = Vec::new();
        for (k, v) in &spec.env {
            if let Ok(s) = CString::new(format!("{k}={v}")) {
                env_cstrings.push(s);
            }
        }
        envp_ptrs.extend(env_cstrings.iter().map(|s| s.as_ptr()));
        envp_ptrs.push(std::ptr::null());

        let cwd = CString::new(spec.cwd.to_string_lossy().as_ref())
            .map_err(|e| err(format!("工作目录编码失败：{e}")))?;

        let stdout_fd = spec.stdout.as_ref().map(|f| f.as_raw_fd());
        let stderr_fd = spec.stderr.as_ref().map(|f| f.as_raw_fd());
        let stdin_fd = spec.stdin.as_ref().map(|f| f.as_raw_fd());

        let profile_ptr = self.profile_cstring.as_ptr();
        let sandbox_init = if self.seatbelt_enabled {
            Some(load_sandbox_init().map_err(&err)?)
        } else {
            None
        };

        // 资源限制（fork 前取值，fork 后子进程仅做 setrlimit，避免分配）
        let res = &self.shared.policy.resource;
        let mem = res.memory_mb;
        let disk = res.disk_mb;
        let cpu = res.cpu_seconds;

        // fork → 子进程：沙箱 → stdio → chdir → exec
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(err("fork 失败".to_string()));
        }
        if pid == 0 {
            unsafe {
                // 独立进程组
                libc::setpgid(0, 0);
                // 资源限制：内存(RLIMIT_AS) / 磁盘写入(RLIMIT_FSIZE) / CPU(RLIMIT_CPU)
                apply_rlimits(mem, disk, cpu);
                // Seatbelt（内核强制托管：文件读写 / 网络 / 执行 / GPU）
                if let Some(init) = sandbox_init {
                    let mut errorbuf: *mut c_char = std::ptr::null_mut();
                    if init(profile_ptr, 0, std::ptr::null(), &mut errorbuf) != 0 {
                        libc::_exit(126);
                    }
                }
                if let Some(fd) = stdin_fd {
                    libc::dup2(fd, 0);
                }
                if let Some(fd) = stdout_fd {
                    libc::dup2(fd, 1);
                }
                if let Some(fd) = stderr_fd {
                    libc::dup2(fd, 2);
                }
                libc::chdir(cwd.as_ptr());
                libc::execve(program.as_ptr(), argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
                libc::_exit(127);
            }
        }

        // 父进程：确保进程组成立
        unsafe {
            libc::setpgid(pid, pid);
        }
        *self.shared.root_pid.lock().unwrap() = pid;
        self.shared.tree.lock().unwrap().push(pid);

        // 监视线程：ps 轮询进程树（危险命令）+ lsof 轮询（网络驱逐）
        {
            let shared = self.shared.clone();
            let tx = self.event_tx.clone();
            std::thread::Builder::new()
                .name("astorm-macos-watch".to_string())
                .spawn(move || watch_loop(shared, tx))
                .expect("创建监视线程失败");
        }

        Ok(ProcessRef {
            pid: pid as u32,
            trace_id: 1,
        })
    }

    fn event_receiver(&mut self) -> Receiver<Event> {
        self.event_rx
            .lock()
            .unwrap()
            .take()
            .expect("event_receiver 只能调用一次")
    }

    fn extra_env(&self) -> Vec<(String, String)> {
        // HOME：按工作区持久（跨命令复用缓存/配置）；未启用持久 HOME 时退回临时目录。
        // TMPDIR：始终逐命令临时（命令结束即删），临时文件不会长期堆积。
        let home = self
            .home
            .as_ref()
            .unwrap_or(&self.session_temp)
            .to_string_lossy()
            .to_string();
        let mut env = vec![
            (
                "TMPDIR".to_string(),
                self.session_temp.to_string_lossy().to_string(),
            ),
            ("HOME".to_string(), home),
        ];
        // 开发工具链环境：不注入则原生编译在沙箱内走不通（Xcode 的 xcodebuild 依赖
        // SIP 受限框架，`cc` shim 必失败）。详见 toolchain::dev_toolchain_env 的说明。
        if let Some(dev) = &self.dev_env {
            env.push((
                "DEVELOPER_DIR".to_string(),
                dev.developer_dir.to_string_lossy().to_string(),
            ));
            if let Some(sdk) = &dev.sdk_root {
                env.push(("SDKROOT".to_string(), sdk.to_string_lossy().to_string()));
            }
        }
        env
    }

    fn wait(&mut self, timeout: Duration) -> Result<WaitOutcome, BackendError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(cancel) = &self.shared.cancel {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(WaitOutcome::Cancelled);
                }
            }
            if self.shared.over.load(Ordering::SeqCst) {
                let code = *self.shared.exit_code.lock().unwrap();
                return Ok(WaitOutcome::Exited {
                    code: code.unwrap_or(-1),
                });
            }
            if self.shared.evicted.load(Ordering::SeqCst) {
                return Ok(WaitOutcome::Evicted);
            }
            // 尝试非阻塞 waitpid
            let root = *self.shared.root_pid.lock().unwrap();
            if root > 0 {
                let mut status: c_int = 0;
                let r = unsafe { libc::waitpid(root, &mut status, libc::WNOHANG) };
                if r == root {
                    let code = exit_code_of(status);
                    *self.shared.exit_code.lock().unwrap() = Some(code);
                    self.shared.over.store(true, Ordering::SeqCst);
                    return Ok(WaitOutcome::Exited { code });
                }
            }
            if Instant::now() >= deadline {
                return Ok(WaitOutcome::Timeout);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill_tree(&mut self) -> Result<(), BackendError> {
        self.kill_tree_internal();
        Ok(())
    }

    fn teardown(self: Box<Self>) -> Result<(), BackendError> {
        self.shared.stop.store(true, Ordering::SeqCst);
        if !self.shared.over.load(Ordering::SeqCst) {
            self.kill_tree_internal();
        }
        let _ = std::fs::remove_dir_all(&self.session_temp);
        Ok(())
    }
}

impl Drop for MacosSession {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if !self.shared.over.load(Ordering::SeqCst) {
            self.kill_tree_internal();
        }
    }
}

/// 应用资源限制（fork 后子进程调用；只做 libc 调用，不分配）。
///
/// - 磁盘写入：`RLIMIT_FSIZE` 单文件写入上限（MB），超限触发 SIGXFSZ
/// - CPU：`RLIMIT_CPU` CPU 时间上限（秒），超限触发 SIGXCPU 终止
/// - 内存：macOS 的 RLIMIT_AS/RLIMIT_DATA 不可设置（EINVAL），改由监视循环按 RSS 驱逐
fn apply_rlimits(_memory_mb: Option<u64>, disk_mb: Option<u64>, cpu_seconds: Option<u64>) {
    unsafe {
        if let Some(mb) = disk_mb {
            let bytes = mb.saturating_mul(1024 * 1024);
            let lim = libc::rlimit {
                rlim_cur: bytes,
                rlim_max: bytes,
            };
            libc::setrlimit(libc::RLIMIT_FSIZE, &lim);
        }
        if let Some(secs) = cpu_seconds {
            let lim = libc::rlimit {
                rlim_cur: secs,
                rlim_max: secs,
            };
            libc::setrlimit(libc::RLIMIT_CPU, &lim);
        }
    }
}

/// 解析要执行的程序路径：裸命令名按自定义 PATH（含运行时注入）搜索，否则原样返回。
///
/// `execve` 不做 PATH 搜索，而环境（PATH 注入等）由核心在 `spec.env` 中给出，
/// 因此必须在父进程侧解析成绝对路径（fork 后仅执行 async-signal-safe 操作）。
fn resolve_program_path(program: &std::path::Path, env: &[(String, String)]) -> PathBuf {
    let s = program.to_string_lossy();
    if s.contains('/') {
        return program.to_path_buf();
    }
    let path = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let cand = PathBuf::from(dir).join(s.as_ref());
        if cand.is_file() {
            return cand;
        }
    }
    program.to_path_buf()
}

fn exit_code_of(status: c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        -1
    }
}

// =====================================================================
// 监视循环（ps 进程树 + lsof 网络）
// =====================================================================

fn watch_loop(shared: Arc<Shared>, tx: Sender<Event>) {
    // 设置了内存限制时加快轮询（RSS 监视为逐周期采样，粒度受轮询间隔影响）
    let cycles = if shared.policy.resource.memory_mb.is_some() {
        1
    } else {
        4
    };
    // CPU 使用率上限是「整机占比」（与 Windows Job 的 CpuRate 同口径），而 ps 的 cputime
    // 是各核累加，故换算成核数倍率当阈值：整机 pct% ≡ pct% × 核数（单位均为「核×100」）。
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1) as f64;
    loop {
        if shared.stop.load(Ordering::SeqCst) || shared.over.load(Ordering::SeqCst) {
            break;
        }
        for _ in 0..cycles {
            if shared.stop.load(Ordering::SeqCst) || shared.over.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let root = *shared.root_pid.lock().unwrap();
        if root <= 0 {
            continue;
        }

        // ---- 进程树：危险命令动态拦截 + 内存监视 ----
        let ps_out = std::process::Command::new("ps")
            .args(["-axwo", "pid=,ppid=,rss=,cputime=,command="])
            .stdin(std::process::Stdio::null())
            .output();
        if let Ok(o) = ps_out {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let rows = parse_ps_rows(&text);
            let tree = descendants_of(&rows, root);

            // 内存上限（RSS 监视，非 setrlimit；macOS 的 RLIMIT_AS/DATA 不可设）
            if let Some(mem_mb) = shared.policy.resource.memory_mb {
                let total = total_rss_bytes(&rows, root);
                let limit = mem_mb.saturating_mul(1024 * 1024);
                if total > limit {
                    let _ = tx.send(Event::Evicted {
                        category: Category::Resource,
                        reason: format!(
                            "进程树内存 {total} 字节超过上限 {limit} 字节，已终止进程树"
                        ),
                    });
                    shared.evicted.store(true, Ordering::SeqCst);
                    let root_pid = *shared.root_pid.lock().unwrap();
                    unsafe {
                        libc::kill(-root_pid, libc::SIGKILL);
                    }
                    return;
                }
            }

            // CPU 使用率节流（cputime 差分 + STOP/CONT 占空比；macOS 无百分比配额原语，
            // 近似限流 ±10%）。STOP 期间 cputime 自然停走；SIGKILL 驱逐对 STOP 进程仍有效。
            // 单位换算：usage 为「核×100」（1 核跑满 = 100），上限按整机占比换成同单位。
            if let Some(pct) = shared.policy.resource.cpu_percent {
                let total = crate::sandbox::macos_support::total_cputime_secs(&rows, root);
                let now = std::time::Instant::now();
                let mut sample = shared.cpu_sample.lock().unwrap();
                if let Some((prev_total, prev_at, notified)) = *sample {
                    let elapsed = now.duration_since(prev_at).as_secs_f64();
                    if elapsed >= 0.4 {
                        let used = (total - prev_total).max(0.0);
                        let usage = used / elapsed * 100.0;
                        let limit = pct as f64 * cpu_cores;
                        // 超 10% 容差才节流（防抖）
                        if usage > limit * 1.1 {
                            let stop_ms = ((usage - limit) / usage * elapsed * 1000.0) as u64;
                            unsafe {
                                libc::kill(-root, libc::SIGSTOP);
                            }
                            std::thread::sleep(Duration::from_millis(stop_ms.clamp(20, 2000)));
                            unsafe {
                                libc::kill(-root, libc::SIGCONT);
                            }
                            if !notified {
                                let _ = tx.send(Event::Note(format!(
                                    "进程树 CPU 使用率约 {:.0}%，超过整机上限 {pct}%，已开始按占空比节流",
                                    usage / cpu_cores
                                )));
                            }
                            // 节流睡眠后重置采样基准（含 sleep 时长，保持占空比一致）
                            *sample = Some((total, std::time::Instant::now(), true));
                            drop(sample);
                            // 跳过本轮网络检查（节流周期内）
                            continue;
                        }
                    }
                    *sample = Some((total, now, notified));
                } else {
                    *sample = Some((total, now, false));
                }
            }

            {
                let mut known = shared.tree.lock().unwrap();
                for pid in &tree {
                    if !known.contains(pid) {
                        // 新后代：读命令行做危险命令判定
                        if let Some(row) = rows.iter().find(|r| r.pid == *pid) {
                            if *pid != root {
                                let hits = shared.policy.danger.matches(&row.command);
                                if !hits.is_empty() {
                                    let _ = tx.send(Event::Denied {
                                        category: Category::DangerCommand,
                                        detail: format!(
                                            "子进程 {pid} 命令行「{}」命中危险命令黑名单，已实时终止",
                                            row.command
                                        ),
                                    });
                                    unsafe {
                                        libc::kill(*pid, libc::SIGKILL);
                                    }
                                }
                            }
                            let _ = tx.send(Event::Spawn {
                                pid: *pid as u32,
                                argv: row.command.clone(),
                            });
                        }
                        known.push(*pid);
                    }
                }
            }
        }

        // ---- 网络：lsof 驱逐 ----
        let lsof_out = std::process::Command::new("lsof")
            .args(["-nP", "-i"])
            .stdin(std::process::Stdio::null())
            .output();
        if let Ok(o) = lsof_out {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let tree = shared.tree.lock().unwrap().clone();
            for net in parse_lsof_rows(&text) {
                if !tree.contains(&net.pid) {
                    continue;
                }
                let decision =
                    crate::decision::decide_net(&shared.policy.net, net.direction, &net.addr);
                if let crate::decision::Decision::Deny(reason) = decision {
                    let dir_text = if net.direction == Direction::Outbound {
                        "出站"
                    } else {
                        "进站"
                    };
                    let _ = tx.send(Event::Evicted {
                        category: Category::from_direction(net.direction),
                        reason: format!(
                            "进程 {} {dir_text}连接违规：{reason}，已终止进程树",
                            net.pid
                        ),
                    });
                    shared.evicted.store(true, Ordering::SeqCst);
                    let root_pid = *shared.root_pid.lock().unwrap();
                    unsafe {
                        libc::kill(-root_pid, libc::SIGKILL);
                    }
                    return;
                }
            }
        }
    }
}
