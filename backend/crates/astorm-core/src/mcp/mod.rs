//! MCP（Model Context Protocol）客户端：`mcp_servers.json` 配置 + stdio/HTTP 传输 +
//! 工具发现与调用。宿主无关（桌面 / 云容器共用同一实现）。
//!
//! 与桌面端主进程历史实现（`frontend/src/main/backend.ts`：握手后即杀进程、工具只展示
//! 不可调用）不同，本模块**保持连接**并支持 `tools/call`——连接后的服务器工具直接进入
//! 会话工具集（`agent::context` / `agent::hook` 每轮按当前连接重建 `ctx.tools`）。
//!
//! 配置（`<config_dir>/mcp_servers.json`，与桌面同格式）：
//! ```json
//! { "mcpServers": { "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
//!                              "env": {"GITHUB_TOKEN": "..."}, "enabled": true } } }
//! ```
//! - `url` 存在 → HTTP 传输，否则 `command` + `args` → stdio 子进程；
//! - `enabled`（缺省 true）→ 启动 / 配置写入后自动连接；
//! - `env` 追加进子进程环境（stdio）。
//!
//! 工具命名：默认用服务器侧原名；与**内置工具**重名时对外暴露 `{server}__{name}`（内置优先，
//! 避免同一会话里两个同名工具互相覆盖）。跨服务器重名按服务器名字典序先到先得，后者跳过并告警。

mod oauth;
mod tool;
mod transport;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use astorm_common::tool::{ToolInfo, ToolSource};
use react_loop::Tool;
use serde_json::{json, Value};

pub use oauth::{OAuthStart, OAuthStatus};
use oauth::OAuthRuntime;
use tool::McpToolHandle;
use transport::{HttpTransport, StdioTransport, Transport, parse_auth_required};

/// 客户端标识（initialize 握手）。
const CLIENT_NAME: &str = "astorm-agent";
const CLIENT_VERSION: &str = "0.1.0";
/// MCP 协议版本（与桌面端历史实现一致）。
const PROTOCOL_VERSION: &str = "2024-11-05";
/// 握手（initialize + tools/list）超时。
///
/// 放宽到 45s：`npx` / `uvx` 系服务器**首次运行要先下载包**（后续走缓存），15s 常不够。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);
/// 单次工具调用超时（MCP 工具可能含浏览器自动化等长任务）。
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// 工具结果文本上限（超出截断，避免超长输出挤爆上下文）。
const MAX_RESULT_CHARS: usize = 50_000;
/// 配置文件缺失时的回退原文（与桌面端一致）。
const DEFAULT_CONFIG_TEXT: &str = "{\n  \"mcpServers\": {}\n}";

/// MCP 服务器管理器（进程级，挂于 `AppState`）。
pub struct McpManager {
    /// 配置文件路径（`<config_dir>/mcp_servers.json`）。
    file: PathBuf,
    /// 服务器槽位（name → 配置 + 活连接），BTreeMap 保证遍历顺序确定（工具命名稳定）。
    servers: RwLock<BTreeMap<String, ServerSlot>>,
    /// 远程服务器 OAuth（发现 / 注册 / 回环回调 / token 存储）。
    oauth: OAuthRuntime,
}

struct ServerSlot {
    /// 配置原文（保留用户写入的未知字段；persist 时只同步 `enabled`）。
    raw: Value,
    cfg: ServerConfig,
    /// 活连接（None = 未连接）；工具随连接一起缓存。
    conn: Option<Conn>,
    /// 上次连接是否因 401 需要授权。
    auth_required: bool,
    /// 401 下发的 OAuth resource metadata 地址（下次授权优先用它发现）。
    auth_hint: Option<String>,
}

struct Conn {
    transport: Arc<Transport>,
    tools: Vec<McpToolDef>,
}

#[derive(Debug, Clone, Default)]
struct ServerConfig {
    command: String,
    args: Vec<String>,
    url: String,
    env: BTreeMap<String, String>,
    /// 远程服务器的自定义请求头（手动授权 / API Key）。
    headers: BTreeMap<String, String>,
    /// 单次请求超时（配置里的 `timeout`，毫秒）。
    timeout: Option<Duration>,
    enabled: bool,
}

