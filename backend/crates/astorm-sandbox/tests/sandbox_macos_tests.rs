//! macOS 强制层实测（Seatbelt：文件读写内核强制 + 网络方向 + 进程监视驱逐）。
//!
//! 全部用例合并在单个测试函数内串行执行，避免并行会话对同一工作目录的
//! 文件访问互相干扰。
//!
//! 能力不足（libsandbox 无法加载）时自动跳过并说明，绝不伪造通过。

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::time::Duration;

use astorm_sandbox::policy::{
    DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg, ProtectionConfig, SandboxInput,
};
use astorm_sandbox::runner::run_sync;
use astorm_sandbox::sandbox::default_backend;

fn base_input(command: impl Into<String>) -> SandboxInput {
    let mut input = SandboxInput::new(command);
    input.timeout = Duration::from_secs(60);
    input
}

/// 探测 Seatbelt 可用性：只有真正进入 macos-seatbelt 才继续。
fn seatbelt_available() -> Option<String> {
    let caps = default_backend().capabilities();
    if !caps.file_runtime_enforced {
        return Some(format!("能力不足：{}", caps.summary()));
    }
    None
}

/// 用 shell 变量隐藏目标路径，绕过静态预检（预检按文本匹配，无法解析变量），
/// 从而验证「运行期」内核强制而非预检拦截。
fn via_var(command_fragment: &str, path: &str) -> String {
    format!("P={path}; {command_fragment}")
}

