//! 服务端 HTTP 客户端：信封解包 + 阻塞式 SSE 帧读取。

use std::io::{BufRead, BufReader};
use std::time::Duration;

use reqwest::blocking::Response;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::Value;

/// astorm-server 客户端（持有地址与令牌，按需构建 HTTP 客户端）。
pub struct Client {
    base: String,
    token: Option<String>,
}

impl Client {
    /// 创建客户端（`base` 形如 `http://127.0.0.1:8787`；`token` 可选）。
    pub fn new(base: String, token: Option<String>) -> Self {
        Client { base: base.trim_end_matches('/').to_string(), token }
    }

    /// 构建 reqwest 客户端（`read_timeout` 用于 SSE「取完回放即止」的扫描模式）。
    fn http(&self, read_timeout: Option<Duration>) -> reqwest::blocking::Client {
        let mut headers = HeaderMap::new();
        if let Some(t) = self.token.as_deref().filter(|t| !t.is_empty()) {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
                headers.insert(AUTHORIZATION, v);
            }
        }
        let builder = reqwest::blocking::Client::builder().default_headers(headers);
        // 注：blocking ClientBuilder 只有总超时（覆盖 body 读取）；扫描模式用它实现
        // 「回放取完 + 静默到期即止」——回放通常亚秒完成，总超时 ≈ 空闲检测。
        let builder = if let Some(t) = read_timeout {
            builder.timeout(t)
        } else {
            builder
        };
        builder.build().expect("构建 HTTP 客户端失败")
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn envelope(&self, resp: Response) -> Result<Value, String> {
        let status = resp.status();
        let body: Value = resp.json().map_err(|e| format!("响应解析失败: {e}"))?;
        if !status.is_success() {
            let msg = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("请求失败");
            return Err(format!("[{status}] {msg}"));
        }
        let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body.get("message").and_then(|m| m.as_str()).unwrap_or("业务错误");
            return Err(format!("[code {code}] {msg}"));
        }
        Ok(body.get("data").cloned().unwrap_or(Value::Null))
    }

    /// GET 并解信封。
    pub fn get(&self, path: &str) -> Result<Value, String> {
        let resp = self.http(None).get(self.url(path)).send().map_err(|e| e.to_string())?;
        self.envelope(resp)
    }

    /// POST JSON 并解信封。
    pub fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let resp = self
            .http(None)
            .post(self.url(path))
            .json(body)
            .send()
            .map_err(|e| e.to_string())?;
        self.envelope(resp)
    }

    /// POST 空体并解信封。
    pub fn post_empty(&self, path: &str) -> Result<Value, String> {
        let resp = self.http(None).post(self.url(path)).send().map_err(|e| e.to_string())?;
        self.envelope(resp)
    }

    /// DELETE 并解信封（P0 命令未用；保留给 rm 等后续命令）。
    #[allow(dead_code)]
    pub fn delete(&self, path: &str) -> Result<Value, String> {
        let resp = self.http(None).delete(self.url(path)).send().map_err(|e| e.to_string())?;
        self.envelope(resp)
    }

    /// 读 SSE 流（长连模式）：逐帧回调 `(seq, event, data)`；回调返回 `false` 即主动断开。
    pub fn sse(&self, path: &str, on_frame: impl FnMut(u64, &str, &Value) -> bool) -> Result<(), String> {
        self.sse_inner(path, None, on_frame)
    }

    /// 读 SSE 流（扫描模式）：空闲 `idle` 时长无新帧即视为回放取完，正常返回。
    /// 用于「只取环形缓冲回放」的场景（`pending` 扫描待审批）。
    pub fn sse_until_idle(
        &self,
        path: &str,
        idle: Duration,
        on_frame: impl FnMut(u64, &str, &Value) -> bool,
    ) -> Result<(), String> {
        self.sse_inner(path, Some(idle), on_frame)
    }

    fn sse_inner(
        &self,
        path: &str,
        idle: Option<Duration>,
        mut on_frame: impl FnMut(u64, &str, &Value) -> bool,
    ) -> Result<(), String> {
        let resp = match self
            .http(idle)
            .get(self.url(path))
            .header("Accept", "text/event-stream")
            .send()
        {
            Ok(r) => r,
            // 扫描模式的总超时可能在等响应头时就到期——视为取完
            Err(e) if idle.is_some() => {
                let _ = e;
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(format!("[{status}] {text}"));
        }
        let mut reader = BufReader::new(resp);
        let mut seq = 0u64;
        let mut event = String::new();
        let mut data = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // 服务端关闭
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    if let Some(rest) = trimmed.strip_prefix("id:") {
                        seq = rest.trim().parse().unwrap_or(seq);
                    } else if let Some(rest) = trimmed.strip_prefix("event:") {
                        event = rest.trim().to_string();
                    } else if let Some(rest) = trimmed.strip_prefix("data:") {
                        data.push_str(rest.trim_start());
                    } else if trimmed.is_empty() {
                        if !data.is_empty() {
                            let parsed: Value =
                                serde_json::from_str(&data).unwrap_or(Value::Null);
                            if !on_frame(seq, &event, &parsed) {
                                return Ok(());
                            }
                        }
                        seq = 0;
                        event.clear();
                        data.clear();
                    }
                }
                // 扫描模式：总超时触发后 reqwest 会以 Decode/其他错误掐断挂起的 body 读
                // （客户端主动放弃连接 → IncompleteMessage 系），一律视为回放取完。
                Err(_) if idle.is_some() => break,
                Err(e) => return Err(format!("读取事件流失败: {e}")),
            }
        }
        Ok(())
    }
}
