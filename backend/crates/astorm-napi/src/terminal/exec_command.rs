//! exec_command：在会话工作空间内执行 shell 命令（前端终端页签）。

use napi_derive::napi;

/// 在指定会话的工作空间目录内执行命令,返回合并输出(20s 超时,输出截断)。
#[napi]
pub fn exec_command(session_id: String, command: String) -> napi::Result<String> {
    crate::app()?
        .terminal()
        .exec_command(session_id, command)
        .map_err(|e| napi::Error::from_reason(e))
}