#[test]
fn seatbelt_全链路实测() {
    if let Some(reason) = seatbelt_available() {
        eprintln!("跳过 seatbelt_全链路实测：{reason}");
        return;
    }

    let tmp = std::env::temp_dir(); // macOS 为 /var/folders/...（真实路径 /private/var/folders/...）
    let allow_dir = tmp.join(format!("astorm-it-mac-allow-{}", std::process::id()));
    let deny_dir = tmp.join(format!("astorm-it-mac-deny-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&allow_dir);
    let _ = std::fs::remove_dir_all(&deny_dir);
    std::fs::create_dir_all(&allow_dir).unwrap();
    std::fs::create_dir_all(&deny_dir).unwrap();
    let allow_glob = format!("{}/**", allow_dir.to_string_lossy().replace('\\', "/"));
    let secret = allow_dir.join("secret.txt");
    std::fs::write(&secret, "top-secret").unwrap();

    // ---------- 0. 基础执行：防护开启 + 内置危险清单，无害命令正常 ----------
    {
        let mut input = base_input("echo harmless");
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_sync(input);
        assert_eq!(report.exit_code, Some(0), "输出：{}", report.output);
        assert!(
            report.output.contains("harmless"),
            "输出：{}",
            report.output
        );
        assert!(!report.killed);
        assert!(!report.evicted);
    }

    // ---------- 1. 写白名单命中：写入成功 ----------
    {
        let target = allow_dir.join("out.txt");
        let mut input = base_input(format!("echo hello-sandbox > \"{}\"", target.display()));
        input.protection = ProtectionConfig {
            enabled: true,
            file: FilePolicyCfg {
                write_white: vec![allow_glob.clone()],
                ..Default::default()
            },
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_sync(input);
        assert_eq!(
            report.exit_code,
            Some(0),
            "白名单写入应成功。输出：{}\n事件：{:?}",
            report.output,
            report.events
        );
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("hello-sandbox"));
    }

    // ---------- 2. 白名单外写入：内核拒绝（默认拒绝） ----------
    {
        let target = deny_dir.join("blocked.txt");
        let mut input = base_input(via_var("echo x > \"$P\"", &target.display().to_string()));
        input.protection = ProtectionConfig {
            enabled: true,
            file: FilePolicyCfg {
                write_white: vec![allow_glob.clone()],
                ..Default::default()
            },
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_sync(input);
        assert_ne!(
            report.exit_code,
            Some(0),
            "白名单外写入必须被内核拒绝。输出：{}",
            report.output
        );
        assert!(!target.exists(), "被拒绝的文件不应存在");
        assert!(
            report.output.contains("Operation not permitted")
                || report.output.contains("not permitted"),
            "应出现内核拒绝提示。输出：{}",
            report.output
        );
    }

    // ---------- 3. 读黑名单（Deny 优先于白名单 Grant）：内容不得泄露 ----------
    {
        let secret_glob = format!("{}/**", allow_dir.to_string_lossy().replace('\\', "/"));
        let secret_path = secret.display().to_string();
        let mut input = base_input(via_var("cat \"$P\"", &secret_path));
        input.protection = ProtectionConfig {
            enabled: true,
            file: FilePolicyCfg {
                read_white: vec![secret_glob.clone()],
                read_black: vec![secret_path.clone()],
                ..Default::default()
            },
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_sync(input);
        assert_ne!(
            report.exit_code,
            Some(0),
            "读黑名单必须拦截。输出：{}",
            report.output
        );
        assert!(
            !report.output.contains("top-secret"),
            "敏感内容不得出现在输出中"
        );
    }

    // ---------- 4. 会话临时目录（TMPDIR/HOME 重定向）可写 ----------
    {
        let mut input = base_input("echo tmp-ok > \"$TMPDIR/probe.txt\"");
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_sync(input);
        assert_eq!(
            report.exit_code,
            Some(0),
            "会话临时目录应可写。输出：{}",
            report.output
        );
    }

    // ---------- 5. 网络出站默认拒绝（空白名单）----------
    {
        let mut input = base_input("ping -c 1 8.8.8.8; echo done");
        input.protection = ProtectionConfig {
            enabled: true,
            net: NetPolicyCfg::default(), // 出站白名单为空 → 默认拒绝
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_sync(input);
        assert!(
            report.output.contains("not permitted")
                || report.output.contains("Operation not permitted")
                || report.evicted
                || report.killed,
            "出站连接应被拒绝/驱逐。输出：{}",
            report.output
        );
    }

    let _ = std::fs::remove_dir_all(&allow_dir);
    let _ = std::fs::remove_dir_all(&deny_dir);
}

/// 防护开关：关闭后同一危险命令直接执行（不检测）。
#[test]
fn 防护关闭_危险命令直通执行() {
    let mut input = base_input("echo markersuperdanger");
    input.protection = ProtectionConfig {
        enabled: false,
        danger: DangerPolicyCfg {
            black: vec!["markersuperdanger".to_string()],
            use_builtin: false,
            builtin_override: None,
        },
        ..Default::default()
    };
    let report = run_sync(input);
    assert_eq!(report.exit_code, Some(0));
    assert!(report.output.contains("markersuperdanger"));
    assert!(report.prevented.is_empty());
    assert_eq!(report.backend, "passthrough");
}

/// 严格模式：能力充足时正常执行；不足时明确失败（不静默执行）。
#[test]
fn 严格模式_能力自适应() {
    let mut input = base_input("echo strict-ok");
    input.strict = true;
    input.protection = ProtectionConfig {
        enabled: true,
        danger: DangerPolicyCfg {
            use_builtin: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let caps = default_backend().capabilities();
    let report = run_sync(input);
    if caps.ok_for_strict() {
        assert_eq!(report.exit_code, Some(0), "输出：{}", report.output);
        assert!(report.prevented.is_empty());
    } else {
        // 非严格能力：应明确失败而非静默执行
        assert!(!report.prevented.is_empty() || !report.ok);
    }
    let _ = PathBuf::new(); // 保持导入整洁
}

/// 资源限制实测：CPU（RLIMIT_CPU）/ 磁盘（RLIMIT_FSIZE）/ 内存（RSS 监视驱逐）/ GPU 禁用。
#[test]
fn 资源限制_实测() {
    if let Some(reason) = seatbelt_available() {
        eprintln!("跳过 资源限制_实测：{reason}");
        return;
    }

    // ---------- CPU：忙循环 1 秒 CPU 时间后应被 SIGXCPU 终止 ----------
    {
        let mut input = base_input("while :; do :; done");
        input.timeout = Duration::from_secs(15);
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        input.protection.resource.cpu_seconds = Some(1);
        let report = run_sync(input);
        // SIGXCPU = 24 → 128 + 24 = 152
        assert_eq!(
            report.exit_code,
            Some(152),
            "忙循环应在 CPU 时间超限后被 SIGXCPU 终止。输出：{}",
            report.output
        );
        assert!(!report.ok);
        // rlimit 信号无平台事件，只能由退出码归因为「资源限制」类
        assert!(
            report.events.iter().any(|e| matches!(
                e,
                astorm_sandbox::sandbox::Event::Denied {
                    category: astorm_sandbox::sandbox::Category::Resource,
                    ..
                }
            )),
            "CPU 时间超限应产出资源类拦截事件。事件：{:?}",
            report.events
        );
    }

    // ---------- 磁盘：单文件写入超过 1MB 应被 SIGXFSZ 终止 ----------
    {
        // perl 为 macOS 自带；写会话临时目录（必然可写），绕开文件白名单干扰
        let mut input = base_input(
            "/usr/bin/perl -e 'open(F,\">$ENV{TMPDIR}/big.txt\"); print F \"x\" x (2*1024*1024); close F'",
        );
        input.timeout = Duration::from_secs(15);
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        input.protection.resource.disk_mb = Some(1);
        let report = run_sync(input);
        // SIGXFSZ = 25 → 128 + 25 = 153
        assert_eq!(
            report.exit_code,
            Some(153),
            "超限写入应被 SIGXFSZ 终止。输出：{} 事件：{:?}",
            report.output,
            report.events
        );
        assert!(!report.ok);
        assert!(
            report.events.iter().any(|e| matches!(
                e,
                astorm_sandbox::sandbox::Event::Denied {
                    category: astorm_sandbox::sandbox::Category::Resource,
                    ..
                }
            )),
            "磁盘写入超限应产出资源类拦截事件。事件：{:?}",
            report.events
        );
    }

    // ---------- 内存：分配 500MB 并持有，超过 50MB 上限应被驱逐 ----------
    {
        let mut input = base_input(
            "/usr/bin/perl -e '$x = \"x\" x (500*1024*1024); print length($x), \"\\n\"; sleep 3'",
        );
        input.timeout = Duration::from_secs(20);
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        input.protection.resource.memory_mb = Some(50);
        let report = run_sync(input);
        assert!(
            report.evicted
                || report
                    .events
                    .iter()
                    .any(|e| matches!(e, astorm_sandbox::sandbox::Event::Evicted { .. })),
            "内存超限应触发驱逐。退出码：{:?} 事件：{:?}",
            report.exit_code,
            report.events
        );
        assert!(!report.ok);
    }

    // ---------- 内存限制不影响正常小内存命令 ----------
    {
        let mut input = base_input("echo mem-ok");
        input.timeout = Duration::from_secs(15);
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        input.protection.resource.memory_mb = Some(200);
        let report = run_sync(input);
        assert_eq!(report.exit_code, Some(0), "输出：{}", report.output);
        assert!(report.ok);
    }

    // ---------- GPU 禁用：基础命令不受影响 ----------
    {
        let mut input = base_input("echo gpu-ok");
        input.timeout = Duration::from_secs(15);
        input.protection = ProtectionConfig {
            enabled: true,
            danger: DangerPolicyCfg {
                use_builtin: false,
                ..Default::default()
            },
            ..Default::default()
        };
        input.protection.resource.gpu = false;
        let report = run_sync(input);
        assert_eq!(report.exit_code, Some(0), "输出：{}", report.output);
        assert!(report.output.contains("gpu-ok"));
        assert!(report.ok);
    }
}

/// P1 实测：持久 HOME（跨命令保留）、工作区内 exec、uv 托管 Python、原生编译链路。
///
/// 每项都对应真实使用中撞到的墙：
/// - `HOME` 逐命令变化 → 工具链「装完就消失」；
/// - `./.toolchain/probe.sh` → `bad interpreter: Operation not permitted`；
/// - `.venv/bin/python`（符号链接到 uv 托管解释器）→ `Operation not permitted`；
/// - `rustc`/`cc` 链接失败：`xcrun` 读不到 `libxcrun`、SDK 目录被拒。
#[test]
fn p1_开发链路实测() {
    if let Some(reason) = seatbelt_available() {
        eprintln!("跳过 p1_开发链路实测：{reason}");
        return;
    }

    let ws = std::env::temp_dir().join(format!("astorm-it-p1-ws-{}", std::process::id()));
    let home = std::env::temp_dir().join(format!("astorm-it-p1-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&ws).unwrap();
    let ws_glob = format!("{}/**", ws.to_string_lossy().replace('\\', "/"));

    let mk = |cmd: &str| {
        let mut input = base_input(cmd);
        input.cwd = Some(ws.clone());
        input.workspace = Some(ws.clone());
        input.session_home = Some(home.clone());
        input.protection = ProtectionConfig {
            enabled: true,
            ..Default::default()
        };
        input.protection.file.read_white = vec![ws_glob.clone()];
        input.protection.file.write_white = vec![ws_glob.clone()];
        input
    };

    // ---------- 1. 持久 HOME：写入的文件在下一条命令里还在，且不是真实 HOME ----------
    {
        let r1 = run_sync(mk("echo $HOME; echo persisted > $HOME/probe.txt"));
        assert_eq!(r1.exit_code, Some(0), "输出：{}", r1.output);
        assert!(
            r1.output.contains(&home.to_string_lossy().to_string()),
            "HOME 应为沙箱持久目录 {}，实际：{}",
            home.display(),
            r1.output
        );
        let r2 = run_sync(mk("cat $HOME/probe.txt"));
        assert_eq!(r2.exit_code, Some(0), "输出：{}", r2.output);
        assert!(
            r2.output.contains("persisted"),
            "持久 HOME 应跨命令保留文件：{}",
            r2.output
        );
    }

    // ---------- 2. 工作区内可执行文件可直接执行（会话里被拒的那类命令） ----------
    {
        let script = ws.join("probe.sh");
        std::fs::write(&script, "#!/bin/sh\necho workspace-exec-ok\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let r = run_sync(mk("./probe.sh"));
        assert_eq!(
            r.exit_code,
            Some(0),
            "工作区脚本应可直接执行（会话 2026-10-08 在此被拒）：{}",
            r.output
        );
        assert!(r.output.contains("workspace-exec-ok"), "输出：{}", r.output);
    }

    // ---------- 3. uv 托管 Python 可直接执行（`.venv/bin/python` 的符号链接目标） ----------
    if let Some(home_env) = std::env::var_os("HOME") {
        let uv_root = PathBuf::from(home_env)
            .join(".local")
            .join("share")
            .join("uv")
            .join("python");
        let mut found: Option<PathBuf> = None;
        if let Ok(entries) = std::fs::read_dir(&uv_root) {
            for e in entries.flatten() {
                let py = e.path().join("bin").join("python3");
                if py.exists() {
                    found = Some(py);
                    break;
                }
            }
        }
        match found {
            Some(py) => {
                let r = run_sync(mk(&format!("{} --version", py.display())));
                assert_eq!(
                    r.exit_code,
                    Some(0),
                    "uv 托管解释器应可执行（uv sync/run 的前提）：{}",
                    r.output
                );
            }
            None => eprintln!("跳过 uv Python 检查：{uv_root:?} 下无托管解释器"),
        }
    }

    // ---------- 4. 原生编译链路：cc（CLT clang）读 SDK + 链接 + 执行产物 ----------
    // 沙箱自行注入 DEVELOPER_DIR/SDKROOT（见 toolchain::dev_toolchain_env）：
    // 这里**不设任何环境变量**，验证开箱可用。
    {
        if astorm_sandbox::toolchain::dev_toolchain_env(&[]).is_some() {
            std::fs::write(ws.join("t.c"), "int main(void){return 0;}\n").unwrap();
            let r = run_sync(mk("cc t.c -o t && ./t; echo cc-exit=$?"));
            assert!(
                r.output.contains("cc-exit=0"),
                "cc 编译链路应走通（会话 2026-10-08 的硬墙：xcrun/SDK）：{}",
                r.output
            );

            // rustc 链接：会话里最终卡死的那一步（rustc → cc → xcodebuild → restricted 框架）
            if astorm_sandbox::toolchain::which_path("rustc").is_some() {
                std::fs::write(ws.join("hello.rs"), "fn main(){println!(\"rust-ok\");}\n").unwrap();
                let r = run_sync(mk("rustc -O hello.rs -o hello && ./hello; echo rust-exit=$?"));
                assert!(
                    r.output.contains("rust-exit=0") && r.output.contains("rust-ok"),
                    "rustc 链接与运行应走通：{}",
                    r.output
                );
            }
        } else {
            eprintln!("跳过原生编译检查：无可用开发工具链目录");
        }
    }

    let _ = std::fs::remove_dir_all(&ws);
}

/// 回环放行实测（默认策略：联网白名单开 + 名单为空）：
/// 本地服务能起、能连，**且驱逐监视不会把整棵树杀掉**；外网仍被内核拒绝。
///
/// 背景：bind 归 `network-inbound` 管、连出归 `network-outbound` 管，
/// 两者名单为空时原本一并禁掉 → "编译通过却起不了本地服务、自测不了"（会话 2026-10-08）。
#[test]
fn 回环放行实测() {
    if let Some(reason) = seatbelt_available() {
        eprintln!("跳过 回环放行实测：{reason}");
        return;
    }

    let ws = std::env::temp_dir().join(format!("astorm-it-loop-ws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let ws_glob = format!("{}/**", ws.to_string_lossy().replace('\\', "/"));
    // 端口按 pid 派生，避免与并行测试撞车
    let port = 18000 + (std::process::id() % 900) as u16;

    let mut input = base_input(format!(
        "python3 -c \"import socket; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); \
         s.bind(('127.0.0.1', {port})); s.listen(1); print('BIND_OK', flush=True)\" & \
         sleep 0.6; \
         python3 -c \"import socket; c=socket.socket(); c.settimeout(3); c.connect(('127.0.0.1', {port})); print('CONNECT_OK')\"; \
         sleep 1.5; \
         python3 -c \"import socket\n\
         c=socket.socket(); c.settimeout(4)\n\
         try:\n\
         \x20   c.connect(('1.1.1.1', 443)); print('EXTERNAL_UNEXPECTEDLY_OK')\n\
         except OSError as e:\n\
         \x20   print('EXTERNAL_BLOCKED', e)\"; \
         echo loop-done"
    ));
    input.timeout = Duration::from_secs(60);
    input.cwd = Some(ws.clone());
    input.workspace = Some(ws.clone());
    input.protection = ProtectionConfig {
        enabled: true,
        ..Default::default()
    };
    input.protection.file.read_white = vec![ws_glob.clone()];
    input.protection.file.write_white = vec![ws_glob.clone()];

    let report = run_sync(input);
    assert_eq!(report.exit_code, Some(0), "输出：{}", report.output);
    assert!(report.output.contains("BIND_OK"), "回环 bind 应放行：{}", report.output);
    assert!(report.output.contains("CONNECT_OK"), "回环连接应放行：{}", report.output);
    assert!(
        report.output.contains("EXTERNAL_BLOCKED"),
        "外网仍应被内核拒绝：{}",
        report.output
    );
    assert!(!report.evicted, "回环连接不得触发驱逐监视：{:?}", report.events);
    assert!(
        !report.events.iter().any(|e| matches!(e, astorm_sandbox::sandbox::Event::Evicted { .. })),
        "回环连接不得产生驱逐事件：{:?}",
        report.events
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// 「安全配置 → 额外可执行目录」实测：配置后，该目录里的可执行文件能直接跑；
/// 不配置时同一命令被内核拒绝（证明放行确实来自这条配置，而不是别处的兜底）。
#[test]
fn 额外可执行目录放行实测() {
    if let Some(reason) = seatbelt_available() {
        eprintln!("跳过 额外可执行目录放行实测：{reason}");
        return;
    }

    // 工具目录放在**工作区之外**（工作区内的 exec 本来就放行，测不出配置项的作用）
    let ws = std::env::temp_dir().join(format!("astorm-it-extra-ws-{}", std::process::id()));
    let tools = std::env::temp_dir().join(format!("astorm-it-extra-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&tools);
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&tools).unwrap();

    let script = tools.join("probe-tool.sh");
    std::fs::write(&script, "#!/bin/sh\necho extra-exec-ok\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    let ws_glob = format!("{}/**", ws.to_string_lossy().replace('\\', "/"));
    let cmd = format!("{} ", script.display()).trim().to_string();

    let mut mk = |extra: bool| {
        let mut input = base_input(cmd.clone());
        input.timeout = Duration::from_secs(30);
        input.cwd = Some(ws.clone());
        input.workspace = Some(ws.clone());
        input.protection = ProtectionConfig {
            enabled: true,
            ..Default::default()
        };
        input.protection.file.read_white = vec![ws_glob.clone()];
        input.protection.file.write_white = vec![ws_glob.clone()];
        if extra {
            input.protection.exec_white_dirs = vec![tools.to_string_lossy().to_string()];
        }
        input
    };

    // 反证：未配置 → 被内核拒绝
    let denied = run_sync(mk(false));
    assert!(
        !denied.ok,
        "未配置额外可执行目录时不应能执行工作区外的工具: {}",
        denied.output
    );
    assert!(
        denied.output.to_lowercase().contains("not permitted")
            || denied.output.to_lowercase().contains("permission denied"),
        "未配置时应是内核拒绝（而非其他错误）: {}",
        denied.output
    );

    // 配置后 → 放行
    let allowed = run_sync(mk(true));
    assert_eq!(allowed.exit_code, Some(0), "配置后应可执行：{}", allowed.output);
    assert!(allowed.output.contains("extra-exec-ok"), "输出：{}", allowed.output);

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&tools);
}

/// P2「按目标扩白名单」依赖的 Seatbelt 语义回归：
/// 只把**目标路径**加进写白名单（父目录不放行）时，能创建该目标目录与其中文件，
/// 而同级的其他路径仍被拒。若不成立，"放行 = 扩白名单"就会让 Auto 模式出现
/// "原来能跑、现在跑不了"的回退（会话 2026-10-08 的 `mkdir -p` 场景）。
#[test]
fn 扩展白名单目标可创建且不越界() {
    if let Some(reason) = seatbelt_available() {
        eprintln!("跳过 扩展白名单目标可创建且不越界：{reason}");
        return;
    }

    let base = std::env::temp_dir().join(format!("astorm-it-ext-{}", std::process::id()));
    let target = base.join("sub");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let target_glob = format!("{}/**", target.to_string_lossy().replace('\\', "/"));

    let mut input = base_input(format!(
        "mkdir -p {t} && echo hi > {t}/f.txt && cat {t}/f.txt; \
         echo sibling > {b}/sibling.txt 2>&1; echo sibling_exit=$?",
        t = target.display(),
        b = base.display()
    ));
    input.timeout = Duration::from_secs(30);
    input.protection = ProtectionConfig {
        enabled: true,
        ..Default::default()
    };
    // 只放行目标（父目录 base 不在白名单里）
    input.protection.file.read_white = vec![target_glob.clone()];
    input.protection.file.write_white = vec![target_glob];

    let report = run_sync(input);
    assert!(
        report.output.contains("hi"),
        "目标目录应可创建并写入：{}",
        report.output
    );
    assert!(
        report.output.contains("sibling_exit=1")
            || report.output.to_lowercase().contains("not permitted"),
        "同级未放行路径应仍被拒：{}",
        report.output
    );

    let _ = std::fs::remove_dir_all(&base);
}
