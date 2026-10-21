//! Windows 强制层实现。
//!
//! 落实 [`SandboxBackend`] 契约：
//! - **文件读写内核强制**：AppContainer 令牌（策略 → DACL：白名单 Grant ACE、
//!   黑名单 Deny ACE，Deny 优先于 Allow 天然满足"黑名单 > 白名单"；
//!   未授权路径由内核直接拒绝——默认拒绝）。
//! - **网络**：管理员下通过 Windows 防火墙（WFP）按沙箱 SID 下发默认拒绝 + 白名单规则；
//!   非管理员降级为 netstat 驱逐监视（发现违规连接立即终止进程树，仍属实时阻断结果，
//!   能力上报中明确说明）。
//! - **命令动态拦截**：以 DEBUG_PROCESS 创建进程树，调试循环实时接收每个子进程的
//!   CREATE_PROCESS 事件并读取其 PEB 命令行，命中危险命令黑名单立即 TerminateProcess。
//! - **超时/进程树终止**：Job Object（KILL_ON_JOB_CLOSE）+ 全树句柄 Terminate +
//!   taskkill /T /F 兜底。

use std::collections::HashMap;
use std::os::raw::c_void;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    BackendError, Category, ChildSpec, Event, KernelNetMode, Platform, ProcessRef,
    ResourceCapabilities, SandboxBackend, SandboxCapabilities, SandboxSession, WaitOutcome,
};
use crate::policy::{Direction, NetAddr, ResolvedPolicy};

pub(crate) mod acl;
pub(crate) mod ffi;

use ffi as f;

// =====================================================================
// 后端
// =====================================================================

pub struct WindowsBackend;

impl WindowsBackend {
    pub fn new() -> Self {
        WindowsBackend
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows-appcontainer"
    }

    fn capabilities(&self) -> SandboxCapabilities {
        let admin = is_admin();
        let mut degraded = Vec::new();
        let net_mode;
        if admin {
            net_mode = KernelNetMode::FullPerRule;
        } else {
            net_mode = KernelNetMode::EvictionOnly;
            degraded.push(
                "未以管理员运行：Windows 防火墙（WFP）按沙箱 SID 的规则不可用，\
                 网络防护为「驱逐监视」（发现违规连接立即终止进程树）；\
                 系统目录的读黑名单 ACL 也无法设置"
                    .to_string(),
            );
        }
        degraded.push(
            "文件防护边界：AppContainer 默认禁止写入任何未授权路径（含系统区），\
             但系统区（C:\\Windows 等）默认可读——读黑名单在系统目录需管理员 ACL 才能精确阻断"
                .to_string(),
        );
        degraded.push(
            "资源限制边界：内存/CPU 通过 Job 对象强制；磁盘写入与 GPU 禁用暂未在 Windows 强制"
                .to_string(),
        );
        SandboxCapabilities {
            platform: Platform::Windows,
            file_runtime_enforced: true,
            net_runtime_enforced: net_mode,
            command_runtime_intercept: true,
            resource: ResourceCapabilities {
                memory: true,
                disk: false,
                cpu: true,
                gpu: false,
            },
            degraded_reasons: degraded,
            admin,
        }
    }

