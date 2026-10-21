//! `websearch` 工具：多引擎网络实时搜索（**全部 HTML 抓取，免 key**）。
//!
//! 引擎：`baidu` / `sogou` / `bing` / `duckduckgo` / `google`。首选由配置
//! `AppConfig.web_search.default_engine` 决定；命中限流 / 网络异常 / 解析空结果时，
//! 按内置顺序（**国内优先**：百度 → 搜狗 → 必应 → DuckDuckGo → Google）自动切下一个，
//! 直到有结果或全部失败（全失败时汇总各引擎原因返回，不抛错）。
//!
//! 选择器/字段来源参考 OpenSERP（github.com/karust/openserp）各引擎的 selectors/parse：
//! - 结果页可能以 200 返回「校验/限流」页 → 先做 `blocked` 标记检测，命中即视为失败以触发回退；
//! - 百度结果卡的 `mu` 属性是真实目标地址（可见链接是不可读的 `/link?url=` 跳转）；
//! - Bing 结果限定 `#b_results > li.b_algo`（主结果列），避免轮播/相关搜索里复用 b_algo 的卡片；
//! - Google 结果取最内层 `div.tF2Cxc`（避免知识卡嵌套重复计数）。
//!
//! 纯解析逻辑抽成 `parse_*` 便于用离线 HTML 固件做单测；抓取与回退在 `search_engine`。

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures::future::join_all;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use scraper::{ElementRef, Html, Selector};
use serde_json::json;

/// 单次结果上限（防御性夹紧）。
const MAX_RESULTS: usize = 10;
/// 浏览器 UA（多数引擎对空/脚本 UA 直接拒绝）。
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 内置尝试顺序（国内优先；默认引擎会被提到最前）。
const ENGINE_ORDER: [Engine; 5] = [
    Engine::Baidu,
    Engine::Sogou,
    Engine::Bing,
    Engine::DuckDuckGo,
    Engine::Google,
];

/// 搜索引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Baidu,
    Sogou,
    Bing,
    DuckDuckGo,
    Google,
}

impl Engine {
    fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "baidu" => Some(Engine::Baidu),
            "sogou" => Some(Engine::Sogou),
            "bing" => Some(Engine::Bing),
            "duckduckgo" | "ddg" => Some(Engine::DuckDuckGo),
            "google" => Some(Engine::Google),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Engine::Baidu => "百度",
            Engine::Sogou => "搜狗",
            Engine::Bing => "必应",
            Engine::DuckDuckGo => "DuckDuckGo",
            Engine::Google => "Google",
        }
    }
}

/// 单条搜索结果。
#[derive(Default)]
struct WebResult {
    title: String,
    url: String,
    snippet: String,
    /// 「读取」模式下抓取并清洗后的正文摘录（默认无）。
    content: Option<String>,
}

/// `websearch` 工具。
pub struct WebSearchTool;

// ---------- 纯工具函数 ----------

/// 查询串 percent-encode（保留 unreserved，其余按 UTF-8 字节 %XX；空格 → %20）。
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// percent-decode（`%XX` + `+`→空格）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 取 URL 查询参数（简单实现，够用）。
fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 选择器编译（静态常量，失败即编码错误）。
fn sel(s: &str) -> Result<Selector, String> {
    Selector::parse(s).map_err(|e| format!("解析选择器失败: {e}"))
}

/// 命中「校验/限流」页的标记检测（引擎返回 200 也可能是挑战页）。
fn blocked(body: &str, markers: &[&str]) -> Option<String> {
    let lower = body.to_lowercase();
    for m in markers {
        if lower.contains(&m.to_lowercase()) {
            return Some(format!("命中校验/限流（含「{m}」）"));
        }
    }
    None
}

/// 去重（按 url 保留首次出现）。
fn dedup_by_url(items: Vec<WebResult>) -> Vec<WebResult> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().filter(|r| seen.insert(r.url.clone())).collect()
}

