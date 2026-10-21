//! 渲染层上报页签状态（`tabs` 工具的镜像）。
//!
//! 页签状态只存在于渲染层（Pinia，运行期内存），而 Rust → 渲染层只有单向异步事件，
//! 没有返回值。因此渲染层在页签发生任何变化（用户操作或 `tabs_command` 执行后）
//! 都把该会话的整份快照推上来，`tabs` 工具的 `list` 据此同步返回。
//!
//! 传入 `state_json` 为 JSON 字符串（与仓库既有 `update_config(config: String)` 同风格），
//! 字段为 snake_case，形状见 `astorm_core::tool::tabs::TabsState`。
use napi_derive::napi;

#[napi]
pub fn update_tabs_state(session_id: String, state_json: String) -> napi::Result<()> {
    if session_id.trim().is_empty() {
        return Err(napi::Error::from_reason("session_id 为空"));
    }
    let state: astorm_core::tool::tabs::TabsState = serde_json::from_str(&state_json)
        .map_err(|e| napi::Error::from_reason(format!("页签状态 JSON 非法: {e}")))?;
    astorm_core::tool::tabs::update_state(&session_id, state);
    Ok(())
}
