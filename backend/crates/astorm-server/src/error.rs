//! 统一响应信封与错误（对齐 docs/backend-api.md 的 `{code,message,data}` 约定）。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;

/// 成功信封。
pub fn ok<T: Serialize>(data: T) -> Json<serde_json::Value> {
    Json(json!({ "code": 0, "message": "ok", "data": data }))
}

/// API 错误：HTTP 状态 + 业务错误码 + 消息。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: i32,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: 40001, message: message.into() }
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self { status: StatusCode::UNAUTHORIZED, code: 40100, message: message.into() }
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, code: 40004, message: message.into() }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, code: 50000, message: message.into() }
    }
}

impl From<String> for ApiError {
    /// core 门面的错误统一是 `String`；映射为 500（业务校验类错误调用方先自行转 400）。
    fn from(message: String) -> Self {
        ApiError::internal(message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "code": self.code, "message": self.message }))).into_response()
    }
}