/// 百度结果卡：优先取 `mu` 属性（真实目标地址），否则返回 None 由调用方用可见链接。
fn canonical_baidu_url(item: &ElementRef, mu_sel: &Selector) -> Option<String> {
    let direct = item.value().attr("mu").map(str::to_string);
    let nested = item.select(mu_sel).next().and_then(|e| e.value().attr("mu").map(str::to_string));
    for mu in direct.into_iter().chain(nested) {
        let mu = mu.trim();
        if mu.starts_with("http://") || mu.starts_with("https://") {
            return Some(mu.to_string());
        }
    }
    None
}

/// Bing 跳转链接 `/ck/a?...&u=a1<base64url>` → 真实 URL（解不出则原样返回）。
fn decode_bing_href(href: &str) -> String {
    if href.starts_with("http") {
        return href.to_string();
    }
    if let Some(u) = query_param(href, "u") {
        if let Some(b64) = u.strip_prefix("a1") {
            if let Ok(bytes) = URL_SAFE_NO_PAD.decode(b64) {
                if let Ok(s) = String::from_utf8(bytes) {
                    if s.starts_with("http") {
                        return s;
                    }
                }
            }
            if let Ok(bytes) = STANDARD.decode(b64) {
                if let Ok(s) = String::from_utf8(bytes) {
                    if s.starts_with("http") {
                        return s;
                    }
                }
            }
        }
    }
    href.to_string()
}

// ---------- 各引擎纯解析（离线可测） ----------

/// DuckDuckGo（lite/html 端点：`.result`；新版 data-testid 亦兼容）。
fn parse_ddg(html: &str, max: usize) -> Result<Vec<WebResult>, String> {
    let doc = Html::parse_document(html);
    let block = sel("article[data-testid=result], div[data-testid=result], div.result")?;
    let title = sel("h2, .result__title, .result__a")?;
    let link = sel("a[data-testid=result-title-a], a.result__a, h2 a, a[href]")?;
    let snip = sel("div[data-result=snippet], .result__snippet")?;
    let items: Vec<WebResult> = doc
        .select(&block)
        .filter_map(|r| {
            let t = r.select(&title).next()?.text().collect::<String>().trim().to_string();
            if t.is_empty() {
                return None;
            }
            let href = r.select(&link).next()?.value().attr("href")?.to_string();
            let real = query_param(&href, "uddg").map(|v| percent_decode(&v)).unwrap_or(href);
            let s = r
                .select(&snip)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            Some(WebResult { title: t, url: real, snippet: s, ..Default::default() })
        })
        .collect();
    Ok(dedup_by_url(items).into_iter().take(max).collect())
}

/// Bing（结果限定主结果列 `#b_results > li.b_algo`）。
fn parse_bing(html: &str, max: usize) -> Result<Vec<WebResult>, String> {
    let doc = Html::parse_document(html);
    let block = sel("#b_results > li.b_algo")?;
    let title = sel("h2 a, h2, a[aria-label]")?;
    let any_a = sel("a[href]")?;
    let snip = sel("div.b_caption p, div.b_caption div")?;
    let items: Vec<WebResult> = doc
        .select(&block)
        .filter_map(|r| {
            let a = r.select(&title).next()?;
            let t = a.text().collect::<String>().trim().to_string();
            if t.is_empty() {
                return None;
            }
            let href = a
                .value()
                .attr("href")
                .or_else(|| r.select(&any_a).next().and_then(|x| x.value().attr("href")))
                .unwrap_or("")
                .to_string();
            let s = r
                .select(&snip)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            Some(WebResult { title: t, url: decode_bing_href(&href), snippet: s, ..Default::default() })
        })
        .collect();
    Ok(dedup_by_url(items).into_iter().take(max).collect())
}

