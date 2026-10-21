//! 执行器：同步（即时结果）与异步（事件日志流）编排。
//!
//! 流程（防护开启）：
//! 1. 编译策略 → 能力探测（Capabilities 事件，绝不静默降级）
//! 2. 静态预检（危险命令 / 文件黑名单 / 网络黑名单），命中即拒绝启动
//! 3. 严格模式能力校验（不足则失败）
//! 4. 平台强制层 init → 会话（AppContainer / Seatbelt / Passthrough）
//! 5. 构建 ChildSpec（解释器参数、内置运行时 PATH 注入、管道）
//! 6. spawn → 事件循环（输出流、spawn/denied/exit、超时→整树强杀、驱逐）
//! 7. teardown → 汇总 ExecReport

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::analyzer;
use crate::analyzer::PreflightDeny;
use crate::enforcement::FieldEnforcement;
use crate::policy::{ExecMode, ResolvedPolicy, SandboxInput};
use crate::runtime::RuntimeResolver;
use crate::sandbox::{
    default_backend, Category, ChildSpec, Event, PassthroughBackend, SandboxBackend,
    SandboxCapabilities, SandboxSession, StreamKind, WaitOutcome, MAX_OUTPUT_BYTES,
};

/// 一次执行的完整报告（对应需求输出：执行时间、执行结果）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecReport {
    /// 开始时间（ISO8601 风格，本地时区）
    pub started_at: String,
    /// 结束时间（ISO8601 风格，本地时区）
    pub ended_at: String,
    /// 执行耗时（毫秒）
    pub elapsed_ms: u64,
    /// 执行命令
    pub command: String,
    /// 操作类型
    pub mode: ExecMode,
    /// 防护开关
    pub protection_on: bool,
    /// 严格模式
    pub strict: bool,
    /// 实际使用的后端名
    pub backend: String,
    /// 平台强制能力（第一条事件）
    pub capabilities: Option<SandboxCapabilities>,
    /// 策略字段级强制力自曝（`enforcement::FieldEnforcement`）。
    /// 防护开启且策略编译成功时填充；防护关闭/编译失败为空（无策略可报告）。
    pub enforcement: Vec<FieldEnforcement>,
    /// 退出码（未启动或被预检拦截时为 None）
    pub exit_code: Option<i32>,
    /// 合并输出（stdout + stderr，8MB 截断）
    pub output: String,
    /// stderr 独立副本（与 `output` 同源数据，供上层把错误流与正常输出分开呈现）
    pub stderr: String,
    /// 因超时强制终止进程树
    pub killed: bool,
    /// 因运行时驱逐（网络/命令违规）终止
    pub evicted: bool,
    /// 预检/严格模式拦截原因（未执行命令）
    pub prevented: Vec<String>,
    /// 预检拦截项（带违规类别；与 `prevented` 同源，供上层分类使用）
    pub denials: Vec<PreflightDeny>,
    /// 事件日志
    pub events: Vec<Event>,
    /// 便捷结论：正常完成且无违规
    pub ok: bool,
}

/// 同步执行：等待完成并返回完整即时结果。
pub fn run_sync(input: SandboxInput) -> ExecReport {
    run_impl(input, Box::new(|_| {}))
}

/// 异步执行：返回 (任务句柄, 事件日志流)。
/// 事件流实时推送 Chunk / Spawn / Denied / Evicted / Exit / Finished 等；
/// 最后一次事件为 Finished，随后可 join 拿到完整报告。
pub fn run_async(input: SandboxInput) -> (JoinHandle<ExecReport>, Receiver<Event>) {
    let (tx, rx) = channel::<Event>();
    let handle = std::thread::Builder::new()
        .name("astorm-runner".to_string())
        .spawn(move || {
            run_impl(
                input,
                Box::new(move |ev| {
                    let _ = tx.send(ev.clone());
                }),
            )
        })
        .expect("创建执行线程失败");
    (handle, rx)
}

