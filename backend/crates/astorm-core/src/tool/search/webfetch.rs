//! `webfetch` 工具：获取网页**正文**（支持 JS 渲染）。
//!
//! 复用 Electron 自带 Chromium（`AppConfig.webfetch.renderer_bin/renderer_script`）：
//! 隐藏窗口渲染 + stealth 反爬预加载（`page-render.cjs` / `stealth-preload.cjs`），
//! 返回 `document.body.innerText`。无需任何外部无头浏览器二进制。

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// 输出默认/最大截断字符数。
const DEFAULT_MAX_CHARS: usize = 8000;
const MAX_CHARS: usize = 20_000;
/// 渲染超时（秒）。
const RENDER_TIMEOUT_SECS: u64 = 30;

/// 工具参数。
#[derive(Debug, Deserialize)]
struct WebFetchArgs {
    /// 目标 URL。
    url: String,
    /// 输出格式：`text`（可读正文，默认）/ `markdown`（保留标题/链接/列表/代码/表格）。
    #[serde(default)]
    format: Option<String>,
    /// 收窄处理范围到某个 CSS 选择器（如 `main` / `article`，跳过导航/侧栏/页脚）。
    #[serde(default)]
    selector: Option<String>,
    /// 最大返回长度（默认 8000，上限 20000）。
    #[serde(default)]
    max_length: Option<usize>,
}

/// UTF-8 安全截断（按字符）。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect::<String>() + "\n...[truncated]"
    }
}

/// `webfetch` 工具。
pub struct WebFetchTool;

#[async_trait]
impl crate::tool::Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "获取网页正文（复用内置 Chromium 渲染，支持 JS 页面并带 stealth 反爬）。\
         返回页面可见文本，供需要正文时使用（搜索结果只有摘要）。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "目标 URL（http/https）" },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown"],
                    "description": "输出格式：text 可读正文（默认）/ markdown 保留标题/链接/列表/代码/表格"
                },
                "selector": { "type": "string", "description": "可选 CSS 选择器，把内容收窄到该区域（如 main / article）" },
                "max_length": { "type": "integer", "description": "最大返回长度（默认 8000，上限 20000）" }
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &crate::tool::ToolContext,
    ) -> Result<String, String> {
        let args: WebFetchArgs = serde_json::from_value(params).map_err(|e| format!("参数错误: {e}"))?;
        let url = args.url.trim().to_string();
        if url.is_empty() {
            return Err("url 不能为空".into());
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("url 必须是 http/https 地址".into());
        }
        let max = args.max_length.unwrap_or(DEFAULT_MAX_CHARS).min(MAX_CHARS);

        let mode = match args.format.as_deref().map(str::trim) {
            Some("markdown") => "markdown",
            _ => "text",
        };
        let selector = args.selector.as_deref().map(str::trim).filter(|s| !s.is_empty());

        let app = crate::app::App::try_global()
            .ok_or("应用未初始化：无法使用内置 Chromium 渲染（webRender）")?;
        let timeout = Duration::from_secs(RENDER_TIMEOUT_SECS);
        match app.web_render.render_ex(mode, &url, selector, timeout).await {
            Ok(text) if !text.trim().is_empty() => Ok(truncate_chars(&text, max)),
            Ok(_) => Ok("（页面无可读正文）".into()),
            Err(e) => Err(format!("内置 Chromium 渲染失败: {e}")),
        }
    }
}
