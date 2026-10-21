//! astorm-render：页面渲染助手（astorm-server 镜像内与 Chromium 配套）。
//!
//! 与 Rust 侧 `WebRender` 说**同种协议**（astorm-core `tool/search/renderer.rs`）：
//! - 启动握手：`{"ready":true,"concurrency":N}`
//! - 请求（stdin 行）：`{"id":1,"mode":"html|text|markdown|links","url":"…","selector":"…"}`
//! - 响应（stdout 行）：`{"id":1,"ok":true,"data":"…"}` / `{"id":1,"ok":false,"error":"…"}`
//! - stdin EOF → 退出；超时/被杀由 Rust 侧负责。
//!
//! 桌面版的同一协议由「Electron 二进制 + page-render.cjs」承担（隐藏窗口池 +
//! executeJavaScript 抽取 + stealth 反爬预加载）。本助手用 chromiumoxide 驱动容器内
//! Chromium，抽取 JS 与 stealth 脚本从 page-render.cjs / stealth-preload.cjs 原样移植。
//!
//! 用法：`astorm-render <script（占位，忽略）> --concurrency N [--profile DIR]`
//! Chromium 路径：`ASTORM_CHROME_BIN` → PATH 上的 `chromium`/`chromium-browser`/`google-chrome`。
//! 容器内必须 `--no-sandbox`（默认开启，可用 `ASTORM_RENDER_NO_SANDBOX=0` 关闭）。

use std::process::Stdio;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::network::{Headers, SetExtraHttpHeadersParams};
use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// 单次渲染总超时（与桌面侧 webfetch 的 30s 对齐）。
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);
/// 页面「看起来稳定」的观察窗口（did-finish-load 后轮询正文长度，同 page-render.cjs settle）。
const SETTLE_POLLS: usize = 12;
const SETTLE_INTERVAL: Duration = Duration::from_millis(250);

/// 请求（stdin 行）。
#[derive(Debug, Deserialize)]
struct Req {
    id: u64,
    /// `html`（outerHTML）/ `text`（可读正文）/ `markdown` / `links`
    mode: Option<String>,
    url: String,
    #[serde(default)]
    selector: Option<String>,
}

