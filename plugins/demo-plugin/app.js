/* 示例插件：演示 astorm 插件 SDK 全部能力 */
(function () {
  'use strict'

  var $ = function (id) {
    return document.getElementById(id)
  }

  var status = $('status')

  function maskSecret(value) {
    if (!value) return '（未设置）'
    if (value.length <= 8) return '*'.repeat(value.length)
    return value.slice(0, 2) + '****' + value.slice(-2)
  }

  /* ---------- 就绪 → 初始化 ---------- */

  window.astorm.ready
    .then(function (info) {
      status.className = 'd-status is-ok'
      status.innerHTML = '<span class="d-dot"></span><span>已连接 · ' + (info.providers ?? 0) + ' 个供应商</span>'
      $('sdk-version').textContent = window.astorm.version
      $('proto-version').textContent = window.astorm.protoVersion
      return window.astorm.config.get()
    })
    .then(function (res) {
      var config = (res && res.config) || {}
      $('greeting').textContent = String(config.greeting || '')
      $('cfg-greeting').textContent = String(config.greeting ?? '（未设置）')
      $('cfg-color').textContent = String(config.color ?? 'blue')
      $('cfg-api').textContent = maskSecret(String(config.apiBase ?? ''))
      applyAccent(String(config.color || 'blue'))
      loadSessionState()
    })
    .catch(function () {
      status.className = 'd-status is-err'
      status.innerHTML = '<span class="d-dot"></span><span>未在应用内运行</span>'
    })

  function applyAccent(color) {
    if (color === 'green') {
      var r = document.querySelector(':root')
      r.style.setProperty('--accent', '#00b42a')
    }
  }

  /* ---------- ② LLM 单次补全 ---------- */

  $('llm-btn').addEventListener('click', function () {
    var btn = $('llm-btn')
    var out = $('llm-out')
    var meta = $('llm-meta')
    var prompt = $('llm-input').value.trim()
    if (!prompt) {
      out.hidden = false
      out.className = 'd-llm-out is-error'
      out.textContent = '请输入提示词'
      return
    }
    btn.disabled = true
    meta.textContent = ''
    out.hidden = false
    out.className = 'd-llm-out'
    out.textContent = ''
    var reasoning = ''

    window.astorm.llm
      .chat({
        messages: [{ role: 'user', content: prompt }],
        onChunk: function (chunk) {
          if (chunk.type === 'chunk') {
            out.textContent += chunk.delta || ''
          } else if (chunk.type === 'reasoning') {
            reasoning += chunk.delta || ''
          } else if (chunk.type === 'error') {
            out.className = 'd-llm-out is-error'
          }
        }
      })
      .then(function (res) {
        btn.disabled = false
        var usage = res.usage
          ? ' · tokens ' + (res.usage.total ?? '?') + ' (prompt ' + (res.usage.prompt ?? 0) + ' / completion ' + (res.usage.completion ?? 0) + ')'
          : ''
        meta.textContent = '完成 · ' + (res.finishReason || 'stop') + usage
        if (reasoning) {
          out.textContent += '\n\n—— 思考链 ——\n' + reasoning
        }
      })
      .catch(function (err) {
        btn.disabled = false
        out.className = 'd-llm-out is-error'
        out.textContent = String(err && err.message ? err.message : err)
      })
  })

  /* ---------- ③ 聊天联动 ---------- */

  $('chat-append').addEventListener('click', function () {
    var text = $('chat-msg').value
    window.astorm.chat.appendInput(text).catch(function (err) {
      alert(err.message)
    })
  })

  $('chat-send').addEventListener('click', function () {
    var btn = $('chat-send')
    btn.disabled = true
    window.astorm.chat
      .send($('chat-msg').value)
      .then(function () {
        btn.disabled = false
      })
      .catch(function (err) {
        btn.disabled = false
        alert(err.message)
      })
  })

  /* ---------- ④ 发给应用 ---------- */

  $('ping-btn').addEventListener('click', function () {
    var text = $('ping-text').value.trim()
    if (!text) return
    window.astorm.plugin.sendToApp({ text: text }).catch(function (err) {
      alert(err.message)
    })
  })

  /* ---------- ⑤ 会话级状态:按 会话×插件 存,切换会话各自恢复 ---------- */

  function loadSessionState() {
    window.astorm.state
      .get('memo')
      .then(function (res) {
        var sid = (res && res.sessionId) || ''
        $('state-sid').textContent = sid ? sid.slice(0, 10) + '…' : '（未绑定会话，状态暂不保存）'
        $('state-memo').value = res && res.value != null ? String(res.value) : ''
      })
      .catch(function () {
        $('state-sid').textContent = '读取失败'
      })
  }

  $('state-save').addEventListener('click', function () {
    window.astorm.state
      .set('memo', $('state-memo').value)
      .then(function () {})
      .catch(function (err) {
        alert('保存失败：' + err.message)
      })
  })

  $('state-clear').addEventListener('click', function () {
    $('state-memo').value = ''
    window.astorm.state.set('memo', '').catch(function (err) {
      alert('清空失败：' + err.message)
    })
  })

  /* ---------- ⑥ 接收应用消息 ---------- */

  // 页签全局共享:切会话时 iframe 不销毁,宿主推送 app.session.change;
  // 收到后重新读取该会话自己的状态(⑤ 卡片随之刷新)
  window.astorm.app.onSessionChange(function () {
    loadSessionState()
  })

  window.astorm.app.onMessage(function (data) {
    var list = $('msg-list')
    var empty = list.querySelector('.d-msg-empty')
    if (empty) empty.remove()
    var li = document.createElement('li')
    var text = String((data && data.text) || '')
    var from = String((data && data.from) || '应用')
    li.appendChild(document.createTextNode(text))
    var meta = document.createElement('span')
    meta.className = 'd-msg-meta'
    meta.textContent = from + ' · ' + new Date(data && data.ts ? data.ts : Date.now()).toLocaleTimeString('zh-CN', { hour12: false })
    li.appendChild(meta)
    list.prepend(li)
  })
})()