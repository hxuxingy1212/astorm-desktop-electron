//! PTY 终端会话管理器(Windows ConPTY / 跨平台):为终端页签(xterm.js)提供交互式 shell。
//!
//! 与一次性 `exec_command` 不同,这里每个会话一个持久的 shell 进程:
//! - 输出由读线程按 `terminal_output` 事件推送到全局事件总线(UTF-8 边界安全切分)
//! - 子进程退出由 watcher 线程轮询 `try_wait` 检测(ConPTY 读端在子进程退出后
//!   不保证 EOF,不能依赖 read 返回空来判定结束),退出后推送 `terminal_exit`
//!   并移除会话,避免残留僵尸 shell
//! - 前端通过 write / resize / close 控制

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use tokio::sync::broadcast::Sender;

/// 交互式 shell 程序:Windows 用 powershell,其它平台(含 macOS)用 sh
/// (找不到 $SHELL 时兜底 sh,保证在无登录 shell 的环境也能启动)。
fn shell_program() -> &'static str {
    if cfg!(target_os = "windows") {
        return "powershell";
    }
    // 优先用户默认 shell($SHELL),否则兜底 /bin/sh
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| Box::leak(s.into_boxed_str()) as &'static str)
        .unwrap_or("/bin/sh")
}

/// 启动参数:Windows 用 -NoLogo;非 Windows 用 -l(登录 shell,加载用户环境)。
fn shell_arg() -> &'static str {
    if cfg!(target_os = "windows") {
        "-NoLogo"
    } else {
        "-l"
    }
}

/// 单个会话的 PTY 运行时(master 用于 resize,writer 用于写输入;child 共享给 watcher 轮询)。
struct PtyRuntime {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
}

/// 终端 PTY 管理器:key = 会话 id(一个会话一个交互式 shell)。
/// 事件发送端在创建时注入,读线程与 watcher 持有 clone;会话结束时从表中移除。
pub struct PtyManager {
    sessions: Arc<Mutex<HashMap<String, PtyRuntime>>>,
    event_tx: Sender<Value>,
}

impl PtyManager {
    pub fn new(event_tx: Sender<Value>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            event_tx,
        }
    }

    /// 打开会话的交互式 shell(已存在则复用并应用目标尺寸)。
    /// 启动 powershell(会话工作空间目录),输出经事件总线异步回传。
    pub fn open(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        if self.sessions.lock().map_err(|e| e.to_string())?.contains_key(session_id) {
            return self.resize(session_id, cols, rows);
        }
        // 后端重启后内存注册表可能为空:先恢复会话记录,保证工作区解析到会话目录
        if let Ok(id) = session_id.parse::<uuid::Uuid>() {
            if let Some(app) = crate::app::App::try_global() {
                let _ = app.session().get_or_rehydrate(&id);
            }
        }
        let cwd = crate::tool::resolve_workspace(session_id);

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("打开 PTY 失败: {e}"))?;

        let mut cmd = CommandBuilder::new(shell_program());
        // Linux 容器场景（无 $SHELL → 兜底 /bin/sh）：改用 bash --norc + 信息式 PS1，
        // 对齐 SSH 终端观感（[root@host /path]#）；sh 默认提示符只有 #，不显路径。
        // 桌面（$SHELL=zsh/fish 等）保持现状，不注入。
        let bash_norc = !cfg!(target_os = "windows")
            && shell_program() == "/bin/sh"
            && std::path::Path::new("/bin/bash").exists();
        if bash_norc {
            cmd = CommandBuilder::new("/bin/bash");
            cmd.arg("--norc");
        } else {
            cmd.arg(shell_arg());
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        if bash_norc {
            cmd.env("PS1", "[\\u@\\h \\w]\\$ ");
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("启动 shell 失败: {e}"))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| format!("获取输出失败: {e}"))?;
        let writer = pair.master.take_writer().map_err(|e| format!("获取写端失败: {e}"))?;
        let child = Arc::new(Mutex::new(child));

        let runtime = PtyRuntime { master: pair.master, writer, child: Arc::clone(&child) };
        if let Ok(mut map) = self.sessions.lock() {
            map.insert(session_id.to_string(), runtime);
        }
        let sender_out = self.event_tx.clone();
        let sender_exit = self.event_tx.clone();
        let owner_out = session_id.to_string();
        let owner_reader = owner_out.clone();
        let map = Arc::clone(&self.sessions);
        let exit_poll_ms = if cfg!(target_os = "windows") { 200 } else { 100 };

        // 读线程:推送输出事件(仅负责转发,不负责会话清理——清理交给 watcher)
        std::thread::spawn(move || {
            let owner = owner_reader;
            let mut pending: Vec<u8> = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&buf[..n]);
                        // 只在 UTF-8 边界处推送,避免切分多字节字符(尾部残留留给下轮)
                        match std::str::from_utf8(&pending) {
                            Ok(text) => {
                                if !text.is_empty() {
                                    let _ = sender_out.send(json!({
                                        "type": "terminal_output",
                                        "session_id": owner,
                                        "data": text,
                                    }));
                                }
                                pending.clear();
                            }
                            Err(e) => {
                                let valid = e.valid_up_to();
                                if valid > 0 {
                                    let _ = sender_out.send(json!({
                                        "type": "terminal_output",
                                        "session_id": owner,
                                        "data": std::str::from_utf8(&pending[..valid]).unwrap_or(""),
                                    }));
                                    pending.drain(..valid);
                                }
                            }
                        }
                    }
                }
            }
        });

        // watcher 线程:轮询子进程退出 → 推送 exit 事件并移除会话
        std::thread::spawn(move || {
            let owner = owner_out;
            loop {
                // 轮询前先确认会话仍在(close 会移除)
                let exited = {
                    match child.lock() {
                        Ok(mut c) => match c.try_wait() {
                            Ok(Some(_)) => true,
                            Ok(None) => false,
                            Err(_) => true,
                        },
                        Err(_) => true,
                    }
                };
                if exited {
                    let _ = sender_exit.send(json!({ "type": "terminal_exit", "session_id": owner }));
                    if let Ok(mut m) = map.lock() {
                        m.remove(&owner);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(exit_poll_ms));
            }
        });

        Ok(())
    }

    /// 写入 shell 输入(xterm onData 原样转发)。
    pub fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        let mut map = self.sessions.lock().map_err(|e| e.to_string())?;
        let rt = map.get_mut(session_id).ok_or_else(|| "终端会话未打开".to_string())?;
        use std::io::Write;
        rt.writer.write_all(data.as_bytes()).map_err(|e| format!("写入终端失败: {e}"))
    }

    /// 调整终端尺寸(前端 fit 后同步行/列)。
    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let map = self.sessions.lock().map_err(|e| e.to_string())?;
        let rt = map.get(session_id).ok_or_else(|| "终端会话未打开".to_string())?;
        rt.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("调整终端尺寸失败: {e}"))
    }

    /// 关闭会话的 shell(强杀子进程并移除运行时)。
    pub fn close(&self, session_id: &str) -> Result<(), String> {
        let rt = self
            .sessions
            .lock()
            .map_err(|e| e.to_string())?
            .remove(session_id);
        if let Some(rt) = rt {
            if let Ok(mut child) = rt.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
            // 关闭 master 后读线程收到 EOF/错误退出,watcher 因会话被移除而停止
        }
        Ok(())
    }

    /// 关闭全部 shell(应用退出时调用,防止子进程残留)。
    pub fn kill_all(&self) {
        let mut map = match self.sessions.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        for (_, rt) in map.drain() {
            if let Ok(mut child) = rt.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}