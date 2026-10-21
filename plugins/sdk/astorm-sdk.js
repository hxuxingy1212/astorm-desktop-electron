/*!
 * astorm plugin SDK v1.0.0
 *
 * 插件页 SDK：经 postMessage 与应用宿主通信（协议见 docs/plugin-system.md）。
 * 用法：<script src="astorm-plugin://__sdk__/astorm-sdk.js"></script>
 * 然后：
 *   const { sdk } = window.astorm          // 或直接 window.astorm.config.get() 等
 *   await sdk.ready                        // 就绪（连接探测完成）
 *   const { config } = await sdk.config.get()
 *   sdk.llm.chat({ messages, onChunk })    // 流式单次补全（复用应用 LLM）
 *   sdk.chat.appendInput('hello') / sdk.chat.send('hello')
 *   sdk.app.onMessage((data) => {})        // 接收应用发来的消息
 *   sdk.plugin.sendToApp({ text })         // 发消息给应用
 *   sdk.platform.info()
 *
 * 零依赖、UMD 风格：暴露 window.astorm。独立打开（window.parent === self）时
 * 方法会给出「请在应用内运行」的拒绝，避免静默挂起。
 */
(function () {
  'use strict'

  var SDK_VERSION = '1.0.0'
  var PROTO_VERSION = 1
  var inApp = typeof window !== 'undefined' && window.parent && window.parent !== window

  var target = inApp ? window.parent : null
  var seq = 0
  var pending = {} // id -> { resolve, reject, acc, timer, onChunk }
  var appMessageHandler = null
  var sessionChangeHandler = null

  /* ---------- 请求 / 事件分发 ---------- */

  function post(method, params) {
    target.postMessage({ astorm: true, kind: 'request', id: seq, method: method, params: params || {} }, '*')
  }

  function request(method, params, timeoutMs) {
    if (!inApp || !target) {
      return Promise.reject(Object.assign(new Error('SDK 未在应用内运行：请通过应用打开插件'), { code: 'NOT_EMBEDDED' }))
    }
    var id = ++seq
    return new Promise(function (resolve, reject) {
      pending[id] = {
        resolve: resolve,
        reject: reject,
        acc: { text: '', reasoning: '', usage: null, finishReason: '' },
        timer: setTimeout(function () {
          if (pending[id]) {
            delete pending[id]
            reject(new Error('插件请求超时：' + method))
          }
        }, timeoutMs || 30000)
      }
      post(method, params)
    })
  }

  function onMessage(e) {
    var msg = e && e.data
    if (!msg || msg.astorm !== true) return

    // 应用 → 插件的事件推送（app.message / app.session.change）
    if (msg.kind === 'event') {
      if (msg.event === 'app.message') {
        try {
          if (appMessageHandler) appMessageHandler(msg.data || {})
        } catch (err) {
          console.error('[astorm-sdk] app.onMessage handler error:', err)
        }
      } else if (msg.event === 'app.session.change') {
        try {
          if (sessionChangeHandler) sessionChangeHandler(msg.data || {})
        } catch (err) {
          console.error('[astorm-sdk] app.onSessionChange handler error:', err)
        }
      }
      return
    }

    var p = pending[msg.id]
    if (!p) return

    // 单次响应（同步方法）
    if (msg.kind === 'response') {
      clearTimeout(p.timer)
      delete pending[msg.id]
      if (msg.ok) {
        p.onChunkDone && p.onChunkDone() // llm.chat:end 之后不应再有 response
        p.resolve(msg.result)
      } else {
        p.reject(Object.assign(new Error((msg.error && msg.error.message) || '插件请求失败'), { code: msg.error && msg.error.code }))
      }
      return
    }

    // 流式事件（llm.chat：chunk / reasoning / end / error）
    if (msg.kind === 'stream') {
      var chunk = msg.chunk || {}
      if (typeof p.onChunk === 'function') {
        try {
          p.onChunk(chunk)
        } catch (err) {
          console.error('[astorm-sdk] onChunk error:', err)
        }
      }
      if (chunk.type === 'chunk') {
        p.acc.text += chunk.delta || ''
      } else if (chunk.type === 'reasoning') {
        p.acc.reasoning += chunk.delta || ''
      } else if (chunk.type === 'error') {
        clearTimeout(p.timer)
        delete pending[msg.id]
        p.reject(new Error(chunk.message || 'LLM 补全失败'))
      } else if (chunk.type === 'end') {
        clearTimeout(p.timer)
        delete pending[msg.id]
        p.acc.usage = chunk.usage || null
        p.acc.finishReason = chunk.finishReason || ''
        p.resolve({
          text: p.acc.text,
          reasoning: p.acc.reasoning,
          usage: p.acc.usage,
          finishReason: p.acc.finishReason
        })
      }
    }
  }

  if (inApp) window.addEventListener('message', onMessage)

  /* ---------- SDK 公共 API ---------- */

  var sdk = {
    config: {
      /** 读取插件配置（含 schema，设置页保存后此处可再取到最新值） */
      get: function () {
        return request('config.get', {}, 10000)
      }
    },
    chat: {
      /** 把文本填入会话发送框（不自动发送；用户按 Enter 生效） */
      appendInput: function (text) {
        return request('chat.appendInput', { text: String(text || '') }, 10000)
      },
      /** 直接发到当前会话（按当前输入框的模式/权限/思考强度） */
      send: function (text) {
        return request('chat.send', { text: String(text || '') }, 10000)
      }
    },
    file: {
      /**
       * 系统对话框选择一个 .pptx（默认定位当前会话空间目录），宿主解包为
       * 产物 deck JSON 返回：{ canceled:true } 或 { canceled:false, file, deck }
       * （deck = { width, height, meta?, theme?, slides }）。PPT 较大时解包可达数十秒，超时放宽。
       */
      openPptx: function () {
        return request('file.openPptx', {}, 90000)
      }
    },
    state: {
      /**
       * 会话级插件状态：读当前会话下的插件数据（按 会话×插件 作用域）。
       * 传 key 取单值；不传返回整表。返回 { sessionId, value }。
       * 切换会话后页面重建，重新 state.get 即读到该会话自己的状态。
       */
      get: function (key) {
        return request('state.get', { key: key }, 10000)
      },
      /** 写当前会话下的插件状态（key → value；立即持久化，切会话/重启可恢复） */
      set: function (key, value) {
        return request('state.set', { key: key, value: value }, 10000)
      }
    },
    llm: {
      /**
       * 单次补全：{ messages:[{role,content}], provider?, model?, reasoningEffort?, onChunk? }
       * 返回 Promise<{ text, reasoning, usage, finishReason }>；
       * 传 onChunk 时逐条收到 {type:'chunk'|'reasoning'|'end'|'error', delta?, ...}。
       */
      chat: function (opts) {
        opts = opts || {}
        return new Promise(function (resolve, reject) {
          if (!inApp || !target) {
            reject(Object.assign(new Error('SDK 未在应用内运行：请通过应用打开插件'), { code: 'NOT_EMBEDDED' }))
            return
          }
          var id = ++seq
          pending[id] = {
            resolve: resolve,
            reject: reject,
            acc: { text: '', reasoning: '', usage: null, finishReason: '' },
            onChunk: typeof opts.onChunk === 'function' ? opts.onChunk : null,
            onChunkDone: null,
            timer: setTimeout(function () {
              if (pending[id]) {
                delete pending[id]
                reject(new Error('llm.chat 超时'))
              }
            }, 180000)
          }
          post('llm.chat', {
            messages: opts.messages,
            provider: opts.provider,
            model: opts.model,
            reasoningEffort: opts.reasoningEffort
          })
        })
      }
    },
    app: {
      /** 订阅应用发来的消息（工具栏「发送消息给插件」/加载欢迎消息等） */
      onMessage: function (handler) {
        if (typeof handler === 'function') appMessageHandler = handler
        return handler
      },
      /** 订阅会话切换事件（页签全局共享,切换会话时 iframe 存活;据此按会话重新取状态） */
      onSessionChange: function (handler) {
        if (typeof handler === 'function') sessionChangeHandler = handler
        return handler
      }
    },
    plugin: {
      /** 向应用发送消息（应用在插件页签内展示） */
      sendToApp: function (data) {
        return request('plugin.messageToApp', { text: (data && data.text) || (data && data.message) || '' }, 10000)
      }
    },
    platform: {
      /** 应用信息（版本 / 已配置供应商数 / 当前会话） */
      info: function () {
        return request('platform.info', {}, 10000)
      }
    }
  }

  /* 就绪探测：platform.info 成功即认为已连接 */
  var ready = inApp
    ? request('platform.info', {}, 10000).then(function (info) {
        return info
      })
    : Promise.reject(new Error('SDK 未在应用内运行'))

  window.astorm = {
    version: SDK_VERSION,
    protoVersion: PROTO_VERSION,
    ready: ready,
    sdk: sdk,
    config: sdk.config,
    chat: sdk.chat,
    state: sdk.state,
    file: sdk.file,
    llm: sdk.llm,
    app: sdk.app,
    plugin: sdk.plugin,
    platform: sdk.platform
  }
})()