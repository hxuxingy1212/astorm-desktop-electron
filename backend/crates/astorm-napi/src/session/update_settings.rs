//! 会话级设置（模式 / 审批 / 思考强度 / 模型 / 供应商）的异步改写。

use astorm_common::session::Session as SessionFacade;
use astorm_common::session::SessionSettings;
use napi_derive::napi;
use uuid::Uuid;

use super::dto::UpdateSessionSettingsRequest;

/// 更新会话级设置：**不必发消息**，立即对后续工具调用 / 下一轮 LLM 生效。
///
/// 字段缺省 = 保持原值；成功后广播 `session_info`（前端据此校正乐观值）。
/// 已在等待用户决定的审批卡不受影响（切到自动授权只作用于后续工具调用）。
#[napi]
pub fn update_session_settings(
    session_id: String,
    request: UpdateSessionSettingsRequest,
) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    let settings: SessionSettings = request.into();
    crate::app()?
        .session()
        .update_settings(&session_id, settings)
        .map_err(|e| napi::Error::from_reason(e))
}