impl ServerConfig {
    /// 连接目标是否相同（配置写入后决定能否沿用旧连接；env / headers 变化也要重连）。
    fn same_target(&self, other: &ServerConfig) -> bool {
        self.command == other.command
            && self.args == other.args
            && self.url == other.url
            && self.env == other.env
            && self.headers == other.headers
    }

    fn request_timeout(&self, fallback: Duration) -> Duration {
        self.timeout.unwrap_or(fallback)
    }
}

#[derive(Debug, Clone)]
struct McpToolDef {
    name: String,
    description: String,
    schema: Value,
}

/// 服务器摘要行（宿主 DTO：桌面 napi / 云端桥命令同形状）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRow {
    pub name: String,
    /// `stdio` / `http`
    pub transport_type: &'static str,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub enabled: bool,
    /// 当前是否有活连接（连接后可调用其工具）
    pub connected: bool,
    /// 远程服务器是否在最近一次连接中要求授权（HTTP 401）。
    pub auth_required: bool,
}

/// 对外暴露的工具（服务器 + 原名 + 暴露名 + schema）。
struct ExposedTool {
    server: String,
    raw: String,
    name: String,
    description: String,
    schema: Value,
}

impl McpManager {
    /// 从配置目录装配（读取 `mcp_servers.json`；文件缺失 / 损坏 → 空清单）。
    pub fn new(config_dir: &Path) -> Self {
        let manager = Self {
            file: config_dir.join("mcp_servers.json"),
            servers: RwLock::new(BTreeMap::new()),
            oauth: OAuthRuntime::new(config_dir),
        };
        manager.reload_from_disk();
        manager
    }

    /// 配置文件路径（宿主诊断用）。
    pub fn file(&self) -> &Path {
        &self.file
    }

    /* ---------- 配置读写（设置页经桥命令调用） ---------- */

    /// 配置文件原文（缺失返回缺省文本）。
    pub fn config_text(&self) -> String {
        std::fs::read_to_string(&self.file).unwrap_or_else(|_| DEFAULT_CONFIG_TEXT.to_string())
    }

