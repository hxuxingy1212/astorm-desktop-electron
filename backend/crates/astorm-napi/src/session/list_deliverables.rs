//! 会话交付物查询：`submit_deliverables` 工具逐轮落库的记录（前端会话结束后拉取渲染）。

use napi_derive::napi;

/// 列出会话全部交付物行（按轮次升序；返回 JSON 数组字符串）。
///
/// 元素形如：`{ sessionId, round, sessionTitle, userMsgSeq, userMessage,
/// deliverables: [{path,name,size?}], changedFiles: [{path,name,dir?,status?,additions,deletions}],
/// memoryIds: [string], createdAt, updatedAt }`。
#[napi]
pub fn list_session_deliverables(session_id: String) -> napi::Result<String> {
    let app = crate::app()?;
    let rows = app
        .list_deliverables(&session_id)
        .map_err(napi::Error::from_reason)?;
    Ok(serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()))
}

/// 跨会话列出交付物行（按更新时间倒序，`limit` 限量，缺省 500；返回 JSON 数组字符串）。
///
/// 「交付物」页跨会话按时间分组展示用，元素形状同 [`list_session_deliverables`]。
#[napi]
pub fn list_all_deliverables(limit: Option<i32>) -> napi::Result<String> {
    let app = crate::app()?;
    let rows = app
        .list_all_deliverables(limit.unwrap_or(500) as i64)
        .map_err(napi::Error::from_reason)?;
    Ok(serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()))
}
