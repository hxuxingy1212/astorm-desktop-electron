//! Windows FFI 声明（kernel32 / advapi32 / userenv / ntdll）。
//!
//! 自包含声明，避免依赖 windows-sys 的 feature 组合。仅声明本项目用到的 API。
//! 目标：x86_64 与 x86（PEB 偏移按架构区分）。

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    clippy::upper_case_acronyms
)]

use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type HANDLE = *mut c_void;
pub type DWORD = u32;
pub type WORD = u16;
pub type BOOL = i32;
pub type ULONG = u32;
pub type LPWSTR = *mut u16;
pub type LPCWSTR = *const u16;
pub type PSID = *mut c_void;

pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;
pub const INVALID_HANDLE_VALUE: HANDLE = (-1isize) as HANDLE;
pub const INFINITE: DWORD = 0xFFFF_FFFF;
pub const WAIT_OBJECT_0: DWORD = 0;
pub const WAIT_TIMEOUT: DWORD = 0x102;
pub const WAIT_FAILED: DWORD = 0xFFFF_FFFF;

// ---------------- 进程创建标志 ----------------
pub const DEBUG_PROCESS: DWORD = 0x1;
pub const CREATE_SUSPENDED: DWORD = 0x4;
pub const CREATE_UNICODE_ENVIRONMENT: DWORD = 0x400;
pub const CREATE_NO_WINDOW: DWORD = 0x0800_0000;
pub const STARTF_USESTDHANDLES: DWORD = 0x100;
pub const HANDLE_FLAG_INHERIT: DWORD = 0x1;

// ---------------- 调试事件 ----------------
pub const EXCEPTION_DEBUG_EVENT: DWORD = 1;
pub const CREATE_THREAD_DEBUG_EVENT: DWORD = 2;
pub const CREATE_PROCESS_DEBUG_EVENT: DWORD = 3;
pub const EXIT_THREAD_DEBUG_EVENT: DWORD = 4;
pub const EXIT_PROCESS_DEBUG_EVENT: DWORD = 5;
pub const LOAD_DLL_DEBUG_EVENT: DWORD = 6;
pub const UNLOAD_DLL_DEBUG_EVENT: DWORD = 7;
pub const OUTPUT_DEBUG_STRING_EVENT: DWORD = 8;
pub const RIP_EVENT: DWORD = 9;

pub const EXCEPTION_BREAKPOINT: DWORD = 0x8000_0003;
pub const DBG_CONTINUE: DWORD = 0x0001_0002;
pub const DBG_EXCEPTION_NOT_HANDLED: DWORD = 0x8001_0001;
pub const EXCEPTION_MAXIMUM_PARAMETERS: usize = 15;

// ---------------- 进程访问 ----------------
pub const PROCESS_TERMINATE: DWORD = 0x0001;
pub const PROCESS_QUERY_LIMITED_INFORMATION: DWORD = 0x1000;
pub const SYNCHRONIZE: DWORD = 0x0010_0000;

// ---------------- 令牌 ----------------
pub const TOKEN_ASSIGN_PRIMARY: DWORD = 0x0001;
pub const TOKEN_DUPLICATE: DWORD = 0x0002;
pub const TOKEN_IMPERSONATE: DWORD = 0x0004;
pub const TOKEN_QUERY: DWORD = 0x0008;
pub const MAXIMUM_ALLOWED: DWORD = 0x0200_0000;
pub const SecurityAnonymous: c_int = 0;
pub const SecurityIdentification: c_int = 1;
pub const SecurityImpersonation: c_int = 2;
pub const SecurityDelegation: c_int = 3;
pub const TokenPrimary: c_int = 1;
pub const TokenImpersonation: c_int = 2;
pub const TokenElevation: c_int = 20;

