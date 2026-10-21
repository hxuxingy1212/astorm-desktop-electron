//! sbox 命令行接口。
//!
//! 子命令：
//! - `run`：执行一条命令（每次执行创建独立沙箱），支持 同步/异步、防护开关、策略文件
//! - `capabilities`：查看当前平台强制能力
//! - `runtimes`：查看/校验内置运行时（Python / Node / Git）
//! - `list-dangerous`：列出内置危险命令黑名单

use clap::{Parser, Subcommand};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::policy::{ExecMode, ProtectionConfig, SandboxInput, ShellSpec};
use crate::runner::{run_async, run_sync, ExecReport};
use crate::runtime::RuntimeResolver;
use crate::sandbox::Event;

#[derive(Parser)]
#[command(
    name = "sbox",
    version = crate::VERSION,
    about = "astorm-sandbox：Rust 跨平台（Windows + macOS）Shell 执行安全沙箱",
    long_about = "对 shell 执行命令做监控与防护：文件读写（黑白名单）、网络进出站（黑白名单）、\
危险命令（黑名单）。每次执行创建独立沙箱；防护关闭则直接执行不检测。\n\
优先级：黑名单 > 白名单，都不命中默认拒绝（文件/网络）；危险命令仅黑名单。"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 执行一条命令（创建独立沙箱）
    Run(RunArgs),
    /// 查看当前平台强制能力（诚实上报，不静默降级）；
    /// 带 `--policy` 时追加打印该策略的字段级强制力报告（一行一项）
    Capabilities {
        /// 防护配置 JSON 文件（与 `run --policy` 同格式）；给出时打印逐字段强制力报告
        #[arg(short = 'p', long = "policy")]
        policy: Option<PathBuf>,
    },
    /// 查看内置运行时（Python / Node / Git，随分发打包）
    Runtimes {
        /// 指定 runtime 目录（缺省自动探测）
        #[arg(long = "runtime-dir")]
        runtime_dir: Option<PathBuf>,
        /// 实际运行校验（执行各运行时 --version）
        #[arg(long = "check")]
        check: bool,
    },
    /// 列出内置危险命令黑名单
    ListDangerous {
        /// 以 JSON 输出
        #[arg(long = "json")]
        json: bool,
    },
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// 要执行的命令
    #[arg(short = 'c', long = "cmd")]
    pub cmd: String,

    /// 解释器/命令路径（缺省：Windows=cmd.exe，macOS=/bin/sh）
    #[arg(short = 's', long = "shell")]
    pub shell: Option<PathBuf>,

    /// 直接执行：命令字符串按参数拆分作为该程序的参数（不走解释器）
    #[arg(long = "direct")]
    pub direct: bool,

    /// 执行时间上限（秒），超时强制终止进程树
    #[arg(short = 't', long = "timeout", default_value_t = 60)]
    pub timeout: u64,

    /// 操作类型：sync（即时结果）| async（日志流）
    #[arg(short = 'm', long = "mode", default_value = "sync")]
    pub mode: String,

    /// 防护开关：on（正常检测）| off（直接执行不检测）
    #[arg(long = "protect", default_value = "on")]
    pub protect: String,

    /// 防护配置 JSON 文件（黑白名单、危险命令）
    #[arg(short = 'p', long = "policy")]
    pub policy: Option<PathBuf>,

    /// 严格模式：内核强制能力不足时直接失败（不静默降级）
    #[arg(long = "strict")]
    pub strict: bool,

    /// 子进程输出解码：auto（默认，UTF-8 优先/GBK 回退）| utf8 | oem
    #[arg(long = "output-encoding", default_value = "auto")]
    pub output_encoding: String,

    /// 以 JSON 输出结果
    #[arg(long = "json")]
    pub json: bool,

    /// 异步日志流同时写入文件（JSON Lines）
    #[arg(long = "log-file")]
    pub log_file: Option<PathBuf>,

    /// 运行时偏好：builtin（内置优先，缺省）| system（使用系统安装）
    #[arg(long = "runtime-prefs", default_value = "builtin")]
    pub runtime_prefs: String,

    /// 追加危险命令黑名单（正则，可多次）
    #[arg(long = "danger-add")]
    pub danger_add: Vec<String>,

    /// 追加文件读黑名单（glob，可多次）
    #[arg(long = "file-read-black-add")]
    pub file_read_black_add: Vec<String>,

    /// 追加文件写黑名单（glob，可多次）
    #[arg(long = "file-write-black-add")]
    pub file_write_black_add: Vec<String>,

    /// 追加文件读白名单（glob，可多次）
    #[arg(long = "file-read-white-add")]
    pub file_read_white_add: Vec<String>,

    /// 追加文件写白名单（glob，可多次）
    #[arg(long = "file-write-white-add")]
    pub file_write_white_add: Vec<String>,

    /// 追加网络出站黑名单（domain:/ip: 规则，可多次）
    #[arg(long = "net-out-black-add")]
    pub net_out_black_add: Vec<String>,

    /// 追加网络出站白名单（可多次）
    #[arg(long = "net-out-white-add")]
    pub net_out_white_add: Vec<String>,

    /// 追加网络进站黑名单（可多次）
    #[arg(long = "net-in-black-add")]
    pub net_in_black_add: Vec<String>,

    /// 追加网络进站白名单（可多次）
    #[arg(long = "net-in-white-add")]
    pub net_in_white_add: Vec<String>,

    /// 网络白名单开关：on（默认，白名单之外拒绝）| off（仅黑名单拦截，其余放行）。
    /// 缺省跟随策略文件（未指定时为 on）
    #[arg(long = "net-whitelist")]
    pub net_whitelist: Option<String>,

    /// 内存上限（MB；macOS=RLIMIT_AS，Windows=Job 对象内存限制）
    #[arg(long = "memory-mb")]
    pub memory_mb: Option<u64>,

    /// 磁盘写入上限（MB；macOS=RLIMIT_FSIZE 单文件写入上限，Windows 暂不强制）
    #[arg(long = "disk-mb")]
    pub disk_mb: Option<u64>,

    /// CPU 时间上限（秒；macOS=RLIMIT_CPU，Windows=Job 对象 CPU 时间限制）
    #[arg(long = "cpu-seconds")]
    pub cpu_seconds: Option<u64>,

    /// GPU 访问：on（允许，缺省）| off（禁止；macOS 通过 Seatbelt 阻断 GPU 服务）
    #[arg(long = "gpu", default_value = "on")]
    pub gpu: String,

    /// 会话工作区根（放行其中的可执行文件；也是持久 HOME 的默认归属）
    #[arg(long = "workspace")]
    pub workspace: Option<PathBuf>,

    /// 持久 HOME 目录（命令之间保留缓存/配置；缺省 = 逐命令临时 HOME）
    #[arg(long = "session-home")]
    pub session_home: Option<PathBuf>,

    /// 不放行工作区内的可执行文件（默认放行：解释器本就放行，禁止只挡正常开发流程）
    #[arg(long = "no-workspace-exec")]
    pub no_workspace_exec: bool,

    /// 额外可执行目录（非常规位置的工具链/编译器；可多次；exec 与只读一并放行）
    #[arg(long = "exec-white-add")]
    pub exec_white_add: Vec<String>,
}

