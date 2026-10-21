//! PTY 终端接口（交互式 shell）：前端 xterm.js 直接对接。
//! 输出不在此同步返回——由事件总线按 `terminal_output` 推送。

use napi_derive::napi;

/// 打开会话的交互式 shell（已存在则复用并应用目标尺寸）。
#[napi]
pub fn pty_open(session_id: String, cols: u16, rows: u16) -> napi::Result<()> {
    crate::app()?
        .pty()
        .open(&session_id, cols, rows)
        .map_err(|e| napi::Error::from_reason(e))
}

/// 写入 shell 输入（xterm onData 原样转发，含回车/退格等控制字节）。
#[napi]
pub fn pty_write(session_id: String, data: String) -> napi::Result<()> {
    crate::app()?
        .pty()
        .write(&session_id, &data)
        .map_err(|e| napi::Error::from_reason(e))
}

/// 调整终端尺寸（前端 fit 后同步行/列）。
#[napi]
pub fn pty_resize(session_id: String, cols: u16, rows: u16) -> napi::Result<()> {
    crate::app()?
        .pty()
        .resize(&session_id, cols, rows)
        .map_err(|e| napi::Error::from_reason(e))
}

/// 关闭会话的 shell（强杀子进程并移除会话）。
#[napi]
pub fn pty_close(session_id: String) -> napi::Result<()> {
    crate::app()?
        .pty()
        .close(&session_id)
        .map_err(|e| napi::Error::from_reason(e))
}