// ---------------- Job Object ----------------
pub const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: DWORD = 0x2000;
pub const JOB_OBJECT_LIMIT_PROCESS_TIME: DWORD = 0x2;
pub const JOB_OBJECT_LIMIT_JOB_TIME: DWORD = 0x4;
pub const JOB_OBJECT_LIMIT_PROCESS_MEMORY: DWORD = 0x100;
pub const JOB_OBJECT_LIMIT_JOB_MEMORY: DWORD = 0x200;
pub const JOBOBJECTINFOCLASS_ExtendedLimitInformation: c_int = 9;
pub const JOBOBJECTINFOCLASS_CpuRateControlInformation: c_int = 15;
pub const JOB_OBJECT_CPU_RATE_CONTROL_ENABLE: DWORD = 0x1;
pub const JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP: DWORD = 0x2;

// ---------------- 错误码 ----------------
pub const ERROR_ACCESS_DENIED: DWORD = 5;
pub const ERROR_PRIVILEGE_NOT_HELD: DWORD = 1314;

pub const STATUS_STILL_ACTIVE: DWORD = 259;

// ============================ 结构体 ============================

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SECURITY_ATTRIBUTES {
    pub nLength: DWORD,
    pub lpSecurityDescriptor: *mut c_void,
    pub bInheritHandle: BOOL,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct STARTUPINFOW {
    pub cb: DWORD,
    pub lpReserved: LPWSTR,
    pub lpDesktop: LPWSTR,
    pub lpTitle: LPWSTR,
    pub dwX: DWORD,
    pub dwY: DWORD,
    pub dwXSize: DWORD,
    pub dwYSize: DWORD,
    pub dwXCountChars: DWORD,
    pub dwYCountChars: DWORD,
    pub dwFillAttribute: DWORD,
    pub dwFlags: DWORD,
    pub wShowWindow: WORD,
    pub cbReserved2: WORD,
    pub lpReserved2: *mut u8,
    pub hStdInput: HANDLE,
    pub hStdOutput: HANDLE,
    pub hStdError: HANDLE,
}

impl Default for STARTUPINFOW {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PROCESS_INFORMATION {
    pub hProcess: HANDLE,
    pub hThread: HANDLE,
    pub dwProcessId: DWORD,
    pub dwThreadId: DWORD,
}

// ---------------- 调试事件 ----------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EXCEPTION_RECORD {
    pub ExceptionCode: DWORD,
    pub ExceptionFlags: DWORD,
    pub ExceptionRecord: *mut EXCEPTION_RECORD,
    pub ExceptionAddress: *mut c_void,
    pub NumberParameters: DWORD,
    pub ExceptionInformation: [usize; EXCEPTION_MAXIMUM_PARAMETERS],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EXCEPTION_DEBUG_INFO {
    pub ExceptionRecord: EXCEPTION_RECORD,
    pub dwFirstChance: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CREATE_THREAD_DEBUG_INFO {
    pub hThread: HANDLE,
    pub lpThreadLocalBase: *mut c_void,
    pub lpStartAddress: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CREATE_PROCESS_DEBUG_INFO {
    pub hFile: HANDLE,
    pub hProcess: HANDLE,
    pub hThread: HANDLE,
    pub lpBaseOfImage: *mut c_void,
    pub dwDebugInfoFileOffset: DWORD,
    pub nDebugInfoSize: DWORD,
    pub lpThreadLocalBase: *mut c_void,
    pub lpStartAddress: *mut c_void,
    pub lpImageName: *mut c_void,
    pub fUnicode: WORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EXIT_THREAD_DEBUG_INFO {
    pub dwExitCode: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EXIT_PROCESS_DEBUG_INFO {
    pub dwExitCode: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LOAD_DLL_DEBUG_INFO {
    pub hFile: HANDLE,
    pub lpBaseOfDll: *mut c_void,
    pub dwDebugInfoFileOffset: DWORD,
    pub nDebugInfoSize: DWORD,
    pub lpImageName: *mut c_void,
    pub fUnicode: WORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UNLOAD_DLL_DEBUG_INFO {
    pub lpBaseOfDll: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OUTPUT_DEBUG_STRING_INFO {
    pub lpDebugStringData: LPWSTR,
    pub fUnicode: WORD,
    pub nDebugStringLength: WORD,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RIP_INFO {
    pub dwError: DWORD,
    pub dwType: DWORD,
}

#[repr(C)]
pub union DEBUG_EVENT_U {
    pub Exception: EXCEPTION_DEBUG_INFO,
    pub CreateThread: CREATE_THREAD_DEBUG_INFO,
    pub CreateProcessInfo: CREATE_PROCESS_DEBUG_INFO,
    pub ExitThread: EXIT_THREAD_DEBUG_INFO,
    pub ExitProcess: EXIT_PROCESS_DEBUG_INFO,
    pub LoadDll: LOAD_DLL_DEBUG_INFO,
    pub UnloadDll: UNLOAD_DLL_DEBUG_INFO,
    pub OutputDebugString: OUTPUT_DEBUG_STRING_INFO,
    pub RipInfo: RIP_INFO,
}

#[repr(C)]
pub struct DEBUG_EVENT {
    pub dwDebugEventCode: DWORD,
    pub dwProcessId: DWORD,
    pub dwThreadId: DWORD,
    pub u: DEBUG_EVENT_U,
}

impl DEBUG_EVENT {
    pub fn zeroed() -> Self {
        DEBUG_EVENT {
            dwDebugEventCode: 0,
            dwProcessId: 0,
            dwThreadId: 0,
            u: DEBUG_EVENT_U {
                ExitProcess: EXIT_PROCESS_DEBUG_INFO { dwExitCode: 0 },
            },
        }
    }
}

// ---------------- PEB 读取 ----------------

#[repr(C)]
pub struct PROCESS_BASIC_INFORMATION {
    pub Reserved1: *mut c_void,
    pub PebBaseAddress: *mut c_void,
    pub Reserved2: [*mut c_void; 2],
    pub UniqueProcessId: usize,
    pub Reserved3: *mut c_void,
}

#[repr(C)]
pub struct UNICODE_STRING {
    pub Length: u16,
    pub MaximumLength: u16,
    pub Buffer: LPWSTR,
}

/// PEB 中 ProcessParameters 字段偏移。
#[cfg(target_arch = "x86_64")]
pub const PEB_PROCESS_PARAMETERS_OFFSET: usize = 0x20;
#[cfg(target_arch = "x86")]
pub const PEB_PROCESS_PARAMETERS_OFFSET: usize = 0x10;

/// RTL_USER_PROCESS_PARAMETERS 中 CommandLine（UNICODE_STRING）偏移。
#[cfg(target_arch = "x86_64")]
pub const PP_COMMANDLINE_OFFSET: usize = 0x70;
#[cfg(target_arch = "x86")]
pub const PP_COMMANDLINE_OFFSET: usize = 0x40;

// ---------------- 目标架构 / WOW64 ----------------
pub const IMAGE_FILE_MACHINE_UNKNOWN: u16 = 0;
pub const IMAGE_FILE_MACHINE_I386: u16 = 0x014c;
pub const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
pub const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;

/// 按**目标子进程架构**（不是宿主编译架构）返回 PEB/命令行的偏移。
///
/// x64 宿主调试 32 位子进程时必须用 x86 偏移，否则读到的是错误地址，
/// 动态危险命令拦截会静默漏掉所有 32 位后代。
pub fn peb_offsets_for_machine(machine: u16) -> Option<(usize, usize)> {
    match machine {
        IMAGE_FILE_MACHINE_I386 => Some((0x10, 0x40)),
        IMAGE_FILE_MACHINE_AMD64 => Some((0x20, 0x70)),
        // ARM64 的 PEB 布局与 x64 不同，未实现 → 调用方回退宿主偏移
        _ => None,
    }
}

/// 归一化为 Win32 扩展长度路径（`\\?\` / `\\?\UNC\`）。
///
/// `CreateProcessAsUserW` 的 `lpCurrentDirectory`、`SetNamedSecurityInfoW` 等
/// API 不走 Rust std 的长路径处理，超过 MAX_PATH 会失败。这里统一加前缀；
/// 相对路径原样返回。已带 `\\?\` 的不重复添加。
pub fn long_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy().replace('/', "\\");
    if s.starts_with(r"\\?\") {
        return PathBuf::from(s);
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{rest}"));
    }
    // 盘符绝对路径（C:\...）→ \\?\C:\...
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        return PathBuf::from(format!(r"\\?\{s}"));
    }
    path.to_path_buf()
}

/// MAX_PATH 兜底：取 8.3 短路径。CreateProcessAsUserW 的 `lpCurrentDirectory`
/// 不接受 `\\?\` 形式，超长 cwd 只能退化为 8.3 短路径。
pub fn short_path(path: &Path) -> Option<PathBuf> {
    let w = to_wide(&path.to_string_lossy());
    // 先问所需长度（含 NUL），再分配
    let need = unsafe { GetShortPathNameW(w.as_ptr(), std::ptr::null_mut(), 0) };
    if need == 0 {
        return None;
    }
    let mut buf = vec![0u16; need as usize];
    let got = unsafe { GetShortPathNameW(w.as_ptr(), buf.as_mut_ptr(), need) };
    if got == 0 || got >= need {
        return None;
    }
    buf.truncate(got as usize);
    let s = String::from_utf16_lossy(&buf);
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// 超长 cwd 的最终兜底：在短路径目录（会话临时目录）下建一个 junction
/// 指向长目录，再把 junction 当作 `lpCurrentDirectory`。
///
/// `CreateProcessAsUserW` 不接受 `\\?\` 形式的 cwd（ERROR_DIRECTORY），
/// 8.3 短名也可能不可用；junction 不需要管理员权限，且路径短。
pub fn junction_for_long_path(target: &Path, parent: &Path) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let link = parent.join(format!("cwd-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir(&link);
    let target_long = long_path(target);
    let status = Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target_long)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if status.success() && std::fs::read_dir(&link).is_ok() {
        Some(link)
    } else {
        None
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SYSTEMTIME {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDayOfWeek: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
    pub wMilliseconds: u16,
}

#[repr(C)]
pub struct TOKEN_ELEVATION {
    pub TokenIsElevated: DWORD,
}

#[repr(C)]
pub struct IO_COUNTERS {
    pub ReadOperationCount: u64,
    pub WriteOperationCount: u64,
    pub OtherOperationCount: u64,
    pub ReadTransferCount: u64,
    pub WriteTransferCount: u64,
    pub OtherTransferCount: u64,
}

#[repr(C)]
pub struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
    pub PerProcessUserTimeLimit: i64,
    pub PerJobUserTimeLimit: i64,
    pub LimitFlags: DWORD,
    pub MinimumWorkingSetSize: usize,
    pub MaximumWorkingSetSize: usize,
    pub ActiveProcessLimit: DWORD,
    pub Affinity: usize,
    pub PriorityClass: DWORD,
    pub SchedulingClass: DWORD,
}

#[repr(C)]
pub struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    pub BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION,
    pub IoInfo: IO_COUNTERS,
    pub ProcessMemoryLimit: usize,
    pub JobMemoryLimit: usize,
    pub PeakProcessMemoryUsed: usize,
    pub PeakJobMemoryUsed: usize,
}

/// CPU 使用率配额（CpuRate 单位 = 百分比 × 100，如 50% = 5000）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
    pub ControlFlags: DWORD,
    pub CpuRate: DWORD,
}

// ============================ 函数 ============================

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn CloseHandle(hObject: HANDLE) -> BOOL;
    pub fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: DWORD) -> DWORD;
    pub fn TerminateProcess(hProcess: HANDLE, uExitCode: u32) -> BOOL;
    pub fn OpenProcess(dwDesiredAccess: DWORD, bInheritHandle: BOOL, dwProcessId: DWORD) -> HANDLE;
    pub fn GetExitCodeProcess(hProcess: HANDLE, lpExitCode: *mut DWORD) -> BOOL;
    pub fn ReadProcessMemory(
        hProcess: HANDLE,
        lpBaseAddress: *const c_void,
        lpBuffer: *mut c_void,
        nSize: usize,
        lpNumberOfBytesRead: *mut usize,
    ) -> BOOL;
    pub fn CreateJobObjectW(lpJobAttributes: *const SECURITY_ATTRIBUTES, lpName: LPCWSTR)
        -> HANDLE;
    pub fn SetInformationJobObject(
        hJob: HANDLE,
        JobObjectInformationClass: c_int,
        lpJobObjectInformation: *mut c_void,
        cbJobObjectInformationLength: DWORD,
    ) -> BOOL;
    pub fn AssignProcessToJobObject(hJob: HANDLE, hProcess: HANDLE) -> BOOL;
    pub fn TerminateJobObject(hJob: HANDLE, uExitCode: u32) -> BOOL;
    pub fn SetHandleInformation(hObject: HANDLE, dwMask: DWORD, dwFlags: DWORD) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn WaitForDebugEvent(lpDebugEvent: *mut DEBUG_EVENT, dwMilliseconds: DWORD) -> BOOL;
    pub fn ContinueDebugEvent(
        dwProcessId: DWORD,
        dwThreadId: DWORD,
        dwContinueStatus: DWORD,
    ) -> BOOL;
    pub fn GetLastError() -> DWORD;
    pub fn IsWow64Process2(
        hProcess: HANDLE,
        pProcessMachine: *mut u16,
        pNativeMachine: *mut u16,
    ) -> BOOL;
    pub fn GetShortPathNameW(
        lpszLongPath: LPCWSTR,
        lpszShortPath: LPWSTR,
        cchBuffer: DWORD,
    ) -> DWORD;
    pub fn LoadLibraryW(lpFileName: LPCWSTR) -> HANDLE;
    pub fn GetProcAddress(hModule: HANDLE, lpProcName: *const c_char) -> *mut c_void;
    pub fn GetLocalTime(lpSystemTime: *mut SYSTEMTIME);
    pub fn SetConsoleOutputCP(wCodePageID: u32) -> BOOL;
    pub fn GetOEMCP() -> u32;
    pub fn MultiByteToWideChar(
        CodePage: u32,
        dwFlags: DWORD,
        lpMultiByteStr: *const u8,
        cbMultiByte: c_int,
        lpWideCharStr: LPWSTR,
        cchWideChar: c_int,
    ) -> c_int;
}

const CP_OEMCP: u32 = 1; // 系统当前 OEM 代码页

/// 按系统 OEM 代码页（中文 Windows = 936/GBK）把字节流解码为字符串。
pub fn decode_oem(bytes: &[u8]) -> String {
    unsafe {
        let cp = GetOEMCP();
        let cp = if cp == 0 { CP_OEMCP } else { cp };
        let len = MultiByteToWideChar(
            cp,
            0,
            bytes.as_ptr(),
            bytes.len() as c_int,
            std::ptr::null_mut(),
            0,
        );
        if len <= 0 {
            return String::from_utf8_lossy(bytes).to_string();
        }
        let mut wide = vec![0u16; len as usize];
        MultiByteToWideChar(
            cp,
            0,
            bytes.as_ptr(),
            bytes.len() as c_int,
            wide.as_mut_ptr(),
            len,
        );
        String::from_utf16_lossy(&wide)
    }
}

/// 控制台输出代码页切换为 UTF-8（中文 Windows 默认 GBK，避免输出乱码）。
pub fn enable_utf8_console() {
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

#[link(name = "advapi32")]
unsafe extern "system" {
    pub fn OpenProcessToken(
        ProcessHandle: HANDLE,
        DesiredAccess: DWORD,
        TokenHandle: *mut HANDLE,
    ) -> BOOL;
    pub fn GetTokenInformation(
        TokenHandle: HANDLE,
        TokenInformationClass: c_int,
        TokenInformation: *mut c_void,
        TokenInformationLength: DWORD,
        ReturnLength: *mut DWORD,
    ) -> BOOL;
    pub fn DuplicateTokenEx(
        hExistingToken: HANDLE,
        dwDesiredAccess: DWORD,
        lpTokenAttributes: *const SECURITY_ATTRIBUTES,
        ImpersonationLevel: c_int,
        TokenType: c_int,
        phNewToken: *mut HANDLE,
    ) -> BOOL;
    pub fn ConvertSidToStringSidW(Sid: PSID, StringSid: *mut LPWSTR) -> BOOL;
    pub fn LocalFree(hMem: HANDLE) -> HANDLE;
}

// CreateProcessAsUserW 与 CreateProcessWithTokenW 在 advapi32
#[link(name = "advapi32")]
unsafe extern "system" {
    pub fn CreateProcessAsUserW(
        hToken: HANDLE,
        lpApplicationName: LPCWSTR,
        lpCommandLine: LPWSTR,
        lpProcessAttributes: *const SECURITY_ATTRIBUTES,
        lpThreadAttributes: *const SECURITY_ATTRIBUTES,
        bInheritHandles: BOOL,
        dwCreationFlags: DWORD,
        lpEnvironment: *mut c_void,
        lpCurrentDirectory: LPCWSTR,
        lpStartupInfo: *mut STARTUPINFOW,
        lpProcessInformation: *mut PROCESS_INFORMATION,
    ) -> BOOL;
    pub fn CreateProcessWithTokenW(
        hToken: HANDLE,
        dwLogonFlags: DWORD,
        lpApplicationName: LPCWSTR,
        lpCommandLine: LPWSTR,
        dwCreationFlags: DWORD,
        lpEnvironment: *mut c_void,
        lpCurrentDirectory: LPCWSTR,
        lpStartupInfo: *mut STARTUPINFOW,
        lpProcessInformation: *mut PROCESS_INFORMATION,
    ) -> BOOL;
}

// AppContainer API 在 userenv（CreateAppContainerToken 依 SDK 版本不同可能不在
// 静态库中，统一走运行时解析，见 get_create_appcontainer_token）
#[link(name = "userenv")]
unsafe extern "system" {
    pub fn CreateAppContainerProfile(
        pszAppContainerName: LPCWSTR,
        pszDisplayName: LPCWSTR,
        pszDescription: LPCWSTR,
        pCapabilities: *mut c_void,
        dwCapabilityCount: DWORD,
        ppSidAppContainerSid: *mut PSID,
    ) -> i32; // HRESULT
    pub fn DeleteAppContainerProfile(pszAppContainerName: LPCWSTR) -> i32; // HRESULT
    pub fn DeriveAppContainerSidFromAppContainerName(
        pszAppContainerName: LPCWSTR,
        ppsidAppContainerSid: *mut PSID,
    ) -> i32; // HRESULT
    pub fn GetAppContainerFolderPath(
        psidAppContainerSid: PSID,
        pdwPathLength: *mut DWORD,
        pszPath: LPWSTR,
    ) -> BOOL;
}

/// CreateAppContainerToken 的函数签名（备用；主路径为 NtCreateLowBoxToken）。
pub type CreateAppContainerTokenFn = unsafe extern "system" fn(
    hToken: HANDLE,
    psidAppContainerSid: PSID,
    hExtraCapabilities: HANDLE,
    CapabilityCount: DWORD,
    pCapabilities: *mut c_void,
    phToken: *mut HANDLE,
) -> BOOL;

/// OBJECT_ATTRIBUTES（NtCreateLowBoxToken 入参；x64=48 字节，x86=24 字节）。
#[repr(C)]
pub struct OBJECT_ATTRIBUTES {
    pub Length: u32,
    pub RootDirectory: HANDLE,
    pub ObjectName: *mut c_void,
    pub Attributes: u32,
    pub SecurityDescriptor: *mut c_void,
    pub SecurityQualityOfService: *mut c_void,
}

/// SID_AND_ATTRIBUTES（能力列表项）。
#[repr(C)]
pub struct SID_AND_ATTRIBUTES {
    pub Sid: PSID,
    pub Attributes: u32,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    pub fn NtQueryInformationProcess(
        ProcessHandle: HANDLE,
        ProcessInformationClass: c_int,
        ProcessInformation: *mut c_void,
        ProcessInformationLength: u32,
        ReturnLength: *mut u32,
    ) -> i32;
    /// 从现有令牌创建 LowBox（AppContainer）令牌 —— Chromium 沙箱同款路径，
    /// 返回令牌类型与基准令牌一致（传主令牌则得到主令牌，可直接 CreateProcessAsUser）。
    pub fn NtCreateLowBoxToken(
        TokenHandle: *mut HANDLE,
        ExistingTokenHandle: HANDLE,
        DesiredAccess: u32,
        ObjectAttributes: *mut OBJECT_ATTRIBUTES,
        PackageSid: PSID,
        CapabilityCount: u32,
        Capabilities: *mut SID_AND_ATTRIBUTES,
        HandleCount: u32,
        Handles: *mut HANDLE,
    ) -> i32; // NTSTATUS
}

pub const PROCESS_BASIC_INFORMATION_CLASS: c_int = 0;

// ============================ 辅助 ============================

/// 把 UTF-8 字符串转为以 NUL 结尾的 UTF-16 缓冲。
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 把宽字符缓冲转为 UTF-8（按 NUL 或长度截断）。
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// 构造 Windows 环境块（UTF-16，双 NUL 结尾）。
pub fn build_environment_block(env: &[(String, String)]) -> Vec<u16> {
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in env {
        for unit in format!("{k}={v}").encode_utf16() {
            block.push(unit);
        }
        block.push(0);
    }
    if block.is_empty() {
        block.push(0); // 空环境也需要一个终止 NUL（再加一个双终止）
    }
    block.push(0);
    block
}

/// 引用单个参数（含空格/引号时加引号，内部引号转义）。
pub fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }
    quote_always(arg)
}

/// 恒定加引号（用于程序路径，确保含空格路径安全）。
pub fn quote_always(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// 构造完整命令行（程序名恒定加引号 + 参数按需引用）。
pub fn build_command_line(program: &str, args: &[String]) -> String {
    let mut s = quote_always(program);
    for a in args {
        s.push(' ');
        s.push_str(&quote_arg(a));
    }
    s
}

/// 读取进程当前错误码（GetLastError 包装）。
#[inline]
pub fn last_error() -> DWORD {
    unsafe { GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_path_adds_verbatim_prefix() {
        assert_eq!(
            long_path(Path::new(r"C:\tmp\x")),
            PathBuf::from(r"\\?\C:\tmp\x")
        );
        assert_eq!(
            long_path(Path::new("C:/tmp/x")),
            PathBuf::from(r"\\?\C:\tmp\x")
        );
        assert_eq!(
            long_path(Path::new(r"\\server\share\x")),
            PathBuf::from(r"\\?\UNC\server\share\x")
        );
        // 已带前缀不重复添加
        assert_eq!(
            long_path(Path::new(r"\\?\C:\tmp\x")),
            PathBuf::from(r"\\?\C:\tmp\x")
        );
        // 相对路径原样
        assert_eq!(long_path(Path::new(r"relative\x")), PathBuf::from(r"relative\x"));
    }

    #[test]
    fn peb_offsets_follow_target_arch() {
        assert_eq!(peb_offsets_for_machine(IMAGE_FILE_MACHINE_I386), Some((0x10, 0x40)));
        assert_eq!(peb_offsets_for_machine(IMAGE_FILE_MACHINE_AMD64), Some((0x20, 0x70)));
        // 未支持架构 → None（调用方回退宿主偏移）
        assert_eq!(peb_offsets_for_machine(IMAGE_FILE_MACHINE_ARM64), None);
    }

    #[test]
    fn short_path_for_long_dir() {
        let base = std::env::temp_dir().join(format!(
            "astorm-shortpath-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut dir = base.clone();
        for _ in 0..8 {
            dir.push("0123456789abcdef0123456789abcdef");
        }
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if dir.to_string_lossy().chars().count() > 260 {
            // 8.3 被禁用时返回 None 属正常；启用时应给出 <= MAX_PATH 的短路径
            if let Some(short) = short_path(&dir) {
                assert!(short.to_string_lossy().chars().count() <= 260, "{short:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn junction_for_long_path_works() {
        let base = std::env::temp_dir().join(format!(
            "astorm-junction-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut dir = base.clone();
        for _ in 0..8 {
            dir.push("0123456789abcdef0123456789abcdef");
        }
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let link = junction_for_long_path(&dir, &std::env::temp_dir());
        assert!(link.is_some(), "junction fallback should work");
        let link = link.unwrap();
        assert!(std::fs::read_dir(&link).is_ok());
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&base);
    }
}
