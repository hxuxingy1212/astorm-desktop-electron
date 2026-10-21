// 浏览器画面流 WS 探针（gap e2e 用）：按 mode 驱动页面并汇总收帧/导航结果。
//
// 用法: node browser-ws-probe.mjs <frames|popup> <port> <token> <sid>
//   frames — 收帧 + 鼠标点击 + 键盘输入，输出 {frames, titles, errors, frameSizes, stateSizes}
//   popup  — 页面内 target=_blank / window.open 本页导航，输出 {urls, titles}
//
// WebSocket 解析：Node 22+ 用内置 WebSocket；旧版本回退到 frontend 的 ws 依赖。
// 任一来源都不可用时打印明确错误并退出码 2（不再让 ReferenceError 被 2>/dev/null 吞掉）。

import { createRequire } from 'node:module'
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const [mode, port, token, sid] = process.argv.slice(2)
if (!mode || !port || !token || !sid) {
  console.error('用法: browser-ws-probe.mjs <frames|popup> <port> <token> <sid>')
  process.exit(2)
}
if (mode !== 'frames' && mode !== 'popup') {
  console.error(`未知 mode: ${mode}`)
  process.exit(2)
}

function resolveWebSocket() {
  if (typeof WebSocket !== 'undefined') return WebSocket
  const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')
  const wsDir = join(root, 'frontend', 'node_modules', 'ws')
  if (!existsSync(wsDir)) {
    console.error('此脚本需要 Node 22+（内置 WebSocket），或先执行 pnpm -C frontend install')
    process.exit(2)
  }
  return createRequire(import.meta.url)(wsDir)
}

const WebSocketImpl = resolveWebSocket()

// 统一事件绑定：global WebSocket 走 addEventListener；ws 库走 EventEmitter。
// message 一律归一化为 { data: string } 再交给业务回调（与浏览器 MessageEvent 对齐）。
function bindEvent(ws, type, handler) {
  if (typeof ws.addEventListener === 'function') {
    ws.addEventListener(type, (ev) => {
      handler(type === 'message' ? { data: String(ev.data) } : ev)
    })
  } else if (type === 'message') {
    ws.on('message', (data) => handler({ data: String(data) }))
  } else {
    ws.on(type, handler)
  }
}

const ws = new WebSocketImpl(
  `ws://127.0.0.1:${port}/api/v1/browser/ws?token=${token}&session_id=${sid}`
)

let done = false
bindEvent(ws, 'error', (ev) => {
  console.error('WS error:', ev?.message || ev?.data || 'unknown')
})
bindEvent(ws, 'close', () => {
  if (!done) {
    console.error('WS 在探针结束前被关闭')
    process.exit(1)
  }
})

const send = (o) => ws.send(JSON.stringify(o))
const finish = (result) => {
  done = true
  console.log(JSON.stringify(result))
  ws.close()
  process.exit(0)
}

if (mode === 'frames') {
  // ①b 画面流：收帧 + 聚焦点击 + 键盘输入（title 变化即证据）
  let frames = 0
  const titles = []
  const errors = []
  const frameSizes = []
  const stateSizes = []
  bindEvent(ws, 'message', ({ data }) => {
    const m = JSON.parse(data)
    if (m.type === 'frame') {
      frames++
      frameSizes.push(`${m.width}x${m.height}`)
    } else if (m.type === 'state') {
      titles.push(m.data.title)
      stateSizes.push(`${m.data.width}x${m.data.height}`)
    } else if (m.type === 'error') {
      errors.push(m.message)
    }
  })
  bindEvent(ws, 'open', () => send({ type: 'nav', url: 'file:///data/workspace/ws-page.html' }))
  setTimeout(() => send({ type: 'mouse', action: 'click', x: 120, y: 215 }), 2500)
  setTimeout(() => send({ type: 'text', text: 'ZCode' }), 3500)
  setTimeout(
    () =>
      finish({
        frames,
        titles,
        errors,
        frameSizes,
        stateSizes
      }),
    6000
  )
} else {
  // ①c popup：页面内 target=_blank / window.open → 本页导航
  const urls = []
  const titles = []
  bindEvent(ws, 'message', ({ data }) => {
    const m = JSON.parse(data)
    if (m.type === 'state') {
      urls.push(m.data.url)
      titles.push(m.data.title)
    }
  })
  bindEvent(ws, 'open', () => send({ type: 'nav', url: 'file:///data/workspace/popup-page.html' }))
  setTimeout(() => send({ type: 'mouse', action: 'click', x: 640, y: 60 }), 2500) // 点 _blank 链接
  setTimeout(() => send({ type: 'nav', url: 'file:///data/workspace/popup-page.html' }), 4500)
  setTimeout(() => send({ type: 'mouse', action: 'click', x: 60, y: 210 }), 6500) // 点 window.open 按钮
  setTimeout(() => finish({ urls, titles }), 9000)
}
