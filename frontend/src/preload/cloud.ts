/**
 * cloud preload —— 云端壳子的**专用** preload（本机模式不用它）。
 *
 * 云端模式下窗口加载的是**远端服务端托管的 Web UI**，所以这里只做一件事：
 * 在页面脚本跑起来之前，把访问令牌写进该 origin 的 localStorage，免得用户对着
 * 登录遮罩再输一遍（Web UI 的令牌键见 `renderer/public/web-bridge.js` 的 TOKEN_KEY）。
 *
 * **刻意不暴露 `window.api`**：Web UI 的 `web-bridge.js` 以「`window.api` 已存在 = 桌面
 * preload 桥」自退（桌面走 IPC，web 走 HTTP/WS）；这里若挂 `window.api`，远端页面会误判成
 * 桌面形态去调本机 IPC，整条数据链路就错了。壳子级的操作（连接方式…）走原生菜单。
 */
import { ipcRenderer } from 'electron'

const TOKEN_KEY = 'astorm:web:token'

try {
  const token = (ipcRenderer.sendSync('shellConnection:token-sync') as string) || ''
  if (token) localStorage.setItem(TOKEN_KEY, token)
} catch (err) {
  // 写不进去也不致命：Web UI 会退回自己的令牌遮罩，用户手输一次即可
  console.warn('[cloud-preload] 令牌注入失败:', (err as Error)?.message)
}