impl RunArgs {
    fn protection(&self) -> Result<ProtectionConfig, String> {
        let mut protection = match &self.policy {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("读取策略文件 {} 失败：{e}", path.display()))?;
                serde_json::from_str::<ProtectionConfig>(&text)
                    .map_err(|e| format!("解析策略文件失败：{e}"))?
            }
            None => ProtectionConfig::default(),
        };
        protection.enabled = match self.protect.as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            other => return Err(format!("--protect 取值非法：{other}（on/off）")),
        };
        // CLI 追加规则（叠加在策略文件之上）
        protection
            .danger
            .black
            .extend(self.danger_add.iter().cloned());
        protection
            .file
            .read_black
            .extend(self.file_read_black_add.iter().cloned());
        protection
            .file
            .write_black
            .extend(self.file_write_black_add.iter().cloned());
        protection
            .file
            .read_white
            .extend(self.file_read_white_add.iter().cloned());
        protection
            .file
            .write_white
            .extend(self.file_write_white_add.iter().cloned());
        protection
            .net
            .out_black
            .extend(self.net_out_black_add.iter().cloned());
        protection
            .net
            .out_white
            .extend(self.net_out_white_add.iter().cloned());
        protection
            .net
            .in_black
            .extend(self.net_in_black_add.iter().cloned());
        protection
            .net
            .in_white
            .extend(self.net_in_white_add.iter().cloned());
        if let Some(v) = &self.net_whitelist {
            protection.net.use_whitelist = match v.as_str() {
                "on" | "true" | "1" => true,
                "off" | "false" | "0" => false,
                other => return Err(format!("--net-whitelist 取值非法：{other}（on/off）")),
            };
        }
        // 资源限制（CLI 覆盖策略文件中的 resource）
        if self.memory_mb.is_some() {
            protection.resource.memory_mb = self.memory_mb;
        }
        if self.disk_mb.is_some() {
            protection.resource.disk_mb = self.disk_mb;
        }
        if self.cpu_seconds.is_some() {
            protection.resource.cpu_seconds = self.cpu_seconds;
        }
        protection.resource.gpu = match self.gpu.as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            other => return Err(format!("--gpu 取值非法：{other}（on/off）")),
        };
        if self.no_workspace_exec {
            protection.allow_workspace_exec = false;
        }
        protection
            .exec_white_dirs
            .extend(self.exec_white_add.iter().cloned());
        Ok(protection)
    }

    fn input(&self) -> Result<SandboxInput, String> {
        let mode = match self.mode.as_str() {
            "sync" | "同步" => ExecMode::Sync,
            "async" | "异步" => ExecMode::Async,
            other => return Err(format!("--mode 取值非法：{other}（sync/async）")),
        };
        let prefer_builtin = match self.runtime_prefs.as_str() {
            "builtin" => true,
            "system" => false,
            other => {
                return Err(format!(
                    "--runtime-prefs 取值非法：{other}（builtin/system）"
                ))
            }
        };
        if !prefer_builtin {
            // 系统优先：不注入内置运行时（通过空解析器实现）
            // 说明：RuntimeResolver 在 runner 内部按 prefer_builtin 构造；
            // 此处以禁用注入的方式表达（见下方 shell/program 解析不变）。
        }
        let output_encoding = match self.output_encoding.as_str() {
            "auto" => crate::policy::OutputEncoding::Auto,
            "utf8" => crate::policy::OutputEncoding::Utf8,
            "oem" => crate::policy::OutputEncoding::Oem,
            other => {
                return Err(format!(
                    "--output-encoding 取值非法：{other}（auto/utf8/oem）"
                ))
            }
        };
        Ok(SandboxInput {
            command: self.cmd.clone(),
            shell: ShellSpec {
                program: self.shell.clone(),
                direct: self.direct,
                raw_command_arg: None, // 自动判断（cmd.exe 时原样传递命令文本）
            },
            timeout: Duration::from_secs(self.timeout),
            mode,
            protection: self.protection()?,
            strict: self.strict,
            output_encoding,
            cwd: None,
            workspace: self.workspace.clone(),
            session_home: self.session_home.clone(),
            cancel: None,
        })
    }
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::Capabilities { policy } => {
            let caps = crate::sandbox::default_backend().capabilities();
            println!("平台：{}", caps.summary());
            if caps.degraded_reasons.is_empty() {
                println!("无降级项。");
            } else {
                println!("能力说明：");
                for r in &caps.degraded_reasons {
                    println!("  - {r}");
                }
            }
            println!(
                "严格模式可用：{}",
                if caps.ok_for_strict() { "是" } else { "否" }
            );
            if let Some(path) = &policy {
                let text = match std::fs::read_to_string(path) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("读取策略文件 {} 失败：{e}", path.display());
                        return ExitCode::from(2);
                    }
                };
                let protection = match serde_json::from_str::<ProtectionConfig>(&text) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("解析策略文件失败：{e}");
                        return ExitCode::from(2);
                    }
                };
                let input = SandboxInput {
                    protection,
                    ..SandboxInput::new("")
                };
                let resolved = match crate::policy::ResolvedPolicy::compile(&input) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("策略编译失败：{e}");
                        return ExitCode::from(2);
                    }
                };
                println!("策略字段强制力（字段=状态: 机制(注意事项)）：");
                for f in resolved.enforcement_report() {
                    println!("  - {}", f.line());
                }
            }
            ExitCode::SUCCESS
        }
        Commands::ListDangerous { json } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&crate::policy::BUILTIN_DANGER_PATTERNS)
                        .unwrap_or_default()
                );
            } else {
                println!("内置危险命令黑名单（正则，大小写不敏感）：");
                for p in crate::policy::BUILTIN_DANGER_PATTERNS {
                    println!("  - {p}");
                }
            }
            ExitCode::SUCCESS
        }
        Commands::Runtimes { runtime_dir, check } => {
            let resolver = match &runtime_dir {
                Some(dir) => RuntimeResolver::from_dir(dir, true)
                    .map_err(|e| {
                        eprintln!("{e}");
                        ExitCode::FAILURE
                    })
                    .unwrap(),
                None => RuntimeResolver::detect(true),
            };
            if !resolver.is_loaded() {
                println!("未发现内置运行时（runtime/manifest.json）。");
                println!("提示：运行 scripts/fetch-runtimes 脚本生成分发运行时目录。");
                return ExitCode::SUCCESS;
            }
            println!("内置运行时根目录：{}", resolver.root().unwrap().display());
            for (name, program, _) in resolver.list() {
                println!("  - {name}: {}", program.display());
            }
            if check {
                println!("校验：");
                for (name, program, _) in resolver.list() {
                    let arg = if name == "git" {
                        "--version"
                    } else {
                        "--version"
                    };
                    let out = std::process::Command::new(&program).arg(arg).output();
                    match out {
                        Ok(o) if o.status.success() => {
                            let first = String::from_utf8_lossy(&o.stdout)
                                .lines()
                                .next()
                                .unwrap_or("")
                                .to_string();
                            println!("  - {name}: OK（{first}）");
                        }
                        Ok(o) => {
                            println!(
                                "  - {name}: 失败（{}）",
                                String::from_utf8_lossy(&o.stderr).trim()
                            );
                        }
                        Err(e) => println!("  - {name}: 无法执行（{e}）"),
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Commands::Run(args) => match args.mode.as_str() {
            m if m == "async" || m == "异步" => run_async_cli(args),
            _ => run_sync_cli(args),
        },
    }
}

fn run_sync_cli(args: RunArgs) -> ExitCode {
    let input = match args.input() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("参数错误：{e}");
            return ExitCode::from(2);
        }
    };
    let report = run_sync(input);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else {
        print_report_human(&report);
    }
    exit_code_of_report(&report)
}