/// 记录 + 实时回调的事件收集器（多线程共享）。
struct EventAccum {
    events: Arc<Mutex<Vec<Event>>>,
    sink: Arc<Mutex<Box<dyn FnMut(&Event) + Send>>>,
    truncated: AtomicBool,
    output_bytes: Arc<Mutex<usize>>,
    /// 合并输出（stdout + stderr）
    output: Arc<Mutex<String>>,
    /// stderr 独立副本（对外分流通用；`output` 仍保留合并语义）
    stderr: Arc<Mutex<String>>,
}

impl EventAccum {
    fn new(sink: Box<dyn FnMut(&Event) + Send>) -> Self {
        EventAccum {
            events: Arc::new(Mutex::new(Vec::new())),
            sink: Arc::new(Mutex::new(sink)),
            truncated: AtomicBool::new(false),
            output_bytes: Arc::new(Mutex::new(0)),
            output: Arc::new(Mutex::new(String::new())),
            stderr: Arc::new(Mutex::new(String::new())),
        }
    }

    /// 克隆一个指向同一存储的工作副本（供转发线程使用）。
    fn clone_handle(&self) -> EventAccum {
        EventAccum {
            events: self.events.clone(),
            sink: self.sink.clone(),
            truncated: AtomicBool::new(self.truncated.load(Ordering::SeqCst)),
            output_bytes: self.output_bytes.clone(),
            output: self.output.clone(),
            stderr: self.stderr.clone(),
        }
    }

    /// 记录事件（进入 events + 实时回调）。
    fn emit(&self, ev: Event) {
        self.events.lock().unwrap().push(ev.clone());
        if let Ok(mut s) = self.sink.lock() {
            s(&ev);
        }
    }

    /// 追加输出（带 8MB 上限截断）。返回是否被接受。
    fn push_output(&self, stream: StreamKind, data: &str) -> bool {
        let mut bytes = self.output_bytes.lock().unwrap();
        if *bytes + data.len() > MAX_OUTPUT_BYTES {
            drop(bytes);
            if !self.truncated.swap(true, Ordering::SeqCst) {
                self.emit(Event::OutputTruncated);
            }
            return false;
        }
        *bytes += data.len();
        self.output.lock().unwrap().push_str(data);
        if stream == StreamKind::Stderr {
            self.stderr.lock().unwrap().push_str(data);
        }
        true
    }

    fn snapshot(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    fn output_text(&self) -> String {
        self.output.lock().unwrap().clone()
    }

    fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }
}

/// 统一收尾：发出 Finished 事件并快照全部事件。
fn finalize(mut report: ExecReport, accum: &EventAccum) -> ExecReport {
    accum.emit(Event::Finished);
    report.events = accum.snapshot();
    // stderr 独立副本：合并 output 照旧（兼容既有消费方），上层可分流呈现
    report.stderr = accum.stderr_text();
    // 驱逐事件也计入失败：监视为轮询式，进程可能在终止落地前已自行退出，
    // 但违规确已发生（如内存超限），报告必须如实反映。
    report.ok = report.exit_code == Some(0)
        && !report.killed
        && !report.evicted
        && report.prevented.is_empty()
        && !report.events.iter().any(|e| {
            matches!(
                e,
                Event::Denied { .. } | Event::Evicted { .. } | Event::Error(_)
            )
        });
    report
}

fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    local_time_string(secs)
}

