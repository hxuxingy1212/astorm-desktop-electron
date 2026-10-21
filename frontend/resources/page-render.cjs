/**
 * 页面渲染**常驻服务**：复用 Electron 自带 Chromium，用一个隐藏窗口池渲染网页，
 * 供 Rust 侧 `websearch`（百度/谷歌）与 `webfetch` 共用。
 *
 * 与 Rust `WebRender` 通过 **stdin/stdout 行分隔 JSON** 通信：
 *   启动  `{"ready":true,"concurrency":N}`
 *   请求  `{"id":1,"mode":"html|text|markdown|links","url":"https://…","selector":"main"}`（selector 可选）
 *   响应  `{"id":1,"ok":true,"data":"…"}` / `{"id":1,"ok":false,"error":"…"}`
 * stdin 关闭（EOF）→ 退出。超时/被杀由 Rust 侧负责（kill_on_drop）。
 *
 * 内容过滤/加工（参考 obscura 的 `--dump`）：
 *   - `text`：可读正文——跳过 `script/style/noscript/template/nav/header/footer/aside/iframe/svg/canvas`，
 *     块级换行 + 空白归一；
 *   - `markdown`：DOM→Markdown（标题/段落/加粗斜体/行内码/链接/图片/列表/引用/代码块/表格），
 *     剥离脚本样式与 1px 跟踪像素；
 *   - `links`：页面内全部 http(s) 链接（去重）；
 *   - `selector`：把处理范围收窄到某个 CSS 选择器（跳过导航/侧栏/页脚）。
 *
 * 参数：`--concurrency N`（窗口池大小，默认 2）、`--profile <dir>`（独立 userData）。
 */
const path = require('path')
const readline = require('readline')
const { app, BrowserWindow } = require('electron')

const UA =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 ' +
  '(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'

const argv = process.argv.slice(2)
function argVal(name, def) {
  const i = argv.indexOf(name)
  return i >= 0 && argv[i + 1] !== undefined ? argv[i + 1] : def
}
const CONCURRENCY = Math.max(1, parseInt(argVal('--concurrency', '2'), 10) || 2)
const PROFILE = argVal('--profile', '')

if (PROFILE) {
  try {
    app.setPath('userData', PROFILE)
  } catch {
    /* ignore */
  }
}
app.commandLine.appendSwitch('disable-gpu')
app.commandLine.appendSwitch('disable-dev-shm-usage')
app.commandLine.appendSwitch('lang', 'zh-CN')
app.commandLine.appendSwitch('disable-blink-features', 'AutomationControlled')
app.on('window-all-closed', () => {})

function out(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n')
}

function createWindow() {
  const win = new BrowserWindow({
    show: false,
    width: 1280,
    height: 900,
    webPreferences: {
      offscreen: true,
      contextIsolation: false,
      sandbox: false,
      nodeIntegration: false,
      images: false,
      javascript: true,
      preload: path.join(__dirname, 'stealth-preload.cjs')
    }
  })
  win.webContents.setUserAgent(UA)
  win.webContents.setAudioMuted(true)
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  return win
}

/** 等页面「看起来稳定」：did-finish-load + 轮询正文长度静止。 */
async function settle(win) {
  const loaded = new Promise((resolve) => win.webContents.once('did-finish-load', resolve))
  await Promise.race([loaded, new Promise((r) => setTimeout(r, 2500))])
  let last = -1
  for (let i = 0; i < 12; i++) {
    await new Promise((r) => setTimeout(r, 250))
    try {
      const len = await win.webContents.executeJavaScript(
        '(document.body&&document.body.innerText||"").length'
      )
      if (len > 0 && len === last) break
      last = len
    } catch {
      break
    }
  }
}