    fn init(
        &self,
        policy: &ResolvedPolicy,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Box<dyn SandboxSession>, BackendError> {
        let session = WindowsSession::new(Arc::new(policy.clone()), cancel)?;
        Ok(Box::new(session))
    }
}

// =====================================================================
// 管理员探测
// =====================================================================

pub fn is_admin() -> bool {
    unsafe {
        let mut token: f::HANDLE = std::ptr::null_mut();
        if f::OpenProcessToken(f::GetCurrentProcess(), f::TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elev = f::TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret: f::DWORD = 0;
        let ok = f::GetTokenInformation(
            token,
            f::TokenElevation,
            &mut elev as *mut _ as *mut c_void,
            std::mem::size_of::<f::TOKEN_ELEVATION>() as f::DWORD,
            &mut ret,
        );
        f::CloseHandle(token);
        ok != 0 && elev.TokenIsElevated != 0
    }
}

// =====================================================================
// 会话共享状态
// =====================================================================

struct Shared {
    /// 调试循环跟踪的 pid → 进程句柄
    handles: Mutex<HashMap<u32, f::HANDLE>>,
    root_pid: AtomicU32,
    over: AtomicBool,
    exit_code: Mutex<Option<i32>>,
    evicted: AtomicBool,
    stop_net: AtomicBool,
    job: Mutex<f::HANDLE>,
    policy: Arc<ResolvedPolicy>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

// Shared 持有内核句柄，仅通过 FFI 使用且受锁保护；跨线程移动是安全的。
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// 调试线程的启动参数（含内核句柄值；仅经 FFI 使用，跨线程移动安全）。
struct SpawnJob {
    token: f::HANDLE,
    job: f::HANDLE,
    h_stdin: f::HANDLE,
    h_stdout: f::HANDLE,
    h_stderr: f::HANDLE,
    /// `spec.stdin` 为空时打开的 NUL 句柄：STARTF_USESTDHANDLES 下 NULL stdin
    /// 会被子进程判为无效句柄（控制台程序可能直接卡住/退出）。
    _stdin_null: Option<std::fs::File>,
    cmdline_w: Vec<u16>,
    cwd_w: Vec<u16>,
    env_block: Vec<u16>,
    shared: Arc<Shared>,
    tx: Sender<Event>,
    pid_tx: Sender<Result<u32, String>>,
}
unsafe impl Send for SpawnJob {}

impl Shared {
    fn kill_all(&self) {
        // 1) Job Object 整树终止
        {
            let job = *self.job.lock().unwrap();
            if !job.is_null() {
                unsafe {
                    f::TerminateJobObject(job, 1);
                }
            }
        }
        // 2) 已跟踪句柄逐个终止
        let handles = self.handles.lock().unwrap();
        for (_, h) in handles.iter() {
            unsafe {
                f::TerminateProcess(*h, 1);
            }
        }
        drop(handles);
        // 3) taskkill /T /F 兜底（覆盖未被调试事件跟踪到的进程）
        let root = self.root_pid.load(Ordering::SeqCst);
        if root != 0 {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &root.to_string(), "/T", "/F"])
                .creation_flags(crate::CREATE_NO_WINDOW)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

// =====================================================================
// 会话
// =====================================================================

pub struct WindowsSession {
    /// 会话策略（供平台层扩展使用；当前判定经 Shared 引用）
    #[allow(dead_code)]
    policy: Arc<ResolvedPolicy>,
    container_name: String,
    container_sid: Option<String>,
    acl_touched: Vec<String>,
    firewall_rule_prefix: Option<String>,
    session_temp: PathBuf,
    session_data: Option<PathBuf>,
    /// 持久 HOME（按工作区；None = 本会话用临时目录当 USERPROFILE）
    home: Option<PathBuf>,
    /// lowbox（AppContainer）主令牌
    token_primary: f::HANDLE,
    job: f::HANDLE,
    event_tx: Sender<Event>,
    /// Receiver 只能被取走一次（trait 契约：event_receiver 调用一次）
    event_rx: Mutex<Option<Receiver<Event>>>,
    shared: Arc<Shared>,
}

// 会话持有内核句柄，仅经 FFI 使用；执行线程间移动句柄是安全的。
unsafe impl Send for WindowsSession {}

impl WindowsSession {
    fn new(
        policy: Arc<ResolvedPolicy>,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self, BackendError> {
        let err = |m: String| BackendError { message: m };

        let short = format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64 + d.as_secs())
                .unwrap_or(0)
        );
        let container_name = format!("astorm.sbox.{short}");

        // ---- 1. 事件通道（先建，供 init 过程中的告警事件缓冲）----
        let (tx, rx) = channel::<Event>();

        // ---- 1.5 运行期策略：域名规则预解析为 IP（netstat 只给 IP）----
        // 解析失败的域名以 Note 事件明示；预检仍使用原始文本规则。
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

        // ---- 2. AppContainer 配置、SID、lowbox 主令牌、Job ----
        let (sid_str, data_dir, lowbox_token, job) =
            setup_container(&container_name, &policy.resource, &err)?;
        // 容器数据目录可能为空串（接口不可用时），视为无
        let data_dir = data_dir.filter(|p| !p.as_os_str().is_empty());

        // ---- 3. 会话临时目录 ----
        let session_temp = std::env::temp_dir().join(format!("astorm-{short}"));
        let _ = std::fs::create_dir_all(&session_temp);
        if let Some(d) = &data_dir {
            let _ = std::fs::create_dir_all(d);
        }
        // ---- 3.5 持久 HOME（工具层按工作区给出）：命令结束**不删除**，cargo/uv/npm/pip 的
        // 缓存与用户级配置跨命令复用；创建失败退化为临时目录（缓存不复用，不阻断执行）。
        let persistent_home: Option<PathBuf> = policy.session_home.as_ref().and_then(|p| {
            std::fs::create_dir_all(p).ok().map(|_| p.clone())
        });

        let shared = Arc::new(Shared {
            handles: Mutex::new(HashMap::new()),
            root_pid: AtomicU32::new(0),
            over: AtomicBool::new(false),
            exit_code: Mutex::new(None),
            evicted: AtomicBool::new(false),
            stop_net: AtomicBool::new(false),
            job: Mutex::new(job),
            policy: shared_policy,
            cancel,
        });

        // ---- 4. ACL 映射（icacls；Deny ACE 优先 → 黑名单 > 白名单）----
        // 只授予**会话工作区**读+执行；绝不把宿主 current_dir 当成工作区
        // （从 C:\Users\me 启动应用会把整个用户目录放开）。
        // 没有 workspace 的调用方（sbox CLI / 单元测试）退化为命令默认 cwd，
        // 此时不存在“会话边界”可保护。
        let cwd = policy.workspace.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| session_temp.clone())
        });
        let mut touched: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        apply_acl(
            &sid_str,
            &policy,
            &cwd,
            &data_dir,
            &session_temp,
            &persistent_home,
            &mut touched,
            &mut notes,
        );
        for n in notes {
            let _ = tx.send(Event::Note(n));
        }

        // ---- 5. 防火墙（仅管理员；失败明确告警并降级为驱逐监视）----
        let mut firewall_rule_prefix = None;
        if is_admin() {
            match apply_firewall(&sid_str, &short, &policy) {
                Ok(prefix) => firewall_rule_prefix = Some(prefix),
                Err(msg) => {
                    let _ = tx.send(Event::Note(format!(
                        "防火墙规则创建失败（{msg}），网络防护实际为驱逐监视"
                    )));
                }
            }
        }

        // ---- 6. 网络驱逐线程 ----
        {
            let shared = shared.clone();
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("astorm-netstat".to_string())
                .spawn(move || netstat_eviction_loop(shared, tx))
                .expect("创建驱逐线程失败");
        }

        Ok(WindowsSession {
            policy,
            container_name,
            container_sid: Some(sid_str),
            acl_touched: touched,
            firewall_rule_prefix,
            session_temp,
            session_data: data_dir,
            home: persistent_home,
            token_primary: lowbox_token,
            job,
            event_tx: tx,
            event_rx: Mutex::new(Some(rx)),
            shared,
        })
    }
}