/// 响应（stdout 行）。
#[derive(Debug, Serialize)]
struct Resp {
    id: u64,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// 与 page-render.cjs 相同的 UA（真实 Chrome 指纹）。
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// stealth 反爬预加载（stealth-preload.cjs 原样移植）：抹掉自动化/无头指纹。
const STEALTH_JS: &str = r#"
;(() => {
  try {
    const nav = navigator
    const proto = Object.getPrototypeOf(nav)
    const def = (obj, prop, getter) => {
      try { Object.defineProperty(obj, prop, { get: getter, configurable: true }) } catch {}
    }
    def(proto, 'webdriver', () => false)
    def(nav, 'webdriver', () => false)
    if (!window.chrome) {
      Object.defineProperty(window, 'chrome', { value: {}, configurable: true, writable: true })
    }
    if (window.chrome && !window.chrome.runtime) {
      Object.defineProperty(window.chrome, 'runtime', { value: {}, configurable: true, writable: true })
    }
    def(proto, 'languages', () => ['zh-CN', 'zh', 'en'])
    def(proto, 'language', () => 'zh-CN')
    const plugin = { name: 'Chrome PDF Plugin', filename: 'internal-pdf-viewer', description: 'Portable Document Format' }
    def(proto, 'plugins', () => [plugin, plugin, plugin])
    def(proto, 'mimeTypes', () => [])
    def(proto, 'hardwareConcurrency', () => 8)
    def(proto, 'hardwareConcurrency', () => 8)
    def(proto, 'deviceMemory', () => 8)
    try {
      const patch = (Ctor) => {
        if (!Ctor) return
        const orig = Ctor.prototype.getParameter
        Ctor.prototype.getParameter = function (p) {
          if (p === 37445) return 'Intel Inc.'
          if (p === 37446) return 'Intel Iris OpenGL Engine'
          return orig.apply(this, [p])
        }
      }
      patch(window.WebGLRenderingContext)
      patch(window.WebGL2RenderingContext)
    } catch {}
    try {
      const origQuery = window.navigator.permissions && window.navigator.permissions.query
      if (origQuery) {
        window.navigator.permissions.query = (params) =>
          params && params.name === 'notifications'
            ? Promise.resolve({ state: Notification.permission, onchange: null })
            : origQuery.call(window.navigator.permissions, params)
      }
    } catch {}
  } catch {}
})()
"#;

/// 页面世界抽取器（page-render.cjs EXTRACT_SRC 原样移植：text/markdown/links +
/// 主体内容启发式；html 模式直接取 outerHTML）。
const EXTRACT_SRC: &str = r#"
  const SKIP = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','NAV','HEADER','FOOTER','ASIDE','IFRAME','SVG','CANVAS','LINK','META']);
  const BLOCK = new Set(['P','DIV','SECTION','ARTICLE','MAIN','UL','OL','LI','TABLE','THEAD','TBODY','TR','H1','H2','H3','H4','H5','H6','BLOCKQUOTE','PRE','HR','FIGCAPTION','DL','DT','DD']);
  const abs = (u) => { try { return u ? new URL(u, location.href).href : '' } catch { return '' } };
  const beacon = (el) => {
    const w = parseInt(el.getAttribute('width')||'0',10), h = parseInt(el.getAttribute('height')||'0',10);
    if ((w && w<=2) || (h && h<=2)) return true;
    const src = (el.getAttribute('src')||'').toLowerCase();
    return /(beacon|pixel|track|analytics|\.gif$)/.test(src) && (w<=1||h<=1);
  };
  function toText(root){
    const out=[];
    (function walk(node){
      if(node.nodeType===3){ out.push(node.nodeValue); return; }
      if(node.nodeType!==1) return;
      const tag=node.tagName;
      if(SKIP.has(tag)) return;
      if(tag==='BR'){ out.push('\n'); return; }
      const block=BLOCK.has(tag);
      if(block) out.push('\n');
      for(const c of node.childNodes) walk(c);
      if(block) out.push('\n');
    })(root);
    return out.join('').replace(/[ \t\u00a0]+/g,' ').replace(/ *\n */g,'\n').replace(/\n{3,}/g,'\n\n').trim();
  }
  function toMarkdown(root){
    const out=[];
    function inline(node){
      let s='';
      for(const c of node.childNodes){
        if(c.nodeType===3){ s+=c.nodeValue; continue; }
        if(c.nodeType!==1) continue;
        const tag=c.tagName;
        if(SKIP.has(tag)) continue;
        if(tag==='BR'){ s+='  \n'; continue; }
        if(tag==='STRONG'||tag==='B'){ s+='**'+inline(c).trim()+'**'; continue; }
        if(tag==='EM'||tag==='I'){ s+='*'+inline(c).trim()+'*'; continue; }
        if(tag==='CODE'){ s+='\u0060'+c.textContent.replace(/\u0060/g,'')+'\u0060'; continue; }
        if(tag==='A'){ const href=abs(c.getAttribute('href')); const t=inline(c).replace(/\s+/g,' ').trim(); s+= href? '['+(t||href)+']('+href+')' : t; continue; }
        if(tag==='IMG'){ if(!beacon(c)){ const src=abs(c.getAttribute('src')); const alt=(c.getAttribute('alt')||'').replace(/\s+/g,' '); if(src) s+='!['+alt+']('+src+')'; } continue; }
        s+=inline(c);
      }
      return s;
    }
    function table(node){
      const rows=[...node.querySelectorAll('tr')];
      const grid=rows.map(r=>[...r.children].filter(x=>x.tagName==='TD'||x.tagName==='TH').map(x=>inline(x).replace(/\n/g,' ').trim()));
      if(!grid.length) return;
      const cols=Math.max(...grid.map(r=>r.length));
      const pad=(r)=>[...r, ...Array(cols-r.length).fill('')];
      out.push('\n| '+pad(grid[0]).join(' | ')+' |');
      out.push('| '+Array(cols).fill('---').join(' | ')+' |');
      for(let i=1;i<grid.length;i++) out.push('| '+pad(grid[i]).join(' | ')+' |');
      out.push('');
    }
    function block(node){
      for(const c of node.childNodes){
        if(c.nodeType===3){ const t=c.nodeValue.replace(/\s+/g,' ').trim(); if(t) out.push(t); continue; }
        if(c.nodeType!==1) continue;
        const tag=c.tagName;
        if(SKIP.has(tag)) continue;
        if(/^H[1-6]$/.test(tag)){ out.push('\n'+'#'.repeat(+tag[1])+' '+inline(c).replace(/\s+/g,' ').trim()+'\n'); continue; }
        if(tag==='P'){ const t=inline(c).replace(/\s+/g,' ').trim(); if(t) out.push('\n'+t+'\n'); continue; }
        if(tag==='UL'||tag==='OL'){ let i=1; for(const li of c.children){ if(li.tagName!=='LI') continue; const t=inline(li).replace(/\s+/g,' ').trim(); if(t) out.push((tag==='OL'? (i++)+'. ' : '- ')+t); } out.push(''); continue; }
        if(tag==='BLOCKQUOTE'){ const t=inline(c).replace(/\s+/g,' ').trim(); if(t) out.push('\n> '+t+'\n'); continue; }
        if(tag==='PRE'){ out.push('\n\u0060\u0060\u0060\n'+c.textContent.replace(/\n+$/,'')+'\n\u0060\u0060\u0060\n'); continue; }
        if(tag==='HR'){ out.push('\n---\n'); continue; }
        if(tag==='TABLE'){ table(c); continue; }
        if(BLOCK.has(tag)){ block(c); continue; }
        const t=inline(c).replace(/\s+/g,' ').trim(); if(t) out.push(t);
      }
    }
    block(root);
    return out.join('\n').replace(/\n{3,}/g,'\n\n').trim();
  }
  function extractLinks(root){
    const set=new Set();
    for(const a of root.querySelectorAll('a[href]')){ const h=abs(a.getAttribute('href')); if(/^https?:/i.test(h)) set.add(h); }
    return [...set].join('\n');
  }
  function contentRoot(){
    const CAND=['article','main','[role=main]','#content','#main','#body','.content','.article','.post','.entry-content','.markdown-body','.article-content'];
    for(const s of CAND){ let el=null; try{ el=document.querySelector(s); }catch{} if(el){ const n=(el.innerText||'').trim().length; if(n>200) return el; } }
    return document.body || document.documentElement;
  }
"#;

/// 组装注入页面的抽取表达式（同 page-render.cjs buildExpr）。
/// 拼接而非 format!：EXTRACT_SRC 里大量 JS 大括号与格式化语法冲突。
fn build_expr(mode: &str, selector: Option<&str>) -> String {
    let sel = serde_json::to_string(&selector).unwrap_or_else(|_| "null".into());
    let m = serde_json::to_string(mode).unwrap_or_else(|_| "\"html\"".into());
    let mut expr = String::from("(function(){ ");
    expr.push_str(EXTRACT_SRC);
    expr.push_str(&format!(
        "\n    const MODE={m};\n    if(MODE==='html') return document.documentElement.outerHTML;\n    const SEL={sel};\n    const root = SEL ? document.querySelector(SEL) : contentRoot();\n    if(!root) return '';\n    if(MODE==='links') return extractLinks(root);\n    if(MODE==='markdown') return toMarkdown(root);\n    return toText(root);\n  }})()"
    ));
    expr
}

#[tokio::main]
async fn main() {
    // 参数：忽略 script 占位；--concurrency N；--profile DIR（Rust 侧传的独立
    // userData 目录；缺省用带 pid 的唯一临时目录，避免多实例 SingletonLock 冲突）
    let mut concurrency = 2usize;
    let mut profile: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--concurrency" => {
                concurrency = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(2)
                    .max(1);
            }
            "--profile" => {
                profile = args.next().filter(|s| !s.is_empty());
            }
            _ => {}
        }
    }

    let chrome = resolve_chrome();
    let mut launch_args = vec![
        "--disable-gpu".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--lang=zh-CN".to_string(),
        "--disable-blink-features=AutomationControlled".to_string(),
        "--window-size=1280,900".to_string(),
    ];

    let mut config = BrowserConfig::builder().chrome_executable(&chrome);
    // 独立 profile：--profile 优先，缺省带 pid+纳秒的临时目录——多实例共用
    // chromiumoxide 默认目录会撞 SingletonLock（File exists → 浏览器直接退出）
    let auto_profile = profile.is_none();
    let user_data = profile.unwrap_or_else(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("astorm-render-{}-{nanos}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    });
    config = config.user_data_dir(&user_data);
    // 容器内默认 no-sandbox（root 用户必须；本地调试可用 ASTORM_RENDER_NO_SANDBOX=0 关闭）
    if std::env::var("ASTORM_RENDER_NO_SANDBOX").as_deref() != Ok("0") {
        config = config.no_sandbox();
    }
    let config = config.args(launch_args).build()
        .unwrap_or_else(|e| die(&format!("Chromium 配置无效（{chrome}）: {e}")));

    let (mut browser, mut handler) = tokio::time::timeout(Duration::from_secs(30), Browser::launch(config))
        .await
        .unwrap_or_else(|_| die("启动 Chromium 超时（30s）"))
        .unwrap_or_else(|e| die(&format!("启动 Chromium 失败（{chrome}）: {e}")));
    // 驱动 CDP 事件循环（不消费则所有 CDP 响应阻塞）
    tokio::spawn(async move {
        while let Some(_evt) = handler.next().await {}
    });

    // 页面池：mpsc 通道天然实现「取空闲页 + 排队」
    let mut pages = Vec::new();
    for _ in 0..concurrency {
        match browser.new_page("about:blank").await {
            Ok(page) => {
                let _ = page.set_user_agent(UA).await;
                let _ = page
                    .execute(SetExtraHttpHeadersParams::new(Headers::new(
                        serde_json::json!({ "Accept-Language": "zh-CN,zh;q=0.9,en;q=0.8" }),
                    )))
                    .await;
                let _ = page
                    .execute(AddScriptToEvaluateOnNewDocumentParams::new(
                        STEALTH_JS.to_string(),
                    ))
                    .await;
                pages.push(page);
            }
            Err(e) => die(&format!("创建渲染页面失败: {e}")),
        }
    }
    let (pool_tx, mut pool_rx) = tokio::sync::mpsc::channel::<chromiumoxide::Page>(concurrency);
    for p in pages {
        let _ = pool_tx.send(p).await;
    }
    let pool_tx = std::sync::Arc::new(pool_tx);

    println!(
        "{}",
        serde_json::json!({ "ready": true, "concurrency": concurrency })
    );
    let _ = tokio::io::stdout().flush().await;

    // stdin 行循环：每行一个请求，投给窗口池并发处理
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(text)) = line else { break }; // EOF → 退出
                let Ok(req) = serde_json::from_str::<Req>(text.trim()) else { continue };
                let Some(page) = pool_rx.recv().await else { break };
                let tx = done_tx.clone();
                let pool = pool_tx.clone();
                tokio::spawn(async move {
                    let resp = render(&page, &req).await;
                    let _ = tx.send(serde_json::to_string(&resp).unwrap_or_default());
                    let _ = pool.send(page).await; // 归还页面
                });
            }
            Some(resp) = done_rx.recv() => {
                out_raw(&resp);
            }
        }
    }
    // stdin 关闭：等在途请求收尾（最多 5s），退出
    let drain = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(resp) = done_rx.recv().await {
            out_raw(&resp);
            if done_rx.len() == 0 {
                break;
            }
        }
    })
    .await;
    let _ = drain;
    let _ = browser.close().await;
    // 自动创建的临时 profile 必须清掉：render 助手每 5 分钟空闲回收一次，
    // 否则容器 /tmp 会持续累积 Chromium 配置目录。
    if auto_profile {
        let _ = std::fs::remove_dir_all(&user_data);
    }
}

