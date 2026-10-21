import { ipcMain, webContents, type WebContents, type WebFrameMain } from 'electron'

/**
 * 内置浏览器「选择元素」主进程侧:
 *
 *   1. findFrame  按 URL 在「宿主页 frame 子树 + 其余 webContents」中定位内置浏览器 iframe。
 *      同进程跨域 iframe 出现在 sender.mainFrame.framesInSubtree 里;独立进程(guest/OOPIF)
 *      则表现为另一个 webContents 的主 frame —— 两条路径都能拿到 WebFrameMain 并注入。
 *   2. PICKER_SCRIPT 注入被测页面:Shadow DOM 覆盖层做悬停高亮 + 深色信息卡片(元素名/尺寸/
 *      计算样式/组件与源码位置),捕获阶段拦截鼠标事件(避免点链接跳转),点击后 resolve 出
 *      鼠标事件(避免点链接跳转),点击后 resolve 出元素信息;Esc / cancel 注入 resolve null。
 *      executeJavaScript 会等待脚本返回的 Promise,点击是「稍后发生」的,故用此形态。
 *
 * 元素信息采集:id / XPath / class 列表 / 标签名 + CSS 选择器 + 文本 + 最近组件
 * (组件探测逻辑移植自 vite-plugin-inspector 的 detector.js:React fiber / Vue3
 *  __vueParentComponent / Vue2 __vue__;生产构建通常无源码位置,按设计降级)。
 */

/** 悬停卡片的一行(值与页面内卡片同源,渲染层直接展示,不再二次格式化) */
export interface PickCardRow {
  /** 字段名(color/background/font/padding/margin/component/path/id/xpath) */
  key: string
  value: string
  /** 需要渲染色块时的颜色值(rgb/rgba) */
  swatch?: string
  /** 该行前插入分隔线 */
  sep?: boolean
  /** 值用高亮色(源码路径) */
  path?: boolean
}

/** 拾取结果(与 preload/index.d.ts 的 PickedElementData 结构一致) */
export interface ElementPickResult {
  ok: boolean
  data?: {
    url: string
    tagName: string
    id: string
    classList: string[]
    xpath: string
    cssSelector: string
    text: string
    component?: { name: string; framework: 'vue2' | 'vue3' | 'react'; file?: string; line?: number; column?: number }
    /** 悬停卡片:与页面内高亮卡片同一模型 */
    card: { name: string; size: string; rows: PickCardRow[] }
  }
  /** 用户主动取消(Esc / 再次点击按钮) */
  canceled?: boolean
  /** 'NO_FRAME' | 'EMPTY_URL' 为约定码,其余为注入/页面异常原文 */
  message?: string
}

/** 去尾斜杠 */
function trimSlash(url: string): string {
  return url.replace(/\/$/, '')
}