/** 在页面世界运行的抽取器（参考 obscura 的 text/markdown/links dump）。 */
const EXTRACT_SRC = `
  const SKIP = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','NAV','HEADER','FOOTER','ASIDE','IFRAME','SVG','CANVAS','LINK','META']);
  const BLOCK = new Set(['P','DIV','SECTION','ARTICLE','MAIN','UL','OL','LI','TABLE','THEAD','TBODY','TR','H1','H2','H3','H4','H5','H6','BLOCKQUOTE','PRE','HR','FIGCAPTION','DL','DT','DD']);
  const abs = (u) => { try { return u ? new URL(u, location.href).href : '' } catch { return '' } };
  const beacon = (el) => {
    const w = parseInt(el.getAttribute('width')||'0',10), h = parseInt(el.getAttribute('height')||'0',10);
    if ((w && w<=2) || (h && h<=2)) return true;
    const src = (el.getAttribute('src')||'').toLowerCase();
    return /(beacon|pixel|track|analytics|\\.gif$)/.test(src) && (w<=1||h<=1);
  };
  function toText(root){
    const out=[];
    (function walk(node){
      if(node.nodeType===3){ out.push(node.nodeValue); return; }
      if(node.nodeType!==1) return;
      const tag=node.tagName;
      if(SKIP.has(tag)) return;
      if(tag==='BR'){ out.push('\\n'); return; }
      const block=BLOCK.has(tag);
      if(block) out.push('\\n');
      for(const c of node.childNodes) walk(c);
      if(block) out.push('\\n');
    })(root);
    return out.join('').replace(/[ \\t\\u00a0]+/g,' ').replace(/ *\\n */g,'\\n').replace(/\\n{3,}/g,'\\n\\n').trim();
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
        if(tag==='BR'){ s+='  \\n'; continue; }
        if(tag==='STRONG'||tag==='B'){ s+='**'+inline(c).trim()+'**'; continue; }
        if(tag==='EM'||tag==='I'){ s+='*'+inline(c).trim()+'*'; continue; }
        if(tag==='CODE'){ s+='\\u0060'+c.textContent.replace(/\\u0060/g,'')+'\\u0060'; continue; }
        if(tag==='A'){ const href=abs(c.getAttribute('href')); const t=inline(c).replace(/\\s+/g,' ').trim(); s+= href? '['+(t||href)+']('+href+')' : t; continue; }
        if(tag==='IMG'){ if(!beacon(c)){ const src=abs(c.getAttribute('src')); const alt=(c.getAttribute('alt')||'').replace(/\\s+/g,' '); if(src) s+='!['+alt+']('+src+')'; } continue; }
        s+=inline(c);
      }
      return s;
    }
    function table(node){
      const rows=[...node.querySelectorAll('tr')];
      const grid=rows.map(r=>[...r.children].filter(x=>x.tagName==='TD'||x.tagName==='TH').map(x=>inline(x).replace(/\\n/g,' ').trim()));
      if(!grid.length) return;
      const cols=Math.max(...grid.map(r=>r.length));
      const pad=(r)=>[...r, ...Array(cols-r.length).fill('')];
      out.push('\\n| '+pad(grid[0]).join(' | ')+' |');
      out.push('| '+Array(cols).fill('---').join(' | ')+' |');
      for(let i=1;i<grid.length;i++) out.push('| '+pad(grid[i]).join(' | ')+' |');
      out.push('');
    }
    function block(node){
      for(const c of node.childNodes){
        if(c.nodeType===3){ const t=c.nodeValue.replace(/\\s+/g,' ').trim(); if(t) out.push(t); continue; }
        if(c.nodeType!==1) continue;
        const tag=c.tagName;
        if(SKIP.has(tag)) continue;
        if(/^H[1-6]$/.test(tag)){ out.push('\\n'+'#'.repeat(+tag[1])+' '+inline(c).replace(/\\s+/g,' ').trim()+'\\n'); continue; }
        if(tag==='P'){ const t=inline(c).replace(/\\s+/g,' ').trim(); if(t) out.push('\\n'+t+'\\n'); continue; }
        if(tag==='UL'||tag==='OL'){ let i=1; for(const li of c.children){ if(li.tagName!=='LI') continue; const t=inline(li).replace(/\\s+/g,' ').trim(); if(t) out.push((tag==='OL'? (i++)+'. ' : '- ')+t); } out.push(''); continue; }
        if(tag==='BLOCKQUOTE'){ const t=inline(c).replace(/\\s+/g,' ').trim(); if(t) out.push('\\n> '+t+'\\n'); continue; }
        if(tag==='PRE'){ out.push('\\n\\u0060\\u0060\\u0060\\n'+c.textContent.replace(/\\n+$/,'')+'\\n\\u0060\\u0060\\u0060\\n'); continue; }
        if(tag==='HR'){ out.push('\\n---\\n'); continue; }
        if(tag==='TABLE'){ table(c); continue; }
        if(BLOCK.has(tag)){ block(c); continue; }
        const t=inline(c).replace(/\\s+/g,' ').trim(); if(t) out.push(t);
      }
    }
    block(root);
    return out.join('\\n').replace(/\\n{3,}/g,'\\n\\n').trim();
  }
  function extractLinks(root){
    const set=new Set();
    for(const a of root.querySelectorAll('a[href]')){ const h=abs(a.getAttribute('href')); if(/^https?:/i.test(h)) set.add(h); }
    return [...set].join('\\n');
  }
  // 主体内容根（无 selector 时）：优先语义/约定容器（article/main/[role=main]/常见正文类），
  // 命中即用（这类容器本身就是正文，避免导航/侧栏/页脚）；都没有则退回 body（同 obscura，按标签剥离）。
  // 刻意不做「密度打分」：对视频播放器/浮层等噪声块易误判，保守优先。
  function contentRoot(){
    const CAND=['article','main','[role=main]','#content','#main','#body','.content','.article','.post','.entry-content','.markdown-body','.article-content'];
    for(const s of CAND){ let el=null; try{ el=document.querySelector(s); }catch{} if(el){ const n=(el.innerText||'').trim().length; if(n>200) return el; } }
    return document.body || document.documentElement;
  }
`