/// 百度（结果卡 `mu` 取真实地址；跳转链接交由 `resolve_all` 还原）。
fn parse_baidu(html: &str) -> Result<Vec<WebResult>, String> {
    let doc = Html::parse_document(html);
    let block = sel(
        "#content_left div.result.c-container, \
         #content_left div.result-op.c-container, \
         div.c-container.new-pmd",
    )?;
    let link = sel("h3 a, a[href]")?;
    let snip = sel("div.c-abstract, [class*=content-right_], [class*=summary-gap_], div.text_2NOr6")?;
    let mu = sel("[mu]")?;
    let items: Vec<WebResult> = doc
        .select(&block)
        .filter_map(|r| {
            let a = r.select(&link).next()?;
            let t = a.text().collect::<String>().trim().to_string();
            if t.is_empty() {
                return None;
            }
            let href = a.value().attr("href").unwrap_or("").to_string();
            // 相对站内链接（相关搜索等）非自然结果
            if href.starts_with('/') || href.starts_with("javascript:") {
                return None;
            }
            let real = canonical_baidu_url(&r, &mu).unwrap_or(href);
            let s = r
                .select(&snip)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            Some(WebResult { title: t, url: real, snippet: s, ..Default::default() })
        })
        .collect();
    Ok(dedup_by_url(items))
}

/// 搜狗（`.vrwrap`/`.rb` 结果块；`/link?url=` 交由 `resolve_all` 还原）。
fn parse_sogou(html: &str) -> Result<Vec<WebResult>, String> {
    let doc = Html::parse_document(html);
    let block = sel(".vrwrap, .rb")?;
    let link = sel("h3 a, a[href]")?;
    let snip = sel(".str-text-info, .space-txt, .str_info, .str_info_cont, .text-layout")?;
    let items: Vec<WebResult> = doc
        .select(&block)
        .filter_map(|r| {
            let a = r.select(&link).next()?;
            let t = a.text().collect::<String>().trim().to_string();
            if t.is_empty() {
                return None;
            }
            let href = a.value().attr("href").unwrap_or("");
            let full = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("https://www.sogou.com{href}")
            };
            let s = r
                .select(&snip)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            Some(WebResult { title: t, url: full, snippet: s, ..Default::default() })
        })
        .collect();
    Ok(dedup_by_url(items))
}

/// Google（最内层 `div.tF2Cxc`，退回广义属性选择器）。
fn parse_google(html: &str, max: usize) -> Result<Vec<WebResult>, String> {
    let doc = Html::parse_document(html);
    let inner = sel("div.tF2Cxc")?;
    let broad = sel("div[data-hveid][data-ved]")?;
    let title = sel("h3")?;
    let link = sel("a[href]")?;
    let snip = sel("div[data-sncf='1'] div, div.VwiC3b, .IsZvec, span.aCOpRe")?;
    let mut blocks: Vec<ElementRef> = doc.select(&inner).collect();
    if blocks.is_empty() {
        blocks = doc.select(&broad).collect();
    }
    let items: Vec<WebResult> = blocks
        .into_iter()
        .filter(|r| r.select(&title).next().is_some())
        .filter_map(|r| {
            let t = r.select(&title).next()?.text().collect::<String>().trim().to_string();
            if t.is_empty() {
                return None;
            }
            // 谷歌可能给加密跳转链接；取块内首个 http 或 /url?q= 链接
            let href = r
                .select(&link)
                .map(|a| a.value().attr("href").unwrap_or("").to_string())
                .find(|h| h.starts_with("http") || h.starts_with("/url?"))?;
            let href = if href.starts_with("/url?") {
                query_param(&href, "q").map(|v| percent_decode(&v)).unwrap_or_default()
            } else {
                href
            };
            if !href.starts_with("http") {
                return None;
            }
            let s = r
                .select(&snip)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            Some(WebResult { title: t, url: href, snippet: s, ..Default::default() })
        })
        .collect();
    Ok(dedup_by_url(items).into_iter().take(max).collect())
}

// ---------- 抓取 + 回退 ----------