    /// 写入配置文件（JSON 文本）：校验 → 落盘 → 重载 → 连接启用项。
    pub async fn write_config(&self, text: &str) -> Result<(), String> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("JSON 格式无效: {e}"))?;
        if value
            .get("mcpServers")
            .and_then(|m| m.as_object())
            .is_none()
        {
            return Err("JSON 格式无效: 缺少 mcpServers 字段".to_string());
        }
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // 保留用户原文（编辑器回读内容一致），仅保证结尾换行
        let mut content = text.to_string();
        if !content.ends_with('\n') {
            content.push('\n');
        }
        astorm_common::fsx::write_atomic(&self.file, content.as_bytes())
            .map_err(|e| format!("写入 MCP 配置失败: {e}"))?;
        self.reload_from_disk();
        self.connect_enabled().await;
        Ok(())
    }

    /// 服务器列表（name / transport_type / command / args / url / enabled / connected）。
    pub fn list_servers(&self) -> Vec<McpServerRow> {
        let Ok(guard) = self.servers.read() else {
            return Vec::new();
        };
        guard
            .iter()
            .map(|(name, slot)| {
                let cfg = &slot.cfg;
                McpServerRow {
                    name: name.clone(),
                    transport_type: if cfg.url.is_empty() { "stdio" } else { "http" },
                    command: cfg.command.clone(),
                    args: cfg.args.clone(),
                    url: cfg.url.clone(),
                    enabled: cfg.enabled,
                    connected: slot.conn.is_some(),
                    auth_required: slot.auth_required,
                }
            })
            .collect()
    }

    /// 启用 / 禁用（禁用即断开；仅改标志，连接由「连接」按钮或启动自动连接负责）。
    pub fn toggle(&self, name: &str, enabled: bool) -> Result<(), String> {
        {
            let Ok(mut guard) = self.servers.write() else {
                return Err("MCP 状态不可用".to_string());
            };
            let slot = guard
                .get_mut(name)
                .ok_or_else(|| format!("MCP 服务器不存在: {name}"))?;
            slot.cfg.enabled = enabled;
            if !enabled && let Some(conn) = slot.conn.take() {
                conn.transport.close();
            }
        }
        self.persist()
    }

    /* ---------- 连接生命周期 ---------- */

    /// 连接指定服务器：起传输 → 握手 → 工具发现 → 落槽位（同名旧连接关闭）。
    pub async fn connect(&self, name: &str) -> Result<String, String> {
        let cfg = {
            let Ok(guard) = self.servers.read() else {
                return Err("MCP 状态不可用".to_string());
            };
            guard
                .get(name)
                .map(|slot| slot.cfg.clone())
                .ok_or_else(|| format!("MCP 服务器不存在: {name}"))?
        };
        let mut headers = cfg.headers.clone();
        // 手动填写的 Authorization 优先于 OAuth token（避免旧 token 覆盖用户刚填的值）
        let manual_auth = headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("authorization"));
        if !manual_auth
            && !cfg.url.trim().is_empty()
            && let Some(token) = self.oauth.access_token(name, &cfg.url).await
        {
            headers.insert("Authorization".to_string(), format!("Bearer {token}"));
        }
        let transport = if cfg.url.trim().is_empty() {
            Transport::Stdio(StdioTransport::spawn(name, &cfg.command, &cfg.args, &cfg.env).await?)
        } else {
            Transport::Http(HttpTransport::new(&cfg.url, &headers)?)
        };
        let tools = match handshake(
            name,
            &transport,
            cfg.request_timeout(HANDSHAKE_TIMEOUT),
        )
        .await
        {
            Ok(tools) => tools,
            Err(e) => {
                transport.close();
                if let Some((message, hint)) = parse_auth_required(&e) {
                    if let Ok(mut guard) = self.servers.write()
                        && let Some(slot) = guard.get_mut(name)
                    {
                        slot.auth_required = true;
                        slot.auth_hint = hint;
                    }
                    return Err(format!("MCP 服务器 {name} 需要授权：{message}"));
                }
                return Err(e);
            }
        };
        let count = tools.len();
        {
            let Ok(mut guard) = self.servers.write() else {
                transport.close();
                return Err("MCP 状态不可用".to_string());
            };
            let Some(slot) = guard.get_mut(name) else {
                transport.close();
                return Err(format!("MCP 服务器不存在: {name}"));
            };
            if let Some(old) = slot.conn.replace(Conn {
                transport: Arc::new(transport),
                tools,
            }) {
                old.transport.close();
            }
            // 显式连接 = 启用（与桌面端一致）
            slot.cfg.enabled = true;
            slot.auth_required = false;
            slot.auth_hint = None;
        }
        if let Err(e) = self.persist() {
            tracing::warn!("[mcp] 连接 {name} 后回写 enabled 失败: {e}");
        }
        Ok(format!("已连接 {name}（{count} 个工具）"))
    }

    /// 保存远程服务器的手动授权头（Bearer Token / 自定义 Header）并重连。
    ///
    /// `headers_json` 为 JSON 对象：`{"Authorization":"Bearer ..."}`；空对象表示清除。
    pub async fn set_server_auth(&self, name: &str, headers_json: &str) -> Result<String, String> {
        let value: Value = serde_json::from_str(headers_json)
            .map_err(|e| format!("授权请求头不是合法 JSON: {e}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| "授权请求头必须是 JSON 对象".to_string())?;
        let mut headers = BTreeMap::new();
        for (key, raw) in object {
            let val = raw
                .as_str()
                .ok_or_else(|| format!("请求头 {key} 的值必须是字符串"))?;
            reqwest::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| format!("非法请求头名称: {key}"))?;
            reqwest::header::HeaderValue::from_str(val)
                .map_err(|_| format!("请求头 {key} 的值非法"))?;
            headers.insert(key.clone(), val.to_string());
        }
        let enabled = {
            let Ok(mut guard) = self.servers.write() else {
                return Err("MCP 状态不可用".to_string());
            };
            let slot = guard
                .get_mut(name)
                .ok_or_else(|| format!("MCP 服务器不存在: {name}"))?;
            if let Some(obj) = slot.raw.as_object_mut() {
                if headers.is_empty() {
                    obj.remove("headers");
                } else {
                    obj.insert("headers".to_string(), value.clone());
                }
            }
            slot.cfg.headers = headers;
            slot.auth_required = false;
            slot.auth_hint = None;
            if let Some(conn) = slot.conn.take() {
                conn.transport.close();
            }
            slot.cfg.enabled
        };
        self.persist()?;
        if enabled {
            self.connect(name).await
        } else {
            Ok(format!("已保存 {name} 的授权信息"))
        }
    }

    /// 开始远程服务器 OAuth：返回浏览器授权地址（本机回环接收回调）。
    pub async fn begin_oauth(&self, name: &str) -> Result<OAuthStart, String> {
        let (url, hint) = {
            let Ok(guard) = self.servers.read() else {
                return Err("MCP 状态不可用".to_string());
            };
            let slot = guard
                .get(name)
                .ok_or_else(|| format!("MCP 服务器不存在: {name}"))?;
            if slot.cfg.url.trim().is_empty() {
                return Err(format!("{name} 不是远程 MCP 服务器，不支持 OAuth 授权"));
            }
            (slot.cfg.url.clone(), slot.auth_hint.clone())
        };
        self.oauth.begin(name, &url, hint.as_deref()).await
    }

    /// 前端轮询 OAuth 状态：none / pending / authorized / failed。
    pub fn oauth_status(&self, name: &str) -> OAuthStatus {
        self.oauth.status(name)
    }

    /// 断开连接（配置保留）。
    pub fn disconnect(&self, name: &str) -> Result<(), String> {
        let Ok(mut guard) = self.servers.write() else {
            return Err("MCP 状态不可用".to_string());
        };
        let slot = guard
            .get_mut(name)
            .ok_or_else(|| format!("MCP 服务器不存在: {name}"))?;
        if let Some(conn) = slot.conn.take() {
            conn.transport.close();
        }
        Ok(())
    }

    /// 启动 / 配置变更后：连接全部启用项（单个失败仅告警，不阻断其余）。
    pub async fn connect_enabled(&self) {
        let names: Vec<String> = {
            let Ok(guard) = self.servers.read() else {
                return;
            };
            guard
                .iter()
                .filter(|(_, slot)| slot.cfg.enabled && slot.conn.is_none())
                .map(|(name, _)| name.clone())
                .collect()
        };
        for name in names {
            match self.connect(&name).await {
                Ok(msg) => tracing::info!("[mcp] {msg}"),
                Err(e) => tracing::warn!("[mcp] 连接 {name} 失败: {e}"),
            }
        }
    }

    /* ---------- 工具目录 / 调用 ---------- */

    /// 已连接服务器的工具目录（`source: mcp`；含全部服务器）。
    pub fn tool_infos(&self) -> Vec<ToolInfo> {
        self.exposed()
            .into_iter()
            .map(|t| ToolInfo {
                name: t.name,
                description: t.description,
                parameters_schema: t.schema,
                source: ToolSource::Mcp { server: t.server },
                dynamic: false,
            })
            .collect()
    }

    /// 指定服务器的工具目录（未连接 → 空）。
    pub fn list_tools(&self, server: &str) -> Vec<ToolInfo> {
        self.tool_infos()
            .into_iter()
            .filter(|t| matches!(&t.source, ToolSource::Mcp { server: s } if s == server))
            .collect()
    }

    /// 已连接服务器名（提示词「已连接 MCP 服务」段用）。
    pub fn connected_servers(&self) -> Vec<String> {
        let Ok(guard) = self.servers.read() else {
            return Vec::new();
        };
        guard
            .iter()
            .filter(|(_, slot)| slot.conn.is_some())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// 已连接服务器的工具句柄（会话循环注入用）。
    pub fn connected_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.exposed()
            .into_iter()
            .map(|t| {
                Arc::new(McpToolHandle {
                    server: t.server,
                    name: t.name,
                    raw: t.raw,
                    description: t.description,
                    schema: t.schema,
                }) as Arc<dyn Tool>
            })
            .collect()
    }

    /// 调用工具（`tools/call`），结果渲染为文本（`isError` → Err）。
    pub async fn call(&self, server: &str, tool: &str, args: Value) -> Result<String, String> {
        let (transport, timeout, manual_auth) = {
            let Ok(guard) = self.servers.read() else {
                return Err("MCP 状态不可用".to_string());
            };
            let slot = guard
                .get(server)
                .ok_or_else(|| format!("MCP 服务器不存在: {server}"))?;
            let conn = slot
                .conn
                .as_ref()
                .ok_or_else(|| format!("MCP 服务器未连接: {server}"))?;
            (
                Arc::clone(&conn.transport),
                slot.cfg.request_timeout(CALL_TIMEOUT),
                slot.cfg
                    .headers
                    .keys()
                    .any(|key| key.eq_ignore_ascii_case("authorization")),
            )
        };
        let arguments = if args.is_null() { json!({}) } else { args };
        let payload = json!({ "name": tool, "arguments": arguments });
        let mut result = transport.request("tools/call", payload.clone(), timeout).await;

        // access token 过期：刷新后原地换 header 并重试一次（手动 Authorization 不参与）
        if !manual_auth
            && let Err(e) = &result
            && parse_auth_required(e).is_some()
            && let Ok(token) = self.oauth.refresh(server).await
        {
            transport.set_bearer(&token);
            result = transport.request("tools/call", payload, timeout).await;
        }

        match result {
            Ok(value) => render_call_result(&value),
            Err(e) => {
                if parse_auth_required(&e).is_some() {
                    if let Ok(mut guard) = self.servers.write()
                        && let Some(slot) = guard.get_mut(server)
                    {
                        slot.auth_required = true;
                    }
                    return Err(format!(
                        "MCP 工具 {server}/{tool} 需要授权，请在设置 → 工具 → MCP 中重新授权"
                    ));
                }
                Err(format!("MCP 工具 {server}/{tool} 调用失败: {e}"))
            }
        }
    }

    /* ---------- 内部 ---------- */

    /// 重载配置：保留「仍存在、连接目标未变**且仍启用**」的活连接，其余（删除 / 改目标 /
    /// 直接改成 `enabled:false`）断开。
    fn reload_from_disk(&self) {
        let value = std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_else(|| json!({ "mcpServers": {} }));
        let parsed = parse_servers(&value);
        let Ok(mut guard) = self.servers.write() else {
            return;
        };
        let old = std::mem::take(&mut *guard);
        for (name, (raw, cfg)) in parsed {
            let previous = old.get(&name).filter(|slot| slot.cfg.same_target(&cfg));
            let conn = previous
                .filter(|_| cfg.enabled)
                .and_then(|slot| slot.conn.as_ref().map(|c| Conn {
                    transport: Arc::clone(&c.transport),
                    tools: c.tools.clone(),
                }));
            let auth_required = conn.is_none()
                && previous.map(|slot| slot.auth_required).unwrap_or(false);
            let auth_hint = if conn.is_none() {
                previous.and_then(|slot| slot.auth_hint.clone())
            } else {
                None
            };
            guard.insert(
                name,
                ServerSlot {
                    raw,
                    cfg,
                    conn,
                    auth_required,
                    auth_hint,
                },
            );
        }
    }

    /// 回写配置（把内存中的 `enabled` 同步进原文；未知字段保留）。
    fn persist(&self) -> Result<(), String> {
        let value = {
            let Ok(guard) = self.servers.read() else {
                return Err("MCP 状态不可用".to_string());
            };
            let mut map = serde_json::Map::new();
            for (name, slot) in guard.iter() {
                let mut raw = slot.raw.clone();
                if let Some(obj) = raw.as_object_mut() {
                    obj.insert("enabled".to_string(), json!(slot.cfg.enabled));
                }
                map.insert(name.clone(), raw);
            }
            json!({ "mcpServers": map })
        };
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
        astorm_common::fsx::write_atomic(&self.file, format!("{text}\n").as_bytes())
            .map_err(|e| format!("写入 MCP 配置失败: {e}"))
    }

    /// 当前对外暴露的工具集（确定性：内置名占用 → `{server}__{name}`；跨服务器重名后者跳过）。
    fn exposed(&self) -> Vec<ExposedTool> {
        let mut taken: Vec<String> = crate::app::App::try_global()
            .map(|app| app.tools.names())
            .unwrap_or_default();
        let Ok(guard) = self.servers.read() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (server, slot) in guard.iter() {
            let Some(conn) = &slot.conn else {
                continue;
            };
            for t in &conn.tools {
                let name = if taken.iter().any(|s| s == &t.name) {
                    format!("{server}__{}", t.name)
                } else {
                    t.name.clone()
                };
                if taken.iter().any(|s| s == &name) {
                    tracing::warn!("[mcp] 工具名冲突，跳过: {server}/{name}");
                    continue;
                }
                taken.push(name.clone());
                out.push(ExposedTool {
                    server: server.clone(),
                    raw: t.name.clone(),
                    name,
                    description: t.description.clone(),
                    schema: t.schema.clone(),
                });
            }
        }
        out
    }
}

