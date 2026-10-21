//! MCP 远程服务器 OAuth 2.1 授权（桌面 loopback 流程）。
//!
//! 流程：401 → 读 `WWW-Authenticate` 的 resource metadata → 发现授权服务器 →
//! 动态客户端注册 → PKCE(S256) → 系统浏览器打开授权页 → 本机回环监听接收 code →
//! 换 token → 落盘（0600）→ 前端轮询到 authorized 后重连。
//!
//! 只实现 MCP 规范要求的公共客户端（`token_endpoint_auth_method: none`）；服务端不支持
//! 动态注册时返回可读错误，由设置页回退到「手动填写访问令牌」。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 等待用户完成浏览器授权的上限。
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
/// 发现 / 注册 / 换 token 的单次 HTTP 超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// 回环回调路径。
const CALLBACK_PATH: &str = "/callback";
/// access token 提前刷新的安全窗口。
const REFRESH_SKEW_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuthToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// epoch 秒；缺失表示服务端未给 expires_in。
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub token_endpoint: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Debug, Clone)]
struct FlowStatus {
    /// pending / authorized / failed
    status: &'static str,
    error: Option<String>,
}

/// OAuth 运行时：流程状态 + token 存储。
#[derive(Clone)]
pub struct OAuthRuntime {
    flows: Arc<Mutex<BTreeMap<String, FlowStatus>>>,
    store_path: PathBuf,
}

/// 开始授权后返回给前端的跳转信息。
#[derive(Debug, Clone)]
pub struct OAuthStart {
    pub authorization_url: String,
    pub redirect_uri: String,
}

/// 前端轮询的授权状态。
#[derive(Debug, Clone)]
pub struct OAuthStatus {
    pub status: &'static str,
    pub error: Option<String>,
}

struct ResourceMetadata {
    resource: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scopes: Vec<String>,
}