/// HTTP 客户端（跟随重定向，用于打开搜索结果页）。
fn page_client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .timeout(timeout)
        .user_agent(UA)
        .redirect(Policy::limited(5))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))
}

/// 解析跳转用客户端（不跟随重定向，读 `Location` 得真实地址）。
fn resolve_client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .timeout(timeout)
        .user_agent(UA)
        .redirect(Policy::none())
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))
}

/// GET 抓取结果页（附中文 Accept-Language，降低被判为非人类访问的概率）。
async fn fetch(client: &Client, url: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    resp.text().await.map_err(|e| format!("读取失败: {e}"))
}

/// 解析跳转链接：GET 不回体，只读 `Location`（失败返回 None，调用方保留原链接）。
async fn resolve_redirect(client: &Client, url: &str) -> Option<String> {
    let resp = client.get(url).send().await.ok()?;
    let s = resp.status();
    if s != StatusCode::FOUND
        && s != StatusCode::MOVED_PERMANENTLY
        && s != StatusCode::TEMPORARY_REDIRECT
        && s != StatusCode::PERMANENT_REDIRECT
    {
        return None;
    }
    let loc = resp.headers().get(reqwest::header::LOCATION)?.to_str().ok()?;
    if loc.starts_with("http") {
        Some(loc.to_string())
    } else if let Some(rest) = loc.strip_prefix("//") {
        Some(format!("https://{rest}"))
    } else {
        let origin = url.split_once("//").map(|(_, r)| r.split('/').next().unwrap_or(""))?;
        Some(format!("https://{origin}{loc}"))
    }
}

/// 并发解析一批 `link?url=` 跳转（失败保留原链接）。
async fn resolve_all(client: &Client, items: Vec<WebResult>) -> Vec<WebResult> {
    let futs = items.into_iter().map(|mut it| async move {
        if it.url.contains("/link?") {
            if let Some(real) = resolve_redirect(client, &it.url).await {
                it.url = real;
            }
        }
        it
    });
    join_all(futs).await
}

/// 「读取」模式：抓取前 N 条结果的正文（走常驻渲染服务 text 模式，含主体内容启发式）并作为摘录附上。
async fn enrich(results: &mut [WebResult], read: usize, timeout: Duration) {
    let Some(app) = crate::app::App::try_global() else {
        return;
    };
    let n = read.min(results.len());
    for r in results.iter_mut().take(n) {
        if r.url.trim().is_empty() {
            continue;
        }
        if let Ok(text) = app.web_render.render("text", &r.url, timeout).await {
            let t = text.trim();
            if !t.is_empty() {
                r.content = Some(t.to_string());
            }
        }
    }
}

/// 取结果页 HTML：百度/谷歌优先走**常驻渲染服务**（复用内置 Chromium，纯抓取常被反爬），
/// 失败即退回纯 HTTP（可能仍拿到校验页，交由 `blocked` 判定）。
async fn get_body(
    client: &Client,
    engine: Engine,
    url: &str,
    timeout: Duration,
) -> Result<String, String> {
    if matches!(engine, Engine::Baidu | Engine::Google) {
        if let Some(app) = crate::app::App::try_global() {
            match app.web_render.render("html", url, timeout).await {
                Ok(html) => return Ok(html),
                Err(e) => {
                    tracing::debug!("[websearch] {} 渲染服务失败，退回纯抓取: {e}", engine.label());
                }
            }
        }
    }
    fetch(client, url).await
}