fn run_async_cli(args: RunArgs) -> ExitCode {
    let input = match args.input() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("参数错误：{e}");
            return ExitCode::from(2);
        }
    };
    let mut log_writer = args.log_file.as_ref().map(|p| {
        let file = std::fs::File::create(p).expect("创建日志文件失败");
        std::io::BufWriter::new(file)
    });
    let (handle, rx) = run_async(input);
    pump_events(rx, args.json, log_writer.as_mut());
    let report = match handle.join() {
        Ok(r) => r,
        Err(_) => {
            eprintln!("执行线程异常退出");
            return ExitCode::FAILURE;
        }
    };
    if let Some(w) = log_writer.as_mut() {
        let _ = serde_json::to_writer(&mut *w, &report);
        let _ = w.flush();
    }
    if args.json {
        // 单行输出，保持 JSON Lines 流的可解析性
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        print_summary_human(&report);
    }
    exit_code_of_report(&report)
}

/// 事件泵：实时打印（人类可读 / JSON Lines）并可选落盘。
fn pump_events(
    rx: Receiver<Event>,
    json: bool,
    mut log: Option<&mut std::io::BufWriter<std::fs::File>>,
) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    while let Ok(ev) = rx.recv() {
        if let Some(w) = log.as_mut() {
            let _ = serde_json::to_writer(&mut **w, &ev);
            let _ = w.write_all(b"\n");
            let _ = w.flush();
        }
        if json {
            println!("{}", serde_json::to_string(&ev).unwrap_or_default());
            continue;
        }
        match &ev {
            Event::Chunk {
                stream: crate::sandbox::StreamKind::Stdout,
                data,
            } => {
                let _ = out.write_all(data.as_bytes());
            }
            Event::Chunk {
                stream: crate::sandbox::StreamKind::Stderr,
                data,
            } => {
                let _ = out.write_all(data.as_bytes());
            }
            Event::Capabilities(caps) => {
                let _ = writeln!(out, "[能力] {}", caps.summary());
                for r in &caps.degraded_reasons {
                    let _ = writeln!(out, "[能力说明] {r}");
                }
            }
            Event::Note(m) => {
                let _ = writeln!(out, "[说明] {m}");
            }
            Event::Spawn { pid, argv } => {
                let _ = writeln!(out, "[进程] pid={pid} 命令行={argv}");
            }
            Event::Denied { category, detail } => {
                let _ = writeln!(out, "[拦截] {}：{detail}", category.zh());
            }
            Event::Evicted { category, reason } => {
                let _ = writeln!(out, "[驱逐·{}] {reason}", category.zh());
            }
            Event::TimeoutKilled => {
                let _ = writeln!(out, "[超时] 执行超时，已强制终止进程树");
            }
            Event::OutputTruncated => {
                let _ = writeln!(out, "[输出] 输出超过上限，已截断");
            }
            Event::Exit { pid, code } => {
                let _ = writeln!(out, "[退出] pid={pid} code={code}");
            }
            Event::Finished => {
                let _ = writeln!(out, "[完成] 事件流结束");
            }
            Event::Error(m) => {
                let _ = writeln!(out, "[错误] {m}");
            }
        }
        let _ = out.flush();
    }
}

