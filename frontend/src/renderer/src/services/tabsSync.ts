/**
 * 页签 ↔ 后端同步（`tabs` 工具的镜像与命令通道）。
 *
 * 背景：页签状态只存在于渲染层，而 Rust → 渲染层只有**单向异步事件**（没有返回值）。
 * 因此这里做两件事：
 * 1. **上报**：页签发生任何变化（用户操作或命令执行后）都把该会话快照推给后端
 *    （`update_tabs_state`），供 `tabs list` 同步读取；
 * 2. **下发**：订阅 `tabs_command` 事件 → 交给 `tabs.applyTabCommand(cmd, localId)` 执行
 *    → 立刻回传新状态，并把执行结果作为 `last_command` 回执
 *    （异步失败会出现在下一次 `list` 输出的告警里）。
 *
 * 会话 id 需要**双向映射**：事件与工具用**后端** session id，而页签 store 按**本地** id 索引。
 */
import { watch } from 'vue'
import type { TabCommandResult, TabsCommand } from '@/types'
import { useTabsStore } from '@/stores/tabs'
import { useSessionsStore } from '@/stores/sessions'
import { invoke } from '@/services/backend'

/** 事件订阅的取消函数（模块级，仅初始化一次） */
let offEvent: (() => void) | null = null
/** 上报串行化：避免并发 invoke 乱序覆盖后端镜像 */
let reportQueue: Promise<void> = Promise.resolve()

/**
 * 初始化页签同步（在 `tabs.bindSessions()` 之后调用一次）。
 * 无后端桥（纯浏览器调试）时静默跳过。
 */
export function initTabsSync(): void {
  const tabs = useTabsStore()
  const sessions = useSessionsStore()
  if (!window.api?.backend?.onEvent || offEvent) return

  const localToBackend = (localId: string): string | undefined =>
    sessions.sessions.find((s) => s.id === localId)?.backendId
  const backendToLocal = (backendId: string): string | undefined =>
    sessions.sessions.find((s) => s.backendId === backendId)?.id

  /** 上报某会话的页签快照（可选带上一条命令的回执） */
  const report = (localId: string, result?: TabCommandResult): void => {
    const backendId = localToBackend(localId)
    // 本地 mock 会话（无 backendId）不参与同步
    if (!backendId) return
    if (result) tabs.setTabCommandResult(localId, result)
    const state_json = JSON.stringify(tabs.snapshotForSync(localId))
    reportQueue = reportQueue
      .then(() => invoke('update_tabs_state', { session_id: backendId, state_json }))
      .catch((err) => {
        console.warn('[tabs-sync] 上报失败:', err)
      })
  }

  // ① 下发：后端 tabs 工具的命令（按事件里的 session_id 定位目标会话，与当前在看哪个会话无关）
  offEvent = window.api.backend.onEvent((chunk) => {
    const ev = chunk as TabsCommand
    if (ev?.type !== 'tabs_command') return
    const localId = backendToLocal(ev.session_id ?? '')
    if (!localId) return
    const res = tabs.applyTabCommand(ev, localId)
    report(localId, {
      request_id: ev.request_id ?? '',
      ok: res.ok,
      message: res.message
    })
  })

  // ② 上报：用户驱动的页签变化（当前会话）—— 不防抖，保证镜像准确
  watch(
    [() => tabs.tabs, () => tabs.activeId, () => tabs.collapsed],
    () => {
      if (tabs.currentSessionId) report(tabs.currentSessionId)
    },
    { deep: true }
  )
  // 切换会话本身也要上报（后端可能是第一次见到这个会话）
  watch(
    () => tabs.currentSessionId,
    (id) => {
      if (id) report(id)
    },
    { immediate: true }
  )

  // ③ 浏览器代理桥：把**本会话全部浏览器页签**（含非激活）上报主进程，由它按会话起停
  //    一条多目标 CDP 桥；并订阅主进程命令（createTarget/closeTarget/activateTarget 与弹窗转页签）。
  //    去抖 600ms：重定向/连续导航只上报一次，也给新 iframe 一点载入时间（主进程侧还会轮询等目标）。
  let browserTimer: ReturnType<typeof setTimeout> | null = null
  const reportBrowserTabs = (): void => {
    const agent = window.api?.browserAgent
    const localId = tabs.currentSessionId
    if (!agent?.setTabs || !localId) return
    const backendId = localToBackend(localId)
    if (!backendId) return
    const list = tabs.browserTabs.map((t) => ({
      id: t.id,
      url: t.web?.url ?? '',
      title: t.web?.title,
      active: t.id === tabs.activeId
    }))
    void agent
      .setTabs({ sessionId: backendId, tabs: list, activeId: tabs.activeId })
      .then((st) => {
        if (st && !st.running && st.reason) console.warn('[browser-agent]', st.reason)
      })
      .catch(() => undefined)
  }
  watch(
    [
      // 页签集合/URL/标题/激活项/会话变化都触发上报（join 成一个字符串做浅比较）
      () =>
        tabs.browserTabs
          .map((t) => `${t.id}:${t.web?.url ?? ''}:${t.web?.title ?? ''}`)
          .join('|'),
      () => tabs.activeId,
      () => tabs.currentSessionId
    ],
    () => {
      if (browserTimer) clearTimeout(browserTimer)
      browserTimer = setTimeout(reportBrowserTabs, 600)
    },
    { immediate: true }
  )

  // 主进程命令 → 页签操作 → 回执（下一步的状态变化会经上面的 watch 上报，闭环）
  window.api?.browserAgent?.onCommand((cmd) => {
    const sid = tabs.currentSessionId
    let ok = false
    let tabId: string | undefined
    let message: string | undefined
    if (!sid) {
      message = '前端未绑定会话'
    } else if (cmd.action === 'create') {
      tabId = tabs.createBrowserTabFor(sid, cmd.url ?? '')
      ok = true
    } else if (cmd.action === 'activate' && cmd.tabId) {
      const r = tabs.activateBrowserTabFor(sid, cmd.tabId)
      ok = r.ok
      message = r.message
    } else if (cmd.action === 'close' && cmd.tabId) {
      const r = tabs.closeBrowserTabFor(sid, cmd.tabId)
      ok = r.ok
      message = r.message
    } else {
      message = `未知命令: ${cmd.action}`
    }
    void window.api?.browserAgent?.commandResult({ requestId: cmd.requestId, ok, tabId, message })
  })
}