/// 抓取一个引擎并解析。返回空向量代表「打开成功但无结果」。
async fn search_engine(
    client: &Client,
    resolver: &Client,
    engine: Engine,
    query: &str,
    max: usize,
    timeout: Duration,
) -> Result<Vec<WebResult>, String> {
    let q = encode_component(query);
    match engine {
        Engine::DuckDuckGo => {
            let url = format!("https://html.duckduckgo.com/html/?q={q}");
            let body = get_body(client, engine, &url, timeout).await?;
            if let Some(reason) = blocked(
                &body,
                &["anomaly", "unusual traffic", "human verification", "bots use duckduckgo"],
            ) {
                return Err(reason);
            }
            parse_ddg(&body, max)
        }
        Engine::Bing => {
            let url =
                format!("https://www.bing.com/search?q={q}&setlang=zh-CN&mkt=zh-CN&count={max}");
            let body = get_body(client, engine, &url, timeout).await?;
            if let Some(reason) = blocked(
                &body,
                &["verify that you are not a robot", "enter the characters you see", "captcha"],
            ) {
                return Err(reason);
            }
            parse_bing(&body, max)
        }
        Engine::Baidu => {
            let url = format!("https://www.baidu.com/s?wd={q}&rn={max}&f=8&ie=utf-8");
            let body = get_body(client, engine, &url, timeout).await?;
            if let Some(reason) = blocked(
                &body,
                &["passmod_dialog-wrapper", "timeout-button", "百度安全验证", "网络不给力"],
            ) {
                return Err(reason);
            }
            let items = parse_baidu(&body)?;
            Ok(resolve_all(resolver, items).await.into_iter().take(max).collect())
        }
        Engine::Sogou => {
            let url = format!("https://www.sogou.com/web?query={q}");
            let body = get_body(client, engine, &url, timeout).await?;
            if let Some(reason) = blocked(&body, &["antispider", "请输入验证码", "seccode"]) {
                return Err(reason);
            }
            let items = parse_sogou(&body)?;
            Ok(resolve_all(resolver, items).await.into_iter().take(max).collect())
        }
        Engine::Google => {
            let url = format!(
                "https://www.google.com/search?q={q}&num={max}&hl=zh-CN&gl=cn&pws=0&sourceid=chrome&ie=UTF-8"
            );
            let body = get_body(client, engine, &url, timeout).await?;
            if let Some(reason) = blocked(
                &body,
                &[
                    "detected unusual traffic",
                    "unusual traffic from your computer network",
                    "not a robot",
                    "/sorry/",
                    "before you continue",
                ],
            ) {
                return Err(reason);
            }
            parse_google(&body, max)
        }
    }
}

/// 单条输出上限（字符）：标题 / 链接 / 摘要。
const FIELD_TITLE_MAX: usize = 120;
const FIELD_URL_MAX: usize = 300;
const FIELD_SNIPPET_MAX: usize = 200;
/// 「读取」模式正文摘录上限（字符）。
const FIELD_CONTENT_MAX: usize = 1200;
/// 「读取」模式最多抓取的结果条数。
const READ_MAX: usize = 3;
/// 整段结果总字符上限（避免把大段文本灌给模型）。
const OUTPUT_TOTAL_MAX: usize = 4000;

/// UTF-8 安全截断（按字符，超长补省略号）。
fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

/// 统一输出（标注实际命中的引擎）；对每条字段与整段总长都做截断，控制喂给模型的体量。
fn format_output(engine: Engine, results: &[WebResult]) -> String {
    let mut body = String::new();
    let mut shown = 0usize;
    for (i, r) in results.iter().enumerate() {
        let title = clip(&r.title, FIELD_TITLE_MAX);
        let url = clip(&r.url, FIELD_URL_MAX);
        let snippet = clip(&r.snippet, FIELD_SNIPPET_MAX);
        let mut block = format!("{}. **{}**\n   {}\n", i + 1, title, url);
        if !snippet.is_empty() {
            block.push_str(&format!("   {snippet}\n"));
        }
        // 读取模式：附上清洗后的正文摘录
        if let Some(content) = r.content.as_deref().filter(|c| !c.trim().is_empty()) {
            block.push_str(&format!("   正文：{}\n", clip(content, FIELD_CONTENT_MAX)));
        }
        // 超总长即停，并在表头注明「仅展示前 N 条」
        if body.len() + block.len() > OUTPUT_TOTAL_MAX && shown > 0 {
            break;
        }
        body.push_str(&block);
        shown += 1;
    }
    let mut header = format!("## 网络搜索结果（{}）", engine.label());
    if shown < results.len() {
        header.push_str(&format!("（共 {} 条，仅列前 {} 条）", results.len(), shown));
    }
    format!("{header}\n\n{body}")
}