/// 容器与令牌初始化：创建 AppContainer 配置 → SID → NtCreateLowBoxToken 生成
/// lowbox 主令牌（沙箱主令牌，未授权路径由内核拒绝）→ Job Object。
#[allow(clippy::type_complexity)]
fn setup_container(
    container_name: &str,
    resource: &crate::policy::ResourceLimits,
    err: &dyn Fn(String) -> BackendError,
) -> Result<(String, Option<PathBuf>, f::HANDLE, f::HANDLE), BackendError> {
    unsafe {
        let name_w = f::to_wide(container_name);
        let disp_w = f::to_wide("astorm-sandbox 会话");
        let desc_w = f::to_wide("astorm-sandbox 一次性沙箱容器");
        let mut sid: f::PSID = std::ptr::null_mut();
        let hr = f::CreateAppContainerProfile(
            name_w.as_ptr(),
            disp_w.as_ptr(),
            desc_w.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut sid,
        );
        if hr != 0 {
            // 已存在（上次异常残留）→ 直接派生 SID
            let hr2 = f::DeriveAppContainerSidFromAppContainerName(name_w.as_ptr(), &mut sid);
            if hr2 != 0 {
                return Err(err(format!(
                    "创建 AppContainer 配置失败：HRESULT=0x{hr:08X}"
                )));
            }
        }

        // SID → 字符串
        let mut sid_str_ptr: f::LPWSTR = std::ptr::null_mut();
        if f::ConvertSidToStringSidW(sid, &mut sid_str_ptr) == 0 {
            return Err(err("转换容器 SID 为字符串失败".to_string()));
        }
        let sid_str = {
            let mut len = 0usize;
            let mut p = sid_str_ptr;
            while *p != 0 {
                len += 1;
                p = p.add(1);
            }
            f::from_wide(std::slice::from_raw_parts(sid_str_ptr, len))
        };
        f::LocalFree(sid_str_ptr as f::HANDLE);

        // 容器数据目录
        let mut buf = [0u16; 1024];
        let mut len: f::DWORD = buf.len() as f::DWORD;
        let data_dir = if f::GetAppContainerFolderPath(sid, &mut len, buf.as_mut_ptr()) != 0 {
            Some(PathBuf::from(f::from_wide(&buf[..len as usize])))
        } else {
            None
        };

        // 当前进程主令牌 → lowbox（AppContainer）主令牌
        let mut process_token: f::HANDLE = std::ptr::null_mut();
        if f::OpenProcessToken(
            f::GetCurrentProcess(),
            f::TOKEN_DUPLICATE | f::TOKEN_QUERY | f::TOKEN_ASSIGN_PRIMARY,
            &mut process_token,
        ) == 0
        {
            return Err(err("打开当前进程令牌失败".to_string()));
        }
        let mut oa: f::OBJECT_ATTRIBUTES = std::mem::zeroed();
        oa.Length = std::mem::size_of::<f::OBJECT_ATTRIBUTES>() as f::DWORD;
        let mut lowbox: f::HANDLE = std::ptr::null_mut();
        let status = f::NtCreateLowBoxToken(
            &mut lowbox,
            process_token,
            f::MAXIMUM_ALLOWED,
            &mut oa,
            sid,
            0,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
        );
        f::CloseHandle(process_token);
        if status != 0 {
            return Err(err(format!(
                "NtCreateLowBoxToken 失败：NTSTATUS=0x{status:08X}"
            )));
        }

        // Job Object：KILL_ON_JOB_CLOSE（会话异常退出时整树随之销毁）
        let job = f::CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            f::CloseHandle(lowbox);
            return Err(err("创建 Job Object 失败".to_string()));
        }
        let mut limit: f::JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limit.BasicLimitInformation.LimitFlags = f::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // 资源限制：内存（整 Job 内存上限）+ CPU（整 Job CPU 时间上限，超限终止）
        if let Some(mb) = resource.memory_mb {
            limit.JobMemoryLimit = (mb as usize).saturating_mul(1024 * 1024);
            limit.BasicLimitInformation.LimitFlags |= f::JOB_OBJECT_LIMIT_JOB_MEMORY;
        }
        if let Some(secs) = resource.cpu_seconds {
            // PerJobUserTimeLimit 单位为 100ns；1 秒 = 10^7 单位
            limit.BasicLimitInformation.PerJobUserTimeLimit =
                (secs as i64).saturating_mul(10_000_000);
            limit.BasicLimitInformation.LimitFlags |= f::JOB_OBJECT_LIMIT_JOB_TIME;
        }
        // CPU 使用率硬上限（OS 级配额调度；CpuRate 单位 = 百分比 × 100，口径为**整机占比**，
        // 与 macOS 侧换算后的阈值一致）
        if let Some(pct) = resource.cpu_percent {
            let ctrl = f::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: f::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
                    | f::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                CpuRate: (pct as u32).saturating_mul(100),
            };
            if f::SetInformationJobObject(
                job,
                f::JOBOBJECTINFOCLASS_CpuRateControlInformation,
                &ctrl as *const _ as *mut c_void,
                std::mem::size_of::<f::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as f::DWORD,
            ) == 0
            {
                f::CloseHandle(job);
                f::CloseHandle(lowbox);
                return Err(err("设置 Job CPU 使用率上限失败".to_string()));
            }
        }
        if f::SetInformationJobObject(
            job,
            f::JOBOBJECTINFOCLASS_ExtendedLimitInformation,
            &mut limit as *mut _ as *mut c_void,
            std::mem::size_of::<f::JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as f::DWORD,
        ) == 0
        {
            f::CloseHandle(job);
            f::CloseHandle(lowbox);
            return Err(err("设置 Job Object 属性失败".to_string()));
        }

        Ok((sid_str, data_dir, lowbox, job))
    }
}

/// 调试线程主体：创建沙箱进程（DEBUG_PROCESS）并在同线程泵调试事件。
fn debug_thread_main(job_data: SpawnJob) {
    let SpawnJob {
        token,
        job,
        h_stdin,
        h_stdout,
        h_stderr,
        // 保持 NUL stdin 文件句柄存活到 CreateProcess 完成
        _stdin_null,
        mut cmdline_w,
        cwd_w,
        mut env_block,
        shared,
        tx,
        pid_tx,
    } = job_data;
    unsafe {
        let mut si: f::STARTUPINFOW = f::STARTUPINFOW::default();
        si.cb = std::mem::size_of::<f::STARTUPINFOW>() as f::DWORD;
        si.dwFlags = f::STARTF_USESTDHANDLES;
        si.hStdInput = h_stdin;
        si.hStdOutput = h_stdout;
        si.hStdError = h_stderr;
        let mut pi: f::PROCESS_INFORMATION = std::mem::zeroed();
        let flags = f::DEBUG_PROCESS | f::CREATE_UNICODE_ENVIRONMENT | f::CREATE_NO_WINDOW;

        let ok = f::CreateProcessAsUserW(
            token,
            std::ptr::null(),
            cmdline_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            f::TRUE,
            flags,
            env_block.as_mut_ptr() as *mut c_void,
            cwd_w.as_ptr(),
            &mut si,
            &mut pi,
        );
        if ok == 0 {
            let gle = f::GetLastError();
            let _ = pid_tx.send(Err(format!(
                "沙箱进程创建失败：GetLastError={gle}（可用 --protect off 直通）"
            )));
            return;
        }

        // 根进程加入 Job Object（KILL_ON_JOB_CLOSE 兜底）
        f::AssignProcessToJobObject(job, pi.hProcess);
        f::CloseHandle(pi.hThread);
        f::CloseHandle(pi.hProcess);

        let root = pi.dwProcessId;
        shared.root_pid.store(root, Ordering::SeqCst);
        if pid_tx.send(Ok(root)).is_err() {
            // 调用方已放弃（初始化失败等）——终止刚创建的进程树
            shared.kill_all();
            return;
        }

        // 同线程泵调试事件（动态命令拦截 + 生命周期跟踪）
        debug_loop(shared, tx);
    }
}

