/**
 * web-bridge.js — 浏览器部署的 window.api 实现（桌面 preload 的等价物）。
 *
 * 由 index.html 在模块脚本之前以普通 <script> 加载（modules 是 deferred，
 * 经典脚本先执行，保证 main.ts 里 window.api.zoom.setFactor 不炸）。
 * 桌面端 preload 已暴露 window.api → 本脚本开头即退出，零影响。
 *
 * 传输：
 * - 请求：fetch → /api/v1/*（信封 {code,message,data} 解包，Bearer 令牌）
 * - 事件：WebSocket /api/v1/ws（断线重连带 after 游标补缺口）
 * - 令牌：localStorage `astorm:web:token`；401 时弹出登录遮罩
 *
 * 桌面独有能力的 web 行为（与 docs/cloud-server.md 的降级矩阵一致）：
 * - dialog.openAttachments → 浏览器文件选择 + 上传 staging（返回服务端路径）
 * - attachment.stageClipboard → navigator.clipboard 读图（需授权）
 * - window/zoom/permission/power → no-op；browser/browserAgent 元素拾取/截图 → 报错
 * - office.preview / git.* / providers.* / llm.ask → 服务端等价端点
 */
;(function () {
  'use strict'
  if (typeof window === 'undefined' || window.api) return // 桌面端已有 preload 桥

  var TOKEN_KEY = 'astorm:web:token'

  /* ---------------- 令牌与登录遮罩 ---------------- */

  function getToken() {
    try {
      return localStorage.getItem(TOKEN_KEY) || ''
    } catch (e) {
      return ''
    }
  }

  function setToken(t) {
    try {
      if (t) localStorage.setItem(TOKEN_KEY, t)
      else localStorage.removeItem(TOKEN_KEY)
    } catch (e) { /* 隐私模式忽略 */ }
  }

  function showLogin(onDone) {
    var existing = document.getElementById('astorm-web-login')
    if (existing) {
      existing.style.display = 'flex'
      var input = existing.querySelector('input')
      if (input) input.focus()
      existing.__onDone = onDone
      return
    }
    var mask = document.createElement('div')
    mask.id = 'astorm-web-login'
    mask.style.cssText =
      'position:fixed;inset:0;z-index:99999;display:flex;align-items:center;justify-content:center;' +
      'background:rgba(0,0,0,.45);backdrop-filter:blur(4px);font-family:system-ui,sans-serif'
    mask.innerHTML =
      '<div style="width:340px;background:var(--color-bg-2,#fff);border-radius:12px;padding:28px;' +
      'box-shadow:0 12px 40px rgba(0,0,0,.2)">' +
      '<div style="font-size:17px;font-weight:600;margin-bottom:6px;color:var(--color-text-1,#1d2129)">Astorm 云端</div>' +
      '<div style="font-size:13px;color:var(--color-text-3,#86909c);margin-bottom:16px">输入服务端访问令牌（ASTORM_TOKEN）</div>' +
      '<input type="password" placeholder="访问令牌" style="width:100%;box-sizing:border-box;padding:9px 12px;' +
      'border:1px solid var(--color-border-2,#e5e6eb);border-radius:8px;font-size:14px;outline:none" />' +
      '<div class="astorm-login-err" style="font-size:12px;color:rgb(var(--danger-6,#f53f3f));margin-top:8px;min-height:16px"></div>' +
      '<button style="width:100%;padding:9px 0;border:none;border-radius:8px;background:rgb(var(--primary-6),' +
      'rgb(0,102,255));color:#fff;font-size:14px;cursor:pointer;margin-top:4px">连接</button></div>'
    document.body.appendChild(mask)
    var input = mask.querySelector('input')
    var btn = mask.querySelector('button')
    var errEl = mask.querySelector('.astorm-login-err')
    mask.__onDone = onDone
    function submit() {
      var t = input.value.trim()
      if (!t) {
        errEl.textContent = '请输入令牌'
        return
      }
      fetch('/api/v1/status', { headers: { Authorization: 'Bearer ' + t } }).then(function (r) {
        if (r.status === 401) {
          errEl.textContent = '令牌无效，请重试'
          return
        }
        setToken(t)
        mask.style.display = 'none'
        if (mask.__onDone) mask.__onDone()
      }).catch(function () {
        errEl.textContent = '无法连接服务端'
      })
    }
    btn.addEventListener('click', submit)
    input.addEventListener('keydown', function (e) {
      if (e.key === 'Enter') submit()
    })
    setTimeout(function () { input.focus() }, 50)
  }

  /** PTY 写入串行链（见 backend.invoke 注释） */
  var ptyChain = Promise.resolve()

  /* ---------------- HTTP 与事件总线 ---------------- */

  /** 信封解包的 fetch；401 触发登录遮罩并抛错。 */
  function http(method, path, body, raw) {
    var headers = { Authorization: 'Bearer ' + getToken() }
    var payload
    if (body !== undefined && !(body instanceof Blob) && !(body instanceof ArrayBuffer)) {
      headers['Content-Type'] = 'application/json'
      payload = JSON.stringify(body)
    } else if (body !== undefined) {
      payload = body
    }
    return fetch(path, { method: method, headers: headers, body: payload }).then(function (r) {
      if (r.status === 401) {
        showLogin(function () { location.reload() })
        throw new Error('未登录或令牌失效')
      }
      if (raw) return r
      return r.json().then(function (env) {
        if (!env || typeof env.code !== 'number') return env
        if (env.code !== 0) throw new Error(env.message || '请求失败')
        return env.data
      })
    })
  }

  /** 全局事件总线：WS 单连接 + 订阅者广播 + 重连续传。 */
  var bus = (function () {
    var subs = []
    var ws = null
    var lastSeq = 0
    var retry = 0

    function connect() {
      if (ws && (ws.readyState === 0 || ws.readyState === 1)) return
      var proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
      ws = new WebSocket(proto + '//' + location.host + '/api/v1/ws?token=' + encodeURIComponent(getToken()) + '&after=' + lastSeq)
      ws.onmessage = function (msg) {
        try {
          var frame = JSON.parse(msg.data)
          if (frame.t !== 'event' || !frame.event) return
          if (frame.seq > lastSeq) lastSeq = frame.seq
          // 调试探针：最近帧与订阅状态（生产可留，量小）
          try {
            window.__wsFrames = window.__wsFrames || []
            window.__wsFrames.push({ seq: frame.seq, type: frame.event.type, sid: (frame.event.session_id || '').slice(0, 8) })
            if (window.__wsFrames.length > 12) window.__wsFrames.shift()
            window.__wsState = { subs: subs.length, open: true, lastSeq: lastSeq }
          } catch (e) { /* ignore */ }
          subs.slice().forEach(function (cb) {
            try { cb(frame.event) } catch (e) { console.error('[web-bridge] 订阅者异常', e) }
          })
        } catch (e) { /* 非 JSON 帧忽略 */ }
      }
      ws.onclose = function () {
        retry = Math.min(retry + 1, 6)
        setTimeout(connect, 500 * Math.pow(2, retry)) // 指数退避重连
      }
      ws.onopen = function () { retry = 0 }
    }

    return {
      subscribe: function (cb) {
        subs.push(cb)
        connect()
        return function () {
          var i = subs.indexOf(cb)
          if (i >= 0) subs.splice(i, 1)
        }
      }
      // 断线/回放补齐完全由 WS 的 after 游标承担（服务端按 seq 回放 backlog，
      // 见 api/ws.rs pump）；不再额外拉 SSE——那条流永不结束，会占住浏览器
      // 同 host 连接池并造成事件重复派发（实测踩坑）。
    }
  })()

  /* ---------------- 工具 ---------------- */

  function fileToBase64(file) {
    return new Promise(function (resolve, reject) {
      var reader = new FileReader()
      reader.onload = function () {
        // dataURL 去前缀
        var s = String(reader.result)
        resolve(s.slice(s.indexOf(',') + 1))
      }
      reader.onerror = function () { reject(new Error('读取文件失败')) }
      reader.readAsDataURL(file)
    })
  }

  function base64ToBytes(b64) {
    var bin = atob(b64)
    var out = new Uint8Array(bin.length)
    for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  }

  /** 桌面契约：dialog.open* 返回 string[]；staged 对象只用于缓存元数据。 */
  function stagedPaths(list) {
    return list.map(function (s) { return s && typeof s === 'object' ? s.path : s })
  }

  /** 浏览器选文件 + 上传 staging，返回 MessageAttachment[]（与桌面 stagePaths 同形状）。 */
  function pickAndUpload(multiple) {
    return new Promise(function (resolve) {
      var input = document.createElement('input')
      input.type = 'file'
      if (multiple) input.multiple = true
      input.onchange = function () {
        var files = Array.from(input.files || [])
        if (!files.length) { resolve([]); return }
        Promise.all(files.map(function (f) {
          return fileToBase64(f).then(function (b64) {
            return http('POST', '/api/v1/attachments', { name: f.name, base64: b64 }).then(function (staged) {
              stagedCache()[staged.path] = staged
              return staged
            })
          })
        })).then(resolve, function () { resolve([]) })
      }
      input.click()
    })
  }

  function stagedCache() {
    if (!window.__astormStaged) window.__astormStaged = {}
    return window.__astormStaged
  }

  /* ---------------- 请求体转换（napi DTO → core serde）----------------
   * 渲染层构造的会话请求是 napi DTO 形状（message/mode/permission），
   * REST 端点直接反序列化 core 类型（userMessage/sessionMode/permissionMode），
   * 其余键（model/provider/workspace/mentions/…）两侧同名。 */

  function toCoreRequest(req) {
    if (!req || typeof req !== 'object') return req
    var out = {}
    for (var k in req) {
      if (k === 'message') out.userMessage = req[k]
      else if (k === 'mode') out.sessionMode = req[k]
      else if (k === 'permission') out.permissionMode = req[k]
      else out[k] = req[k]
    }
    return out
  }

  /* ---------------- window.api 组装 ---------------- */

  var noop = function () {}
  var unsupported = function (what) {
    return Promise.reject(new Error(what + ' 需要桌面应用（浏览器部署不支持）'))
  }

  window.api = {
    window: {
      minimize: noop,
      toggleMaximize: noop,
      close: noop,
      isMaximized: function () { return Promise.resolve(false) },
      onMaximizedChange: function () { return function () {} },
      isFullscreen: function () { return Promise.resolve(false) },
      onFullscreenChange: function () { return function () {} }
    },
    zoom: { setFactor: noop },
    office: {
      preview: function (filePath) {
        return http('POST', '/api/v1/office/preview', { path: filePath })
      }
    },
    browser: {
      openDevtools: function () { return unsupported('iframe DevTools') },
      pickElement: function () { return unsupported('元素拾取') },
      cancelPick: function () { return Promise.resolve(false) },
      capture: function () { return unsupported('页签截图') },
      frame: function () { return unsupported('页签取帧') },
      recordSave: function () { return unsupported('页签录制') },
      /**
       * 面板画面流 WS 地址（容器形态）：服务端 headless Chromium 的
       * `Page.startScreencast` 帧下行 + 鼠标/键盘/导航上行（见 server browser 模块）。
       * 桌面走 iframe，不需要此地址（返回空串）。
       */
      screencastUrl: function (sessionId) {
        if (!sessionId) return ''
        var proto = location.protocol === 'https:' ? 'wss://' : 'ws://'
        return proto + location.host + '/api/v1/browser/ws?token=' +
          encodeURIComponent(getToken()) + '&session_id=' + encodeURIComponent(sessionId)
      }
    },
    browserAgent: {
      setTabs: function (payload) {
        // 云端无内置浏览器面板的 CDP 桥；静默上报（tabs 工具镜像照常，browser 工具会给出可读错误）
        return Promise.resolve({ ok: true })
      },
      onCommand: function () { return function () {} },
      commandResult: function () { return Promise.resolve({ ok: false }) },
      status: function () { return Promise.resolve({ running: false, endpointFile: '' }) },
      stop: function () { return Promise.resolve() }
    },
    dialog: {
      openDirectory: function () { return unsupported('目录选择') },
      // 知识库导入：浏览器选文件 → 上传 staging → 返回服务端路径（kb_import 接受）
      openFiles: function () { return pickAndUpload(true).then(stagedPaths) },
      saveFile: function () { return unsupported('另存为') },
      openPptx: function () { return unsupported('PPTX 选择') },
      // 桌面契约是 string[]；对象数组会让 MailCompose 的 path.split 崩溃
      openAttachments: function () { return pickAndUpload(true).then(stagedPaths) }
    },
    attachment: {
      stagePaths: function (paths) {
        // 入参可能是：openAttachments 刚返回的 **staged 对象**（对象直通）或字符串路径
        // （缓存命中 / 会话工作区引用 → 就地元数据）。返回桌面同款 StageResult 包装。
        var cache = stagedCache()
        var ok = paths.map(function (p) {
          if (p && typeof p === 'object' && p.path) return p
          return cache[p] || { path: p, name: String(p).split(/[\\/]/).pop() || p }
        })
        return Promise.resolve({ ok: ok, failed: [] })
      },
      stageBytes: function (bytes, name) {
        var u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)
        var bin = ''
        for (var i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i])
        var b64 = btoa(bin)
        return http('POST', '/api/v1/attachments', { name: name, base64: b64 }).then(function (staged) {
          stagedCache()[staged.path] = staged
          return { ok: [staged], failed: [] }
        })
      },
      stageClipboard: function () {
        // 浏览器剪贴板读图（需用户授权；桌面静默读系统剪贴板）
        if (!navigator.clipboard || !navigator.clipboard.read) {
          return unsupported('剪贴板图片（浏览器需授权 navigator.clipboard）')
        }
        return navigator.clipboard.read().then(function (items) {
          for (var _i = 0; _i < items.length; _i++) {
            var item = items[_i]
            var type = item.types.find(function (t) { return t.indexOf('image/') === 0 })
            if (!type) continue
            return item.getType(type).then(function (blob) {
              var name = 'clipboard.' + type.split('/')[1]
              return fileToBase64(blob).then(function (b64) {
                return http('POST', '/api/v1/attachments', { name: name, base64: b64 }).then(function (staged) {
                  stagedCache()[staged.path] = staged
                  return { ok: [staged], failed: [] }
                })
              })
            })
          }
          throw new Error('剪贴板中没有图片')
        })
      },
      pathForFile: function () { return '' },
      toUrl: function (path) {
        var segs = path.split(/[\\/]+/).filter(Boolean).map(encodeURIComponent)
        return '/api/v1/attachment/file/' + segs.join('/')
      }
    },
    fs: {
      openFolder: function () { return unsupported('打开文件夹') },
      // 目录选择器（web 无原生目录对话框）：单层列目录（仅目录，数据根内）+ 新建文件夹
      listDirs: function (path) {
        return http('GET', '/api/v1/fs/list-dirs' + (path ? '?path=' + encodeURIComponent(path) : ''))
      },
      mkdir: function (parent, name) {
        return http('POST', '/api/v1/fs/mkdir', { parent: parent, name: name })
      },
      readFileBase64: function (path) {
        return http('GET', '/api/v1/fs/read-base64?path=' + encodeURIComponent(path)).then(function (d) {
          return d ? d.base64 || '' : ''
        })
      },
      readArrayBuffer: function (path) {
        return http('GET', '/api/v1/fs/read-bytes?path=' + encodeURIComponent(path), undefined, true)
          .then(function (r) { return r.ok ? r.arrayBuffer() : null })
      },
      writeTextFile: function (path, content) {
        return http('POST', '/api/v1/fs/write', { path: path, content: content })
      }
    },
    pptx: {
      unpack: function () { return unsupported('PPTX 解包') }
    },
    git: {
      currentBranch: function (dir) {
        return http('POST', '/api/v1/bridge', { command: 'git_current_branch', args: { dir: dir } })
      },
      isRepo: function (dir) {
        return http('POST', '/api/v1/bridge', { command: 'git_is_repo', args: { dir: dir } })
      },
      listBranches: function (dir) {
        return http('POST', '/api/v1/bridge', { command: 'git_list_branches', args: { dir: dir } })
      },
      branchState: function (dir) {
        return http('POST', '/api/v1/bridge', { command: 'git_branch_state', args: { dir: dir } })
      },
      log: function (dir, branch, limit) {
        return http('POST', '/api/v1/bridge', { command: 'git_log', args: { dir: dir, branch: branch, limit: limit } })
      },
      checkout: function (dir, branch) {
        return http('POST', '/api/v1/bridge', { command: 'git_checkout', args: { dir: dir, branch: branch } })
      },
      createBranch: function (dir, name) {
        return http('POST', '/api/v1/bridge', { command: 'git_create_branch', args: { dir: dir, name: name } })
      }
    },
    providers: {
      // 桌面 preload 契约返回**聚合** {providers: [...]}（渲染层 services/providers.ts
      // 自己取 res.providers——返回裸数组会让模型页永远空白）
      list: function () {
        return http('GET', '/api/v1/providers').then(function (d) {
          return d && d.providers ? d : { providers: [] }
        })
      },
      sync: function (providers) {
        return http('POST', '/api/v1/providers/sync', { providers: providers })
      },
      test: function (name, model) {
        return http('POST', '/api/v1/providers/test', { name: name, model: model })
      }
    },
    llm: {
      ask: function (request, onChunk) {
        var callId = 'llm-' + Date.now() + '-' + Math.random().toString(36).slice(2, 10)
        var aborter = new AbortController()
        fetch('/api/v1/llm/ask', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + getToken() },
          body: JSON.stringify(request),
          signal: aborter.signal
        }).then(function (r) {
          if (!r.ok || !r.body) throw new Error('LLM 补全请求失败')
          var reader = r.body.getReader()
          var decoder = new TextDecoder()
          var buf = ''
          function pump() {
            return reader.read().then(function (chunk) {
              if (chunk.done) return
              buf += decoder.decode(chunk.value, { stream: true })
              var frames = buf.split('\n\n')
              buf = frames.pop()
              frames.forEach(function (block) {
                var evName = 'message'
                var data = ''
                block.split('\n').forEach(function (line) {
                  if (line.indexOf('event:') === 0) evName = line.slice(6).trim()
                  else if (line.indexOf('data:') === 0) data += line.slice(5).trim()
                })
                if (!data) return
                var ev = JSON.parse(data)
                var kind = evName === 'message' ? (ev.kind || 'chunk') : evName
                onChunk({ callId: ev.call_id || callId, chunk: { type: kind, delta: ev.delta, message: ev.message, usage: ev.usage, finishReason: ev.finishReason } })
              })
              return pump()
            })
          }
          return pump()
        }).catch(function (err) {
          if (err && err.name === 'AbortError') return // 主动取消不报错
          onChunk({ callId: callId, chunk: { type: 'error', message: err.message || 'LLM 补全请求失败' } })
          onChunk({ callId: callId, chunk: { type: 'end', usage: null, finishReason: 'error' } })
        })
        return function () { aborter.abort() }
      }
    },
    chat: {
      createSession: function (request) {
        return http('POST', '/api/v1/sessions', toCoreRequest(request)).then(function (d) { return d.id })
      },
      sendMessage: function (sessionId, request) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/messages', toCoreRequest(request)).then(function () {})
      },
      startSession: function (sessionId) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/start').then(function () {})
      },
      interruptMessage: function (sessionId, request) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/interrupt', toCoreRequest(request)).then(function () {})
      },
      submitInterrupt: function (sessionId, request) {
        // 渲染层发的是 napi DTO 形状（decision: 'allow'|'deny'），core 契约是
        // allow 布尔（serde 不认识 decision → 缺 allow → validate 必败）——此处转换
        var out = {}
        for (var k in request) {
          if (k === 'decision') continue
          out[k] = request[k]
        }
        if (request.interruptType === 'approval') {
          out.allow = request.decision === 'allow'
        }
        return http('POST', '/api/v1/sessions/' + sessionId + '/interrupts/resolve', out).then(function () {})
      },
      reopenInterrupt: function (sessionId, toolCallId) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/interrupts/' + encodeURIComponent(toolCallId) + '/reopen').then(function () {})
      },
      cancel: function (sessionId) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/cancel').then(function () {})
      },
      rewindToUserMessage: function (sessionId, keepUserMessages) {
        // core RewindBody 是 snake_case（服务端自定义结构无 rename_all）
        return http('POST', '/api/v1/sessions/' + sessionId + '/rewind', { keep_user_messages: keepUserMessages }).then(function () {})
      },
      forkSession: function (sessionId, beforeFinal) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/fork', { beforeFinal: beforeFinal }).then(function (d) { return d.id })
      },
      summarizeSession: function (sessionId) {
        return http('POST', '/api/v1/sessions/' + sessionId + '/summarize').then(function (d) { return !!d.summarized })
      },
      updateSessionSettings: function (sessionId, request) {
        return http('PATCH', '/api/v1/sessions/' + sessionId + '/settings', toCoreRequest(request)).then(function () {})
      },
      updateSessionGoal: function (sessionId, goal, goalLang) {
        // 服务端 bridge 的 update_session_goal 走 common Session trait（snake_case 参数）
        return http('POST', '/api/v1/bridge', {
          command: 'update_session_goal',
          args: { session_id: sessionId, goal: goal, goal_lang: goalLang || null }
        }).then(function () {})
      },
      listSessions: function () {
        return http('GET', '/api/v1/sessions')
      },
      listMessages: function (sessionId) {
        return http('GET', '/api/v1/sessions/' + sessionId + '/messages')
      },
      archivedBefore: function (sessionId, beforeSeq, maxUserTurns) {
        return http('GET', '/api/v1/sessions/' + sessionId + '/messages/archived?before_seq=' + beforeSeq + '&max_user_turns=' + (maxUserTurns || 5))
      },
      getSystemPrompt: function (sessionId) {
        return http('GET', '/api/v1/sessions/' + sessionId + '/system-prompt').then(function (d) { return d.prompt })
      },
      deleteSession: function (sessionId) {
        return http('DELETE', '/api/v1/sessions/' + sessionId).then(function () {})
      },
      onEvent: function (callback) {
        return bus.subscribe(callback)
      }
    },
    backend: {
      invoke: function (command, args) {
        // PTY 写入必须**有序**：桌面 Electron IPC 同 renderer 串行；web 的 fetch 并发
        // 到达顺序不定（xterm 逐键一次 invoke），乱序会把 "pwd"+"whoami" 交错成
        // "pwdci…"。对 pty_write/pty_resize 挂全局串行链（保持调用语义，只定序）。
        //
        // 链尾失败必须**不再粘住**：容器/后端重启后首个 resize/write 会 500（PTY 已随
        // 重启消失），若沿用 ptyChain.then(run)，整条链此后永久处于 rejected 态——
        // 后续每次键入都短路回放旧错误、请求根本不发出（表现为"终端敲什么都没反应"，
        // 刷新页面才恢复）。用 .then(run, run) 让失败后链照样推进到下一次调用。
        if (command === 'pty_write' || command === 'pty_resize') {
          var run = function () {
            return http('POST', '/api/v1/bridge', { command: command, args: args || {} })
          }
          ptyChain = ptyChain.then(run, run)
          return ptyChain
        }
        return http('POST', '/api/v1/bridge', { command: command, args: args || {} }).then(
          function (data) {
            // 服务端 JSON 与桌面 napi 的二进制返回形态对齐：
            // - mail_attachment: 桌面是 {filename, data: Buffer, contentType}
            // - kb_read_buffer: 桌面是 Buffer
            if (command === 'mail_attachment' && data && typeof data.base64 === 'string') {
              return {
                filename: data.filename || '',
                data: base64ToBytes(data.base64),
                contentType: data.contentType || ''
              }
            }
            if (command === 'kb_read_buffer' && data && typeof data.base64 === 'string') {
              return base64ToBytes(data.base64)
            }
            return data
          }
        )
      },
      onEvent: function (callback) {
        return bus.subscribe(callback)
      }
    },
    permission: {
      status: function () { return Promise.resolve([]) },
      request: function () { return Promise.resolve([]) },
      openSettings: function () { return Promise.resolve(false) }
    },
    power: {
      setKeepAwake: function () { return Promise.resolve(false) }
    },
    versions: {
      electron: '',
      chrome: (navigator.userAgent.match(/Chrome\/([\d.]+)/) || [])[1] || '',
      node: ''
    },
    platform: (function () {
      var p = (navigator.userAgentData && navigator.userAgentData.platform) ||
        navigator.platform || ''
      if (/mac/i.test(p)) return 'darwin'
      if (/win/i.test(p)) return 'win32'
      if (/linux|x11/i.test(p)) return 'linux'
      return ''
    })(),
    // 浏览器部署没有 astorm-icon:// 协议（图标缓存只在 Electron 主进程里），渲染层回落直连
    iconProtocol: false,
    // 浏览器部署标记:渲染层据此按「无窗口装饰」布局(无红黄绿/无自绘窗口按钮),并隐藏桌面独有设置页
    __web: true
  }

  /* ---------------- 外链路由：新窗口 → 内置浏览器页签 ----------------
   * 只管 `window.open`（= 桌面主进程看到的那一层）：应用里的 `<a target="_blank">` 一律
   * 自带 click 处理（MarkdownLink 会 preventDefault 并弹确认框），在捕获阶段抢锚点会
   * **越过**那道确认流程，语义反而不对。
   * 桌面上 `window.open` / `target=_blank` 由主进程 setWindowOpenHandler 接管：有浏览器桥
   * 就**转成内置新页签**，没有才交系统浏览器。浏览器部署没有主进程，链接会直接开在用户自己的
   * 浏览器里（"点链接跳出应用"）。这里补上同一层接管：渲染层监听 `astorm:open-url`
   * （cancelable），转成内置浏览器页签并 preventDefault；无人接管（如纯静态预览）则放行走原逻辑。
   */
  var rawWindowOpen = window.open
  function routeToBuiltinBrowser(url) {
    try {
      var ev = new CustomEvent('astorm:open-url', { detail: { url: url }, cancelable: true })
      // dispatchEvent 返回 false = 有人 preventDefault（渲染层已开内置页签）
      return !window.dispatchEvent(ev)
    } catch (e) {
      return false
    }
  }
  window.open = function (url, target, features) {
    var u = url === undefined || url === null ? '' : String(url)
    if (/^(https?|file):/i.test(u) && routeToBuiltinBrowser(u)) return null
    return rawWindowOpen.call(window, url, target, features)
  }
  // 启动即探测：无令牌或令牌失效 → 登录遮罩（成功后 reload 拉数据）
  http('GET', '/api/v1/status').catch(function () { /* 登录遮罩已弹出 */ })
})()
