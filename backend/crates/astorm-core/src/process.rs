//! 子进程整树回收工具。
//!
//! `kill_on_drop` / `Child::kill` 只终止直接子进程；office CLI、agent-browser
//! 这类工具会再拉起脚本/浏览器/daemon 子进程。这里统一：
//! - Unix：spawn 时 `process_group(0)`，超时/取消 `kill(-pgid)`；
//! - Windows：`taskkill /PID <pid> /T /F`（并静默控制台窗口）。
//!
//! `output_with_tree_kill` 还负责并发读取 stdout/stderr，避免大输出把管道写满
//! 后子进程阻塞、超时误判。

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// 一次命令的完整输出。
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

/// 命令执行失败类型。
#[derive(Debug)]
pub enum TreeRunError {
    Spawn(std::io::Error),
    Io(std::io::Error),
    Timeout,
    Cancelled,
}

/// 把命令配置成“独立进程组 / 静默窗口”，供 [`kill_tree`] 整组回收。
pub fn configure_tokio(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
}

/// 回收整棵进程树。
#[cfg(windows)]
pub fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();
}

/// Unix：子进程以独立进程组启动，负 pid 即整组。
#[cfg(unix)]
pub fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    // SAFETY: kill(2) 只读取 pid 参数；失败忽略。
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// 运行命令并返回完整输出；超时/取消时终止整棵进程树。
pub async fn output_with_tree_kill(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<CommandOutput, TreeRunError> {
    configure_tokio(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(TreeRunError::Spawn)?;
    let pid = child.id();

    let stdout_task = child.stdout.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf).await;
            buf
        })
    });
    let stderr_task = child.stderr.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf).await;
            buf
        })
    });

    let cancel_fut = async {
        match cancel {
            Some(c) => c.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    let status = tokio::select! {
        _ = cancel_fut => {
            kill_tree(pid);
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
        res = tokio::time::timeout(timeout, child.wait()) => match res {
            Ok(Ok(status)) => Some(status),
            Ok(Err(e)) => return Err(TreeRunError::Io(e)),
            Err(_) => {
                kill_tree(pid);
                let _ = child.kill().await;
                let _ = child.wait().await;
                None
            }
        },
    };

    let stdout = match stdout_task {
        Some(t) => t.await.unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = match stderr_task {
        Some(t) => t.await.unwrap_or_default(),
        None => Vec::new(),
    };
    match status {
        Some(status) => Ok(CommandOutput {
            stdout,
            stderr,
            status,
        }),
        None => {
            if cancel.is_some_and(|c| c.is_cancelled()) {
                Err(TreeRunError::Cancelled)
            } else {
                Err(TreeRunError::Timeout)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_pid_file(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astorm-tree-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("grandchild.pid")
    }

    #[tokio::test]
    async fn success_path_returns_output() {
        #[cfg(unix)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "echo tree-ok"]);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/D", "/C", "echo tree-ok"]);
            c
        };
        let out = output_with_tree_kill(cmd, Duration::from_secs(10), None)
            .await
            .expect("command should succeed");
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("tree-ok"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_grandchild() {
        let pidfile = temp_pid_file("unix");
        let script = format!(
            "sh -c 'echo $$ > {}; sleep 30' & sleep 30",
            pidfile.display()
        );
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", &script]);
        let res = output_with_tree_kill(cmd, Duration::from_millis(300), None).await;
        assert!(matches!(res, Err(TreeRunError::Timeout)));
        let pid = wait_pid_file(&pidfile);
        assert!(!alive(pid), "grandchild {pid} should be killed");
        let _ = std::fs::remove_dir_all(pidfile.parent().unwrap());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_kills_grandchild() {
        let pidfile = temp_pid_file("win");
        let script = format!(
            "$p = Start-Process -FilePath ping -ArgumentList '-n','30','127.0.0.1' -PassThru -WindowStyle Hidden; \
             $p.Id | Set-Content -Path '{}'; Start-Sleep 30",
            pidfile.display()
        );
        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        let res = output_with_tree_kill(cmd, Duration::from_millis(1500), None).await;
        assert!(matches!(res, Err(TreeRunError::Timeout)));
        let pid = wait_pid_file(&pidfile);
        assert!(!alive(pid), "grandchild {pid} should be killed");
        let _ = std::fs::remove_dir_all(pidfile.parent().unwrap());
    }

    fn wait_pid_file(path: &std::path::Path) -> u32 {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return pid;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("grandchild pid file not written: {}", path.display());
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        // SAFETY: kill(pid, 0) 只做存在性检查
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[cfg(windows)]
    fn alive(pid: u32) -> bool {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .creation_flags(0x0800_0000)
            .output();
        out.map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some(&pid.to_string()))
        })
        .unwrap_or(false)
    }
}
