//! Bearer 令牌鉴权中间件：`Authorization: Bearer <token>`。
//!
//! `/health` 不走本中间件（编排器探活用）；其余 `/api/v1/*` 全部要求令牌，
//! 服务端未配置令牌（--no-auth）时放行。

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::ServerState;

/// 常量时间比较（等长遍历累加差异，避免逐字符提前返回暴露时序）。
pub(crate) fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 鉴权中间件本体。
pub async fn require_auth(
    State(state): State<ServerState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = &state.token else {
        return next.run(req).await;
    };
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match provided {
        Some(t) if token_eq(t, expected) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "code": 40100,
                "message": "缺少或无效的 Bearer 令牌"
            })),
        )
            .into_response(),
    }
}