fn elapsed_ms(started: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(started)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn run_impl(input: SandboxInput, sink: Box<dyn FnMut(&Event) + Send>) -> ExecReport {
    let started = SystemTime::now();
    let accum = EventAccum::new(sink);
    let deadline = Instant::now() + input.timeout;

    let mut report = ExecReport {
        started_at: now_stamp(),
        ended_at: String::new(),
        elapsed_ms: 0,
        command: input.command.clone(),
        mode: input.mode,
        protection_on: input.protection.enabled,
        strict: input.strict,
        backend: String::new(),
        capabilities: None,
        enforcement: Vec::new(),
        exit_code: None,
        output: String::new(),
        stderr: String::new(),
        killed: false,
        evicted: false,
        prevented: Vec::new(),
        denials: Vec::new(),
        events: Vec::new(),
        ok: false,
    };

    let done = |mut report: ExecReport| -> ExecReport {
        report.ended_at = now_stamp();
        report.elapsed_ms = elapsed_ms(started);
        finalize(report, &accum)
    };

    // ---------- 1. 编译策略 ----------
    let policy = match ResolvedPolicy::compile(&input) {
        Ok(p) => p,
        Err(e) => {
            report.backend = "none".to_string();
            report.prevented.push(format!("策略编译失败：{e}"));
            accum.emit(Event::Error(format!("策略编译失败：{e}")));
            return done(report);
        }
    };

    // 字段级强制力自曝（只描述策略本身；单次执行的降级看 backend/capabilities）。
    // 防护关闭 = 没有策略在生效：保持空数组，工具结果里用 protected=false 表达。
    if policy.enabled {
        report.enforcement = policy.enforcement_report();
    }

    // ---------- 2. 后端与能力探测（绝不静默降级） ----------
    let backend: Box<dyn SandboxBackend> = if policy.enabled {
        default_backend()
    } else {
        Box::new(PassthroughBackend::new("防护已关闭，直接执行不检测"))
    };
    let caps = backend.capabilities();
    report.capabilities = Some(caps.clone());
    report.backend = backend.name().to_string();
    accum.emit(Event::Capabilities(caps.clone()));
    for r in &caps.degraded_reasons {
        accum.emit(Event::Note(format!("能力说明：{r}")));
    }

    if policy.enabled {
        // ---------- 3. 静态预检：危险命令 / 文件黑名单 / 网络黑名单 ----------
        let denies = analyzer::preflight(&policy, &input.command);
        for d in &denies {
            accum.emit(Event::Denied {
                category: d.category,
                detail: d.detail.clone(),
            });
        }
        if !denies.is_empty() {
            report.prevented = denies.iter().map(|d| d.detail.clone()).collect();
            report.denials = denies;
            accum.emit(Event::Note("预检拦截：命令未被执行".to_string()));
            return done(report);
        }

        // ---------- 4. 严格模式能力校验 ----------
        if policy.strict && !caps.ok_for_strict() {
            let msg = format!(
                "严格模式要求完整内核强制，当前能力：{}；未启用项：{}",
                caps.summary(),
                caps.degraded_reasons.join("；")
            );
            accum.emit(Event::Error(msg.clone()));
            report.prevented.push(msg);
            return done(report);
        }
    }

    // ---------- 5. 会话初始化（平台强制层） ----------
    let mut session: Box<dyn SandboxSession> = match backend.init(&policy, input.cancel.clone()) {
        Ok(s) => s,
        Err(e) => {
            if policy.strict {
                let msg = format!("强制层初始化失败且处于严格模式：{}", e.message);
                accum.emit(Event::Error(msg.clone()));
                report.prevented.push(msg);
                return done(report);
            }
            accum.emit(Event::Note(format!(
                "强制层初始化失败（{}），降级为直通执行（明确告警，非静默）",
                e.message
            )));
            let pb = PassthroughBackend::new(&format!("强制层不可用：{}", e.message));
            // 降级后的能力以直通后端为准（诚实上报，覆盖此前的能力事件）
            let pb_caps = pb.capabilities();
            report.capabilities = Some(pb_caps.clone());
            report.backend = "passthrough".to_string();
            accum.emit(Event::Capabilities(pb_caps));
            match pb.init(&policy, input.cancel.clone()) {
                Ok(s) => {
                    report.backend = "passthrough".to_string();
                    s
                }
                Err(e2) => {
                    accum.emit(Event::Error(format!(
                        "直通执行初始化也失败：{}",
                        e2.message
                    )));
                    report
                        .prevented
                        .push(format!("直通执行初始化失败：{}", e2.message));
                    return done(report);
                }
            }
        }
    };

    // ---------- 6. 构建 ChildSpec ----------
    let (program, args) = resolve_shell(&input);
    let program = resolve_runtime_program(&program);
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    inject_runtime_path(&mut env);
    env.extend(session.extra_env());
    // 去重：会话注入（如 TMPDIR/HOME 重定向到沙箱目录）必须覆盖系统同名变量，
    // 保留最后一次出现的值（Win 环境块 / macOS envp 都按此语义取值）。
    env = dedup_env(env);

    let (out_r, out_w) = match os_pipe::pipe() {
        Ok(p) => p,
        Err(e) => {
            accum.emit(Event::Error(format!("创建 stdout 管道失败：{e}")));
            let _ = session.teardown();
            return done(report);
        }
    };
    let (err_r, err_w) = match os_pipe::pipe() {
        Ok(p) => p,
        Err(e) => {
            accum.emit(Event::Error(format!("创建 stderr 管道失败：{e}")));
            let _ = session.teardown();
            return done(report);
        }
    };
    let spec = ChildSpec {
        program: program.clone(),
        args,
        cwd: input.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        env,
        stdout: Some(pipe_writer_to_file(out_w)),
        stderr: Some(pipe_writer_to_file(err_w)),
        stdin: None,
        raw_command_arg: input
            .shell
            .raw_command_arg
            .unwrap_or_else(|| is_cmd_shell(&program)),
    };

    // ---------- 7. spawn ----------
    if let Err(e) = session.spawn(spec) {
        accum.emit(Event::Error(format!("进程启动失败：{}", e.message)));
        let _ = session.teardown();
        return done(report);
    }

    // ---------- 8. 输出读取线程 + 会话事件转发线程 ----------
    let (chunk_tx, chunk_rx) = channel::<Event>();
    spawn_reader(
        pipe_reader_to_file(out_r),
        StreamKind::Stdout,
        chunk_tx.clone(),
        input.output_encoding,
    );
    spawn_reader(
        pipe_reader_to_file(err_r),
        StreamKind::Stderr,
        chunk_tx,
        input.output_encoding,
    );

    let session_rx = session.event_receiver();
    let fwd = {
        let accum = accum.clone_handle();
        // done：等待阶段结束的信号。会话对象持有通道 sender 直到 teardown（晚于 join），
        // 因此转发线程不能依赖 Disconnected 收尾，改用 recv_timeout + done 标志。
        let done = Arc::new(AtomicBool::new(false));
        let done_for_runner = done.clone();
        let thread = std::thread::Builder::new()
            .name("astorm-fwd".to_string())
            .spawn(move || loop {
                match session_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(ev) => {
                        let ev = match ev {
                            Event::Chunk { stream, data } => {
                                if accum.push_output(stream, &data) {
                                    Event::Chunk { stream, data }
                                } else {
                                    continue;
                                }
                            }
                            other => other,
                        };
                        accum.emit(ev);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if done.load(Ordering::SeqCst) {
                            // 排空剩余事件后退出
                            while let Ok(ev) = session_rx.try_recv() {
                                let ev = match ev {
                                    Event::Chunk { stream, data } => {
                                        if accum.push_output(stream, &data) {
                                            Event::Chunk { stream, data }
                                        } else {
                                            continue;
                                        }
                                    }
                                    other => other,
                                };
                                accum.emit(ev);
                            }
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            })
            .expect("创建会话事件转发线程失败");
        (thread, done_for_runner)
    };
    let (fwd, fwd_done) = fwd;
    let out_thread = {
        let accum = accum.clone_handle();
        let done = fwd_done.clone();
        std::thread::Builder::new()
            .name("astorm-output".to_string())
            .spawn(move || loop {
                match chunk_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(ev) => {
                        let ev = match ev {
                            Event::Chunk { stream, data } => {
                                if accum.push_output(stream, &data) {
                                    Event::Chunk { stream, data }
                                } else {
                                    continue;
                                }
                            }
                            other => other,
                        };
                        accum.emit(ev);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if done.load(Ordering::SeqCst) {
                            while let Ok(ev) = chunk_rx.try_recv() {
                                let ev = match ev {
                                    Event::Chunk { stream, data } => {
                                        if accum.push_output(stream, &data) {
                                            Event::Chunk { stream, data }
                                        } else {
                                            continue;
                                        }
                                    }
                                    other => other,
                                };
                                accum.emit(ev);
                            }
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            })
            .expect("创建输出线程失败")
    };

    // ---------- 9. 等待 / 超时 / 驱逐 ----------
    match session.wait(deadline.saturating_duration_since(Instant::now())) {
        Ok(WaitOutcome::Exited { code }) => {
            report.exit_code = Some(code);
        }
        Ok(WaitOutcome::Timeout) => {
            report.killed = true;
            accum.emit(Event::TimeoutKilled);
            let _ = session.kill_tree();
            if let Ok(WaitOutcome::Exited { code }) = session.wait(Duration::from_secs(2)) {
                report.exit_code = Some(code);
            }
        }
        Ok(WaitOutcome::Cancelled) => {
            report.killed = true;
            accum.emit(Event::Note("执行被取消".to_string()));
            let _ = session.kill_tree();
        }
        Ok(WaitOutcome::Evicted) => {
            report.evicted = true;
            let _ = session.kill_tree();
            if let Ok(WaitOutcome::Exited { code }) = session.wait(Duration::from_secs(2)) {
                report.exit_code = Some(code);
            }
        }
        Err(e) => {
            accum.emit(Event::Error(format!("等待进程失败：{}", e.message)));
        }
    }

    // 等待阶段结束：通知转发线程收尾，随后等待输出线程（EOF 即退出）
    fwd_done.store(true, Ordering::SeqCst);
    let _ = fwd.join();
    let _ = out_thread.join();

    // ---------- 9.5 资源限制归因（rlimit 信号只有退出码，无平台事件） ----------
    // 磁盘写入超限 → SIGXFSZ(25)、CPU 时间超限 → SIGXCPU(24)；shell 约定退出码 = 128+信号。
    // 仅在确实配置了该限制时才归因，避免把用户命令自身的退出码误判为资源拦截。
    if policy.enabled && !report.killed && !report.evicted {
        if let Some(code) = report.exit_code {
            let res = &policy.resource;
            let hit = if res.disk_mb.is_some() && code == 128 + 25 {
                Some("磁盘写入超过单文件上限（SIGXFSZ）")
            } else if res.cpu_seconds.is_some() && code == 128 + 24 {
                Some("CPU 时间超过上限（SIGXCPU）")
            } else {
                None
            };
            if let Some(detail) = hit {
                accum.emit(Event::Denied {
                    category: Category::Resource,
                    detail: detail.to_string(),
                });
            }
        }
    }

    // ---------- 10. teardown ----------
    let _ = session.teardown();

    // ---------- 11. 汇总 ----------
    report.output = accum.output_text();
    done(report)
}

/// os_pipe 写端 → std File（取得句柄所有权）。
fn pipe_writer_to_file(w: os_pipe::PipeWriter) -> std::fs::File {
    #[cfg(windows)]
    {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        let raw = w.into_raw_handle();
        unsafe { std::fs::File::from_raw_handle(raw) }
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::{FromRawFd, IntoRawFd};
        let fd = w.into_raw_fd();
        unsafe { std::fs::File::from_raw_fd(fd) }
    }
}

/// os_pipe 读端 → std File（取得句柄所有权）。
fn pipe_reader_to_file(r: os_pipe::PipeReader) -> std::fs::File {
    #[cfg(windows)]
    {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        let raw = r.into_raw_handle();
        unsafe { std::fs::File::from_raw_handle(raw) }
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::{FromRawFd, IntoRawFd};
        let fd = r.into_raw_fd();
        unsafe { std::fs::File::from_raw_fd(fd) }
    }
}

/// 把管道读端读成 chunk 事件（按编码策略解码，逐块推送）。
fn spawn_reader(
    mut read: std::fs::File,
    stream: StreamKind,
    tx: Sender<Event>,
    encoding: crate::policy::OutputEncoding,
) {
    std::thread::Builder::new()
        .name(format!("astorm-reader-{stream:?}"))
        .spawn(move || {
            let mut buf = [0u8; 4096];
            let mut pending: Vec<u8> = Vec::new(); // 未完成的多字节序列残留
            loop {
                let n = match read.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                pending.extend_from_slice(&buf[..n]);
                let text = decode_chunk(&mut pending, encoding, n == buf.len());
                if !text.is_empty() && tx.send(Event::Chunk { stream, data: text }).is_err() {
                    break;
                }
            }
            // 收尾：残留字节按最终策略解码
            if !pending.is_empty() {
                let text = decode_chunk(&mut pending, encoding, false);
                let _ = tx.send(Event::Chunk { stream, data: text });
            }
        })
        .expect("创建读取线程失败");
}

/// 解码一块字节：auto = 先试 UTF-8 严格，失败按系统 OEM 代码页（中文 Windows = GBK）。
/// `may_continue` 表示后面还有数据（保留可能的跨块多字节序列）。
fn decode_chunk(
    pending: &mut Vec<u8>,
    encoding: crate::policy::OutputEncoding,
    may_continue: bool,
) -> String {
    use crate::policy::OutputEncoding as Enc;
    // UTF-8 严格解码；失败信息里给出"消费到多少合法前缀"
    let decode_utf8 = |bytes: &[u8]| -> Option<(String, usize)> {
        match std::str::from_utf8(bytes) {
            Ok(s) => Some((s.to_string(), bytes.len())),
            Err(e) if e.valid_up_to() > 0 => Some((
                String::from_utf8_lossy(&bytes[..e.valid_up_to()]).to_string(),
                e.valid_up_to(),
            )),
            Err(_) => None,
        }
    };
    let take = |consumed: usize, buf: &mut Vec<u8>| buf.drain(..consumed).for_each(drop);

    match encoding {
        Enc::Utf8 => {
            if let Some((s, used)) = decode_utf8(pending) {
                take(used, pending);
                if used == 0 && !pending.is_empty() {
                    pending.clear(); // 非法字节，丢弃避免死循环
                    return "\u{FFFD}".to_string();
                }
                return s;
            }
            pending.clear();
            "\u{FFFD}".to_string()
        }
        Enc::Oem => {
            let s = oem_decode(pending);
            pending.clear();
            s
        }
        Enc::Auto => {
            // 完整 UTF-8 → utf8；否则若看似 GBK（含高位字节且 UTF-8 首前缀失败）→ OEM
            if let Ok(s) = std::str::from_utf8(pending) {
                let s = s.to_string();
                pending.clear();
                return s;
            }
            // 不完整的多字节序列可能在下一块补全：仅当"非法点不在末尾附近"时判定为非 UTF-8
            if let Some((s, used)) = decode_utf8(pending) {
                take(used, pending);
                if used == 0 && !pending.is_empty() && may_continue {
                    // 首字节即非法：可能是 GBK 双字节首字节（0x81-0xFE），等待/走 OEM
                    if pending[0] < 0x80 {
                        pending.clear();
                        return "\u{FFFD}".to_string();
                    }
                    // 交给 OEM（OEM 解码整块）
                    let s = oem_decode(pending);
                    pending.clear();
                    return s;
                }
                return s;
            }
            // 完全非法 UTF-8 → OEM
            let s = oem_decode(pending);
            pending.clear();
            s
        }
    }
}

fn oem_decode(bytes: &[u8]) -> String {
    #[cfg(windows)]
    {
        crate::sandbox::windows::ffi::decode_oem(bytes)
    }
    #[cfg(not(windows))]
    {
        String::from_utf8_lossy(bytes).to_string()
    }
}

/// 解析解释器与参数（对应"命令路径"输入）。
fn resolve_shell(input: &SandboxInput) -> (PathBuf, Vec<String>) {
    let program = match &input.shell.program {
        Some(p) => p.clone(),
        None => default_shell(),
    };
    let args = if input.shell.direct {
        crate::cmdline::split_command_line(&input.command)
    } else {
        default_shell_args(&input.command)
    };
    (program, args)
}

#[cfg(windows)]
fn default_shell() -> PathBuf {
    std::env::var("COMSPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\System32\cmd.exe"))
}

#[cfg(unix)]
fn default_shell() -> PathBuf {
    PathBuf::from("/bin/sh")
}

#[cfg(windows)]
fn default_shell_args(command: &str) -> Vec<String> {
    // /D：跳过 AutoRun（安全）；/C：执行后退出
    vec!["/D".to_string(), "/C".to_string(), command.to_string()]
}

#[cfg(unix)]
fn default_shell_args(command: &str) -> Vec<String> {
    vec!["-c".to_string(), command.to_string()]
}

/// 判断解释器是否为 cmd.exe（Windows：/C 后的命令文本需原样传递）。
fn is_cmd_shell(program: &PathBuf) -> bool {
    if !cfg!(windows) {
        return false;
    }
    program
        .file_stem()
        .map(|s| s.eq_ignore_ascii_case("cmd"))
        .unwrap_or(false)
}

/// 直接执行（或解释器为裸命令名）时，用内置运行时改写解释器路径。
fn resolve_runtime_program(program: &PathBuf) -> PathBuf {
    let resolver = RuntimeResolver::detect(true);
    if let Some(p) = resolver.resolve_argv0(&program.to_string_lossy()) {
        return p;
    }
    program.clone()
}

/// 去重环境变量：保留最后一次出现的值（会话注入覆盖系统同名变量）。
fn dedup_env(env: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in env.into_iter().rev() {
        if seen.insert(k.clone()) {
            out.push((k, v));
        }
    }
    out.reverse();
    out
}

/// PATH 注入：内置运行时 bin 目录置于最前（子进程解析 python/node/git 时优先命中内置）。
fn inject_runtime_path(env: &mut Vec<(String, String)>) {
    let resolver = RuntimeResolver::detect(true);
    let entries = resolver.path_entries();
    if entries.is_empty() {
        return;
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    let paths: Vec<String> = entries
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let mut new_path = paths.join(sep);
    if let Some((_, v)) = env.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("PATH")) {
        if !v.is_empty() {
            new_path.push_str(sep);
            new_path.push_str(v);
        }
        *v = new_path;
    } else {
        env.push(("PATH".to_string(), new_path));
    }
}

// ============================ 时间格式化（无第三方依赖） ============================

fn local_time_string(secs: i64) -> String {
    #[cfg(unix)]
    {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = secs as libc::time_t;
        unsafe {
            libc::localtime_r(&t, &mut tm);
        }
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
    #[cfg(windows)]
    {
        // 本地时间取自 Win32 GetLocalTime（避免依赖 CRT 导出符号）
        use crate::sandbox::windows::ffi::{GetLocalTime, SYSTEMTIME};
        let mut st = SYSTEMTIME::default();
        unsafe {
            GetLocalTime(&mut st);
        }
        let _ = secs;
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
}