#[async_trait]
impl crate::tool::Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "通过网络搜索获取实时信息（百度 / 搜狗 / 必应 / DuckDuckGo / Google，均免 key HTML 抓取），\
         返回标题 / 链接 / 摘要。首选引擎由设置决定，被限流或异常时自动按顺序回退到下一个引擎。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "max_results": { "type": "integer", "description": "最大结果数（默认 5，上限 10）" },
                "read": { "type": "integer", "description": "读取前 N 条结果的正文（0-3，默认 0；会逐条渲染取正文并清洗，较慢）" }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &crate::tool::ToolContext,
    ) -> Result<String, String> {
        self.run(params).await.map(|(text, _)| text)
    }

    /// 附带结构化结果元数据（`web_results`）：前端「相关资料」直接读它，不再解析文本。
    async fn execute_with_meta(
        &self,
        params: serde_json::Value,
        _ctx: &crate::tool::ToolContext,
    ) -> Result<(String, Option<serde_json::Value>), String> {
        let (text, results) = self.run(params).await?;
        let meta = (!results.is_empty()).then(|| {
            json!({
                "web_results": results
                    .iter()
                    .map(|r| json!({ "title": r.title, "url": r.url, "snippet": r.snippet }))
                    .collect::<Vec<_>>()
            })
        });
        Ok((text, meta))
    }
}

impl WebSearchTool {
    async fn run(&self, params: serde_json::Value) -> Result<(String, Vec<WebResult>), String> {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if query.is_empty() {
            return Err("query 不能为空".into());
        }
        let max = params
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, MAX_RESULTS as u64) as usize;
        // 读取模式：抓取前 N 条结果正文（默认 0）
        let read = params.get("read").and_then(|v| v.as_u64()).unwrap_or(0).min(READ_MAX as u64)
            as usize;

        let cfg = crate::app::App::try_global()
            .map(|app| app.config().web_search.clone())
            .unwrap_or_default();
        let timeout = Duration::from_secs(cfg.timeout_sec.max(1));
        let default = Engine::from_key(&cfg.default_engine).unwrap_or(Engine::Baidu);
        // 尝试顺序：默认引擎提到最前，其余按内置顺序（国内优先）
        let mut order = vec![default];
        order.extend(ENGINE_ORDER.into_iter().filter(|e| *e != default));

