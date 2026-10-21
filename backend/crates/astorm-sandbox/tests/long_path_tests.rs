//! 长路径（> MAX_PATH）cwd 的端到端测试。
//!
//! 单独成一个 test binary：`cargo test` 逐个运行 test binary，避免与
//! runner_tests 里并行的 AppContainer 会话互相干扰（junction/ACL 时序敏感）。

#![cfg(windows)]

use std::time::Duration;

use astorm_sandbox::policy::{DangerPolicyCfg, ProtectionConfig, SandboxInput};
use astorm_sandbox::runner::run_sync;

fn echo_cmd(text: &str) -> String {
    format!("echo {text}")
}

#[test]
fn protection_on_long_cwd_runs() {
    let base = std::env::temp_dir().join(format!(
        "astorm-long-cwd-{}-{}",
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
        return; // 系统未启用长路径：跳过
    }
    if dir.to_string_lossy().len() <= 260 {
        return;
    }
    let mut input = SandboxInput::new(echo_cmd("long-cwd-ok"));
    input.protection = ProtectionConfig {
        enabled: true,
        danger: DangerPolicyCfg {
            use_builtin: true,
            ..Default::default()
        },
        ..Default::default()
    };
    input.cwd = Some(dir.clone());
    // 长 cwd 本身作为工作区，确保 apply_acl 给它授权（junction 目标需要可读）
    input.workspace = Some(dir);
    input.timeout = Duration::from_secs(60);
    let report = run_sync(input);
    assert_eq!(
        report.exit_code,
        Some(0),
        "输出：{}；事件：{:?}",
        report.output,
        report.events
    );
    assert!(report.output.contains("long-cwd-ok"));
    let _ = std::fs::remove_dir_all(&base);
}
