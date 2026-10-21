//! 审计日志读取：设置页「审计日志」模块按会话查询。

use napi_derive::napi;

/// 读取会话的审计日志（JSON 数组字符串，元素：{session_id,time,type,detail,approval,approved_at,command?}）。
#[napi]
pub fn read_audit_log(session_id: String) -> napi::Result<String> {
    let app = crate::app()?;
    Ok(app.audit_log().read_session_json(&session_id))
}
