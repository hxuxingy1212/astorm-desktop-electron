//! 浏览器后端抽象：**桌面 = agent-browser CLI**（附着主进程 iframe 作用域 CDP 桥），
//! **容器 = 宿主自有 headless Chromium**（直连 CDP，无外部二进制）。
//!
//! 两种后端共用同一套动作词汇：核心把动作编译成 `argv`（与 CLI 子命令同形，
//! 见 [`super::cli_args`]），再交给后端执行——桌面交给 CLI 进程，容器交给
//! 宿主实现（`astorm-server` 的浏览器模块）。这样分级授权 / 审批 / 审计 / 结果整形
//! 全部留在核心一处，后端只负责"把这条命令变成页面上的动作"。
//!
//! 截图例外：落盘路径由核心决定（工作区 `.astorm/browser-snapshot/`），以最后一个
//! argv 参数传给后端。

use async_trait::async_trait;
use serde_json::Value;

use super::agent_cli::CliOutput;

/// 服务端浏览器后端（容器形态；由宿主注册到 [`crate::app::AppState`]）。
#[async_trait]
pub trait NativeBrowser: Send + Sync {
    /// 当前活动页 URL（审批文案 / 结果来源用；会话浏览器未启动 → None）。
    async fn origin(&self, session_id: &str) -> Option<String>;

    /// 执行一次动作。`argv` 与 CLI 子命令同形（首个元素是动作名）；
    /// `screenshot_path` 仅 screenshot 动作有值（后端把 PNG 写到该路径）。
    async fn execute(
        &self,
        session_id: &str,
        argv: Vec<String>,
        screenshot_path: Option<String>,
    ) -> Result<CliOutput, String>;

    /// 关闭某会话的浏览器（会话删除 / 空闲回收 / 服务退出）。
    async fn close(&self, session_id: &str);

    /// 结果里的 `data` 之外的兜底来源（`_boundary.origin` 语义）。
    fn label(&self) -> &'static str {
        "服务端浏览器"
    }
}

/// 后端选择结果（工具侧）：
/// - [`Backend::Cli`]：桌面——端点文件存在（主进程桥在跑）；
/// - [`Backend::Native`]：容器——宿主注册了原生后端。
pub(crate) enum Backend {
    Cli(super::agent_cli::Endpoint),
    Native(std::sync::Arc<dyn NativeBrowser>),
}

impl Backend {
    /// 动作来源（审批文案 / 内容边界头用）。
    pub(crate) async fn origin(&self, session_id: &str) -> String {
        match self {
            Backend::Cli(ep) => ep.origin(),
            Backend::Native(nb) => nb
                .origin(session_id)
                .await
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "about:blank".to_string()),
        }
    }

    /// 审批卡里的「页面」描述（桌面是 iframe 的 frame_url；容器即当前 URL）。
    pub(crate) async fn page_label(&self, session_id: &str) -> String {
        match self {
            Backend::Cli(ep) => ep.frame_url.clone(),
            Backend::Native(nb) => nb.origin(session_id).await.unwrap_or_default(),
        }
    }

    /// 跑一条 argv（截图路径已在 argv 末尾）。
    pub(crate) async fn run(
        &self,
        session_id: &str,
        argv: &[String],
        timeout: std::time::Duration,
        max_output: usize,
        cancel: tokio_util::sync::CancellationToken,
        screenshot_path: Option<String>,
    ) -> Result<CliOutput, String> {
        match self {
            Backend::Cli(ep) => {
                super::agent_cli::run(ep, argv, timeout, max_output, cancel).await
            }
            Backend::Native(nb) => {
                tokio::select! {
                    r = nb.execute(session_id, argv.to_vec(), screenshot_path) => r,
                    _ = cancel.cancelled() => Err("已取消".to_string()),
                }
            }
        }
    }
}

/// 后端执行结果里可选的原始 JSON（后端可返回任意结构，核心按既有键位整形）。
pub(crate) fn data_object(value: Value) -> Value {
    if value.is_object() || value.is_array() || value.is_string() {
        value
    } else {
        Value::Null
    }
}
