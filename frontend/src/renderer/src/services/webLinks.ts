/**
 * webLinks — 浏览器部署的「新窗口 → 内置浏览器」路由（桌面主进程 setWindowOpenHandler 的等价物）。
 *
 * 背景：桌面上 `window.open` / `target=_blank` 由主进程接管——有浏览器桥就转成**内置新页签**
 * （`openPopupAsTab`），没有桥才交系统浏览器。浏览器部署（web 壳）没有主进程，`window.open`
 * 会直接开在用户自己的浏览器里，表现为「点链接跳出应用」。
 *
 * 分工：`web-bridge.js` 只负责**拦截**（http(s)/file 的 window.open → cancelable 的
 * `astorm:open-url` 事件）；本模块负责**落地**：
 *
 * - 面板已开且当前激活的是浏览器页签 → 派发 `astorm:browser-navigate`，由该面板实例**就地导航**。
 *   容器里一个会话只有一台服务端浏览器、面板页签都镜像它的活动页，因此「开新面板页签」只会
 *   看到同一页；就地导航才是用户预期（与页面内 `target=_blank` 的服务端处理保持一致）。
 * - 面板没开（或激活的不是浏览器页签）→ 新建一个内置浏览器页签，并留下待办导航请求，
 *   由新面板实例挂载后消费（`takePendingNav`），避免「开了页签却还停在旧页」。
 *
 * 桌面（Electron，`isWebShell=false`）不启用：那条路由由主进程统一处理。
 */
import { isWebShell } from '@/utils/platform'
import { useSessionsStore } from '@/stores/sessions'
import { useTabsStore } from '@/stores/tabs'
import { BROWSER_DESKTOP_1280 } from '@/utils/browserViewport'

interface OpenUrlDetail {
  url?: string
}

/** 面板尚未挂载时暂存的导航请求（新开浏览器页签后由该面板实例消费一次） */
let pendingNav: string | null = null

/** 面板实例挂载时调用：若本页签的网址正是待办导航目标 → 消费并返回 true（只消费一次） */
export function takePendingNav(url: string): boolean {
  if (pendingNav && url && pendingNav === url) {
    pendingNav = null
    return true
  }
  return false
}

/** 初始化（在 App.vue 启动时调用一次；非 web 壳或重复调用都安全跳过） */
export function initWebShellLinkRouting(): void {
  if (!isWebShell) return
  if ((window as unknown as { __astormLinkRouting?: boolean }).__astormLinkRouting) return
  ;(window as unknown as { __astormLinkRouting?: boolean }).__astormLinkRouting = true

  const sessions = useSessionsStore()
  const tabs = useTabsStore()

  window.addEventListener('astorm:open-url', (raw) => {
    const ev = raw as CustomEvent<OpenUrlDetail>
    const url = ev.detail?.url?.trim()
    if (!url) return
    // 只接内置浏览器吃得下的协议；其余（mailto:、自定义协议…）交回浏览器默认行为
    if (!/^(https?|file):/i.test(url)) return
    // 页签集所属会话优先（bindSessions 绑定的那个），其次全局 activeId：
    // 启动早期 activeId 可能还没就绪，漏掉会让链接退回浏览器原生行为
    const sessionId = tabs.currentSessionId || sessions.activeId
    if (!sessionId) return

    // ① 让当前激活的浏览器面板就地导航（cancelable：无人接管 = 面板没开/激活的不是浏览器页签）
    const nav = new CustomEvent('astorm:browser-navigate', { detail: { url }, cancelable: true })
    if (!window.dispatchEvent(nav)) {
      ev.preventDefault()
      return
    }
    // ② 面板没开：开一个内置浏览器页签，并把导航请求交给它的面板实例
    pendingNav = url
    tabs.openBrowserTabFor(sessionId, url, BROWSER_DESKTOP_1280, { newTab: true })
    ev.preventDefault()
  })
}