impl SandboxSession for WindowsSession {
    fn spawn(&mut self, spec: ChildSpec) -> Result<ProcessRef, BackendError> {
        // 关键约束：WaitForDebugEvent 必须与 CreateProcess(DEBUG_PROCESS) 同线程，
        // 因此"创建进程 + 调试循环"整体在独立线程执行，pid 经通道回传。
        unsafe {
            // 标准流句柄设为可继承
            let mut h_stdin: f::HANDLE = std::ptr::null_mut();
            let mut h_stdout: f::HANDLE = std::ptr::null_mut();
            let mut h_stderr: f::HANDLE = std::ptr::null_mut();
            // 无 stdin 时显式接 NUL：STARTF_USESTDHANDLES 要求 stdin 是有效句柄，
            // NULL 会让 cmd.exe 等控制台程序挂起或直接失败。
            let stdin_null = if spec.stdin.is_none() {
                std::fs::OpenOptions::new().read(true).open("NUL").ok()
            } else {
                None
            };
            if let Some(file) = &spec.stdin {
                h_stdin = file.as_raw_handle();
                f::SetHandleInformation(h_stdin, f::HANDLE_FLAG_INHERIT, f::HANDLE_FLAG_INHERIT);
            } else if let Some(file) = &stdin_null {
                h_stdin = file.as_raw_handle();
                f::SetHandleInformation(h_stdin, f::HANDLE_FLAG_INHERIT, f::HANDLE_FLAG_INHERIT);
            }
            if let Some(file) = &spec.stdout {
                h_stdout = file.as_raw_handle();
                f::SetHandleInformation(h_stdout, f::HANDLE_FLAG_INHERIT, f::HANDLE_FLAG_INHERIT);
            }
            if let Some(file) = &spec.stderr {
                h_stderr = file.as_raw_handle();
                f::SetHandleInformation(h_stderr, f::HANDLE_FLAG_INHERIT, f::HANDLE_FLAG_INHERIT);
            }

            // 命令行构造：raw_command_arg 时最后一个参数（命令文本）不加引号。
            // 程序路径走 `\\?\` 扩展长度形式（CreateProcess 不走 Rust std 的长路径处理）。
            let program = f::long_path(&spec.program).to_string_lossy().into_owned();
            let cmdline = if spec.raw_command_arg && !spec.args.is_empty() {
                let head = &spec.args[..spec.args.len() - 1];
                let last = spec.args.last().unwrap();
                let mut s = f::quote_always(&program);
                for a in head {
                    s.push(' ');
                    s.push_str(&f::quote_arg(a));
                }
                s.push(' ');
                s.push_str(last);
                s
            } else {
                f::build_command_line(&program, &spec.args)
            };
            let cmdline_w = f::to_wide(&cmdline);
            // cwd：超过 MAX_PATH 时依次尝试 8.3 短路径 → junction。
            // CreateProcessAsUserW 不接受 `\\?\` 形式的 cwd（ERROR_DIRECTORY），
            // 8.3 短名也可能被禁用；junction 指向长目录但自身路径很短。
            let cwd_owned = if spec.cwd.to_string_lossy().chars().count() > 260 {
                f::short_path(&spec.cwd)
                    .or_else(|| f::junction_for_long_path(&spec.cwd, &self.session_temp))
                    .unwrap_or_else(|| spec.cwd.clone())
            } else {
                spec.cwd.clone()
            };
            let cwd_w = f::to_wide(&cwd_owned.to_string_lossy());
            let env_block = f::build_environment_block(&spec.env);

            let (pid_tx, pid_rx) = channel::<Result<u32, String>>();

            let job_data = SpawnJob {
                token: self.token_primary,
                job: self.job,
                h_stdin,
                h_stdout,
                h_stderr,
                _stdin_null: stdin_null,
                cmdline_w,
                cwd_w,
                env_block,
                shared: self.shared.clone(),
                tx: self.event_tx.clone(),
                pid_tx,
            };

            std::thread::Builder::new()
                .name("astorm-debugloop".to_string())
                .spawn(move || debug_thread_main(job_data))
                .map_err(|e| BackendError {
                    message: format!("创建调试线程失败：{e}"),
                })?;

            let pid = pid_rx
                .recv()
                .map_err(|_| BackendError {
                    message: "调试线程异常退出".to_string(),
                })?
                .map_err(BackendError::from_message)?;

            Ok(ProcessRef { pid, trace_id: 1 })
        }
    }

    fn event_receiver(&mut self) -> Receiver<Event> {
        self.event_rx
            .lock()
            .unwrap()
            .take()
            .expect("event_receiver 只能调用一次")
    }