/// 单次渲染：goto → settle → 抽取。
async fn render(page: &chromiumoxide::Page, req: &Req) -> Resp {
    let mode = req
        .mode
        .as_deref()
        .filter(|m| ["text", "markdown", "links"].contains(m))
        .unwrap_or("html")
        .to_string();
    let resp = tokio::time::timeout(RENDER_TIMEOUT, async {
        if let Err(e) = page.goto(&req.url).await {
            return Err(format!("导航失败: {e}"));
        }
        settle(page).await?;
        let expr = build_expr(&mode, req.selector.as_deref());
        let result = page
            .evaluate(expr)
            .await
            .map_err(|e| format!("抽取失败: {e}"))?;
        let value = result
            .into_value::<String>()
            .map_err(|e| format!("抽取结果转换失败: {e}"))?;
        Ok(value)
    })
    .await;
    match resp {
        Ok(Ok(data)) => Resp { id: req.id, ok: true, data: Some(data), error: None },
        Ok(Err(e)) => Resp { id: req.id, ok: false, data: None, error: Some(e) },
        Err(_) => Resp { id: req.id, ok: false, data: None, error: Some("渲染超时".into()) },
    }
}

/// 等页面「看起来稳定」：正文长度连续两轮不变即认为加载完成（同 page-render.cjs settle）。
async fn settle(page: &chromiumoxide::Page) -> Result<(), String> {
    let mut last: i64 = -1;
    for _ in 0..SETTLE_POLLS {
        tokio::time::sleep(SETTLE_INTERVAL).await;
        let len = page
            .evaluate("(document.body&&document.body.innerText||'').length")
            .await
            .ok()
            .and_then(|r| r.into_value::<i64>().ok());
        match len {
            Some(n) if n > 0 && n == last => break,
            Some(n) => last = n,
            None => break,
        }
    }
    Ok(())
}

