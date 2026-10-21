//! 系统端点：探活与状态。

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::error::ok;
use crate::local;
use crate::state::ServerState;

/// `GET /health`：免鉴权探活（编排器 HEALTHCHECK 用）。
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

/// `GET /api/v1/status`：版本 / 平台 / 路径 / LLM 配置摘要。
pub async fn status(State(state): State<ServerState>) -> Json<serde_json::Value> {
    let app = &state.app;
    let dirs = app.dir();
    let cfg = app.config();
    ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "dataDir": dirs.data_dir.to_string_lossy(),
        "workspace": cfg.workspace,
        "locale": cfg.locale,
        // 生效供应商：providers.json（Web 设置页写的真源）优先，空则回退运行配置
        "llm": local::effective_providers(&state.local, cfg),
    }))
}