/** 组装注入到页面的抽取表达式。 */
function buildExpr(mode, selector) {
  const sel = selector ? JSON.stringify(selector) : 'null'
  const m = JSON.stringify(mode)
  return `(function(){ ${EXTRACT_SRC}
    const MODE=${m};
    if(MODE==='html') return document.documentElement.outerHTML;
    const SEL=${sel};
    // 有 selector 用之；否则用主体内容启发式收窄（text/markdown/links），尽量减少导航噪声
    const root = SEL ? document.querySelector(SEL) : contentRoot();
    if(!root) return '';
    if(MODE==='links') return extractLinks(root);
    if(MODE==='markdown') return toMarkdown(root);
    return toText(root);
  })()`
}

async function renderOn(win, mode, url, selector) {
  await win.loadURL(url, { extraHeaders: 'Accept-Language: zh-CN,zh;q=0.9,en;q=0.8' })
  await settle(win)
  return String(await win.webContents.executeJavaScript(buildExpr(mode, selector)))
}

app.whenReady().then(() => {
  if (process.platform === 'darwin' && app.dock) app.dock.hide()

  const pool = Array.from({ length: CONCURRENCY }, () => ({ win: createWindow(), busy: false }))
  const waiters = []

  function acquire() {
    const free = pool.find((s) => !s.busy)
    if (free) {
      free.busy = true
      return Promise.resolve(free)
    }
    return new Promise((resolve) => waiters.push(resolve))
  }
  function release(slot) {
    const next = waiters.shift()
    if (next) next(slot)
    else slot.busy = false
  }

  async function handle(req) {
    const slot = await acquire()
    try {
      const mode = ['text', 'markdown', 'links'].includes(req.mode) ? req.mode : 'html'
      const data = await renderOn(slot.win, mode, req.url, req.selector)
      return { id: req.id, ok: true, data }
    } catch (e) {
      return { id: req.id, ok: false, error: String((e && e.message) || e) }
    } finally {
      release(slot)
    }
  }

  const rl = readline.createInterface({ input: process.stdin })
  rl.on('line', (line) => {
    const text = line.trim()
    if (!text) return
    let req
    try {
      req = JSON.parse(text)
    } catch {
      return
    }
    if (!req || !req.url) return
    handle(req).then(out)
  })
  rl.on('close', () => app.exit(0))

  out({ ready: true, concurrency: CONCURRENCY })
})