impl OAuthRuntime {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            flows: Arc::new(Mutex::new(BTreeMap::new())),
            store_path: config_dir.join("mcp_oauth.json"),
        }
    }

    /// 开始一次浏览器授权：发现元数据 → 本机监听 → 动态注册 → 返回授权 URL。
    pub async fn begin(
        &self,
        server: &str,
        resource_url: &str,
        resource_metadata: Option<&str>,
    ) -> Result<OAuthStart, String> {
        let discovery = discover(resource_url, resource_metadata).await?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("启动本地授权回调失败: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("读取本地授权回调地址失败: {e}"))?;
        let redirect_uri = format!("http://{addr}{CALLBACK_PATH}");
        let client = http_client();

        let registration = register_client(&client, &discovery, &redirect_uri).await?;
        let verifier = random_token(2);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_token(1);
        let authorization_url = build_authorization_url(
            &discovery,
            &registration.client_id,
            &redirect_uri,
            &challenge,
            &state,
        )?;

        if let Ok(mut flows) = self.flows.lock() {
            flows.insert(
                server.to_string(),
                FlowStatus {
                    status: "pending",
                    error: None,
                },
            );
        }

        let runtime = self.clone();
        let server_name = server.to_string();
        let client_id = registration.client_id.clone();
        let client_secret = registration.client_secret.clone();
        let redirect = redirect_uri.clone();
        let expected_state = state.clone();
        let discovery_for_task = discovery;
        tokio::spawn(async move {
            let result = callback_and_exchange(
                listener,
                &expected_state,
                &discovery_for_task.token_endpoint,
                &client_id,
                client_secret.as_deref(),
                &redirect,
                &verifier,
                discovery_for_task.resource.as_str(),
                discovery_for_task.scopes.as_slice(),
            )
            .await;
            match result {
                Ok(token) => {
                    if let Err(e) = runtime.save_token(&server_name, token) {
                        runtime.set_failed(&server_name, format!("保存授权令牌失败: {e}"));
                        return;
                    }
                    if let Ok(mut flows) = runtime.flows.lock()
                        && let Some(flow) = flows.get_mut(&server_name)
                    {
                        flow.status = "authorized";
                        flow.error = None;
                    }
                }
                Err(e) => runtime.set_failed(&server_name, e),
            }
        });

        Ok(OAuthStart {
            authorization_url,
            redirect_uri,
        })
    }

    pub fn status(&self, server: &str) -> OAuthStatus {
        let flow = self
            .flows
            .lock()
            .ok()
            .and_then(|flows| flows.get(server).cloned());
        match flow {
            Some(flow) => OAuthStatus {
                status: flow.status,
                error: flow.error,
            },
            None => OAuthStatus {
                status: "none",
                error: None,
            },
        }
    }

    /// 取可用 access token：与当前资源 URL 匹配、必要时先刷新。
    pub async fn access_token(&self, server: &str, resource_url: &str) -> Option<String> {
        let token = self.load_store().remove(server)?;
        if token
            .resource
            .as_deref()
            .is_some_and(|resource| resource != resource_url)
        {
            return None;
        }
        if token
            .expires_at
            .map(|at| at <= now_secs() + REFRESH_SKEW_SECS)
            .unwrap_or(false)
            && token.refresh_token.is_some()
        {
            return self.refresh(server).await.ok();
        }
        Some(token.access_token)
    }

    /// 用 refresh_token 换新 access token；成功后落盘。
    pub async fn refresh(&self, server: &str) -> Result<String, String> {
        let mut store = self.load_store();
        let token = store
            .get(server)
            .cloned()
            .ok_or_else(|| "没有可刷新的 MCP 授权".to_string())?;
        let refresh_token = token
            .refresh_token
            .clone()
            .ok_or_else(|| "该授权没有 refresh_token，请重新授权".to_string())?;
        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token),
            ("client_id", token.client_id.clone()),
        ];
        if let Some(secret) = &token.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        if let Some(resource) = &token.resource {
            form.push(("resource", resource.clone()));
        }
        let value = post_form(&http_client(), &token.token_endpoint, &form).await?;
        let access_token = value
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| "刷新授权失败：响应缺少 access_token".to_string())?
            .to_string();
        let mut refreshed = token;
        refreshed.access_token = access_token.clone();
        if let Some(rt) = value.get("refresh_token").and_then(|t| t.as_str()) {
            refreshed.refresh_token = Some(rt.to_string());
        }
        refreshed.expires_at = expires_at_from(&value);
        store.insert(server.to_string(), refreshed);
        self.write_store(&store)?;
        Ok(access_token)
    }

    fn set_failed(&self, server: &str, error: String) {
        if let Ok(mut flows) = self.flows.lock()
            && let Some(flow) = flows.get_mut(server)
        {
            flow.status = "failed";
            flow.error = Some(error);
        }
    }

    fn save_token(&self, server: &str, token: OAuthToken) -> Result<(), String> {
        let mut store = self.load_store();
        store.insert(server.to_string(), token);
        self.write_store(&store)
    }

    fn load_store(&self) -> BTreeMap<String, OAuthToken> {
        std::fs::read_to_string(&self.store_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_store(&self, store: &BTreeMap<String, OAuthToken>) -> Result<(), String> {
        let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        // 临时文件创建时即 0600，rename 覆盖后不会保留旧文件的宽松权限。
        astorm_common::fsx::write_atomic_mode(
            &self.store_path,
            format!("{text}\n").as_bytes(),
            Some(0o600),
        )
        .map_err(|e| format!("写入 {} 失败: {e}", self.store_path.display()))
    }
}

struct ClientRegistration {
    client_id: String,
    client_secret: Option<String>,
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_default()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn random_token(uuid_count: usize) -> String {
    (0..uuid_count)
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect()
}

fn expires_at_from(value: &Value) -> Option<i64> {
    value
        .get("expires_in")
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
        .map(|secs| now_secs() + secs - REFRESH_SKEW_SECS)
}

/// 远程 MCP 的 OAuth 发现：优先使用 401 下发的 resource metadata，否则尝试
/// `/.well-known/oauth-protected-resource`（含 path-aware 变体）。
async fn discover(resource_url: &str, hint: Option<&str>) -> Result<ResourceMetadata, String> {
    let client = http_client();
    let mut resource_meta = None;
    if let Some(hint) = hint {
        resource_meta = get_json(&client, hint).await.ok();
    }
    if resource_meta.is_none() {
        for url in protected_resource_urls(resource_url) {
            if let Ok(value) = get_json(&client, &url).await {
                resource_meta = Some(value);
                break;
            }
        }
    }
    let meta = resource_meta.ok_or_else(|| {
        "无法发现该服务的 OAuth 元数据（未提供 resource metadata）".to_string()
    })?;

    let mut authorization_endpoint = meta
        .get("authorization_endpoint")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut token_endpoint = meta
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut registration_endpoint = meta
        .get("registration_endpoint")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut scopes: Vec<String> = meta
        .get("scopes_supported")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if authorization_endpoint.is_none() || token_endpoint.is_none() {
        let issuer = meta
            .get("authorization_servers")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .ok_or_else(|| "OAuth 元数据缺少 authorization_servers".to_string())?;
        let as_meta = authorization_server_metadata(&client, issuer).await?;
        authorization_endpoint = as_meta
            .get("authorization_endpoint")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        token_endpoint = as_meta
            .get("token_endpoint")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if registration_endpoint.is_none() {
            registration_endpoint = as_meta
                .get("registration_endpoint")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
        if scopes.is_empty() {
            scopes = as_meta
                .get("scopes_supported")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    Ok(ResourceMetadata {
        resource: meta
            .get("resource")
            .and_then(|v| v.as_str())
            .unwrap_or(resource_url)
            .to_string(),
        authorization_endpoint: authorization_endpoint
            .ok_or_else(|| "OAuth 元数据缺少 authorization_endpoint".to_string())?,
        token_endpoint: token_endpoint
            .ok_or_else(|| "OAuth 元数据缺少 token_endpoint".to_string())?,
        registration_endpoint,
        scopes,
    })
}

fn protected_resource_urls(resource_url: &str) -> Vec<String> {
    let Ok(url) = Url::parse(resource_url) else {
        return Vec::new();
    };
    let origin = match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
        (Some(host), None) => format!("{}://{host}", url.scheme()),
        _ => return Vec::new(),
    };
    let path = url.path().trim_end_matches('/');
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.push(format!("{origin}/.well-known/oauth-protected-resource{path}"));
    }
    urls.push(format!("{origin}/.well-known/oauth-protected-resource"));
    urls
}

async fn authorization_server_metadata(
    client: &reqwest::Client,
    issuer: &str,
) -> Result<Value, String> {
    let issuer = issuer.trim_end_matches('/');
    let urls = [
        format!("{issuer}/.well-known/oauth-authorization-server"),
        format!("{issuer}/.well-known/openid-configuration"),
    ];
    let mut last_error = String::new();
    for url in urls {
        match get_json(client, &url).await {
            Ok(value) => return Ok(value),
            Err(e) => last_error = e,
        }
    }
    Err(format!("获取 OAuth 授权服务器元数据失败: {last_error}"))
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("请求 {url} 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("请求 {url} 返回 {}", resp.status()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("解析 {url} 响应失败: {e}"))
}

async fn register_client(
    client: &reqwest::Client,
    discovery: &ResourceMetadata,
    redirect_uri: &str,
) -> Result<ClientRegistration, String> {
    let endpoint = discovery.registration_endpoint.as_deref().ok_or_else(|| {
        "该服务未提供动态客户端注册，请使用下方「手动填写授权信息」".to_string()
    })?;
    let mut body = json!({
        "client_name": "Astorm",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if !discovery.scopes.is_empty()
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("scope".to_string(), json!(discovery.scopes.join(" ")));
    }
    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("注册 OAuth 客户端失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("注册 OAuth 客户端失败（{status}）: {}", text.trim()));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("解析客户端注册响应失败: {e}"))?;
    let client_id = value
        .get("client_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "客户端注册响应缺少 client_id".to_string())?
        .to_string();
    Ok(ClientRegistration {
        client_id,
        client_secret: value
            .get("client_secret")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn build_authorization_url(
    discovery: &ResourceMetadata,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> Result<String, String> {
    let mut url = Url::parse(&discovery.authorization_endpoint)
        .map_err(|e| format!("授权地址无效: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("授权地址必须是 http(s)".to_string());
    }
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("resource", &discovery.resource);
        if !discovery.scopes.is_empty() {
            query.append_pair("scope", &discovery.scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn callback_and_exchange(
    listener: TcpListener,
    expected_state: &str,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
    verifier: &str,
    resource: &str,
    scopes: &[String],
) -> Result<OAuthToken, String> {
    let code = wait_for_callback(listener, expected_state).await?;
    let client = http_client();
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", redirect_uri.to_string()),
        ("client_id", client_id.to_string()),
        ("code_verifier", verifier.to_string()),
        ("resource", resource.to_string()),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    let value = post_form(&client, token_endpoint, &form).await?;
    let access_token = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "换取授权令牌失败：响应缺少 access_token".to_string())?
        .to_string();
    Ok(OAuthToken {
        access_token,
        refresh_token: value
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        expires_at: expires_at_from(&value),
        token_endpoint: token_endpoint.to_string(),
        client_id: client_id.to_string(),
        client_secret: client_secret.map(str::to_string),
        scope: value
            .get("scope")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                if scopes.is_empty() {
                    None
                } else {
                    Some(scopes.join(" "))
                }
            }),
        resource: Some(resource.to_string()),
    })
}

async fn post_form(
    client: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, String)],
) -> Result<Value, String> {
    let resp = client
        .post(endpoint)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|e| format!("请求授权令牌失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("授权令牌接口返回 {status}: {}", text.trim()));
    }
    serde_json::from_str(&text).map_err(|e| format!("解析授权令牌响应失败: {e}"))
}

/// 等待浏览器回调，校验 state 并返回 code；同时给浏览器一个完成页。
async fn wait_for_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    let accepted = tokio::time::timeout(AUTH_TIMEOUT, listener.accept())
        .await
        .map_err(|_| "等待浏览器授权超时".to_string())?
        .map_err(|e| format!("接收授权回调失败: {e}"))?;
    let (mut socket, _) = accepted;
    let mut buffer = vec![0u8; 8192];
    let read = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buffer))
        .await
        .map_err(|_| "读取授权回调超时".to_string())?
        .map_err(|e| format!("读取授权回调失败: {e}"))?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let parsed = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|e| format!("授权回调地址无效: {e}"))?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => state = Some(value.to_string()),
            "error" => error = Some(value.to_string()),
            "error_description" => {
                if let Some(existing) = error.take() {
                    error = Some(format!("{existing}: {value}"));
                } else {
                    error = Some(value.to_string());
                }
            }
            _ => {}
        }
    }

    let (status_line, message) = if let Some(error) = &error {
        ("400 Bad Request", format!("授权失败：{error}"))
    } else if state.as_deref() != Some(expected_state) {
        (
            "400 Bad Request",
            "授权回调校验失败（state 不匹配），请回到 Astorm 重试。".to_string(),
        )
    } else if code.is_none() {
        ("400 Bad Request", "授权回调缺少 code。".to_string())
    } else {
        (
            "200 OK",
            "授权成功，请返回 Astorm。可以关闭这个页面。".to_string(),
        )
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Astorm</title></head>\
         <body style=\"font-family:-apple-system,system-ui,sans-serif;padding:40px\">\
         <h2>{}</h2></body></html>",
        escape_html(&message)
    );
    let response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if let Some(error) = error {
        return Err(format!("授权失败: {error}"));
    }
    if state.as_deref() != Some(expected_state) {
        return Err("授权回调 state 不匹配".to_string());
    }
    code.ok_or_else(|| "授权回调缺少 code".to_string())
}

/// 回调结果页只展示服务端错误文本，必须转义后再拼 HTML。
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_html_escapes_error_text() {
        assert_eq!(
            escape_html("<script>alert('x')</script>"),
            "&lt;script&gt;alert('x')&lt;/script&gt;"
        );
    }
    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn protected_resource_urls_include_path_and_origin() {
        let urls = protected_resource_urls("https://api.example.com/mcp");
        assert_eq!(
            urls,
            vec![
                "https://api.example.com/.well-known/oauth-protected-resource/mcp".to_string(),
                "https://api.example.com/.well-known/oauth-protected-resource".to_string()
            ]
        );
        let urls = protected_resource_urls("https://api.example.com");
        assert_eq!(
            urls,
            vec!["https://api.example.com/.well-known/oauth-protected-resource".to_string()]
        );
    }

    #[test]
    fn authorization_url_carries_pkce_and_resource() {
        let discovery = ResourceMetadata {
            resource: "https://api.example.com/mcp".to_string(),
            authorization_endpoint: "https://api.example.com/authorize".to_string(),
            token_endpoint: "https://api.example.com/token".to_string(),
            registration_endpoint: Some("https://api.example.com/register".to_string()),
            scopes: vec!["read".to_string()],
        };
        let url = build_authorization_url(
            &discovery,
            "client-1",
            "http://127.0.0.1:1234/callback",
            "challenge",
            "state",
        )
        .unwrap();
        assert!(url.starts_with("https://api.example.com/authorize?"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("resource=https%3A%2F%2Fapi.example.com%2Fmcp"));
        assert!(url.contains("scope=read"));
    }
}
