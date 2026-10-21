//! 执行器集成测试（防护关闭 → 直通后端；预检拦截路径）。
//!
//! 这些用例不依赖内核强制层，可在 Windows 与 macOS 上运行。

use std::time::Duration;

use astorm_sandbox::policy::{
    DangerPolicyCfg, ExecMode, NetPolicyCfg, ProtectionConfig, SandboxInput,
};
use astorm_sandbox::runner::{run_async, run_sync};
use astorm_sandbox::sandbox::Event;

#[cfg(windows)]
fn echo_cmd(text: &str) -> String {
    // 命令 = 传给解释器（cmd.exe /D /C）的字符串，无需自行包装 cmd
    format!("echo {text}")
}

#[cfg(not(windows))]
fn echo_cmd(text: &str) -> String {
    format!("echo {text}")
}

#[cfg(windows)]
fn sleep_cmd(secs: u32) -> String {
    format!("ping -n {secs} 127.0.0.1 >nul")
}

#[cfg(not(windows))]
fn sleep_cmd(secs: u32) -> String {
    format!("sleep {secs}")
}

fn protect_off() -> ProtectionConfig {
    ProtectionConfig {
        enabled: false,
        ..Default::default()
    }
}

#[test]
fn 同步_直通执行_即时结果() {
    let mut input = SandboxInput::new(echo_cmd("hello-sandbox"));
    input.protection = protect_off();
    input.timeout = Duration::from_secs(30);
    let report = run_sync(input);
    assert_eq!(report.exit_code, Some(0), "输出：{}", report.output);
    assert!(report.output.contains("hello-sandbox"), "输出：{}", report.output);
    assert!(!report.killed);
    assert!(report.ok, "事件：{:?}", report.events);
    // 防护关闭时后端为直通
    assert_eq!(report.backend, "passthrough");
}

#[test]
fn 异步_事件日志流() {
    let mut input = SandboxInput::new(echo_cmd("async-stream"));
    input.protection = protect_off();
    input.mode = ExecMode::Async;
    input.timeout = Duration::from_secs(30);
    let (handle, rx) = run_async(input);
    let mut saw_chunk = false;
    let mut finished = false;
    while let Ok(ev) = rx.recv() {
        match ev {
            Event::Chunk { data, .. } => {
                if data.contains("async-stream") {
                    saw_chunk = true;
                }
            }
            Event::Finished => finished = true,
            _ => {}
        }
    }
    assert!(saw_chunk, "应收到包含输出的 Chunk 事件");
    assert!(finished, "事件流最后应为 Finished");
    let report = handle.join().expect("执行线程不应崩溃");
    assert_eq!(report.exit_code, Some(0));
}

#[test]
fn 超时_强制终止进程树() {
    let mut input = SandboxInput::new(sleep_cmd(30));
    input.protection = protect_off();
    input.timeout = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let report = run_sync(input);
    let elapsed = started.elapsed();
    assert!(report.killed, "应被超时强杀");
    assert!(elapsed < Duration::from_secs(15), "强杀应 promptly：{:?}", elapsed);
    // 进程树应已消失（父 cmd 与子 ping 均被终止）
    assert!(!report.ok);
}

#[test]
fn 预检_危险命令命中拒绝启动() {
    let mut input = SandboxInput::new(echo_cmd("markersuperdanger"));
    input.protection = ProtectionConfig {
        enabled: true,
        danger: DangerPolicyCfg {
            black: vec!["markersuperdanger".to_string()],
            use_builtin: false,
            builtin_override: None,
        },
        ..Default::default()
    };
    input.timeout = Duration::from_secs(10);
    let report = run_sync(input);
    assert!(report.prevented.iter().any(|p| p.contains("危险命令")));
    assert_eq!(report.exit_code, None, "被拦截的命令不应执行");
    assert!(report
        .events
        .iter()
        .any(|e| matches!(e, Event::Denied { category: astorm_sandbox::sandbox::Category::DangerCommand, .. })));
    assert!(!report.ok);
}

#[test]
fn 预检_网络黑名单命中拒绝启动() {
    let mut input = SandboxInput::new("ping 8.8.8.8");
    if cfg!(not(windows)) {
        input.command = "ping -c 1 8.8.8.8".to_string();
    }
    input.protection = ProtectionConfig {
        enabled: true,
        net: NetPolicyCfg {
            out_black: vec!["ip:8.8.8.8".to_string()],
            ..Default::default()
        },
        danger: DangerPolicyCfg {
            use_builtin: false,
            ..Default::default()
        },
        ..Default::default()
    };
    input.timeout = Duration::from_secs(10);
    let report = run_sync(input);
    assert!(
        report.prevented.iter().any(|p| p.contains("黑名单")),
        "应命中网络黑名单：{:?}",
        report.prevented
    );
    assert_eq!(report.exit_code, None);
}

#[test]
fn protection_on_harmless_echo_runs() {
    // 预检只拦黑名单；默认拒绝由运行期强制层逐访问判定（本用例无文件/网络访问）
    let mut input = SandboxInput::new(echo_cmd("harmless"));
    input.protection = ProtectionConfig {
        enabled: true,
        danger: DangerPolicyCfg {
            use_builtin: true,
            ..Default::default()
        },
        ..Default::default()
    };
    input.timeout = Duration::from_secs(30);
    let report = run_sync(input);
    assert_eq!(
        report.exit_code,
        Some(0),
        "输出：{}；事件：{:?}",
        report.output,
        report.events
    );
    assert!(report.output.contains("harmless"));
}