/// 写一行响应到 stdout（结构化）。
fn out(resp: &Resp) {
    println!("{}", serde_json::to_string(resp).unwrap_or_default());
}

/// 写一行原始字符串到 stdout。
fn out_raw(line: &str) {
    println!("{line}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

/// 致命错误：写 stderr 后退出（协议里无法表达「服务未起来」）。
fn die(msg: &str) -> ! {
    eprintln!("astorm-render: {msg}");
    std::process::exit(1);
}

/// 定位 Chromium：ASTORM_CHROME_BIN → PATH（chromium / chromium-browser / google-chrome）。
fn resolve_chrome() -> String {
    if let Ok(p) = std::env::var("ASTORM_CHROME_BIN") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    for name in ["chromium", "chromium-browser", "google-chrome"] {
        if which(name) {
            return name.to_string();
        }
    }
    die("未找到 Chromium：请设置 ASTORM_CHROME_BIN 或安装 chromium");
}

/// 极简 which（PATH 上可执行探测）。
fn which(bin: &str) -> bool {
    let Ok(paths) = std::env::var("PATH") else { return false };
    for dir in std::env::split_paths(&paths) {
        let cand = dir.join(bin);
        if cand.is_file() {
            return true;
        }
    }
    let _ = Stdio::null();
    false
}
