//! 服务端浏览器（容器形态）：**一会话一 headless Chromium**，直连 CDP 执行 `browser`
//! 工具的动作（路由 b：不依赖 agent-browser 二进制）。
//!
//! 设计要点（对齐 `docs/browser-agent.md` §22 与桌面语义）：
//! - **动作词汇在 core**：core 把工具入参编译成 argv（与 CLI 子命令同形），本模块只负责
//!   "把 argv 变成页面上的动作"；分级授权 / 审批 / 审计 / 结果整形全在 core，此处不重复。
//! - **会话隔离**：按**根会话 id** 一份浏览器（子会话与主会话共享，同桌面「一会话一实例」）；
//!   空闲 `idle_secs`（默认 20 分钟，同桌面）回收，进程退出即杀（`kill_on_drop` 语义由
//!   `Browser` 自带 child 管理 + 我们显式 close 保证）。
//! - **元素引用**：snapshot 给可见可交互元素打 `data-astorm-ref="eN"`，`@eN` 即解析为
//!   该属性选择器；页面变化后引用失效（与桌面提示一致，重新 snapshot 即可）。
//! - **画面流**（面板侧）：同一 Page 供 [`hub`] 的 screencast 订阅（见 `screencast` 子模块），
//!   与 AI 操控共享同一浏览器实例——AI 点哪里，用户看得见。

pub mod screencast;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType,
};
use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, CaptureScreenshotFormat, EventScreencastFrame,
    ScreencastFrameAckParams, StartScreencastFormat, StartScreencastParams, StopScreencastParams,
};
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use serde_json::{json, Value};

use astorm_core::tool::browser::agent_cli::CliOutput;
use astorm_core::tool::browser::native::NativeBrowser;

/// 会话浏览器空闲回收（秒）；与桌面「一会话一实例 20 分钟回收」一致。
const DEFAULT_IDLE_SECS: u64 = 20 * 60;
/// 画面流参数（面板按比例缩放显示；JPEG 质量 60 兼顾清晰与带宽）。
const FRAME_QUALITY: i64 = 60;
const FRAME_MAX_W: i64 = 1280;
const FRAME_MAX_H: i64 = 900;
/// 单页导航等待上限。
const NAV_TIMEOUT: Duration = Duration::from_secs(30);
/// 默认视口（与 astorm-render 一致，面板按此比例显示）。
const VIEWPORT: (u32, u32) = (1280, 900);

/// 精简 stealth：去掉 `navigator.webdriver`（容器 IP 信誉另说，先补最常见的指纹）。
const STEALTH_JS: &str = r#"Object.defineProperty(navigator,'webdriver',{get:()=>undefined});
if(!window.chrome){window.chrome={runtime:{}}}
Object.defineProperty(navigator,'languages',{get:()=>['zh-CN','zh','en']});
"#;
/// 页面内「开新窗口」一律本页导航：headless 里 `window.open` / `target=_blank` 会新开一个
/// **页签**，而面板（与 `browser` 工具的动作表）都只看活动页——旧行为是点了像没反应，
/// 新页签在后台悄悄堆着。这里把 http(s)/file 目标改成当前页导航（内容立刻可见，AI 也看得到）；
/// 其余（空 url / about:blank 这类需要 window 句柄的写法）保持原样，避免弄坏弹窗探测。
/// 与桌面语义的差别：桌面把弹窗转成**内置新页签**（面板有多页签 UI），容器面板只有一块画布，
/// 故改为就地导航——单页语义下这是用户预期最近的行为。
const POPUP_JS: &str = r#"(function(){
try{
  var origOpen = window.open;
  window.open = function(url){
    var u = (url === undefined || url === null) ? '' : String(url);
    if(/^(https?|file):/i.test(u)){ window.location.assign(u); return null; }
    return origOpen.apply(window, arguments);
  };
  document.addEventListener('click', function(e){
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if(!a) return;
    var t = (a.getAttribute('target') || '').toLowerCase();
    if(t !== '_blank' && t !== '_new') return;
    var href = a.href || '';
    if(!/^(https?|file):/i.test(href)) return;
    e.preventDefault();
    window.location.assign(href);
  }, true);
}catch(err){}
})();
"#;

/// 桌面 UA 里的 Chrome 版本随容器 Chromium 变化，这里用通用现代 UA。
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 服务端浏览器中枢（挂到 core 的 `AppState.native_browser`）。
pub struct BrowserHub {
    /// 根会话 id → 会话浏览器（每会话一把 tokio 锁：同会话动作串行，跨会话并行）
    sessions: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Session>>>>,
    /// 启动串行化（并发首次动作只起一个浏览器）
    launching: tokio::sync::Mutex<()>,
    chrome: String,
    no_sandbox: bool,
    idle_secs: u64,
    /// 当前活动页 URL 的旁路缓存（面板/screencast 侧只读用；`origin` 不阻塞会话锁）
    active_urls: Mutex<HashMap<String, String>>,
    /// 会话画面流广播端（单 CDP 监听 → 多面板订阅；同页面重复 start 会报错）
    streams: Mutex<HashMap<String, tokio::sync::broadcast::Sender<String>>>,
}

struct Session {
    browser: Browser,
    pages: Vec<Page>,
    active: usize,
    last_used: Arc<AtomicU64>,
    /// 事件循环驱动（drop 时 abort；浏览器子进程随之回收）
    driver: tokio::task::JoinHandle<()>,
    profile: PathBuf,
    /// Chromium 的**浏览器级** CDP WS 地址（`ws://127.0.0.1:<port>/devtools/browser/<id>`）：
    /// agent-browser CLI 靠它附着（`--cdp`），与面板画面流共用同一浏览器。
    cdp_ws: String,
}

impl Session {
    fn page(&self) -> Result<Page, String> {
        self.pages
            .get(self.active)
            .cloned()
            .ok_or_else(|| "浏览器会话没有可用页签".to_string())
    }
}