    fn extra_env(&self) -> Vec<(String, String)> {
        let mut env = vec![
            (
                "TEMP".to_string(),
                self.session_temp.to_string_lossy().to_string(),
            ),
            (
                "TMP".to_string(),
                self.session_temp.to_string_lossy().to_string(),
            ),
        ];
        // HOME 类变量按工作区持久（缓存/用户配置跨命令复用）；
        // LOCALAPPDATA 仍指向 AppContainer 数据目录（容器自身需要，逐会话）。
        // 只设 USERPROFILE/APPDATA：HOMEDRIVE/HOMEPATH 是遗留变量，设错反而害工具。
        if let Some(home) = &self.home {
            let h = home.to_string_lossy().to_string();
            env.push(("USERPROFILE".to_string(), h.clone()));
            env.push(("APPDATA".to_string(), h));
        }
        if let Some(d) = &self.session_data {
            env.push(("LOCALAPPDATA".to_string(), d.to_string_lossy().to_string()));
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
            if Instant::now() >= deadline {
                return Ok(WaitOutcome::Timeout);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill_tree(&mut self) -> Result<(), BackendError> {
        self.shared.kill_all();
        Ok(())
    }

    fn teardown(self: Box<Self>) -> Result<(), BackendError> {
        let mut this: WindowsSession = *self;
        this.shared.stop_net.store(true, Ordering::SeqCst);
        if !this.shared.over.load(Ordering::SeqCst) {
            this.shared.kill_all();
        }
        // 关闭令牌与 Job 句柄
        unsafe {
            if !this.token_primary.is_null() {
                f::CloseHandle(this.token_primary);
                this.token_primary = std::ptr::null_mut();
            }
            {
                let mut job = this.shared.job.lock().unwrap();
                if !(*job).is_null() {
                    f::CloseHandle(*job);
                    *job = std::ptr::null_mut();
                }
            }
        }
        // 移除防火墙规则
        if let Some(prefix) = &this.firewall_rule_prefix {
            let script = format!(
                "Get-NetFirewallRule -DisplayName '{prefix}*' -ErrorAction SilentlyContinue | Remove-NetFirewallRule"
            );
            use std::os::windows::process::CommandExt as _;
            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .creation_flags(crate::CREATE_NO_WINDOW)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        // 撤销 ACL（只移除容器 SID 的 ACE）。优先原生 API（毫秒级）；
        // 失败再回退 icacls。
        if let Some(sid) = &this.container_sid {
            for path in &this.acl_touched {
                if acl::revoke(path, sid) {
                    continue;
                }
                use std::os::windows::process::CommandExt as _;
                let _ = std::process::Command::new("icacls")
                    .arg(path)
                    .arg("/remove")
                    .arg(format!("*{sid}"))
                    .creation_flags(crate::CREATE_NO_WINDOW)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        // 删除容器配置
        unsafe {
            let name_w = f::to_wide(&this.container_name);
            let _ = f::DeleteAppContainerProfile(name_w.as_ptr());
        }
        // 清理会话临时目录
        let _ = std::fs::remove_dir_all(&this.session_temp);
        Ok(())
    }
}

impl Drop for WindowsSession {
    fn drop(&mut self) {
        self.shared.stop_net.store(true, Ordering::SeqCst);
        if !self.shared.over.load(Ordering::SeqCst) {
            self.shared.kill_all();
        }
        // teardown 之外的异常路径：尽力关闭句柄（teardown 已置空的会被跳过）
        unsafe {
            if !self.token_primary.is_null() {
                f::CloseHandle(self.token_primary);
                self.token_primary = std::ptr::null_mut();
            }
            let mut job = self.shared.job.lock().unwrap();
            if !(*job).is_null() {
                f::CloseHandle(*job);
                *job = std::ptr::null_mut();
            }
        }
    }
}

// =====================================================================
// 调试循环：动态命令拦截 + 生命周期跟踪
// =====================================================================

fn debug_loop(shared: Arc<Shared>, tx: Sender<Event>) {
    unsafe {
        let mut first_breakpoint = true;
        loop {
            let mut ev = f::DEBUG_EVENT::zeroed();
            if f::WaitForDebugEvent(&mut ev, f::INFINITE) == 0 {
                break;
            }
            match ev.dwDebugEventCode {
                f::CREATE_PROCESS_DEBUG_EVENT => {
                    let info = ev.u.CreateProcessInfo;
                    let pid = ev.dwProcessId;
                    let is_root = shared.root_pid.load(Ordering::SeqCst) == pid;
                    shared.handles.lock().unwrap().insert(pid, info.hProcess);

                    let cmdline = read_command_line(info.hProcess).unwrap_or_default();
                    // 动态命令拦截：仅对根进程之外的子进程生效（根命令已被预检覆盖）
                    if !is_root {
                        let hits = shared.policy.danger.matches(&cmdline);
                        if !hits.is_empty() {
                            let _ = tx.send(Event::Denied {
                                category: Category::DangerCommand,
                                detail: format!(
                                    "子进程 {pid} 命令行「{cmdline}」命中危险命令黑名单，已实时终止"
                                ),
                            });
                            f::TerminateProcess(info.hProcess, 1);
                        }
                    }
                    let _ = tx.send(Event::Spawn { pid, argv: cmdline });
                    if !info.hFile.is_null() {
                        f::CloseHandle(info.hFile);
                    }
                }
                f::EXIT_PROCESS_DEBUG_EVENT => {
                    let pid = ev.dwProcessId;
                    let code = ev.u.ExitProcess.dwExitCode as i32;
                    let _ = tx.send(Event::Exit { pid, code });
                    shared.handles.lock().unwrap().remove(&pid);
                    if pid == shared.root_pid.load(Ordering::SeqCst) {
                        *shared.exit_code.lock().unwrap() = Some(code);
                        shared.over.store(true, Ordering::SeqCst);
                        break; // 调试会话结束
                    }
                }
                f::EXCEPTION_DEBUG_EVENT => {
                    let code = ev.u.Exception.ExceptionRecord.ExceptionCode;
                    let status = if first_breakpoint && code == f::EXCEPTION_BREAKPOINT {
                        first_breakpoint = false;
                        f::DBG_CONTINUE
                    } else {
                        // 其余异常按正常方式交还被调试进程处理
                        f::DBG_EXCEPTION_NOT_HANDLED
                    };
                    f::ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, status);
                    continue;
                }
                f::CREATE_THREAD_DEBUG_EVENT => {
                    let h = ev.u.CreateThread.hThread;
                    if !h.is_null() {
                        f::CloseHandle(h);
                    }
                }
                f::LOAD_DLL_DEBUG_EVENT => {
                    let h = ev.u.LoadDll.hFile;
                    if !h.is_null() {
                        f::CloseHandle(h);
                    }
                }
                _ => {}
            }
            f::ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, f::DBG_CONTINUE);
        }
    }
}

/// 读取目标进程的完整命令行（PEB → RTL_USER_PROCESS_PARAMETERS → CommandLine）。
/// 按目标子进程架构选择 PEB / 命令行偏移。
///
/// x64 宿主调试 32 位子进程时，PEB 布局是 x86 的（0x10/0x40），
/// 用宿主编译期偏移（0x20/0x70）会读错地址，动态拦截静默失效。
fn command_line_offsets(h_process: f::HANDLE) -> (usize, usize) {
    unsafe {
        let mut process_machine: u16 = 0;
        let mut native_machine: u16 = 0;
        if f::IsWow64Process2(h_process, &mut process_machine, &mut native_machine) != 0 {
            // UNKNOWN = 子进程与操作系统同架构，用 nativeMachine
            let machine = if process_machine == f::IMAGE_FILE_MACHINE_UNKNOWN {
                native_machine
            } else {
                process_machine
            };
            if let Some(off) = f::peb_offsets_for_machine(machine) {
                return off;
            }
        }
    }
    // 老系统（Win10 1709 前）或未支持架构：回退宿主编译期偏移
    (
        f::PEB_PROCESS_PARAMETERS_OFFSET,
        f::PP_COMMANDLINE_OFFSET,
    )
}

fn read_command_line(h_process: f::HANDLE) -> Option<String> {
    unsafe {
        let (peb_off, pp_off) = command_line_offsets(h_process);
        let mut pbi: f::PROCESS_BASIC_INFORMATION = std::mem::zeroed();
        let mut ret_len: f::DWORD = 0;
        let status = f::NtQueryInformationProcess(
            h_process,
            f::PROCESS_BASIC_INFORMATION_CLASS,
            &mut pbi as *mut _ as *mut c_void,
            std::mem::size_of::<f::PROCESS_BASIC_INFORMATION>() as f::DWORD,
            &mut ret_len,
        );
        if status != 0 || pbi.PebBaseAddress.is_null() {
            return None;
        }
        // PEB → ProcessParameters
        let mut params_ptr: *mut c_void = std::ptr::null_mut();
        if !read_mem(
            h_process,
            (pbi.PebBaseAddress as usize + peb_off) as *const c_void,
            &mut params_ptr as *mut *mut c_void as *mut c_void,
            std::mem::size_of::<*mut c_void>(),
        ) {
            return None;
        }
        if params_ptr.is_null() {
            return None;
        }
        // ProcessParameters → CommandLine UNICODE_STRING
        let mut us: f::UNICODE_STRING = std::mem::zeroed();
        if !read_mem(
            h_process,
            (params_ptr as usize + pp_off) as *const c_void,
            &mut us as *mut f::UNICODE_STRING as *mut c_void,
            std::mem::size_of::<f::UNICODE_STRING>(),
        ) {
            return None;
        }
        if us.Buffer.is_null() || us.Length == 0 {
            return None;
        }
        let len = us.Length as usize / 2;
        let mut buf = vec![0u16; len];
        if !read_mem(
            h_process,
            us.Buffer as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            len * 2,
        ) {
            return None;
        }
        Some(String::from_utf16_lossy(&buf))
    }
}

fn read_mem(h: f::HANDLE, addr: *const c_void, buf: *mut c_void, size: usize) -> bool {
    unsafe {
        let mut read: usize = 0;
        f::ReadProcessMemory(h, addr, buf, size, &mut read) != 0
    }
}

// =====================================================================
// 网络驱逐（netstat -ano 轮询；黑 > 白 > 默认拒绝，违规即终止进程树）
// =====================================================================

/// 一行 netstat 输出的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetstatRow {
    pub proto: String,
    pub local: String,
    pub foreign: String,
    pub state: Option<String>,
    pub pid: u32,
}

/// 解析 `netstat -ano` 输出中的 TCP/UDP 行。
pub fn parse_netstat_rows(output: &str) -> Vec<NetstatRow> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let t = line.trim_start();
        if !(t.starts_with("TCP") || t.starts_with("UDP")) {
            continue;
        }
        let parts: Vec<&str> = t.split_whitespace().collect();
        // TCP: proto local foreign state pid
        // UDP: proto local foreign pid
        let row = if parts[0].eq_ignore_ascii_case("TCP") && parts.len() >= 5 {
            NetstatRow {
                proto: "TCP".into(),
                local: parts[1].to_string(),
                foreign: parts[2].to_string(),
                state: Some(parts[3].to_string()),
                pid: parts[4].parse().unwrap_or(0),
            }
        } else if parts[0].eq_ignore_ascii_case("UDP") && parts.len() >= 4 {
            NetstatRow {
                proto: "UDP".into(),
                local: parts[1].to_string(),
                foreign: parts[2].to_string(),
                state: None,
                pid: parts[3].parse().unwrap_or(0),
            }
        } else {
            continue;
        };
        rows.push(row);
    }
    rows
}

/// "1.2.3.4:80" / "[::1]:443" → NetAddr。
///
/// 通配地址（0.0.0.0 / *）返回仅含端口的端点（用于监听判定）；端口缺失或为 0 则跳过。
pub fn parse_sockaddr(s: &str) -> Option<NetAddr> {
    if s.contains('[') {
        let start = s.find('[')?;
        let end = s.find(']')?;
        let ip: std::net::IpAddr = s[start + 1..end].parse().ok()?;
        let port = s[end + 1..]
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok());
        return Some(NetAddr {
            host: None,
            ip: Some(ip),
            port,
        });
    }
    let (host, port) = s.rsplit_once(':')?;
    let wildcard = host.eq_ignore_ascii_case("*") || host == "0.0.0.0";
    let port: Option<u16> = port.parse().ok();
    if wildcard {
        if matches!(port, Some(0) | None) {
            return None;
        }
        return Some(NetAddr {
            host: None,
            ip: None,
            port,
        });
    }
    let ip: std::net::IpAddr = host.parse().ok()?;
    Some(NetAddr {
        host: None,
        ip: Some(ip),
        port,
    })
}

