//! 共享请求状态。

use std::path::PathBuf;
use std::sync::Arc;

use astorm_core::app::AppState;

use crate::events::EventHub;
use crate::local::LocalStore;

/// axum 提取用的服务端状态。
#[derive(Clone)]
pub struct ServerState {
    /// 应用门面（core 全局单例）。
    pub app: Arc<AppState>,
    /// 事件中枢（常驻订阅 + 环形缓冲 + SSE 扇出）。
    pub hub: Arc<EventHub>,
    /// Bearer 令牌（None = 关闭鉴权）。
    pub token: Option<String>,
    /// 优雅关闭信号（SSE/WS 泵据此断开，避免 graceful shutdown 被长连接挂住）。
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    /// 服务端本地文件存储（settings/providers/AGENTS.md/staging）。
    pub local: Arc<LocalStore>,
    /// 服务端浏览器中枢（容器形态的 browser 工具后端 + 面板画面流）。
    pub browser: Arc<crate::browser::BrowserHub>,
    /// Web 静态资源目录（Some 时托管 SPA；None = 纯 API）。
    pub web_dir: Option<PathBuf>,
}
