//! 库用法示例：同步（即时结果）与异步（事件日志流）。

use std::time::Duration;

use astorm_sandbox::policy::{
    DangerPolicyCfg, ExecMode, FilePolicyCfg, NetPolicyCfg, ProtectionConfig, ResourceLimits,
    SandboxInput,
};
use astorm_sandbox::runner::{run_async, run_sync};
use astorm_sandbox::sandbox::Event;

fn main() {
    // ---------- 输入模型：执行时间 / 执行命令 / 命令路径 / 操作类型 / 防护配置 / 防护开关 ----------
    let protection = ProtectionConfig {
        enabled: true, // 防护开关：false = 直接执行不检测
        file: FilePolicyCfg {
            read_black: vec!["C:/Users/**/secret*".into()],
            write_black: vec!["C:/Windows/**".into()],
            write_white: vec![],
            read_white: vec![],
        },
        net: NetPolicyCfg {
            use_whitelist: true, // 白名单之外默认拒绝；false = 仅黑名单拦截
            out_black: vec!["ip:10.0.0.0/8".into()],
            out_white: vec!["domain:example.com".into()],
            in_black: vec![],
            in_white: vec![],
        },
        danger: DangerPolicyCfg {
            use_builtin: true, // 内置危险命令清单 + 自定义
            black: vec![r"my-danger-cmd".into()],
            builtin_override: None,
        },
        resource: ResourceLimits::default(), // 资源限制（内存/磁盘/CPU/GPU）
        toolchain_disabled: Vec::new(),      // 已禁用沙箱自动注入的工具 bin 名
        allow_workspace_exec: true,          // 放行工作区内可执行文件（.venv/bin、编译产物…）
        exec_white_dirs: Vec::new(),         // 额外可执行目录（安全配置「额外可执行目录」）
    };

    // ---------- 同步：即时结果 ----------
    // 命令 = 传给解释器的字符串（Windows 默认 cmd.exe /D /C，macOS 默认 /bin/sh -c）
    let mut input = SandboxInput::new("echo hello-from-sandbox");
    input.protection = protection.clone();
    input.timeout = Duration::from_secs(30); // 执行时间
    input.mode = ExecMode::Sync; // 操作类型
    input.shell.program = None; // 命令路径：None = 平台默认解释器

    let report = run_sync(input);
    println!(
        "[同步] exit={:?} elapsed={}ms ok={} 输出：{}",
        report.exit_code,
        report.elapsed_ms,
        report.ok,
        report.output.trim()
    );

    // ---------- 异步：事件日志流 ----------
    let net_probe = if cfg!(windows) {
        "ping -n 2 127.0.0.1"
    } else {
        "ping -c 2 127.0.0.1"
    };
    let mut input = SandboxInput::new(net_probe);
    input.protection = protection;
    input.mode = ExecMode::Async;
    input.timeout = Duration::from_secs(30);
    let (handle, rx) = run_async(input);
    while let Ok(ev) = rx.recv() {
        match ev {
            Event::Chunk { stream, data } => print!("[{stream:?}] {data}"),
            Event::Spawn { pid, argv } => println!("[进程] pid={pid} {argv}"),
            Event::Denied { category, detail } => println!("[拦截] {}：{detail}", category.zh()),
            Event::Evicted { category, reason } => println!("[驱逐·{}] {reason}", category.zh()),
            Event::TimeoutKilled => println!("[超时] 进程树已终止"),
            Event::Finished => {
                println!("[完成]");
                break;
            }
            _ => {}
        }
    }
    let report = handle.join().expect("执行线程不应崩溃");
    println!(
        "[异步] exit={:?} killed={} evicted={} 事件数={}",
        report.exit_code,
        report.killed,
        report.evicted,
        report.events.len()
    );
}