/// 握手：`initialize` → `notifications/initialized` → `tools/list`。
async fn handshake(
    name: &str,
    transport: &Transport,
    timeout: Duration,
) -> Result<Vec<McpToolDef>, String> {
    transport
        .request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION },
            }),
            timeout,
        )
        .await
        .map_err(|e| format!("MCP 服务器 {name} 握手失败: {e}"))?;
    transport.notify("notifications/initialized", json!({})).await;
    let result = transport
        .request("tools/list", json!({}), timeout)
        .await
        .map_err(|e| format!("MCP 服务器 {name} 获取工具列表失败: {e}"))?;
    Ok(parse_tools(&result))
}

/// 解析 `tools/list` 结果。
fn parse_tools(result: &Value) -> Vec<McpToolDef> {
    result
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(|n| n.as_str())?.to_string();
                    Some(McpToolDef {
                        name,
                        description: t
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                        schema: t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 解析配置文件 → 服务器清单（缺 `command` 且缺 `url` 的条目跳过并告警）。
fn parse_servers(value: &Value) -> BTreeMap<String, (Value, ServerConfig)> {
    let mut out = BTreeMap::new();
    let Some(map) = value.get("mcpServers").and_then(|m| m.as_object()) else {
        return out;
    };
    for (name, entry) in map {
        let command = entry
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let args: Vec<String> = entry
            .get("args")
            .and_then(|a| a.as_array())
            .map(|xs| {
                xs.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let url = entry
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        let env: BTreeMap<String, String> = entry
            .get("env")
            .and_then(|e| e.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        // 远程服务器自定义请求头（手动授权 / API Key；OAuth token 在连接时另行注入）
        let headers: BTreeMap<String, String> = entry
            .get("headers")
            .and_then(|h| h.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        // 单次请求超时（毫秒，兼容部分 MCP 客户端的 timeout 字段）
        let timeout = entry
            .get("timeout")
            .and_then(|t| t.as_u64())
            .map(|ms| Duration::from_millis(ms.clamp(1_000, 300_000)));
        // 缺省启用（与桌面端「未写 enabled = 默认连接」的直觉一致）
        let enabled = entry
            .get("enabled")
            .and_then(|e| e.as_bool())
            .unwrap_or(true);
        if command.trim().is_empty() && url.trim().is_empty() {
            tracing::warn!("[mcp] 服务器 {name} 缺少 command/url，已跳过");
            continue;
        }
        out.insert(
            name.clone(),
            (
                entry.clone(),
                ServerConfig {
                    command,
                    args,
                    url,
                    env,
                    headers,
                    timeout,
                    enabled,
                },
            ),
        );
    }
    out
}

/// `tools/call` 结果 → 文本（文本拼接；`isError` → Err）。
fn render_call_result(result: &Value) -> Result<String, String> {
    let is_error = result
        .get("isError")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => parts.push(
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                ),
                Some("image") => parts.push("[图片内容]".to_string()),
                Some("audio") => parts.push("[音频内容]".to_string()),
                Some("resource") => parts.push(
                    item.pointer("/resource/text")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| "[资源内容]".to_string()),
                ),
                other => parts.push(format!("[{}]", other.unwrap_or("未知内容"))),
            }
        }
    } else if let Some(structured) = result.get("structuredContent") {
        parts.push(structured.to_string());
    }
    let mut text = parts.join("\n");
    if text.chars().count() > MAX_RESULT_CHARS {
        let kept: String = text.chars().take(MAX_RESULT_CHARS).collect();
        text = format!("{kept}\n…（输出超长，已截断）");
    }
    if is_error {
        return Err(if text.trim().is_empty() {
            "MCP 工具返回错误".to_string()
        } else {
            text
        });
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_servers_reads_stdio_and_http_entries() {
        let value = json!({
            "mcpServers": {
                "fs": { "command": "npx", "args": ["-y", "server-filesystem", "/tmp"], "env": {"K": "V"} },
                "remote": {
                    "type": "streamableHttp",
                    "url": "https://example.com/mcp",
                    "headers": {"Authorization": "Bearer test"},
                    "timeout": 45000,
                    "enabled": false
                },
                "broken": { "args": [] }
            }
        });
        let parsed = parse_servers(&value);
        assert_eq!(parsed.len(), 2, "缺 command/url 的条目应跳过");
        let (_, fs) = &parsed["fs"];
        assert_eq!(fs.command, "npx");
        assert_eq!(fs.args.len(), 3);
        assert_eq!(fs.env.get("K").map(String::as_str), Some("V"));
        assert!(fs.enabled, "未写 enabled 缺省为启用");
        let (_, remote) = &parsed["remote"];
        assert_eq!(remote.url, "https://example.com/mcp");
        assert_eq!(
            remote.headers.get("Authorization").map(String::as_str),
            Some("Bearer test")
        );
        assert_eq!(remote.timeout, Some(Duration::from_millis(45_000)));
        assert!(!remote.enabled);
    }

    #[test]
    fn parse_tools_tolerates_missing_schema() {
        let result = json!({"tools": [
            {"name": "echo", "description": "回显"},
            {"description": "无名字，跳过"}
        ]});
        let tools = parse_tools(&result);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].schema["type"], "object");
    }

    #[test]
    fn render_call_result_joins_text_and_maps_error() {
        let ok = json!({"content": [{"type": "text", "text": "第一行"}, {"type": "text", "text": "第二行"}]});
        assert_eq!(render_call_result(&ok).unwrap(), "第一行\n第二行");

        let err = json!({"isError": true, "content": [{"type": "text", "text": "权限不足"}]});
        assert_eq!(render_call_result(&err).unwrap_err(), "权限不足");

        let image = json!({"content": [{"type": "image", "data": "..."}]});
        assert_eq!(render_call_result(&image).unwrap(), "[图片内容]");

        let structured = json!({"structuredContent": {"count": 3}});
        assert!(render_call_result(&structured).unwrap().contains("\"count\":3"));
    }

    #[test]
    fn render_call_result_truncates_long_output() {
        let long = "字".repeat(MAX_RESULT_CHARS + 100);
        let out = render_call_result(&json!({"content": [{"type": "text", "text": long}]})).unwrap();
        assert!(out.ends_with("（输出超长，已截断）"));
        assert!(out.chars().count() < MAX_RESULT_CHARS + 32);
    }
}