impl BrowserHub {
    /// 从环境装配（`ASTORM_CHROME_BIN` 优先；容器镜像已内置 chromium）。
    pub fn new() -> Self {
        let no_sandbox = std::env::var("ASTORM_BROWSER_NO_SANDBOX").as_deref() != Ok("0");
        let idle_secs = std::env::var("ASTORM_BROWSER_IDLE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_IDLE_SECS);
        Self {
            sessions: Mutex::new(HashMap::new()),
            launching: tokio::sync::Mutex::new(()),
            chrome: resolve_chrome(),
            no_sandbox,
            idle_secs,
            active_urls: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// 空闲回收：由宿主在启动时 spawn（每 60s 扫一次）。
    pub async fn reap_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let now = unix_secs();
            let stale: Vec<String> = {
                let Ok(map) = self.sessions.lock() else { continue };
                map.iter()
                    .filter(|(_, s)| {
                        s.try_lock()
                            .map(|g| now.saturating_sub(g.last_used.load(Ordering::Relaxed)) > self.idle_secs)
                            .unwrap_or(false)
                    })
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            for sid in stale {
                tracing::info!("[browser] 会话 {sid} 空闲超时，回收浏览器");
                self.close(&sid).await;
            }
            // 端点回写节拍：AI 经 CLI 导航时服务端看不到，这里每分钟把 frameUrl 拉回真实页面
            if let Some(owner) = self.endpoint_owner()
                && self.has_session(&owner)
            {
                self.refresh_endpoint(&owner).await;
            }
        }
    }

    /// 取（或懒启动）某会话的浏览器。
    async fn session_for(&self, sid: &str) -> Result<Arc<tokio::sync::Mutex<Session>>, String> {
        if let Ok(map) = self.sessions.lock()
            && let Some(s) = map.get(sid)
        {
            return Ok(Arc::clone(s));
        }
        let _guard = self.launching.lock().await;
        if let Ok(map) = self.sessions.lock()
            && let Some(s) = map.get(sid)
        {
            return Ok(Arc::clone(s));
        }
        let session = self.launch(sid).await?;
        let arc = Arc::new(tokio::sync::Mutex::new(session));
        if let Ok(mut map) = self.sessions.lock() {
            map.insert(sid.to_string(), Arc::clone(&arc));
        }
        Ok(arc)
    }

    /// 启动 headless Chromium（独立 profile；事件循环必须驱动，否则 CDP 全部阻塞）。
    async fn launch(&self, sid: &str) -> Result<Session, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let profile = std::env::temp_dir().join(format!("astorm-browser-{}-{nanos}", std::process::id()));
        let mut args = vec![
            "--disable-gpu".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--lang=zh-CN".to_string(),
            "--disable-blink-features=AutomationControlled".to_string(),
            format!("--window-size={},{}", VIEWPORT.0, VIEWPORT.1),
        ];
        args.push("--hide-scrollbars".to_string());
        let mut config = BrowserConfig::builder()
            .chrome_executable(&self.chrome)
            .user_data_dir(profile.to_string_lossy().into_owned())
            .args(args);
        if self.no_sandbox {
            config = config.no_sandbox();
        }
        let config = config
            .build()
            .map_err(|e| format!("Chromium 配置无效（{}）: {e}", self.chrome))?;
        let (browser, mut handler) = tokio::time::timeout(Duration::from_secs(30), Browser::launch(config))
            .await
            .map_err(|_| "启动 Chromium 超时（30s）".to_string())?
            .map_err(|e| format!("启动 Chromium 失败（{}）: {e}", self.chrome))?;
        let driver = tokio::spawn(async move {
            while let Some(_evt) = handler.next().await {}
        });
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| format!("创建浏览器页签失败: {e}"))?;
        prepare_page(&page).await;
        let cdp_ws = browser_cdp_ws(&profile).unwrap_or_default();
        tracing::info!(
            "[browser] 会话 {sid} 已启动 headless Chromium（profile={}，cdp={}）",
            profile.display(),
            if cdp_ws.is_empty() { "未探测到" } else { &cdp_ws }
        );
        let session = Session {
            browser,
            pages: vec![page],
            active: 0,
            last_used: Arc::new(AtomicU64::new(unix_secs())),
            driver,
            profile,
            cdp_ws,
        };
        // 端点落盘：让 core 的 browser 工具走 agent-browser CLI（与桌面同一条路），
        // CLI 附着到这台 Chromium；失败只告警——工具侧会自动回落到 NativeBrowser。
        publish_endpoint(sid, &session, "");
        Ok(session)
    }

    /* ---------------- 动作执行 ---------------- */

    /// 执行一条 argv（core 编译好的动作词汇）。
    async fn act(
        &self,
        sid: &str,
        argv: &[String],
        screenshot_path: Option<String>,
    ) -> Result<CliOutput, String> {
        let session = self.session_for(sid).await?;
        let mut s = session.lock().await;
        s.last_used.store(unix_secs(), Ordering::Relaxed);
        let action = argv.first().map(String::as_str).unwrap_or("").to_string();
        let rest: Vec<String> = argv.iter().skip(1).cloned().collect();
        let out = match action.as_str() {
            "get" => self.get(&s, &rest).await,
            "snapshot" => self.snapshot(&s, &rest).await,
            "screenshot" => self.screenshot(&s, &rest, screenshot_path).await,
            "wait" => self.wait(&s, &rest).await,
            "scroll" => self.scroll(&s, &rest).await,
            "scrollintoview" => self.scroll_into_view(&s, &rest).await,
            "click" => self.click(&s, &rest).await,
            "type" | "fill" => self.type_text(&s, &rest, action == "fill").await,
            "press" => self.press(&s, &rest).await,
            "select" => self.select(&s, &rest).await,
            "check" | "uncheck" => self.check(&s, &rest, action == "check").await,
            "open" => self.open(&mut s, &rest).await,
            "back" | "forward" | "reload" => self.history(&s, &action).await,
            "tab" => self.tab(&mut s, &rest).await,
            other => Err(format!("服务端浏览器不支持的动作: {other}")),
        };
        // 记录活动 URL（面板/审批文案旁路读）+ 刷新端点（下次工具调用即可走 CLI）
        if let Ok(page) = s.page()
            && let Ok(Some(url)) = page.url().await
        {
            if let Ok(mut urls) = self.active_urls.lock() {
                urls.insert(sid.to_string(), url.clone());
            }
            publish_endpoint(sid, &s, &url);
        }
        out
    }

