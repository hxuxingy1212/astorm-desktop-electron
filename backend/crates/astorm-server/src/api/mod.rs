//! 路由装配：`/health`（免鉴权）+ `/api/v1/*`（Bearer 鉴权）+ Web 静态托管（SPA）。

use axum::routing::{get, post};
use axum::{middleware, Router};
use axum::http::{header, HeaderValue};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth;
use crate::state::ServerState;

mod bridge;
mod llm;
mod sessions;
mod system;
mod webdata;
mod ws;

/// 组装完整路由。
pub fn router(state: ServerState) -> Router {
    let api = Router::new()
        // 系统
        .route("/status", get(system::status))
        // 事件流（SSE 全局）
        .route("/events", get(sessions::sse_global))
        // 会话
        .route("/sessions", post(sessions::create).get(sessions::list))
        .route(
            "/sessions/{id}",
            get(sessions::get_one).delete(sessions::delete),
        )
        .route(
            "/sessions/{id}/messages",
            post(sessions::send_message).get(sessions::messages),
        )
        .route("/sessions/{id}/messages/archived", get(sessions::archived))
        .route("/sessions/{id}/start", post(sessions::start))
        .route("/sessions/{id}/cancel", post(sessions::cancel))
        .route("/sessions/{id}/interrupt", post(sessions::interrupt))
        .route("/sessions/{id}/interrupts/resolve", post(sessions::resolve_interrupt))
        .route("/sessions/{id}/interrupts/{tool_call_id}/reopen", post(sessions::reopen_interrupt))
        .route("/sessions/{id}/fork", post(sessions::fork))
        .route("/sessions/{id}/rewind", post(sessions::rewind))
        .route("/sessions/{id}/summarize", post(sessions::summarize))
        .route("/sessions/{id}/settings", axum::routing::patch(sessions::update_settings))
        .route("/sessions/{id}/system-prompt", get(sessions::system_prompt))
        .route("/sessions/{id}/events", get(sessions::sse_session))
        .route("/sessions/{id}/deliverables", get(sessions::session_deliverables))
        // 交付物
        .route("/deliverables", get(sessions::all_deliverables))
        // 单次 LLM 补全（SSE）
        .route("/llm/ask", post(llm::ask))
        // Web 桥：backend.invoke 命令路由 + 文件类端点
        .route("/bridge", post(bridge::invoke))
        .route("/fs/read-base64", get(webdata::read_base64))
        .route("/fs/read-bytes", get(webdata::read_bytes))
        .route("/fs/write", post(webdata::write_text))
        // 目录选择器（web 端替代原生目录对话框）：单层列目录 + 新建文件夹
        .route("/fs/list-dirs", get(webdata::list_dirs))
        .route("/fs/mkdir", post(webdata::make_dir))
        .route("/attachments", post(webdata::upload_attachment))
        .route("/attachment/file/{*path}", get(webdata::serve_attachment))
        .route("/kb/file/{*path}", get(webdata::serve_kb_file))
        .route("/office/preview", post(webdata::office_preview))
        .route("/providers", get(webdata::providers_list))
        .route("/providers/sync", post(webdata::providers_sync))
        .route("/providers/test", post(webdata::providers_test))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_auth))
        .with_state(state.clone());

    let mut router = Router::new()
        .route("/health", get(system::health))
        // web-bridge.js 显式路由：no-cache——迭代期浏览器缓存旧桥会造成新旧契约混跑
        .route("/web-bridge.js", get(webdata::web_bridge_js))
        .nest("/api/v1", api)
        // WS 挂在受保护组之外：中间件按 Bearer 头鉴权，而浏览器 WebSocket 无法
        // 自定义头——令牌走查询串由端点自行校验（nest 内的相对路径判断不可靠）
        .route("/api/v1/ws", get(ws::events_ws))
        // 面板画面流（同 WS 令牌语义）：服务端浏览器的帧下行 + 输入上行
        .route("/api/v1/browser/ws", get(ws::browser_ws))
        .with_state(state.clone());

    // Web 静态托管：`--web-dir` 指定时托管 SPA（hash 路由，fallback 到 index.html）
    if let Some(dir) = &state.web_dir {
        let index = dir.join("index.html");
        if index.is_file() {
            // 缓存策略：hash 资源可长缓存，**index.html 必须 no-cache**——否则容器升级后
            // 用户浏览器还拿着旧 index.html（指向已消失的旧 bundle 或跑着旧 UI，实测踩过：
            // 面板改了行为但页面仍是旧逻辑）。
            // 静态资源统一 `no-cache`（带 Last-Modified/ETag，浏览器每次都校验 → 304 极便宜，
            // 但绝不会再拿旧 bundle 跑旧 UI）；hash 资源本可长缓存，收益远小于升级踩坑的代价。
            let serve = tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                .service(ServeDir::new(dir).fallback(ServeFile::new(index)));
            router = router.fallback_service(serve);
            println!("astorm-server: 托管 Web UI → {}", dir.display());
        } else {
            eprintln!(
                "astorm-server: web 目录缺少 index.html（{}），仅提供 API",
                index.display()
            );
        }
    }
    router
}
