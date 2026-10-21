//! 会话目标（`/goal`）的异步设置 / 修改 / 删除：不必发消息，立即写活记录并广播。

use astorm_common::session::Session as SessionFacade;
use napi_derive::napi;
use uuid::Uuid;

/// 更新会话目标：`goal` 为 `null` / 空白 = 删除目标；非空 = 新设或修改（预算重置）。
///
/// 写的是内存活记录，下一次目标判定 / 续跑即读取新值；成功后广播 `session_info`。
#[napi]
pub fn update_session_goal(
    session_id: String,
    goal: Option<String>,
    goal_lang: Option<String>,
) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .update_goal(&session_id, goal, goal_lang)
        .map_err(|e| napi::Error::from_reason(e))
}