    /// `get url|title|text|html|value|count|box TARGET|attr TARGET NAME`
    async fn get(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let what = rest.first().map(String::as_str).unwrap_or("url");
        let data = match what {
            "url" => json!({ "url": page.url().await.map_err(eval_err)?.unwrap_or_default() }),
            "title" => json!({ "title": eval_str(&page, "document.title").await? }),
            "text" => match rest.get(1) {
                Some(t) => json!({ "text": eval_str(&page, &format!("(function(){{const el=document.querySelector({}); return el ? (el.innerText || el.value || '') : '';}})()", js_sel(t))).await? }),
                None => json!({ "text": eval_str(&page, "document.body ? document.body.innerText : ''").await? }),
            },
            "html" => match rest.get(1) {
                Some(t) => json!({ "html": eval_str(&page, &format!("(function(){{const el=document.querySelector({}); return el ? el.outerHTML : '';}})()", js_sel(t))).await? }),
                None => json!({ "html": eval_str(&page, "document.documentElement.outerHTML").await? }),
            },
            "value" => {
                let t = rest.get(1).ok_or("get value 需要 target")?;
                json!({ "value": eval_str(&page, &format!("(function(){{const el=document.querySelector({}); return el ? (el.value ?? '') : '';}})()", js_sel(t))).await? })
            }
            "attr" => {
                let t = rest.get(1).ok_or("get attr 需要 target")?;
                let name = rest.get(2).ok_or("get attr 需要属性名")?;
                let expr = format!(
                    "(function(){{const el=document.querySelector({}); return el ? (el.getAttribute({}) ?? '') : '';}})()",
                    js_sel(t),
                    serde_json::to_string(name).unwrap_or_else(|_| "\"\"".into())
                );
                json!({ "value": eval_str(&page, &expr).await? })
            }
            "count" => {
                let expr = match rest.get(1) {
                    Some(t) => format!("document.querySelectorAll({}).length", js_sel(t)),
                    None => "0".to_string(),
                };
                json!({ "count": eval_value(&page, &expr).await? })
            }
            "box" => {
                let t = rest.get(1).ok_or("get box 需要 target")?;
                let expr = format!(
                    "(function(){{const el=document.querySelector({}); if(!el) return null; const r=el.getBoundingClientRect(); return {{x:Math.round(r.x),y:Math.round(r.y),width:Math.round(r.width),height:Math.round(r.height)}};}})()",
                    js_sel(t)
                );
                json!({ "box": eval_value(&page, &expr).await? })
            }
            other => return Err(format!("get 不支持: {other}")),
        };
        Ok(CliOutput {
            data,
            origin: Some(page.url().await.ok().flatten().unwrap_or_default()),
        })
    }

