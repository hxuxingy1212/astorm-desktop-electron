//! MCP 服务器 napi 接口：服务器列表 / 配置读写 / 连接 · 断开 · 启停。
//!
//! 委托 [`astorm_core::app::AppState::mcp`]（[`astorm_core::mcp::McpManager`]）——**客户端在
//! core（Rust）**，桌面与云容器同一实现（stdio 子进程常驻 + HTTP JSON-RPC）。本模块只做
//! DTO 形状转换与错误映射，不含业务逻辑。

use napi_derive::napi;

use astorm_core::mcp::{McpServerRow, OAuthStart, OAuthStatus};

fn err(e: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(e.to_string())
}

/// MCP 服务器摘要（渲染层 `McpServerSummary` 契约）。
#[napi(object)]
#[derive(Debug, Clone)]
pub struct McpServerSummary {
    pub name: String,
    /// `stdio` / `http`
    pub transport_type: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub enabled: bool,
    /// 当前是否有活连接（连接后可调用其工具）
    pub connected: bool,
    /// 远程服务器最近一次连接是否要求授权（HTTP 401）
    pub auth_required: bool,
}

impl From<McpServerRow> for McpServerSummary {
    fn from(r: McpServerRow) -> Self {
        Self {
            name: r.name,
            transport_type: r.transport_type.to_string(),
            command: r.command,
            args: r.args,
            url: r.url,
            enabled: r.enabled,
            connected: r.connected,
            auth_required: r.auth_required,
        }
    }
}

/// OAuth 授权启动结果（渲染层打开 `authorization_url`）。
#[napi(object)]
#[derive(Debug, Clone)]
pub struct McpOAuthStart {
    pub authorization_url: String,
    pub redirect_uri: String,
}

impl From<OAuthStart> for McpOAuthStart {
    fn from(s: OAuthStart) -> Self {
        Self {
            authorization_url: s.authorization_url,
            redirect_uri: s.redirect_uri,
        }
    }
}

/// OAuth 轮询状态：none / pending / authorized / failed。
#[napi(object)]
#[derive(Debug, Clone)]
pub struct McpOAuthStatus {
    pub status: String,
    pub error: Option<String>,
}

impl From<OAuthStatus> for McpOAuthStatus {
    fn from(s: OAuthStatus) -> Self {
        Self {
            status: s.status.to_string(),
            error: s.error,
        }
    }
}

/// 服务器列表（含 `connected` 实况）。
#[napi]
pub fn mcp_list_servers() -> napi::Result<Vec<McpServerSummary>> {
    Ok(crate::app()?
        .mcp()
        .list_servers()
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 配置文件原文（`<config_dir>/mcp_servers.json`；缺失返回缺省文本）。
#[napi]
pub fn mcp_read_config_file() -> napi::Result<String> {
    Ok(crate::app()?.mcp().config_text())
}

/// 写入配置（JSON 文本）：校验 → 落盘 → 重载 → 连接启用项。
#[napi]
pub async fn mcp_write_config_file(content: String) -> napi::Result<()> {
    crate::app()?.mcp().write_config(&content).await.map_err(err)
}

/// 连接服务器（握手 + 工具发现），返回可读回执（如「已连接 x（3 个工具）」）。
#[napi]
pub async fn mcp_connect_server(name: String) -> napi::Result<String> {
    crate::app()?.mcp().connect(&name).await.map_err(err)
}

/// 断开连接（配置保留）。
#[napi]
pub fn mcp_disconnect_server(name: String) -> napi::Result<()> {
    crate::app()?.mcp().disconnect(&name).map_err(err)
}

/// 启用 / 禁用（禁用即断开；仅改标志，连接由 `mcp_connect_server` 或启动自动连接负责）。
#[napi]
pub fn mcp_toggle_server(name: String, enabled: bool) -> napi::Result<()> {
    crate::app()?.mcp().toggle(&name, enabled).map_err(err)
}

/// 保存远程服务器手动授权请求头（JSON 对象，如 `{"Authorization":"Bearer ..."}`）并重连。
#[napi]
pub async fn mcp_set_server_auth(name: String, headers_json: String) -> napi::Result<String> {
    crate::app()?
        .mcp()
        .set_server_auth(&name, &headers_json)
        .await
        .map_err(err)
}

/// 开始 OAuth 授权：返回浏览器授权地址（本机回环接收回调）。
#[napi]
pub async fn mcp_begin_oauth(name: String) -> napi::Result<McpOAuthStart> {
    crate::app()?
        .mcp()
        .begin_oauth(&name)
        .await
        .map(Into::into)
        .map_err(err)
}

/// 查询 OAuth 授权状态（前端轮询）。
#[napi]
pub fn mcp_oauth_status(name: String) -> napi::Result<McpOAuthStatus> {
    Ok(crate::app()?.mcp().oauth_status(&name).into())
}
