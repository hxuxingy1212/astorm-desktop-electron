//! 终端门面：在会话工作空间内执行 shell 命令（供前端终端页签使用）。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 命令输出截断上限（防止大输出撑爆面板）。
const OUTPUT_MAX_CHARS: usize = 8000;
/// 单条命令执行超时（秒）。
const CMD_TIMEOUT: Duration = Duration::from_secs(20);
/// Windows 下子进程不创建控制台窗口（GUI 宿主拉起控制台程序会闪黑窗）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 终端门面。
pub struct TerminalImpl;

impl TerminalImpl {
    /// 在会话工作空间目录内执行命令，返回合并的 stdout/stderr 文本。
    /// Windows 用 powershell 执行，其余平台用 sh；工作区解析与工具链一致（相对名归入默认会话空间基目录）。
    pub fn exec_command(&self, session_id: String, command: String) -> Result<String, String> {
        if command.trim().is_empty() {
            return Ok(String::new());
        }
        // 后端重启后内存注册表可能为空:先按 id 恢复会话记录(resolve_workspace 依赖它),
        // 保证相对工作区归入正确的会话目录(而非默认基目录)
        if let Ok(id) = session_id.parse::<uuid::Uuid>() {
            if let Some(app) = crate::app::App::try_global() {
                let _ = app.session().get_or_rehydrate(&id);
            }
        }
        // 会话工作区目录(绝对;相对自动归入基目录并幂等创建)
        let cwd = crate::tool::resolve_workspace(&session_id);

        // Windows 用 powershell 执行，其余平台（macOS/Linux/容器）用 sh -c。
        // 显式把 PowerShell 的输出编码切到 UTF-8：中文系统默认 OEM/GBK，
        // 按 UTF-8 读取会静默截断中文输出。
        let (program, shell_args): (&str, Vec<String>) = if cfg!(windows) {
            (
                "powershell",
                vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                    format!(
                        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;\
                         $OutputEncoding=[System.Text.Encoding]::UTF8;{command}"
                    ),
                ],
            )
        } else {
            ("sh", vec!["-c".to_string(), command.clone()])
        };
        let mut cmd = Command::new(program);
        cmd.args(&shell_args)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 子进程自成进程组（Unix），超时可整组回收；Windows 走 taskkill /T。
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        // GUI 宿主进程拉起 powershell 会闪控制台窗口,必须静默
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动命令失败: {e}"))?;
        let pid = child.id();

        // 必须先并发读取管道：子进程输出超过管道缓冲（Windows ~4KB / Unix 64KB）
        // 会阻塞在 write，父进程若先等退出就永远等不到（误报超时）。
        use std::io::Read;
        let stdout_thread = child.stdout.take().map(|mut out| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                buf
            })
        });
        let stderr_thread = child.stderr.take().map(|mut err| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
                buf
            })
        });

        // 等待退出(超时则杀进程树)
        let start = Instant::now();
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if start.elapsed() > CMD_TIMEOUT {
                        crate::process::kill_tree(Some(pid));
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(format!("等待命令失败: {e}")),
            }
        }

        // 接收并发读取结果（GBK 等非 UTF-8 输出用 lossy 兜底，不再静默丢内容）
        let stdout = stdout_thread
            .and_then(|t| t.join().ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let stderr = stderr_thread
            .and_then(|t| t.join().ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();

        let mut output = format!("{stdout}{stderr}");
        if timed_out {
            output.push_str("\n[终端] 命令超时,已终止");
        }
        let trimmed = output.trim().to_string();
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        if trimmed.chars().count() > OUTPUT_MAX_CHARS {
            Ok(format!(
                "…（输出过长已截断）\n{}",
                trimmed.chars().take(OUTPUT_MAX_CHARS).collect::<String>()
            ))
        } else {
            Ok(trimmed)
        }
    }
}