    /// `snapshot [-i] [-s SEL]`：可见可交互元素 + `@eN` 引用。
    async fn snapshot(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let mut selector: Option<String> = None;
        let mut interactive_only = false;
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "-i" => interactive_only = true,
                "-s" => selector = it.next().cloned(),
                _ => {}
            }
        }
        let sel_js = match &selector {
            Some(s) => serde_json::to_string(s).unwrap_or_else(|_| "null".into()),
            None => "null".to_string(),
        };
        let sel_set = if interactive_only {
            "INTERACTIVE"
        } else {
            "ALL"
        };
        let expr = SNAPSHOT_JS
            .replace("__SEL__", &sel_js)
            .replace("__SET__", sel_set);
        let raw = eval_str(&page, &expr).await?;
        let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        let tree = parsed
            .get("tree")
            .and_then(|t| t.as_str())
            .unwrap_or("(快照失败)")
            .to_string();
        let count = parsed.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
        let url = page.url().await.ok().flatten().unwrap_or_default();
        let title = eval_str(&page, "document.title").await.unwrap_or_default();
        // 只放 snapshot 一个键：core 的 content_of 按固定键序取正文（含 count 会只显示数字）
        Ok(CliOutput {
            data: json!({ "snapshot": format!("# {title} — {url}\n{tree}\n(共 {count} 个元素)") }),
            origin: Some(url),
        })
    }

    async fn screenshot(
        &self,
        s: &Session,
        rest: &[String],
        path: Option<String>,
    ) -> Result<CliOutput, String> {
        let page = s.page()?;
        let full = rest.iter().any(|a| a == "--full");
        let out = path
            .or_else(|| rest.iter().find(|a| a.ends_with(".png")).cloned())
            .ok_or("screenshot 缺少落盘路径")?;
        let bytes = page
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .full_page(full)
                    .build(),
            )
            .await
            .map_err(|e| format!("截图失败: {e}"))?;
        if let Some(dir) = std::path::Path::new(&out).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&out, bytes).map_err(|e| format!("写入截图失败（{out}）: {e}"))?;
        Ok(CliOutput {
            data: json!({ "path": out }),
            origin: page.url().await.ok().flatten(),
        })
    }

    /// `wait --text T | wait MS | wait SEL`
    async fn wait(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let ok = if let Some(idx) = rest.iter().position(|a| a == "--text") {
            let text = rest.get(idx + 1).cloned().unwrap_or_default();
            let expr = format!(
                "document.body && document.body.innerText.includes({})",
                serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".into())
            );
            poll_true(&page, &expr, deadline).await
        } else if let Some(sel) = rest.iter().find(|a| !a.chars().all(|c| c.is_ascii_digit())) {
            let expr = format!("!!document.querySelector({})", js_sel(sel));
            poll_true(&page, &expr, deadline).await
        } else {
            let ms: u64 = rest
                .first()
                .and_then(|m| m.parse().ok())
                .unwrap_or(1000)
                .min(60_000);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            true
        };
        if !ok {
            return Err("等待超时（45s 内条件未满足）".to_string());
        }
        Ok(CliOutput {
            data: json!({ "text": "ok" }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn scroll(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let dir = rest.first().cloned().unwrap_or_else(|| "down".into());
        let px: i64 = rest.get(1).and_then(|p| p.parse().ok()).unwrap_or(600);
        let (dx, dy) = match dir.as_str() {
            "up" => (0, -px),
            "down" => (0, px),
            "left" => (-px, 0),
            "right" => (px, 0),
            _ => (0, px),
        };
        let expr = format!("(function(){{window.scrollBy({dx},{dy}); return {{y: Math.round(window.scrollY)}};}})()");
        let v = eval_value(&page, &expr).await?;
        Ok(CliOutput {
            data: json!({ "text": format!("已滚动（{dir} {px}px，scrollY={}）", v.get("y").and_then(|y| y.as_i64()).unwrap_or(0)) }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn scroll_into_view(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let target = rest.first().ok_or("scrollintoview 需要 target")?;
        let el = page
            .find_element(selector_of(target))
            .await
            .map_err(|e| format!("元素不存在（{target}）: {e}"))?;
        el.scroll_into_view()
            .await
            .map_err(|e| format!("滚动到元素失败: {e}"))?;
        Ok(CliOutput {
            data: json!({ "text": format!("已滚动到 {target}") }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn click(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let target = rest.first().ok_or("click 需要 target")?;
        let sel = selector_of(target);
        let el = page
            .find_element(&sel)
            .await
            .map_err(|e| format!("元素不存在（{target}）: {e}"))?;
        let desc = element_desc(&el).await;
        el.click().await.map_err(|e| format!("点击失败（{target}）: {e}"))?;
        settle(&page).await;
        Ok(CliOutput {
            data: json!({ "text": format!("已点击 {target}（{desc}）") }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn type_text(&self, s: &Session, rest: &[String], clear: bool) -> Result<CliOutput, String> {
        let page = s.page()?;
        let target = rest.first().ok_or("type 需要 target")?;
        let text = rest.get(1).cloned().unwrap_or_default();
        let el = page
            .find_element(selector_of(target))
            .await
            .map_err(|e| format!("元素不存在（{target}）: {e}"))?;
        let desc = element_desc(&el).await;
        el.focus().await.map_err(|e| format!("聚焦失败: {e}"))?;
        if clear {
            let _ = el.click().await;
            let _ = eval_value(
                &page,
                &format!(
                    "(function(){{const el=document.querySelector({}); if(el && 'value' in el) {{ el.value=''; el.dispatchEvent(new Event('input',{{bubbles:true}})); }} return true;}})()",
                    js_sel(target)
                ),
            )
            .await;
        }
        el.type_str(&text).await.map_err(|e| format!("输入失败: {e}"))?;
        Ok(CliOutput {
            data: json!({ "text": format!("已输入 {target}（{desc}）：{text}") }),
            origin: page.url().await.ok().flatten(),
        })
    }

    /// `press KEY`（支持 Enter/Tab/Esc/方向键/翻页键 + `Ctrl+a` 形式组合键）。
    async fn press(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let key = rest.first().ok_or("press 需要 key")?.clone();
        press_key(&page, &key).await?;
        settle(&page).await;
        Ok(CliOutput {
            data: json!({ "text": format!("已按键 {key}") }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn select(&self, s: &Session, rest: &[String]) -> Result<CliOutput, String> {
        let page = s.page()?;
        let target = rest.first().ok_or("select 需要 target")?;
        let values: Vec<String> = rest.iter().skip(1).cloned().collect();
        let expr = format!(
            "(function(){{const el=document.querySelector({}); if(!el) return {{error:'元素不存在'}};\
             const want={}; const picked=[];\
             for (const opt of el.options) {{ const v=(opt.value??'').trim(); const t=(opt.textContent??'').trim();\
             if (want.includes(v) || want.includes(t)) {{ opt.selected = true; picked.push(v || t); }} }}\
             el.dispatchEvent(new Event('input',{{bubbles:true}})); el.dispatchEvent(new Event('change',{{bubbles:true}}));\
             return {{value: picked.join(',')}};}})()",
            js_sel(target),
            serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
        );
        let v = eval_value(&page, &expr).await?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return Err(format!("{err}（{target}）"));
        }
        Ok(CliOutput {
            data: json!({ "value": v.get("value").and_then(|x| x.as_str()).unwrap_or("") }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn check(&self, s: &Session, rest: &[String], want: bool) -> Result<CliOutput, String> {
        let page = s.page()?;
        let target = rest.first().ok_or("check 需要 target")?;
        let expr = format!(
            "(function(){{const el=document.querySelector({}); if(!el) return {{error:'元素不存在'}};\
             const on = !!el.checked; if (on !== {}) {{ el.click(); }}\
             return {{value: String(!!el.checked)}};}})()",
            js_sel(target),
            if want { "true" } else { "false" }
        );
        let v = eval_value(&page, &expr).await?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return Err(format!("{err}（{target}）"));
        }
        Ok(CliOutput {
            data: json!({ "text": format!("{} → {}", target, v.get("value").and_then(|x| x.as_str()).unwrap_or("")) }),
            origin: page.url().await.ok().flatten(),
        })
    }

    async fn open(&self, s: &mut Session, rest: &[String]) -> Result<CliOutput, String> {
        let url = rest.first().ok_or("open 需要 url")?.clone();
        let page = s.page()?;
        page.goto(url.clone()).await.map_err(|e| format!("导航失败（{url}）: {e}"))?;
        let _ = tokio::time::timeout(NAV_TIMEOUT, page.wait_for_navigation()).await;
        settle(&page).await;
        let now = page.url().await.ok().flatten().unwrap_or(url);
        Ok(CliOutput {
            data: json!({ "url": now }),
            origin: Some(now),
        })
    }

    async fn history(&self, s: &Session, action: &str) -> Result<CliOutput, String> {
        let page = s.page()?;
        let expr = match action {
            "back" => "history.back()",
            "forward" => "history.forward()",
            _ => "location.reload()",
        };
        let _ = eval_value(&page, expr).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        let url = page.url().await.ok().flatten().unwrap_or_default();
        Ok(CliOutput {
            data: json!({ "url": url }),
            origin: Some(url),
        })
    }

    /// `tab list | tab new [URL] | tab N | tab close [N]`
    async fn tab(&self, s: &mut Session, rest: &[String]) -> Result<CliOutput, String> {
        match rest.first().map(String::as_str) {
            Some("list") => {
                let mut lines = Vec::new();
                for (i, p) in s.pages.iter().enumerate() {
                    let url = p.url().await.ok().flatten().unwrap_or_default();
                    let title = eval_str(p, "document.title").await.unwrap_or_default();
                    lines.push(format!(
                        "{}. {}{} — {}",
                        i + 1,
                        if i == s.active { "[当前] " } else { "" },
                        if title.is_empty() { "(无标题)" } else { &title },
                        url
                    ));
                }
                let origin = match s.page() {
                    Ok(p) => p.url().await.ok().flatten(),
                    Err(_) => None,
                };
                Ok(CliOutput {
                    data: json!({ "text": lines.join("\n") }),
                    origin,
                })
            }
            Some("new") => {
                let url = rest.get(1).cloned().unwrap_or_else(|| "about:blank".into());
                let page = s
                    .browser
                    .new_page(url.as_str())
                    .await
                    .map_err(|e| format!("新建页签失败: {e}"))?;
                prepare_page(&page).await;
                if url != "about:blank" {
                    let _ = tokio::time::timeout(NAV_TIMEOUT, page.wait_for_navigation()).await;
                }
                s.pages.push(page.clone());
                s.active = s.pages.len() - 1;
                Ok(CliOutput {
                    data: json!({ "text": format!("已新建页签 {}（{url}）", s.pages.len()) }),
                    origin: page.url().await.ok().flatten(),
                })
            }
            Some("close") => {
                let idx = match rest.get(1).and_then(|t| t.parse::<usize>().ok()) {
                    Some(n) if n >= 1 && n <= s.pages.len() => n - 1,
                    _ => s.active,
                };
                let page = s.pages.remove(idx);
                let _ = page.close().await;
                if s.pages.is_empty() {
                    let fresh = s
                        .browser
                        .new_page("about:blank")
                        .await
                        .map_err(|e| format!("新建页签失败: {e}"))?;
                    prepare_page(&fresh).await;
                    s.pages.push(fresh);
                }
                s.active = s.active.min(s.pages.len() - 1);
                let origin = match s.page() {
                    Ok(p) => p.url().await.ok().flatten(),
                    Err(_) => None,
                };
                Ok(CliOutput {
                    data: json!({ "text": format!("已关闭页签 {}（剩余 {} 个）", idx + 1, s.pages.len()) }),
                    origin,
                })
            }
            Some(other) => {
                // tab N / tab <targetId 前缀>
                let idx = match other.parse::<usize>() {
                    Ok(n) if n >= 1 && n <= s.pages.len() => n - 1,
                    _ => {
                        let mut found = None;
                        for (i, p) in s.pages.iter().enumerate() {
                            let tid = p.target_id().as_ref().to_string();
                            if tid.starts_with(other) {
                                found = Some(i);
                                break;
                            }
                        }
                        found.ok_or_else(|| format!("页签不存在: {other}"))?
                    }
                };
                s.active = idx;
                let url = s.pages[idx].url().await.ok().flatten().unwrap_or_default();
                Ok(CliOutput {
                    data: json!({ "text": format!("已切换到页签 {}（{url}）", idx + 1) }),
                    origin: Some(url),
                })
            }
            None => Err("tab 需要子命令（list / new / N / close）".to_string()),
        }
    }
}

/// 面板画面流用的小工具：取某会话当前活动页（不存在则 None）。
impl BrowserHub {
    /// 取（或启动）会话画面流订阅端：**单 CDP 监听 → 广播给所有面板连接**。
    ///
    /// 同一页面重复 `Page.startScreencast` 会报错，故按会话复用一份监听；
    /// 帧 ack 由监听任务统一发送（Chrome 收到 ack 才发下一帧，天然限速）。
    pub async fn subscribe_frames(
        self: &Arc<Self>,
        sid: &str,
    ) -> Result<tokio::sync::broadcast::Receiver<String>, String> {
        if let Some(tx) = self.streams.lock().ok().and_then(|m| m.get(sid).cloned()) {
            return Ok(tx.subscribe());
        }
        let page = self.ensure_page(sid).await?;
        // 旧流先停（浏览器可能从上一轮会话残留着 screencast 状态）
        let _ = page.execute(StopScreencastParams {}).await;
        page.execute(
            StartScreencastParams::builder()
                .format(StartScreencastFormat::Jpeg)
                .quality(FRAME_QUALITY)
                .max_width(FRAME_MAX_W)
                .max_height(FRAME_MAX_H)
                .every_nth_frame(1)
                .build(),
        )
        .await
        .map_err(|e| format!("启动画面流失败: {e}"))?;
        let mut frames = page
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|e| format!("订阅画面帧失败: {e}"))?;
        let (tx, _) = tokio::sync::broadcast::channel::<String>(64);
        let tx_task = tx.clone();
        let page_task = page.clone();
        tokio::spawn(async move {
            while let Some(frame) = frames.next().await {
                let msg = json!({
                    "type": "frame",
                    "data": <chromiumoxide::Binary as AsRef<str>>::as_ref(&frame.data),
                    "width": frame.metadata.device_width,
                    "height": frame.metadata.device_height,
                })
                .to_string();
                // 无人订阅（面板都关了）时继续 ack 但不积压：send 失败忽略
                let _ = tx_task.send(msg);
                let _ = page_task
                    .execute(ScreencastFrameAckParams::new(frame.session_id))
                    .await;
            }
        });
        if let Ok(mut map) = self.streams.lock() {
            map.insert(sid.to_string(), tx.clone());
        }
        Ok(tx.subscribe())
    }

    /// 取（或懒启动）活动页——面板一打开就要有画面，故这里也会启动浏览器。
    pub async fn ensure_page(&self, sid: &str) -> Result<Page, String> {
        let session = self.session_for(sid).await?;
        let guard = session.lock().await;
        let page = guard.page()?;
        let mut url = String::new();
        if let Ok(Some(u)) = page.url().await {
            url = u;
            if let Ok(mut urls) = self.active_urls.lock() {
                urls.insert(sid.to_string(), url.clone());
            }
        }
        // 端点跟随最近使用的会话：面板一打开/一取页，就把 CLI 的落点指到这台浏览器
        publish_endpoint(sid, &guard, &url);
        Ok(page)
    }

    /// 面板用的会话状态快照（URL / 标题 / 视口 / 页签列表）。
    pub async fn state_of(&self, sid: &str) -> Value {
        let Some(session) = self.sessions.lock().ok().and_then(|m| m.get(sid).cloned()) else {
            return json!({ "url": "", "title": "", "width": VIEWPORT.0, "height": VIEWPORT.1, "tabs": [] });
        };
        let guard = session.lock().await;
        let mut tabs = Vec::new();
        for (i, p) in guard.pages.iter().enumerate() {
            let url = p.url().await.ok().flatten().unwrap_or_default();
            let title = eval_str(p, "document.title").await.unwrap_or_default();
            tabs.push(json!({ "index": i + 1, "title": title, "url": url, "active": i == guard.active }));
        }
        let (url, title) = match guard.page() {
            Ok(p) => (
                p.url().await.ok().flatten().unwrap_or_default(),
                eval_str(&p, "document.title").await.unwrap_or_default(),
            ),
            Err(_) => (String::new(), String::new()),
        };
        json!({
            "url": url,
            "title": title,
            "width": VIEWPORT.0,
            "height": VIEWPORT.1,
            "tabs": tabs,
        })
    }

    /// 面板页签操作（tab / tab_new / tab_close）。
    pub async fn ws_tab(&self, sid: &str, cmd: &Value) -> Result<(), String> {
        let session = self.session_for(sid).await?;
        let mut guard = session.lock().await;
        let kind = cmd.get("type").and_then(|t| t.as_str()).unwrap_or("tab");
        let mut argv: Vec<String> = vec!["tab".to_string()];
        match kind {
            "tab_new" => {
                argv.push("new".into());
                if let Some(url) = cmd.get("url").and_then(|u| u.as_str()) {
                    argv.push(url.to_string());
                }
            }
            "tab_close" => {
                argv.push("close".into());
                if let Some(i) = cmd.get("index").and_then(|v| v.as_i64()) {
                    argv.push(i.to_string());
                }
            }
            _ => {
                let i = cmd.get("index").and_then(|v| v.as_i64()).unwrap_or(1);
                argv.push(i.to_string());
            }
        }
        let out = self.tab(&mut guard, &argv[1..]).await;
        if let Ok(page) = guard.page()
            && let Ok(Some(url)) = page.url().await
        {
            publish_endpoint(sid, &guard, &url);
        }
        out.map(|_| ())
    }

    /// 回写端点文件：`frameUrl` 跟随活动页——面板与 AI（经 CLI）都可能**绕过服务端动作路径**
    /// 直接导航，端点里的 URL 会过期（审批 origin / 展示读它）。面板命令后与空闲节拍各回写一次。
    pub async fn refresh_endpoint(&self, sid: &str) {
        let Some(session) = self.sessions.lock().ok().and_then(|m| m.get(sid).cloned()) else {
            return;
        };
        let guard = session.lock().await;
        let Ok(page) = guard.page() else { return };
        let url = page.url().await.ok().flatten().unwrap_or_default();
        if let Ok(mut urls) = self.active_urls.lock() {
            urls.insert(sid.to_string(), url.clone());
        }
        publish_endpoint(sid, &guard, &url);
    }

    /// 端点文件里记录的会话 id（无文件 / 不可解析 → None）。
    fn endpoint_owner(&self) -> Option<String> {
        let raw = std::fs::read_to_string(astorm_core::tool::browser::agent_cli::endpoint_path()).ok()?;
        serde_json::from_str::<Value>(&raw)
            .ok()?
            .get("sessionId")
            .and_then(|s| s.as_str())
            .map(str::to_string)
    }

    /// 当前活动页 URL（旁路缓存；不抢会话锁）。
    pub fn cached_url(&self, sid: &str) -> Option<String> {
        self.active_urls.lock().ok().and_then(|m| m.get(sid).cloned())
    }

    /// 取活动页（供 screencast / 输入回传；会话不存在 → None）。
    pub async fn active_page(&self, sid: &str) -> Option<Page> {
        let session = self.sessions.lock().ok().and_then(|m| m.get(sid).cloned())?;
        let guard = session.lock().await;
        guard.page().ok()
    }

    /// 该会话是否已有浏览器。
    pub fn has_session(&self, sid: &str) -> bool {
        self.sessions.lock().map(|m| m.contains_key(sid)).unwrap_or(false)
    }
}

#[async_trait]
impl NativeBrowser for BrowserHub {
    async fn origin(&self, session_id: &str) -> Option<String> {
        if let Some(url) = self.cached_url(session_id)
            && !url.is_empty()
            && url != "about:blank"
        {
            return Some(url);
        }
        let page = self.active_page(session_id).await?;
        page.url().await.ok().flatten()
    }

    async fn execute(
        &self,
        session_id: &str,
        argv: Vec<String>,
        screenshot_path: Option<String>,
    ) -> Result<CliOutput, String> {
        self.act(session_id, &argv, screenshot_path).await
    }

    async fn close(&self, session_id: &str) {
        let session = self.sessions.lock().ok().and_then(|mut m| m.remove(session_id));
        if let Some(session) = session {
            let mut guard = session.lock().await;
            for page in guard.pages.drain(..) {
                let _ = page.close().await;
            }
            guard.driver.abort();
            let _ = guard.browser.close().await;
            let _ = std::fs::remove_dir_all(&guard.profile);
            if let Ok(mut urls) = self.active_urls.lock() {
                urls.remove(session_id);
            }
            if let Ok(mut streams) = self.streams.lock() {
                streams.remove(session_id);
            }
            retract_endpoint(session_id);
            tracing::info!("[browser] 会话 {session_id} 浏览器已关闭");
        }
    }

    fn label(&self) -> &'static str {
        "服务端浏览器"
    }
}

/* ---------------- 页面辅助 ---------------- */

/// 读 Chromium 写下的 `DevToolsActivePort`（首行 = 端口，次行 = 浏览器级 ws 路径）。
/// `--remote-debugging-port=0` 时端口由 Chrome 自选，这个文件是唯一权威来源；
/// 启动后立刻读取可能还没落盘，故短暂重试。
fn browser_cdp_ws(profile: &std::path::Path) -> Option<String> {
    let file = profile.join("DevToolsActivePort");
    for _ in 0..40 {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let mut lines = text.lines();
            let port = lines.next().unwrap_or("").trim();
            let path = lines.next().unwrap_or("").trim();
            if !port.is_empty() && !path.is_empty() {
                return Some(format!("ws://127.0.0.1:{port}{path}"));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// 写桥接端点文件（core 的 `agent_cli::read_endpoint` 读它决定走 CLI 还是宿主浏览器）。
///
/// 容器语义与桌面一致：**一桥一会话**——文件描述"当前可操控的那台浏览器"，pid = 本进程
/// （服务端），sessionId = 根会话，token 由浏览器 ws 地址派生（换浏览器即换 daemon 名）。
/// `frame_url` 为空时用活动页当前 URL 兜底（审批 origin / 展示）。
fn publish_endpoint(sid: &str, session: &Session, frame_url: &str) {
    if session.cdp_ws.is_empty() {
        return;
    }
    let path = astorm_core::tool::browser::agent_cli::endpoint_path();
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!("[browser] 端点目录创建失败（{}）: {e}", dir.display());
        return;
    }
    let token: String = session
        .cdp_ws
        .rsplit(['/', ':'])
        .find(|seg| !seg.is_empty() && *seg != "devtools")
        .unwrap_or("cdp")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let body = json!({
        "wsUrl": session.cdp_ws,
        "token": token,
        "frameUrl": frame_url,
        "tabId": "",
        "sessionId": sid,
        "pid": std::process::id(),
    });
    match std::fs::write(&path, body.to_string()) {
        Ok(_) => tracing::debug!("[browser] 端点已发布（{} 会话 {sid}）", path.display()),
        Err(e) => tracing::warn!("[browser] 端点写入失败（{}）: {e}", path.display()),
    }
}

/// 浏览器回收时撤掉端点文件（仅在文件确实描述本会话时）——避免工具读到死端点。
fn retract_endpoint(sid: &str) {
    let path = astorm_core::tool::browser::agent_cli::endpoint_path();
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let owner = serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("sessionId").and_then(|s| s.as_str()).map(str::to_string));
    if owner.as_deref() == Some(sid) {
        let _ = std::fs::remove_file(&path);
    }
}

/// 新页初始化：视口 + UA + stealth。
/// 视口用 CDP 覆盖而非只靠 `--window-size`（headless 下窗口参数不保证生效，实测页面仍是
/// 800×600）：覆盖后页面 innerWidth/innerHeight、截图与画面流帧都稳定为 1280×900，
/// 面板坐标映射（按视口换算）才不会偏。
async fn prepare_page(page: &Page) {
    let _ = page
        .execute(SetDeviceMetricsOverrideParams::new(
            VIEWPORT.0 as i64,
            VIEWPORT.1 as i64,
            1.0,
            false,
        ))
        .await;
    let _ = page.set_user_agent(UA).await;
    let _ = page
        .evaluate_on_new_document(AddScriptToEvaluateOnNewDocumentParams::new(
            STEALTH_JS.to_string(),
        ))
        .await;
    let _ = page
        .evaluate_on_new_document(AddScriptToEvaluateOnNewDocumentParams::new(
            POPUP_JS.to_string(),
        ))
        .await;
}

/// 目标引用解析：`@eN` → `[data-astorm-ref="eN"]`；其余按 CSS 选择器原样。
/// 按键分发（工具与面板共用）：支持命名键 + `Ctrl+a` 形式组合键。
pub(crate) async fn press_key(page: &Page, key: &str) -> Result<(), String> {
    let (mods, base) = match key.rsplit_once('+') {
        Some((m, b)) => {
            let mut mods = 0i64;
            for part in m.split('+') {
                match part.trim().to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => mods |= 2,
                    "alt" => mods |= 1,
                    "shift" => mods |= 8,
                    "meta" | "cmd" | "command" => mods |= 4,
                    _ => {}
                }
            }
            (mods, b.trim().to_string())
        }
        None => (0, key.to_string()),
    };
    let (vk, code, text) = key_spec(&base).ok_or_else(|| format!("不支持的按键: {key}"))?;
    let down = DispatchKeyEventParams::builder()
        .r#type(DispatchKeyEventType::KeyDown)
        .key(base.clone())
        .code(code.to_string())
        .windows_virtual_key_code(vk)
        .native_virtual_key_code(vk)
        .modifiers(mods)
        .text(text.to_string())
        .build()
        .map_err(|e| format!("构造按键事件失败: {e}"))?;
    page.execute(down).await.map_err(|e| format!("按键失败: {e}"))?;
    let up = DispatchKeyEventParams::builder()
        .r#type(DispatchKeyEventType::KeyUp)
        .key(base.clone())
        .code(code.to_string())
        .windows_virtual_key_code(vk)
        .native_virtual_key_code(vk)
        .modifiers(mods)
        .build()
        .map_err(|e| format!("构造按键事件失败: {e}"))?;
    page.execute(up).await.map_err(|e| format!("按键失败: {e}"))?;
    Ok(())
}

pub fn selector_of(target: &str) -> String {
    let t = target.trim();
    match t.strip_prefix('@') {
        Some(r) if !r.is_empty() => format!("[data-astorm-ref=\"{r}\"]"),
        _ => t.to_string(),
    }
}

fn js_sel(target: &str) -> String {
    serde_json::to_string(&selector_of(target)).unwrap_or_else(|_| "\"body\"".into())
}

fn eval_err(e: impl std::fmt::Display) -> String {
    format!("页面求值失败: {e}")
}

async fn eval_value(page: &Page, expr: &str) -> Result<Value, String> {
    let r = page.evaluate(expr.to_string()).await.map_err(eval_err)?;
    Ok(r.value().cloned().unwrap_or(Value::Null))
}

async fn eval_str(page: &Page, expr: &str) -> Result<String, String> {
    let v = eval_value(page, expr).await?;
    Ok(match v {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

/// 轮询等待 JS 条件为真（页面侧 250ms 一次，直到 deadline）。
async fn poll_true(page: &Page, expr: &str, deadline: tokio::time::Instant) -> bool {
    loop {
        if let Ok(v) = eval_value(page, expr).await
            && v.as_bool().unwrap_or(false)
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 动作后的小停顿：让 SPA 有机会响应（桌面 CLI 侧同样有 settle）。
async fn settle(page: &Page) {
    let _ = tokio::time::timeout(Duration::from_secs(5), page.wait_for_navigation()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// 元素的一句描述（用于结果回执；取不到就退化为「元素」）。
async fn element_desc(el: &chromiumoxide::element::Element) -> String {
    let text = el.inner_text().await.ok().flatten().unwrap_or_default();
    let text = text.trim().replace('\n', " ");
    let short: String = text.chars().take(30).collect();
    if short.is_empty() {
        "元素".to_string()
    } else {
        format!("「{short}」")
    }
}

/// 字母键的 CDP `code` 查表（`A` → `KeyA`）。
const LETTER_CODES: [(char, &str); 26] = [
    ('A', "KeyA"), ('B', "KeyB"), ('C', "KeyC"), ('D', "KeyD"), ('E', "KeyE"), ('F', "KeyF"),
    ('G', "KeyG"), ('H', "KeyH"), ('I', "KeyI"), ('J', "KeyJ"), ('K', "KeyK"), ('L', "KeyL"),
    ('M', "KeyM"), ('N', "KeyN"), ('O', "KeyO"), ('P', "KeyP"), ('Q', "KeyQ"), ('R', "KeyR"),
    ('S', "KeyS"), ('T', "KeyT"), ('U', "KeyU"), ('V', "KeyV"), ('W', "KeyW"), ('X', "KeyX"),
    ('Y', "KeyY"), ('Z', "KeyZ"),
];
/// 数字键的 CDP `code` 查表（0-9）。
const DIGIT_CODES: [&str; 10] = [
    "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
    "Digit9",
];

/// 按键 → (虚拟键码, code, 文本)。
fn key_spec(key: &str) -> Option<(i64, &'static str, &'static str)> {
    let k = key.trim();
    let spec = match k {
        "Enter" | "enter" => (13, "Enter", "\r"),
        "Tab" | "tab" => (9, "Tab", "\t"),
        "Escape" | "escape" | "Esc" => (27, "Escape", ""),
        "Backspace" => (8, "Backspace", ""),
        "Delete" => (46, "Delete", ""),
        "ArrowUp" => (38, "ArrowUp", ""),
        "ArrowDown" => (40, "ArrowDown", ""),
        "ArrowLeft" => (37, "ArrowLeft", ""),
        "ArrowRight" => (39, "ArrowRight", ""),
        "Home" => (36, "Home", ""),
        "End" => (35, "End", ""),
        "PageUp" => (33, "PageUp", ""),
        "PageDown" => (34, "PageDown", ""),
        "Space" | " " => (32, "Space", " "),
        other => {
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if c.is_ascii_alphabetic() {
                let up = c.to_ascii_uppercase();
                // code 需要 'static：字母表查表
                let code = LETTER_CODES
                    .iter()
                    .find(|(k, _)| *k == up)
                    .map(|(_, v)| *v)
                    .unwrap_or("KeyA");
                (up as i64, code, "")
            } else if c.is_ascii_digit() {
                (c as i64, DIGIT_CODES[(c as u8 - b'0') as usize], "")
            } else {
                (c as i64, "", "")
            }
        }
    };
    Some(spec)
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Chromium 可执行文件解析（与 astorm-render 同源：env → PATH）。
fn resolve_chrome() -> String {
    if let Ok(p) = std::env::var("ASTORM_CHROME_BIN")
        && !p.trim().is_empty()
    {
        return p;
    }
    #[cfg(target_os = "macos")]
    {
        let p = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        if std::path::Path::new(p).is_file() {
            return p.to_string();
        }
    }
    for name in ["chromium", "chromium-browser", "google-chrome", "chrome"] {
        if which(name) {
            return name.to_string();
        }
    }
    "chromium".to_string()
}

/// PATH 查找（Windows 必须用 `split_paths` + PATHEXT，不能按 ':' 切）。
fn which(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    #[cfg(windows)]
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    #[cfg(not(windows))]
    let exts: Vec<String> = vec![String::new()];
    for dir in std::env::split_paths(&paths) {
        for ext in &exts {
            if dir.join(format!("{bin}{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

/// snapshot 注入 JS：给可见可交互元素打 `data-astorm-ref`，返回 `{tree, count}` JSON 字符串。
/// `__SEL__` 替换为范围选择器（null = 全文档）；`__SET__` = INTERACTIVE / ALL。
const SNAPSHOT_JS: &str = r#"(function(){
  const SEL = __SEL__;
  const SET = "__SET__";
  document.querySelectorAll('[data-astorm-ref]').forEach(e => e.removeAttribute('data-astorm-ref'));
  const scope = SEL ? document.querySelector(SEL) : document;
  if (!scope) return JSON.stringify({tree: '(选择器无匹配: ' + SEL + ')', count: 0});
  const INTERACTIVE = 'a,button,input,select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=radio],[role=tab],[role=menuitem],[contenteditable=true],[onclick]';
  const ALL = 'a,button,input,select,textarea,h1,h2,h3,h4,label,li,td,th,p,span,div,img,[role=button],[role=link]';
  const nodes = Array.from(scope.querySelectorAll(SET === 'ALL' ? ALL : INTERACTIVE));
  const lines = [];
  let i = 0;
  for (const el of nodes) {
    const r = el.getBoundingClientRect();
    const st = getComputedStyle(el);
    if (r.width < 1 || r.height < 1 || st.visibility === 'hidden' || st.display === 'none' || st.opacity === '0') continue;
    if (el.closest('[data-astorm-ignore]')) continue;
    i++;
    const ref = 'e' + i;
    el.setAttribute('data-astorm-ref', ref);
    const tag = el.tagName.toLowerCase();
    const type = el.getAttribute('type');
    const name = (el.getAttribute('name') || el.getAttribute('id') || '').trim();
    const ph = (el.getAttribute('placeholder') || '').trim();
    const label = (el.innerText || el.value || el.getAttribute('aria-label') || el.getAttribute('title') || el.getAttribute('alt') || '').trim().replace(/\s+/g, ' ').slice(0, 60);
    let line = '@' + ref + ' ' + tag + (type ? '[' + type + ']' : '');
    if (name) line += ' name=' + name;
    if (ph) line += ' placeholder="' + ph + '"';
    if (label) line += ' "' + label + '"';
    if (el.disabled) line += ' (disabled)';
    lines.push(line);
  }
  return JSON.stringify({tree: lines.join('\n') || '(无可交互元素)', count: i});
})()"#;
