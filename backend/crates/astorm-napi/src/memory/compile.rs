//! memory_compile：触发一次编译（异步；进度与结果走事件总线）。

use astorm_core::facade::memory::MemoryCompileRequest;
use napi_derive::napi;

/// 启动一次记忆编译（**立即返回**，进度与结果经 `type === "memory"` 事件推送）。
///
/// - `force` 省略 / `false`：增量（指纹未变的会话跳过）
/// - `dryRun` 省略 / `false`：正常调用 LLM；`true` 只跑确定性阶段（审阅提示词用）
///
/// 错误只在「无法启动」时抛出（未启用记忆 / 已有任务在跑 / 未配置供应商或模型）；
/// 管道内部的失败以 `kind = "failed"` 事件通知。
#[napi]
pub fn memory_compile(force: Option<bool>, dry_run: Option<bool>) -> napi::Result<()> {
    let request = MemoryCompileRequest {
        force: force.unwrap_or(false),
        dry_run: dry_run.unwrap_or(false),
    };
    crate::app()?
        .memory()
        .compile(request)
        .map_err(|e| napi::Error::from_reason(e))
}