fn netstat_eviction_loop(shared: Arc<Shared>, tx: Sender<Event>) {
    loop {
        if shared.stop_net.load(Ordering::SeqCst) || shared.over.load(Ordering::SeqCst) {
            break;
        }
        // 1 秒间隔（小步检查停止标志）
        for _ in 0..20 {
            if shared.stop_net.load(Ordering::SeqCst) || shared.over.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let tracked: Vec<u32> = shared.handles.lock().unwrap().keys().copied().collect();
        if tracked.is_empty() {
            continue;
        }
        let output = {
            use std::os::windows::process::CommandExt as _;
            std::process::Command::new("netstat")
                .arg("-ano")
                .creation_flags(crate::CREATE_NO_WINDOW)
                .stdin(std::process::Stdio::null())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        };
        let output = match output {
            Ok(o) => o,
            Err(_) => continue,
        };
        for row in parse_netstat_rows(&output) {
            if !tracked.contains(&row.pid) {
                continue;
            }
            let (dir, addr) = match row.proto.as_str() {
                "TCP" => {
                    if row.state.as_deref() == Some("LISTENING") {
                        (Direction::Inbound, parse_sockaddr(&row.local))
                    } else {
                        (Direction::Outbound, parse_sockaddr(&row.foreign))
                    }
                }
                _ => match parse_sockaddr(&row.foreign) {
                    // UDP 无连接（*:*）跳过
                    Some(a) => (Direction::Outbound, Some(a)),
                    None => continue,
                },
            };
            let addr = match addr {
                Some(a) => a,
                None => continue,
            };
            let decision = crate::decision::decide_net(&shared.policy.net, dir, &addr);
            if let crate::decision::Decision::Deny(reason) = decision {
                let _ = tx.send(Event::Evicted {
                    category: Category::from_direction(dir),
                    reason: format!(
                        "进程 {pid} {dir_text}连接 {host} 违规：{reason}，已终止进程树",
                        pid = row.pid,
                        dir_text = if dir == Direction::Outbound {
                            "出站"
                        } else {
                            "进站"
                        },
                        host = addr
                            .ip
                            .map(|i| i.to_string())
                            .or(addr.host)
                            .unwrap_or_default(),
                    ),
                });
                shared.evicted.store(true, Ordering::SeqCst);
                shared.kill_all();
                return;
            }
        }
    }
}

// =====================================================================
// ACL 映射（icacls；Deny ACE 优先于 Allow ACE —— 内核实现 黑名单 > 白名单）
// =====================================================================

/// 从 glob 模式提取可操作的固定前缀路径（遇到通配符截断）。
pub fn glob_fixed_prefix(pattern: &str) -> Option<String> {
    let mut out = String::new();
    for c in pattern.chars() {
        if c == '*' || c == '?' || c == '[' {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end_matches(['/', '\\']).to_string();
    if out.is_empty() || !out.contains(['/', '\\', ':']) {
        return None;
    }
    Some(out)
}

#[allow(clippy::too_many_arguments)]
fn apply_acl(
    sid: &str,
    policy: &ResolvedPolicy,
    cwd: &std::path::Path,
    session_data: &Option<PathBuf>,
    session_temp: &std::path::Path,
    session_home: &Option<PathBuf>,
    touched: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    // 会话自有目录
    if let Some(d) = session_data {
        acl_grant(d.to_string_lossy().as_ref(), sid, "F", touched, notes);
    }
    acl_grant(
        session_temp.to_string_lossy().as_ref(),
        sid,
        "F",
        touched,
        notes,
    );
    // 持久 HOME：HOME 指向它，读写都要给全（命令结束不删除）
    if let Some(d) = session_home {
        acl_grant(d.to_string_lossy().as_ref(), sid, "F", touched, notes);
    }
    // 工作目录默认可读（脚本运行需要）；写保护由 AppContainer 默认拒绝保证
    acl_grant(cwd.to_string_lossy().as_ref(), sid, "RX", touched, notes);

    // 「安全配置 → 额外可执行目录」：非常规安装位置的工具链/编译器（读+执行，不给写）
    for d in &policy.exec_white_dirs {
        let d = d.trim();
        if !d.is_empty() {
            acl_grant(d, sid, "RX", touched, notes);
        }
    }

    // 工具链授权：默认只放行**实际 bin 目录**（~/.cargo/bin、~/.rustup/toolchains/*/bin、
    // AppData\Roaming\npm、Python\Scripts、Git\cmd…）。这些目录通常很小，原生 DACL
    // 修改是毫秒级。
    //
    // **不默认注入父目录**（~/.cargo、~/.rustup、AppData 根）：在这些大目录上即使
    // 原生、即使 ACE 不继承，Windows 仍会同步处理子项，实测单条 10–37s。
    // 需要完整 lib/registry 访问的环境可显式开启：
    //   ASTORM_WINDOWS_TOOLCHAIN_ACL=0        关闭全部工具链授权
    //   ASTORM_WINDOWS_TOOLCHAIN_READ_DIRS=1  额外授权父目录（慢，慎用）
    if std::env::var("ASTORM_WINDOWS_TOOLCHAIN_ACL").as_deref() != Ok("0") {
        for d in crate::toolchain::exec_allow_dirs(&policy.toolchain_disabled) {
            acl_grant_native(d.to_string_lossy().as_ref(), sid, "RX", true, touched, notes);
        }
        if std::env::var("ASTORM_WINDOWS_TOOLCHAIN_READ_DIRS").as_deref() == Ok("1") {
            for d in crate::toolchain::read_allow_dirs(&policy.toolchain_disabled) {
                acl_grant_native(d.to_string_lossy().as_ref(), sid, "RX", false, touched, notes);
            }
        }
    }

    // 黑名单（Deny ACE）
    for pat in &policy.file.read_black_raw {
        if let Some(p) = glob_fixed_prefix(pat) {
            acl_deny(&p, sid, "R", touched, notes);
        }
    }
    for pat in &policy.file.write_black_raw {
        if let Some(p) = glob_fixed_prefix(pat) {
            acl_deny(&p, sid, "W,DC", touched, notes);
        }
    }
    // 白名单（Grant ACE）
    for pat in &policy.file.read_white_raw {
        if let Some(p) = glob_fixed_prefix(pat) {
            acl_grant(&p, sid, "RX", touched, notes);
        }
    }
    for pat in &policy.file.write_white_raw {
        if let Some(p) = glob_fixed_prefix(pat) {
            acl_grant(&p, sid, "M", touched, notes);
        }
    }
}

/// 构造 icacls 的 SID 权限串：文件不带继承标志，目录带 (OI)(CI)。
fn sid_spec(path: &str, sid: &str, rights: &str) -> String {
    let is_file = std::path::Path::new(path).is_file();
    if is_file {
        format!("*{sid}:({rights})")
    } else {
        format!("*{sid}:(OI)(CI)({rights})")
    }
}

/// 把路径规整成 icacls 可接受的形式：
/// - 去掉 `\\?\` 前缀（icacls 不认 verbatim 路径）
/// - 跳过盘符/UNC 根（整盘授权既过宽，又会让会话初始化极慢）
fn acl_path(path: &str) -> Option<String> {
    let p = path.strip_prefix(r"\\?\").unwrap_or(path).trim();
    if p.is_empty() {
        return None;
    }
    let pb = std::path::Path::new(p);
    let mut comps = pb.components();
    let is_root = matches!(comps.next(), Some(std::path::Component::Prefix(_)))
        && matches!(comps.next(), Some(std::path::Component::RootDir))
        && comps.next().is_none();
    if is_root {
        None
    } else {
        Some(p.to_string())
    }
}

/// 仅原生授权（不回退 icacls）。用于自动探测的工具链目录：
/// 这些路径数量多、单条 icacls 可能耗时数秒，失败时宁可只记录 Note。
fn acl_grant_native(
    path: &str,
    sid: &str,
    rights: &str,
    inherit: bool,
    touched: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    let Some(path) = acl_path(path) else { return };
    let path = path.as_str();
    let ok = rights_mask(rights)
        .map(|mask| acl::grant(path, sid, mask, inherit))
        .unwrap_or(false);
    if ok {
        touched.push(path.to_string());
    } else {
        notes.push(format!(
            "无法为沙箱容器授权路径 {path}（{rights}）；该路径对沙箱不可用"
        ));
    }
}

fn acl_grant(
    path: &str,
    sid: &str,
    rights: &str,
    touched: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    let Some(path) = acl_path(path) else { return };
    let path = path.as_str();
    let native = rights_mask(rights)
        .map(|mask| acl::grant(path, sid, mask, true))
        .unwrap_or(false);
    if native || icacls(path, "/grant", &sid_spec(path, sid, rights)) {
        touched.push(path.to_string());
    } else {
        notes.push(format!(
            "无法为沙箱容器授权路径 {path}（{rights}）；该路径对沙箱不可用"
        ));
    }
}

fn acl_deny(
    path: &str,
    sid: &str,
    rights: &str,
    touched: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    let Some(path) = acl_path(path) else { return };
    let path = path.as_str();
    let native = rights_mask(rights)
        .map(|mask| acl::deny(path, sid, mask, true))
        .unwrap_or(false);
    if native || icacls(path, "/deny", &sid_spec(path, sid, rights)) {
        touched.push(path.to_string());
    } else {
        notes.push(format!(
            "无法为沙箱容器设置拒绝规则 {path}（{rights}）；该黑名单路径可能不生效"
        ));
    }
}/// icacls 权限串 → Win32 访问掩码。
fn rights_mask(rights: &str) -> Option<u32> {
    Some(match rights {
        // FILE_ALL_ACCESS
        "F" => 0x1F01FF,
        // FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE
        "M" => 0x1301BF,
        // FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
        "RX" => 0x1200A9,
        // FILE_GENERIC_READ
        "R" => 0x120089,
        // FILE_GENERIC_WRITE
        "W" => 0x120116,
        // FILE_GENERIC_WRITE | DELETE_CHILD
        "W,DC" => 0x120156,
        // DELETE
        "D" => 0x0001_0000,
        // DELETE_CHILD
        "DC" => 0x0000_0040,
        _ => return None,
    })
}

fn icacls(path: &str, mode: &str, spec: &str) -> bool {
    use std::os::windows::process::CommandExt as _;
    std::process::Command::new("icacls")
        .arg(path)
        .arg(mode)
        .arg(spec)
        .creation_flags(crate::CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// =====================================================================
// 防火墙（WFP，仅管理员）
// =====================================================================

/// 为沙箱 SID 下发网络规则。
///
/// 白名单开关开启（默认）：默认拒绝（进/出）+ 白名单允许（IP/CIDR/域名解析为 IP）。
/// 白名单开关关闭（仅黑名单模式）：不下发默认拒绝，仅对黑名单条目下发 Block 规则；
/// 黑名单为空时不创建任何规则（全部放行由策略语义保证）。
fn apply_firewall(sid: &str, short: &str, policy: &ResolvedPolicy) -> Result<String, String> {
    let prefix = format!("astorm-{short}");
    let mut ps = String::from("$ErrorActionPreference='Stop';");
    ps.push_str(&format!("$sid='{sid}';"));

    // 规则集合构建：白名单模式 = 默认拒绝 + 白名单放行；黑名单模式 = 仅黑名单 Block
    let mut block_out: Vec<String> = Vec::new();
    let mut block_in: Vec<String> = Vec::new();
    let mut allow_out: Vec<String> = Vec::new();
    let mut allow_in: Vec<String> = Vec::new();

    let push_target =
        |dst: &mut Vec<String>, r: &crate::policy::NetRule, what: &str| -> Result<(), String> {
            match &r.target {
                crate::policy::NetTarget::Ip(ip) => dst.push(ip.to_string()),
                crate::policy::NetTarget::Cidr { addr, prefix: p } => {
                    dst.push(format!("{addr}/{p}"));
                }
                crate::policy::NetTarget::Domain(d) => match resolve_domain(d) {
                    Ok(ips) => dst.extend(ips),
                    Err(e) => return Err(format!("域名解析失败 {d}: {e}")),
                },
                crate::policy::NetTarget::Any => {
                    return Err(format!("{what}包含 any，防火墙层无法按规则表达"))
                }
            }
            Ok(())
        };

    if policy.net.use_whitelist {
        ps.push_str(&format!(
            "New-NetFirewallRule -DisplayName '{prefix}-out-deny' -Direction Outbound -Action Block -LocalUser $sid | Out-Null;"
        ));
        ps.push_str(&format!(
            "New-NetFirewallRule -DisplayName '{prefix}-in-deny' -Direction Inbound -Action Block -LocalUser $sid | Out-Null;"
        ));
        for r in &policy.net.out_white {
            push_target(&mut allow_out, r, "出站白名单")?;
        }
        for r in &policy.net.in_white {
            push_target(&mut allow_in, r, "进站白名单")?;
        }
    } else {
        // 仅黑名单模式：黑名单条目 → Block 规则；其余放行
        for r in &policy.net.out_black {
            push_target(&mut block_out, r, "出站黑名单")?;
        }
        for r in &policy.net.in_black {
            push_target(&mut block_in, r, "进站黑名单")?;
        }
    }

    if !allow_out.is_empty() {
        ps.push_str(&format!(
            "New-NetFirewallRule -DisplayName '{prefix}-out-allow' -Direction Outbound -Action Allow -LocalUser $sid -RemoteAddress @({}) | Out-Null;",
            allow_out.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")
        ));
    }
    if !allow_in.is_empty() {
        ps.push_str(&format!(
            "New-NetFirewallRule -DisplayName '{prefix}-in-allow' -Direction Inbound -Action Allow -LocalUser $sid -RemoteAddress @({}) | Out-Null;",
            allow_in.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")
        ));
    }
    if !block_out.is_empty() {
        ps.push_str(&format!(
            "New-NetFirewallRule -DisplayName '{prefix}-out-block' -Direction Outbound -Action Block -LocalUser $sid -RemoteAddress @({}) | Out-Null;",
            block_out.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")
        ));
    }
    if !block_in.is_empty() {
        ps.push_str(&format!(
            "New-NetFirewallRule -DisplayName '{prefix}-in-block' -Direction Inbound -Action Block -LocalUser $sid -RemoteAddress @({}) | Out-Null;",
            block_in.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")
        ));
    }

    // 全部规则都无需创建（黑名单模式且黑名单为空）→ 不调用 PowerShell
    let has_rules = ps.contains("New-NetFirewallRule");
    if !has_rules {
        return Ok(prefix);
    }

    let output = {
        use std::os::windows::process::CommandExt as _;
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .creation_flags(crate::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("无法调用 PowerShell：{e}"))
    };
    let output = match output {
        Ok(o) => o,
        Err(e) => return Err(e),
    };
    if !output.status.success() {
        return Err(format!(
            "PowerShell 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(prefix)
}

fn resolve_domain(domain: &str) -> Result<Vec<String>, String> {
    use std::net::ToSocketAddrs;
    let host = format!("{domain}:443");
    let addrs: Vec<String> = host
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .map(|a| a.ip().to_string())
        .collect();
    if addrs.is_empty() {
        Err("无解析结果".to_string())
    } else {
        Ok(addrs)
    }
}

// ============================ 测试 ============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_command_line_offsets_match_host() {
        let off = command_line_offsets(unsafe { f::GetCurrentProcess() });
        assert_eq!(
            off,
            (f::PEB_PROCESS_PARAMETERS_OFFSET, f::PP_COMMANDLINE_OFFSET),
            "native 进程应使用宿主架构偏移"
        );
    }

    #[test]
    fn netstat行解析() {
        let sample = concat!(
            "\n  TCP    127.0.0.1:49832       127.0.0.1:4444      ESTABLISHED     1234\n",
            "  TCP    0.0.0.0:135           0.0.0.0:0              LISTENING       999\n",
            "  UDP    127.0.0.1:5353        *:*                                    777\n",
        );
        let rows = parse_netstat_rows(sample);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pid, 1234);
        assert_eq!(rows[0].state.as_deref(), Some("ESTABLISHED"));
        assert_eq!(rows[1].state.as_deref(), Some("LISTENING"));
        assert_eq!(rows[2].proto, "UDP");
        assert_eq!(rows[2].pid, 777);
    }

    #[test]
    fn sockaddr解析() {
        let a = parse_sockaddr("8.8.8.8:443").unwrap();
        assert_eq!(a.ip, Some("8.8.8.8".parse().unwrap()));
        assert_eq!(a.port, Some(443));

        let a = parse_sockaddr("[::1]:8080").unwrap();
        assert_eq!(a.ip, Some("::1".parse().unwrap()));

        // 通配监听：保留端口（进站判定用）
        let a = parse_sockaddr("0.0.0.0:135").unwrap();
        assert_eq!(a.ip, None);
        assert_eq!(a.port, Some(135));
        // 端口缺失/为 0 → 跳过
        assert!(parse_sockaddr("0.0.0.0:0").is_none());
        assert!(parse_sockaddr("*:*").is_none());
    }

    #[test]
    fn 命令行构造() {
        assert_eq!(
            f::build_command_line(
                r"C:\Windows\System32\cmd.exe",
                &["/D".into(), "/C".into(), "echo hi".into()]
            ),
            "\"C:\\Windows\\System32\\cmd.exe\" /D /C \"echo hi\""
        );
    }

    #[test]
    fn 环境块构造() {
        let env = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        let block = f::build_environment_block(&env);
        let s: String = block
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap())
            .collect();
        assert_eq!(s, "A=1\u{0}B=2\u{0}\u{0}");
    }

    #[test]
    fn glob前缀提取() {
        assert_eq!(
            glob_fixed_prefix("C:/Windows/**"),
            Some("C:/Windows".to_string())
        );
        assert_eq!(
            glob_fixed_prefix("/etc/secret*"),
            Some("/etc/secret".to_string())
        );
        assert_eq!(glob_fixed_prefix("*"), None);
    }

    #[test]
    fn 管理员探测不崩溃() {
        let _ = is_admin();
    }
}