/** 宽松归一:去掉 query / hash / 尾斜杠(应对重定向后的 URL 差异) */
function looseUrl(url: string): string {
  return trimSlash(url.replace(/[#?].*$/, ''))
}

function sameOrigin(a: string, b: string): boolean {
  try {
    return new URL(a).origin === new URL(b).origin
  } catch {
    return false
  }
}

function frameUrl(frame: WebFrameMain): string {
  try {
    return frame.url
  } catch {
    return ''
  }
}

/** webContents 的主 frame + 子树全部 frame(已销毁时返回空) */
function framesOf(wc: WebContents): WebFrameMain[] {
  if (wc.isDestroyed()) return []
  try {
    return [wc.mainFrame, ...wc.mainFrame.framesInSubtree].filter((f) => f && !f.isDestroyed())
  } catch {
    return []
  }
}

/**
 * 定位内置浏览器 iframe 的 frame:
 *   精确匹配 URL → 宽松匹配 → 同源唯一子 frame 兜底。
 * 宿主页自身(应用渲染进程)的 frame 永不注入。
 */
function findFrame(sender: WebContents, url: string): WebFrameMain | null {
  const wanted = url.trim()
  if (!wanted) return null

  const candidates: WebFrameMain[] = []
  for (const wc of [sender, ...webContents.getAllWebContents()]) {
    if (wc.isDestroyed()) continue
    for (const frame of framesOf(wc)) {
      if (frame === sender.mainFrame) continue
      candidates.push(frame)
    }
  }

  const exact = candidates.find((f) => trimSlash(frameUrl(f)) === trimSlash(wanted))
  if (exact) return exact

  const loose = looseUrl(wanted)
  const near = candidates.find((f) => looseUrl(frameUrl(f)) === loose)
  if (near) return near

  // 兜底:重定向/URL 规范化导致对不上时,取宿主页子树中同源的唯一子 frame
  const own = framesOf(sender).filter((f) => f !== sender.mainFrame && sameOrigin(frameUrl(f), wanted))
  return own.length === 1 ? own[0] : null
}

/**
 * 注入被测页面的拾取脚本。整体为 IIFE,返回 Promise(executeJavaScript 会等待):
 *   点击元素 → resolve 元素信息;Esc / 取消 → resolve null。
 * 用 String.raw 保留正则里的 \s 等转义;除 __ASTORM_OPTIONS__ 占位符外无任何 ${ 插值。
 */
const PICKER_SCRIPT = String.raw`(function () {
  var prev = window.__ASTORM_PICK__;
  if (prev && typeof prev.cancel === 'function') { try { prev.cancel(); } catch (e) {} }

  var OPTIONS = __ASTORM_OPTIONS__;
  var L = OPTIONS.labels || {};
  var HINT = OPTIONS.hint || '';
  var MAX_TEXT = 120;
  // React fiber.tag:0=函数组件 1=类组件 2=Indeterminate(React18 及以前;19 起废弃)
  // 11=forwardRef 14=memo(带 compare) 15=SimpleMemoComponent(React.memo 无 compare,type 即内层函数)
  // 16=Lazy 17=IncompleteClass 28=IncompleteFunctionComponent(React19 新增,异步/use 场景)
  // 注意 26=HostHoistable / 27=HostSingleton 是宿主节点,绝不能列入
  var COMP_TAGS = [0, 1, 2, 11, 14, 15, 16, 17, 28];
  var raf = 0;
  var pending = null;
  var current = null;
  var host = null;
  var box = null;
  var card = null;
  /** 卡片内容按元素缓存:同一元素重复 hover 不重算 */
  var cachedEl = null;
  var done = false;
  var settle = null;

  function style(el, props) {
    for (var k in props) { el.style.setProperty(k, props[k]); }
    return el;
  }

  // ---------- 采集 ----------

  function cssEscape(s) {
    if (window.CSS && CSS.escape) return CSS.escape(String(s));
    return String(s).replace(/[^a-zA-Z0-9_-]/g, function (c) { return '\\' + c; });
  }

  function xpathOf(el) {
    var parts = [];
    var cur = el;
    while (cur && cur.nodeType === 1) {
      var tag = cur.nodeName.toLowerCase();
      if (tag === 'html') { parts.unshift('html'); break; }
      var index = 1;
      var sib = cur.previousElementSibling;
      while (sib) {
        if (sib.nodeName === cur.nodeName) index++;
        sib = sib.previousElementSibling;
      }
      parts.unshift(tag + '[' + index + ']');
      cur = cur.parentElement;
    }
    return '/' + parts.join('/');
  }

  function selectorOf(el) {
    if (el.id) {
      var byId = '#' + cssEscape(el.id);
      try { if (document.querySelectorAll(byId).length === 1) return byId; } catch (e) {}
    }
    var parts = [];
    var cur = el;
    while (cur && cur.nodeType === 1) {
      var tag = cur.nodeName.toLowerCase();
      if (tag === 'html') { parts.unshift('html'); break; }
      if (cur.id) { parts.unshift('#' + cssEscape(cur.id)); break; }
      var seg = tag;
      if (cur.classList && cur.classList.length) {
        for (var i = 0; i < cur.classList.length; i++) { seg += '.' + cssEscape(cur.classList[i]); }
      }
      var parent = cur.parentElement;
      if (parent) {
        var same = [];
        for (var j = 0; j < parent.children.length; j++) {
          if (parent.children[j].nodeName === cur.nodeName) same.push(parent.children[j]);
        }
        if (same.length > 1) seg += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
      }
      parts.unshift(seg);
      cur = parent;
    }
    return parts.join(' > ');
  }

  function textOf(el) {
    var t = '';
    try { t = el.innerText || el.textContent || ''; } catch (e) { t = ''; }
    t = String(t).replace(/\s+/g, ' ').trim();
    return t.length > MAX_TEXT ? t.slice(0, MAX_TEXT) + '\u2026' : t;
  }

  // ---------- 组件探测(移植自 vite-plugin-inspector)----------

  function splitFile(str) {
    if (!str) return null;
    var m = String(str).match(/^(.+):([0-9]+):([0-9]+)$/);
    if (m && m[1].length > 2) return { file: m[1], line: parseInt(m[2], 10), column: parseInt(m[3], 10) };
    m = String(str).match(/^(.+):([0-9]+)$/);
    if (m && m[1].length > 2) return { file: m[1], line: parseInt(m[2], 10) };
    return { file: String(str) };
  }

  /**
   * 名字是否可信:打包压缩后函数名会变成 1~2 个字符(压缩器用 a-zA-Z0-9_$ 混合字母表,
   * 如 Zt),displayName 又常被移除,这种名字显示在 chip 上等于乱码,判为不可信。
   * 不可信时 reactComponent 视为探测失败(不继续上溯,避免把元素错归到外层组件),
   * 由调用方回退到标签名。Vue 的名字来自 SFC 编译期字符串(不参与压缩),不走这里。
   */
  function trustedName(name) {
    if (!name || typeof name !== 'string') return '';
    return /^[A-Za-z0-9_$]{1,2}$/.test(name) ? '' : name;
  }

  function reactName(type) {
    if (typeof type === 'string') return type;
    if (type.displayName) return trustedName(type.displayName);
    if (typeof type === 'function') return trustedName(type.name);
    try {
      if (type.$$typeof === Symbol.for('react.forward_ref')) {
        var render = type.render;
        return render ? trustedName(render.displayName || render.name) : '';
      }
      if (type.$$typeof === Symbol.for('react.memo')) {
        var inner = type.type;
        return inner ? trustedName(inner.displayName || inner.name) : '';
      }
    } catch (e) {}
    return '';
  }

  /**
   * fiber 自身没有 _debugSource 时向下找(深度 ≤5)。
   * React 只在 dev 的 JSX dev transform(jsxDEV 带 source 参数,如 @vitejs/plugin-react)
   * 才给 fiber 挂 _debugSource;某些包装层 fiber 上没有,真正的宿主元素/子组件才有。
   */
  function findReactDebugSource(fiber, depth) {
    if (!fiber || depth > 5) return null;
    if (fiber._debugSource) return fiber._debugSource;
    if (fiber.child) {
      var fromChild = findReactDebugSource(fiber.child, depth + 1);
      if (fromChild) return fromChild;
    }
    if (fiber.sibling) {
      var fromSibling = findReactDebugSource(fiber.sibling, depth + 1);
      if (fromSibling) return fromSibling;
    }
    return null;
  }

  /**
   * React 19 移除了 _debugSource:改成在 jsxDEV 调用点捕获一个 Error 挂在 fiber._debugStack 上
   * (React 内部 getOwnerStackByFiberInDev 也是这么取的,/react-stack-top-frame/ 前缀即其手动
   * 加的可识别标记;JSX 编译期带的 source 对象被 React19 丢弃,ReactElement() 已无 source 形参)。
   * 栈形如:
   *   Error: react-stack-top-frame
   *       at jsxDEV (...react-dom 内部,构造点)
   *       at App (/src/main.jsx:12:9)        ← 真实调用点,即该组件 JSX 出现的位置
   *       at react_stack_bottom_frame (...)
   * 所以跳过前两行,取第一条带 file:line:col 的帧;碰到底部标记就停。
   *
   * 注意:这是运行时 Error,浏览器不会对原始 .stack 套 source map,因此行号/列号是
   * **服务端返回的那个模块**(dev 下 = Vite 转换产物)里的位置,文件名也是 URL 而非磁盘路径。
   * 与 React 18 的 _debugSource 语义一致(都用「使用点」而非组件定义点),但坐标系不同。
   */
  function sourceFromDebugStack(err) {
    if (!err || !err.stack) return null;
    var prevPrepare = Error.prepareStackTrace;
    var raw = '';
    try {
      Error.prepareStackTrace = undefined;   // 装了 source-map-support 等会改写 stack 取值
      raw = err.stack || '';
    } catch (e) {
      raw = '';
    }
    Error.prepareStackTrace = prevPrepare;
    if (!raw) return null;
    var lines = raw.split('\n');
    for (var i = 2; i < lines.length; i++) {
      var line = lines[i].replace(/\s+$/, '');
      if (line.indexOf('react_stack_bottom_frame') >= 0) break;
      // 跳过 React 自身/构建工具内部帧,避免报出 react-dom 的行号
      if (/react-dom|react\.development|react\.production|react-refresh|jsx-dev-runtime/.test(line)) continue;
      var m = line.match(/\(?([^()\s]+):([0-9]+):([0-9]+)\)?$/);
      if (m) return { fileName: m[1], lineNumber: parseInt(m[2], 10), columnNumber: parseInt(m[3], 10) };
    }
    return null;
  }

  function reactComponent(el) {
    try {
      var fiber = null;
      for (var key in el) {
        if (key.indexOf('__reactFiber$') === 0) { fiber = el[key]; break; }
      }
      var f = fiber;
      while (f) {
        var t = f.type;
        if (COMP_TAGS.indexOf(f.tag) >= 0 && t && (typeof t === 'function' || typeof t === 'object')) {
          var name = reactName(t);
          if (!name) return null;
          // React18:_debugSource;React19:fibers._debugStack
          var src = f._debugSource || findReactDebugSource(f, 0) || sourceFromDebugStack(f._debugStack);
          var out = { name: name, framework: 'react' };
          if (src) {
            if (src.fileName) out.file = src.fileName;
            if (src.lineNumber != null) out.line = src.lineNumber;
            if (src.columnNumber != null) out.column = src.columnNumber;
          }
          return out;
        }
        f = f.return;
      }
      return null;
    } catch (e) { return null; }
  }

  function vue3Component(el) {
    try {
      var cur = el;
      while (cur && cur !== document.documentElement) {
        var vnode = cur.__vueParentComponent;
        if (vnode && vnode.type) {
          var t = vnode.type;
          var name = t.name || t.__name || '';
          if (!name) return null;
          var out = { name: name, framework: 'vue3' };
          var parsed = splitFile(t.__file);
          if (parsed) {
            out.file = parsed.file;
            if (parsed.line != null) out.line = parsed.line;
            if (parsed.column != null) out.column = parsed.column;
          }
          return out;
        }
        cur = cur.parentElement;
      }
      return null;
    } catch (e) { return null; }
  }

  function vue2Component(el) {
    try {
      var cur = el;
      while (cur && cur !== document.documentElement) {
        var vm = cur.__vue__;
        if (vm && vm.$options) {
          var name = vm.$options.name || '';
          if (!name) return null;
          var out = { name: name, framework: 'vue2' };
          var parsed = splitFile(vm.$options.__file);
          if (parsed) {
            out.file = parsed.file;
            if (parsed.line != null) out.line = parsed.line;
          }
          return out;
        }
        cur = cur.parentElement;
      }
      return null;
    } catch (e) { return null; }
  }

  function componentOf(el) {
    return reactComponent(el) || vue3Component(el) || vue2Component(el) || null;
  }

  function describe(el) {
    var s = el.nodeName ? el.nodeName.toLowerCase() : '?';
    if (el.id) s += '#' + el.id;
    else if (el.classList && el.classList.length) s += '.' + Array.prototype.slice.call(el.classList).join('.');
    return s;
  }

  function collect(el) {
    var classes = [];
    try { classes = Array.prototype.slice.call(el.classList || []); } catch (e) { classes = []; }
    var data = {
      url: location.href,
      tagName: el.nodeName ? el.nodeName.toLowerCase() : '',
      id: el.id || '',
      classList: classes,
      xpath: xpathOf(el),
      cssSelector: selectorOf(el),
      text: textOf(el)
    };
    var component = componentOf(el);
    if (component) data.component = component;
    // 悬停卡片数据(与页面内卡片同源),供发送框弹层原样展示
    data.card = cardModel(el);
    return data;
  }

  // ---------- 覆盖层 ----------

  /** HTML 转义:卡片内容里的文案/属性值(元素名、路径等)都过一遍 */
  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  /** 色块背景:只放行 calc 返回值里的字符,避免把任意计算值塞进 style */
  function safeColor(v) {
    return /^[a-zA-Z0-9(),.%\s/-]*$/.test(String(v || '')) ? String(v) : 'transparent';
  }

  /** rgb()/rgba() → #RRGGBB(带透明度时附百分比),与 Chrome 测量浮层一致的展示习惯 */
  function toHex(color) {
    var m = String(color || '').match(/^rgba?\(([^)]+)\)$/i);
    if (!m) return String(color || '');
    var p = m[1].split(',').map(function (v) { return parseFloat(v); });
    var alpha = p.length > 3 ? p[3] : 1;
    if (alpha === 0) return 'transparent';
    var hex = '#' + [p[0], p[1], p[2]].map(function (v) {
      var h = Math.max(0, Math.min(255, Math.round(v || 0))).toString(16);
      return h.length === 1 ? '0' + h : h;
    }).join('').toUpperCase();
    return alpha < 1 ? hex + ' ' + Math.round(alpha * 100) + '%' : hex;
  }

  /** padding/margin 四值按 CSS 惯例折叠:全等 1 值、上下+左右 2 值、否则 4 值 */
  function sideShorthand(cs, prop) {
    var t = Math.round(parseFloat(cs[prop + 'Top']) || 0);
    var r = Math.round(parseFloat(cs[prop + 'Right']) || 0);
    var b = Math.round(parseFloat(cs[prop + 'Bottom']) || 0);
    var l = Math.round(parseFloat(cs[prop + 'Left']) || 0);
    if (t === r && r === b && b === l) return t + 'px';
    if (t === b && r === l) return t + 'px ' + r + 'px';
    return t + 'px ' + r + 'px ' + b + 'px ' + l + 'px';
  }

  /** 字体摘要:字号 + 字族首项 + 粗体/斜体标记 */
  function fontSummary(cs) {
    var family = String(cs.fontFamily || '').split(',')[0].trim().replace(/^["']|["']$/g, '');
    var flags = '';
    if (parseInt(cs.fontWeight, 10) >= 600) flags += ' \u00b7 ' + (L.bold || 'bold');
    if (cs.fontStyle === 'italic') flags += ' \u00b7 ' + (L.italic || 'italic');
    return cs.fontSize + ' ' + family + flags;
  }

  function cardRow(label, valueHtml, mono) {
    return (
      '<div class="r"><span class="k">' + esc(label) + '</span>' +
      '<span class="v' + (mono ? ' mono' : '') + '">' + valueHtml + '</span></div>'
    );
  }

  function swatch(color) {
    return '<i class="sw" style="background:' + safeColor(color) + '"></i>';
  }

  function labelOf(key) {
    if (key === 'id') return L.id || 'id';
    return L[key] || key;
  }

  /**
   * 卡片模型:同一份数据既渲染页面内卡片、也随拾取结果回传渲染层,
   * 保证「发送框里悬停弹出的卡片」与「页面里高亮卡片」内容逐字一致。
   */
  function cardModel(el) {
    var cs = window.getComputedStyle(el);
    var r = el.getBoundingClientRect();
    var rows = [];
    rows.push({ key: 'color', value: toHex(cs.color), swatch: cs.color });
    rows.push({ key: 'background', value: toHex(cs.backgroundColor), swatch: cs.backgroundColor });
    rows.push({ key: 'font', value: fontSummary(cs) });
    rows.push({ key: 'padding', value: sideShorthand(cs, 'padding') });
    rows.push({ key: 'margin', value: sideShorthand(cs, 'margin') });
    var c = componentOf(el);
    if (c) {
      rows.push({ key: 'component', value: c.name + ' (' + c.framework + ')', sep: true });
      if (c.file) {
        var loc = c.file;
        if (c.line != null) loc += ':' + c.line;
        if (c.column != null) loc += ':' + c.column;
        rows.push({ key: 'path', value: loc, path: true });
      }
    }
    if (el.id) rows.push({ key: 'id', value: el.id });
    rows.push({ key: 'xpath', value: xpathOf(el) });
    return {
      name: describe(el),
      size: Math.round(r.width) + '\u00d7' + Math.round(r.height),
      rows: rows
    };
  }

  /** 页面内卡片 DOM(内容来自 cardModel,与渲染层弹层同源) */
  function cardHtml(model) {
    var out =
      '<div class="hd"><span class="nm">' + esc(model.name) + '</span>' +
      '<span class="sz">' + esc(model.size) + '</span></div>';
    for (var i = 0; i < model.rows.length; i++) {
      var row = model.rows[i];
      if (row.sep) out += '<div class="sep"></div>';
      var value = esc(row.value);
      if (row.swatch) value = swatch(row.swatch) + value;
      if (row.path) value = '<span class="path">' + value + '</span>';
      out += cardRow(labelOf(row.key), value, true);
    }
    return out;
  }

  function targetOf(e) {
    var path = e.composedPath ? e.composedPath() : null;
    if (path) {
      for (var i = 0; i < path.length; i++) {
        var node = path[i];
        if (node && node.nodeType === 1 && node !== host) return node;
      }
    }
    return document.elementFromPoint(e.clientX, e.clientY);
  }

  /** 卡片跟随元素:优先贴元素下方,放不下翻到上方,都放不下则夹在视口内 */
  function placeCard(el) {
    var r = el.getBoundingClientRect();
    var w = card.offsetWidth;
    var h = card.offsetHeight;
    var gap = 8;
    var left = r.left;
    if (left + w > window.innerWidth - 8) left = window.innerWidth - w - 8;
    if (left < 8) left = 8;
    var top;
    if (r.bottom + gap + h <= window.innerHeight - 8) top = r.bottom + gap;
    else if (r.top - gap - h >= 8) top = r.top - gap - h;
    else top = Math.max(8, Math.min(window.innerHeight - h - 8, r.top));
    style(card, { left: left + 'px', top: top + 'px' });
  }

  function highlight(el) {
    if (!el || el === host || el.nodeType !== 1) {
      current = null;
      cachedEl = null;
      if (box) box.style.setProperty('display', 'none');
      if (card) card.style.setProperty('display', 'none');
      return;
    }
    current = el;
    var r = el.getBoundingClientRect();
    style(box, {
      display: 'block',
      left: r.left + 'px',
      top: r.top + 'px',
      width: r.width + 'px',
      height: r.height + 'px'
    });
    // 同一元素不重算内容(getComputedStyle + 组件探测 + xpath 都不便宜),滚动时只重定位
    if (el !== cachedEl) {
      card.innerHTML = cardHtml(cardModel(el));
      cachedEl = el;
    }
    card.style.setProperty('display', 'block');
    placeCard(el);
  }

  function buildOverlay() {
    host = document.createElement('div');
    style(host, { position: 'fixed', inset: '0', 'z-index': '2147483647', 'pointer-events': 'none' });
    var shadow = host.attachShadow({ mode: 'open' });

    box = style(document.createElement('div'), {
      position: 'fixed',
      display: 'none',
      'pointer-events': 'none',
      'box-sizing': 'border-box',
      border: '2px solid #1a73e8',
      background: 'rgba(26, 115, 232, 0.08)',
      'border-radius': '2px'
    });

    card = style(document.createElement('div'), {
      position: 'fixed',
      display: 'none',
      'pointer-events': 'none'
    });
    card.className = 'c';
    card.style.setProperty('position', 'fixed');

    // 卡片内部样式放 shadow 内 <style>,比逐节点内联清爽;外层定位仍走 style()
    var css = document.createElement('style');
    css.textContent =
      '.c{position:fixed;display:none;pointer-events:none;box-sizing:border-box;min-width:190px;max-width:380px;' +
      'background:#202124;color:#e8eaed;border-radius:8px;padding:8px 10px;font-family:system-ui,-apple-system,"Segoe UI",sans-serif;' +
      'font-size:11px;line-height:16px;box-shadow:0 8px 24px rgba(0,0,0,.45)}' +
      '.c .hd{display:flex;align-items:baseline;gap:10px;margin-bottom:4px}' +
      '.c .nm{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:12px;font-weight:600;color:#fff;' +
      'overflow:hidden;text-overflow:ellipsis;white-space:nowrap}' +
      '.c .sz{margin-left:auto;flex-shrink:0;color:#9aa0a6;font-family:ui-monospace,Menlo,Consolas,monospace}' +
      '.c .r{display:flex;align-items:flex-start;gap:8px}' +
      '.c .k{flex:0 0 66px;color:#9aa0a6}' +
      '.c .v{display:flex;align-items:center;gap:5px;min-width:0;color:#e8eaed;word-break:break-all}' +
      '.c .mono{font-family:ui-monospace,Menlo,Consolas,monospace}' +
      '.c .sw{width:10px;height:10px;flex-shrink:0;border-radius:2px;border:1px solid rgba(255,255,255,.25)}' +
      '.c .sep{height:1px;margin:5px 0;background:rgba(255,255,255,.12)}' +
      '.c .path{color:#8ab4f8}';

    var bar = style(document.createElement('div'), {
      position: 'fixed',
      left: '50%',
      bottom: '14px',
      transform: 'translateX(-50%)',
      'pointer-events': 'none',
      background: 'rgba(0, 0, 0, 0.78)',
      color: '#ffffff',
      padding: '5px 12px',
      'border-radius': '999px',
      'font-family': 'system-ui, -apple-system, sans-serif',
      'font-size': '12px',
      'line-height': '18px',
      'white-space': 'nowrap'
    });
    bar.textContent = HINT;

    shadow.appendChild(css);
    shadow.appendChild(box);
    shadow.appendChild(card);
    shadow.appendChild(bar);
    // 挂到 documentElement 而非 body:body 上若存在 transform/filter 会让 fixed 定位
    // 相对该祖先而非视口,坐标与 getBoundingClientRect 对不上
    (document.documentElement || document.body).appendChild(host);
  }

  // ---------- 事件 ----------

  function onMove(e) {
    pending = targetOf(e);
    if (raf) return;
    raf = requestAnimationFrame(function () {
      raf = 0;
      highlight(pending);
      pending = null;
    });
  }

  function onScroll() {
    if (current) highlight(current);
  }

  function onPointer(e) {
    if (done) return;
    // 整个手势期间都要拦住:否则松手后 page 自己的 click(链接跳转/表单提交)会照常触发
    e.preventDefault();
    e.stopImmediatePropagation();
    // 只在 click 上结算 —— pointerup/mouseup 先到,若在那里 finish,清理后紧跟的 click 就没人拦了
    if (e.type !== 'click') return;
    var el = current || targetOf(e);
    if (!el) return;
    finish(collect(el));
  }

  function onKey(e) {
    if (e.key === 'Escape' || e.keyCode === 27) {
      e.preventDefault();
      e.stopImmediatePropagation();
      finish(null);
    }
  }

  function onPageHide() {
    finish(null);
  }

  var POINTER_EVENTS = ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'click', 'dblclick', 'contextmenu', 'auxclick'];

  function addListeners() {
    window.addEventListener('mousemove', onMove, true);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pagehide', onPageHide, true);
    for (var i = 0; i < POINTER_EVENTS.length; i++) {
      window.addEventListener(POINTER_EVENTS[i], onPointer, true);
    }
  }

  function removeListeners() {
    window.removeEventListener('mousemove', onMove, true);
    window.removeEventListener('scroll', onScroll, true);
    window.removeEventListener('keydown', onKey, true);
    window.removeEventListener('pagehide', onPageHide, true);
    for (var i = 0; i < POINTER_EVENTS.length; i++) {
      window.removeEventListener(POINTER_EVENTS[i], onPointer, true);
    }
  }

  function cleanup() {
    if (raf) { cancelAnimationFrame(raf); raf = 0; }
    removeListeners();
    if (host && host.parentNode) host.parentNode.removeChild(host);
    host = null;
    box = null;
    card = null;
    cachedEl = null;
    current = null;
    pending = null;
    if (window.__ASTORM_PICK__) {
      try { delete window.__ASTORM_PICK__; } catch (e) { window.__ASTORM_PICK__ = null; }
    }
  }

  function finish(result) {
    if (done) return;
    done = true;
    cleanup();
    if (settle) settle(result);
  }

  return new Promise(function (resolve) {
    settle = resolve;
    window.__ASTORM_PICK__ = { cancel: function () { finish(null); } };
    buildOverlay();
    addListeners();
  });
})();`

/** 取消脚本:若页面仍挂着拾取实例则让其 resolve null(无实例时 no-op) */
const CANCEL_SCRIPT = `(function () {
  try {
    var p = window.__ASTORM_PICK__;
    if (p && typeof p.cancel === 'function') p.cancel();
  } catch (e) {}
})()`

/** 拾取选项:页面内提示条文案 + 卡片字段标签(标签随界面语言,由渲染层传入;主进程无 i18n) */
export interface ElementPickerOptions {
  hint?: string
  labels?: Record<string, string>
}

const DEFAULT_LABELS: Record<string, string> = {
  color: 'Color',
  background: 'Background',
  font: 'Font',
  padding: 'Padding',
  margin: 'Margin',
  component: 'Component',
  path: 'Source',
  id: 'id',
  xpath: 'XPath',
  bold: 'bold',
  italic: 'italic'
}

function pickerScript(options: unknown): string {
  const raw = (options ?? {}) as ElementPickerOptions
  const payload = {
    hint: typeof raw.hint === 'string' && raw.hint.trim() ? raw.hint.trim() : 'Click an element',
    labels: { ...DEFAULT_LABELS, ...(raw.labels ?? {}) }
  }
  return PICKER_SCRIPT.replace('__ASTORM_OPTIONS__', () => JSON.stringify(payload))
}

export function registerElementPickerIpc(): void {
  // 拾取元素:注入脚本并等待用户点击;返回 { ok, data?, canceled?, message? }
  ipcMain.handle('browser:pick-element', async (event, url: unknown, options: unknown): Promise<ElementPickResult> => {
    const target = typeof url === 'string' ? url.trim() : ''
    if (!target) return { ok: false, message: 'EMPTY_URL' }
    const frame = findFrame(event.sender, target)
    if (!frame) return { ok: false, message: 'NO_FRAME' }
    try {
      // 脚本返回元素信息对象;取消/异常返回 null
      const data = (await frame.executeJavaScript(pickerScript(options), false)) as ElementPickResult['data'] | null
      if (!data) return { ok: false, canceled: true }
      return { ok: true, data }
    } catch (err) {
      return { ok: false, message: err instanceof Error ? err.message : String(err) }
    }
  })

  // 取消拾取:让进行中的 Promise resolve null(页面已销毁/未挂载则返回 false)
  ipcMain.handle('browser:cancel-pick', (event, url: unknown): boolean => {
    const target = typeof url === 'string' ? url.trim() : ''
    if (!target) return false
    const frame = findFrame(event.sender, target)
    if (!frame) return false
    try {
      void frame.executeJavaScript(CANCEL_SCRIPT, false)
      return true
    } catch {
      return false
    }
  })
}