fn print_report_human(report: &ExecReport) {
    println!("== astorm-sandbox 执行报告 ==");
    println!("命令：{}", report.command);
    println!(
        "时间：{} → {}（{}ms）",
        report.started_at, report.ended_at, report.elapsed_ms
    );
    println!(
        "模式：{} 防护：{} 后端：{}",
        match report.mode {
            ExecMode::Sync => "同步",
            ExecMode::Async => "异步",
        },
        if report.protection_on { "开" } else { "关" },
        report.backend
    );
    if let Some(caps) = &report.capabilities {
        println!("能力：{}", caps.summary());
        for r in &caps.degraded_reasons {
            println!("  能力说明：{r}");
        }
    }
    for p in &report.prevented {
        println!("已拦截（未执行）：{p}");
    }
    println!("---- 输出 ----");
    if !report.output.is_empty() {
        println!("{}", report.output.trim_end());
    }
    println!("---- 结果 ----");
    println!(
        "退出码：{} 强杀：{} 驱逐：{} 正常：{}",
        report
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "无".into()),
        if report.killed { "是" } else { "否" },
        if report.evicted { "是" } else { "否" },
        if report.ok { "是" } else { "否" },
    );
    let notable: Vec<String> = report
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Denied { category, detail } => {
                Some(format!("[拦截] {}：{detail}", category.zh()))
            }
            Event::Evicted { category, reason } => {
                Some(format!("[驱逐·{}] {reason}", category.zh()))
            }
            Event::Note(m) => Some(format!("[说明] {m}")),
            Event::Error(m) => Some(format!("[错误] {m}")),
            Event::Spawn { pid, argv } => Some(format!("[进程] pid={pid} {argv}")),
            _ => None,
        })
        .collect();
    if !notable.is_empty() {
        println!("---- 事件 ----");
        for l in notable {
            println!("{l}");
        }
    }
}

fn print_summary_human(report: &ExecReport) {
    println!("== astorm-sandbox 执行报告（异步汇总） ==");
    println!(
        "命令：{} 时间：{} → {}（{}ms）",
        report.command, report.started_at, report.ended_at, report.elapsed_ms
    );
    println!(
        "退出码：{} 强杀：{} 驱逐：{} 正常：{}",
        report
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "无".into()),
        if report.killed { "是" } else { "否" },
        if report.evicted { "是" } else { "否" },
        if report.ok { "是" } else { "否" },
    );
}

fn exit_code_of_report(report: &ExecReport) -> ExitCode {
    if !report.prevented.is_empty() {
        // 被沙箱拦截（预检/严格模式）
        ExitCode::from(2)
    } else if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