        let client = page_client(timeout)?;
        let resolver = resolve_client(timeout)?;
        // 渲染器仅在 bin + script 都配了时启用
        let mut failures: Vec<String> = Vec::new();
        for engine in order {
            match search_engine(&client, &resolver, engine, &query, max, timeout).await {
                Ok(mut results) if !results.is_empty() => {
                    if read > 0 {
                        enrich(&mut results, read, timeout).await;
                    }
                    let text = format_output(engine, &results);
                    return Ok((text, results));
                }
                Ok(_) => failures.push(format!("{} 无结果", engine.label())),
                Err(e) => failures.push(format!("{} {}", engine.label(), e)),
            }
        }
        Ok((
            format!(
                "未获取到搜索结果（已依次尝试：{}）。可稍后重试，或在设置中更换默认搜索引擎。",
                failures.join("；")
            ),
            Vec::new(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_component_escapes_reserved() {
        assert_eq!(encode_component("rust async"), "rust%20async");
        assert_eq!(encode_component("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_component("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn percent_decode_handles_percent_and_plus() {
        assert_eq!(percent_decode("https%3A%2F%2Fx%2Fa+b"), "https://x/a b");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn query_param_extracts_ddg_bing_google() {
        let ddg = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc";
        assert_eq!(query_param(ddg, "uddg").as_deref(), Some("https%3A%2F%2Fexample.com"));
        let g = "https://www.google.com/url?q=https%3A%2F%2Fy.example&sa=U";
        assert_eq!(query_param(g, "q").as_deref(), Some("https%3A%2F%2Fy.example"));
        assert_eq!(query_param("https://x/", "uddg"), None);
    }

    #[test]
    fn decode_bing_href_recovers_real_url() {
        assert_eq!(decode_bing_href("https://real.example/x"), "https://real.example/x");
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("https://real.example/y");
        let href = format!("/ck/a?u=a1{b64}&ntb=1");
        assert_eq!(decode_bing_href(&href), "https://real.example/y");
    }

    #[test]
    fn engine_from_key_and_builtin_order() {
        assert_eq!(Engine::from_key("Baidu"), Some(Engine::Baidu));
        assert_eq!(Engine::from_key("ddg"), Some(Engine::DuckDuckGo));
        assert_eq!(Engine::from_key("nope"), None);
        assert_eq!(ENGINE_ORDER[0], Engine::Baidu);
        assert_eq!(ENGINE_ORDER[4], Engine::Google);
    }

    #[test]
    fn blocked_detects_challenge_pages() {
        assert!(blocked("<button class=\"timeout-button\">返回首页</button>", &["timeout-button"]).is_some());
        assert!(blocked("百度安全验证", &["百度安全验证"]).is_some());
        assert!(blocked("<div id=\"b_results\">ok</div>", &["captcha"]).is_none());
    }

    // ---- 离线固件解析测试（不依赖实时网络）----

    #[test]
    fn parse_bing_fixture() {
        let html = include_str!("testdata/bing.html");
        let results = parse_bing(html, 10).expect("解析 bing 固件");
        assert!(!results.is_empty(), "bing 固件应解析出结果");
        assert!(results.iter().all(|r| !r.title.is_empty() && r.url.starts_with("http")));
    }

    #[test]
    fn parse_sogou_fixture() {
        let html = include_str!("testdata/sogou.html");
        let results = parse_sogou(html).expect("解析 sogou 固件");
        assert!(!results.is_empty(), "sogou 固件应解析出结果");
        assert!(results.iter().all(|r| !r.title.is_empty()));
    }

    #[test]
    fn format_output_caps_fields_and_total() {
        let long = "字".repeat(1000);
        let results: Vec<WebResult> = (0..10)
            .map(|i| WebResult {
                title: format!("标题{i}{long}"),
                url: format!("https://example.com/{long}"),
                snippet: long.clone(),
                content: None,
            })
            .collect();
        let out = format_output(Engine::Bing, &results);
        assert!(out.contains("网络搜索结果（必应）"));
        // 总长受限（含表头/格式的少量余量）
        assert!(out.len() <= OUTPUT_TOTAL_MAX + 200, "总长应受限: {}", out.len());
        // 被截断时注明仅列前 N 条
        assert!(out.contains("仅列前"), "应注明截断: {out:.80}");
    }

    #[test]
    fn parse_baidu_rendered_fixture() {
        // 隐藏 Chromium 窗口渲染后的百度结果页（真实抓取）→ 应解析出自然结果
        let html = include_str!("testdata/baidu_rendered.html");
        let results = parse_baidu(html).expect("解析 baidu 渲染固件");
        assert!(!results.is_empty(), "渲染后的百度页应解析出结果");
        assert!(results.iter().all(|r| !r.title.is_empty()));
    }

    #[test]
    fn parse_baidu_challenge_has_no_results() {
        // 百度对本机返回的「安全验证/超时」页 → 不应解析出任何结果（触发回退）
        let html = include_str!("testdata/baidu_challenge.html");
        assert!(blocked(html, &["button.timeout-button", "百度安全验证"]).is_some());
        let results = parse_baidu(html).expect("解析 baidu 挑战页");
        assert!(results.is_empty(), "挑战页不应有自然结果");
    }
}
