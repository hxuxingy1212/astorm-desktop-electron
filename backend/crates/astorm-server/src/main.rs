//! astorm 无头服务宿主：把 astorm-core 门面直接暴露为 HTTP + SSE API。
//!
//! 与 astorm-napi 的关系：两者是**同一 core 的两个宿主**——本 crate 不经 napi，
//! 直接调用 `App::init` + 各业务门面（见 docs/cloud-server.md 的架构说明）。
//!
//! 生命周期约束：`App::init` 会自建 tokio runtime，且必须在任何 async 上下文之外
//! 调用（见 astorm-tests/security_matrix.rs 的注记）。因此 `main` 全程同步：
//! 先 init core，再把 axum 通过 `AppState::handle()` 挂到应用自己的 runtime 上。
//!
//! 启动：`astorm-server [--config app.json] [--host 0.0.0.0] [--port 8787]
//!        [--token XXX | --no-auth]`
//! 环境变量：`ASTORM_DATA_DIR`（数据根，core 原生支持）、`ASTORM_TOKEN`、
//! `ASTORM_LLM_BASE_URL` / `ASTORM_LLM_API_KEY` / `ASTORM_LLM_MODEL`（可选预置：
//! 配置无供应商时合成一个 default 供应商；日常由 Web 设置页配置）。
//! 工作空间根缺省 = 数据根（容器内即挂载卷 → `<卷>/workspace/<会话时间戳>`）。

mod api;
mod auth;
mod browser;
mod config;
mod error;
mod events;
mod local;
mod state;

use std::sync::Arc;

use astorm_common::types::Storage;
use astorm_core::app::{App, AppDir};
use astorm_storage::StorageManager;

use crate::config::ServerOptions;
use crate::events::EventHub;
use crate::state::ServerState;

fn main() {
    let opts = ServerOptions::parse();

    // 数据根：core 的 AppDir 原生读 ASTORM_DATA_DIR；--data-dir 在单线程期提前写入 env。
    // SAFETY: main 启动早期、runtime 线程尚未创建，无并发读 env 的可能（edition 2024 起 set_var 为 unsafe）。
    if let Some(dir) = &opts.data_dir {
        unsafe { std::env::set_var("ASTORM_DATA_DIR", dir) };
    }

    // 1) 组装 AppConfig（JSON 文件 + env 覆盖 + LLM env 合成）
    let app_config = match config::build_app_config(&opts) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("astorm-server: 配置错误: {e}");
            std::process::exit(2);
        }
    };

    // 2) 初始化 core（自建 runtime / 事件总线 / LLM 注册表 / 存储；必须在 async 外）
    let dirs = AppDir::new(&app_config);
    let manager = match StorageManager::open_default(&dirs.data_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "astorm-server: 初始化存储失败（{}）: {e}",
                dirs.data_dir.display()
            );
            std::process::exit(2);
        }
    };
    let storage: Arc<dyn Storage> = Arc::new(manager);
    if let Err(e) = astorm_core::app::App::init(app_config, storage) {
        eprintln!("astorm-server: 应用初始化失败: {e}");
        std::process::exit(2);
    }
    let app = App::global();
    println!(
        "astorm-server: 数据目录 {}，工作空间 {}",
        app.dir().data_dir.display(),
        app.config().workspace
    );

    // 启动即把**已保存的配置**热更进核心（providers.json / settings.json 存在时）：
    // Web 设置页配置的供应商与设置必须跨重启生效——否则核心退回 env/--config 默认，
    // 重启后模型「消失」（实测：/status providers=[] 且对话空回复）。
    // 与保存路径走同一 sync（含 snake→camel 映射、${ENV} 展开、补 models[].id）。
    {
        let store0 = local::LocalStore::new();
        let has_saved = !store0.read("providers.json").is_null() || !store0.read("settings.json").is_null();
        if has_saved {
            if let Err(e) = local::sync_config_to_core(&store0) {
                eprintln!("astorm-server: 加载已保存配置失败（继续用 env/--config 默认值）: {e}");
            }
        }
        // env 注入的 LLM 供应商物化为 providers.json 种子（UI 设置页可读可改；
        // 否则 App.vue 的 models watcher 启动即写空列表，把 env 配置清掉）
        local::seed_providers(&store0, app.config());
    }

    // 服务端浏览器（容器形态的 browser 工具后端）：注册到 core + 空闲回收。
    // 桌面宿主不注册 → browser 工具继续走 agent-browser CLI 的 iframe 桥，行为不变。
    let browser_hub = std::sync::Arc::new(browser::BrowserHub::new());
    app.set_native_browser(browser_hub.clone());
    app.handle()
        .spawn(browser::BrowserHub::reap_loop(browser_hub.clone()));

    // 3) 访问令牌：未提供则生成随机令牌打印（本地试用零配置；容器部署建议显式注入）
    let env_token = std::env::var("ASTORM_TOKEN").ok().filter(|t| !t.trim().is_empty());
    let token = match opts.token.clone().or(env_token) {
        Some(t) => Some(t),
        None if opts.no_auth => None,
        None => {
            let t = uuid::Uuid::new_v4().simple().to_string();
            eprintln!("astorm-server: 未设置访问令牌，已生成临时令牌（也可用 --token / ASTORM_TOKEN 显式指定）：");
            eprintln!("  ASTORM_TOKEN={t}");
            Some(t)
        }
    };

    // 4) 事件中枢：启动即常驻订阅广播总线（补齐「HTTP 下订阅晚于事件」的竞态），
    //    按 session_id 分桶写环形缓冲供 SSE 回放；shutdown 通道用于掐断 SSE 长连接。
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let hub = EventHub::start(app.clone(), opts.event_buffer);
    let local = local::LocalStore::new();

    // 5) axum 挂到应用 runtime（core 的会话/工具/调度器都在这套 runtime 上）
    let state = ServerState {
        app: app.clone(),
        hub,
        token,
        shutdown: shutdown_rx,
        local,
        browser: browser_hub.clone(),
        web_dir: opts.web_dir.clone(),
    };
    let router = api::router(state);
    let handle = app.handle();
    let host = opts.host.clone();
    let port = opts.port;
    let serve_result = handle.block_on(async move {
        let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
            Ok(l) => l,
            Err(e) => return Err(format!("监听 {host}:{port} 失败: {e}")),
        };
        println!("astorm-server: listening on http://{host}:{port} (API 前缀 /api/v1)");
        // 信号 → 置位 shutdown（SSE 泵任务据此断开）→ axum graceful 收尾
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_signal().await;
                let _ = shutdown_tx.send(true);
            })
            .await
            .map_err(|e| format!("HTTP 服务异常退出: {e}"))
    });
    if let Err(e) = serve_result {
        eprintln!("astorm-server: {e}");
    }

    // 6) 优雅退出：杀残存 PTY + 取消会话循环 + flush 日志（与 napi shutdown 同款）
    app.pty().kill_all();
    app.shutdown();
}

/// SIGTERM / SIGINT → 返回即触发优雅关闭。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    println!("astorm-server: 收到退出信号，开始优雅关闭");
}
