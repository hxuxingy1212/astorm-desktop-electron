//! 服务端启动选项与 AppConfig 组装。
//!
//! 配置优先级：JSON 文件（`--config`，与桌面版 `start(config)` 的 JSON 同构）为底，
//! env 覆盖其上（`ASTORM_LLM_*` 预置等）。`ASTORM_DATA_DIR` 由 core
//! 的 [`astorm_core::app::AppDir`] 原生处理，本层不重复。
//! 工作空间根（用户目录）缺省 = 数据根：容器里即挂载卷，workspace/knowledge 等
//! 都落在卷内，且不会出现 `workspace/workspace` 双层（core 的 workspace_dir =
//! 用户目录根/workspace）。

use std::path::PathBuf;

use astorm_core::app::AppConfig;
use clap::Parser;
use serde_json::{json, Value};

/// astorm-server 启动选项。
#[derive(Debug, Clone, Parser)]
#[command(name = "astorm-server", about = "astorm 无头服务宿主（HTTP + SSE API）")]
pub struct ServerOptions {
    /// 应用配置 JSON 文件（与桌面版传给 napi start() 的 JSON 同构；缺省用全默认值）
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// 监听地址
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,
    /// 监听端口
    #[arg(long, default_value_t = 8787)]
    pub port: u16,
    /// 访问令牌（Bearer；缺省读 ASTORM_TOKEN，再缺省随机生成并打印）
    #[arg(long)]
    pub token: Option<String>,
    /// 关闭鉴权（仅限本机调试；容器部署务必显式设令牌）
    #[arg(long, default_value_t = false)]
    pub no_auth: bool,
    /// 数据根目录（等价 ASTORM_DATA_DIR；缺省按平台约定）
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// 用户目录根（工作空间根的上一级；覆盖配置文件 workspace；会话仍可按请求覆盖）
    #[arg(long)]
    pub workspace: Option<String>,
    /// 每会话事件环形缓冲条数（SSE 回放深度）
    #[arg(long, default_value_t = 200)]
    pub event_buffer: usize,
    /// Web UI 静态资源目录（out/renderer 产物；提供后同源托管网页，缺省纯 API）
    #[arg(long)]
    pub web_dir: Option<PathBuf>,
}

impl ServerOptions {
    /// 解析命令行并应用 env 兜底（flag 优先于 env）。
    ///
    /// 必须用 trait 的 [`clap::Parser::parse`]（它从含 argv[0] 的 `env::args_os()` 解析）：
    /// `get_matches_from` 无条件把迭代器第一个元素当二进制名，若自行 `skip(1)`
    /// 再传参，第一个真实参数会被吞成程序名，静默解析成全默认值。
    pub fn parse() -> Self {
        let mut opts = <Self as clap::Parser>::parse();
        if opts.web_dir.is_none() {
            opts.web_dir = std::env::var("ASTORM_WEB_DIR").ok().map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
        }
        opts
    }
}

/// 组装 AppConfig：JSON 文件为底 → env 覆盖 → LLM env 合成（无供应商时）。
pub fn build_app_config(opts: &ServerOptions) -> Result<AppConfig, String> {
    let mut value: Value = match &opts.config {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("读取配置文件 {} 失败: {e}", path.display()))?;
            serde_json::from_str(&text)
                .map_err(|e| format!("配置文件 {} 解析失败: {e}", path.display()))?
        }
        None => json!({}),
    };

    // 用户目录根（工作空间根的上一级）：`--workspace` / 配置文件优先；缺省 = 数据根
    // （容器内即挂载卷 /data）→ core 的 workspace_dir = <根>/workspace，会话按时间戳
    // 建在其下（`/data/workspace/<ts>`，不再出现 workspace/workspace 双层）。
    if let Some(ws) = &opts.workspace {
        value["workspace"] = json!(ws);
    } else if value
        .get("workspace")
        .and_then(|w| w.as_str())
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        value["workspace"] = json!(astorm_core::app::user_data_dir().to_string_lossy());
    }

    // env 覆盖：页面渲染服务（websearch/webfetch 共用）。容器形态由 astorm-render
    // 助手 + Chromium 承担：bin=助手路径，script 仅作占位（助手忽略该参数）。
    if let Some(bin) = std::env::var("ASTORM_WEB_RENDER_BIN").ok().filter(|s| !s.is_empty()) {
        value["webRender"]["bin"] = json!(bin);
    }
    if let Some(script) = std::env::var("ASTORM_WEB_RENDER_SCRIPT").ok().filter(|s| !s.is_empty()) {
        value["webRender"]["script"] = json!(script);
    }

    // office：bin 目录 + 内置 skill 目录（容器镜像 ENV 注入；skillDir 无 core 侧
    // 环境变量回退，必须在启动配置里——否则内置 office skill 不注册）
    if let Some(dir) = std::env::var("ASTORM_OFFICE_BIN_DIR").ok().filter(|s| !s.is_empty()) {
        value["office"]["binDir"] = json!(dir);
    }
    if let Some(dir) = std::env::var("ASTORM_OFFICE_SKILL_DIR").ok().filter(|s| !s.is_empty()) {
        value["office"]["skillDir"] = json!(dir);
    }

    // LLM env 合成：配置里没有任何 provider 时，用 ASTORM_LLM_* 三件套拼一个 default 供应商。
    // 模型引用 id 直接取 API 模型名（解析时未匹配即原样透传，见 config.rs 的 LlmModel 约定）。
    let has_providers = value
        .get("llm")
        .and_then(|l| l.get("providers"))
        .and_then(|p| p.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if !has_providers {
        let base = std::env::var("ASTORM_LLM_BASE_URL").ok().filter(|s| !s.is_empty());
        let key = std::env::var("ASTORM_LLM_API_KEY").ok();
        if let Some(base) = base {
            let key = key.ok_or("设置了 ASTORM_LLM_BASE_URL 但缺少 ASTORM_LLM_API_KEY")?;
            let model = std::env::var("ASTORM_LLM_MODEL")
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or("设置了 ASTORM_LLM_BASE_URL 但缺少 ASTORM_LLM_MODEL")?;
            value["llm"] = json!({
                "providers": [{
                    "name": "default",
                    "baseUrl": base,
                    "apiKey": key,
                    "models": [{ "id": model, "name": model }]
                }],
                "defaultProvider": "default",
                "defaultModel": model
            });
        }
    }

    serde_json::from_value(value).map_err(|e| format!("应用配置无效: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn help_报错或退出而非静默成功() {
        let r = <ServerOptions as clap::Parser>::try_parse_from(["astorm-server", "--help"]);
        eprintln!("try_parse_from(--help) = {r:?}");
        assert!(r.is_err(), "--help 不应解析成功");
    }

    #[test]
    fn 缺值应报错() {
        let r = <ServerOptions as clap::Parser>::try_parse_from(["astorm-server", "--port"]);
        eprintln!("try_parse_from(--port) = {r:?}");
        assert!(r.is_err(), "--port 缺值不应解析成功");
    }
}
