/**
 * 定时任务**定义**变更的实时通道（后端 `scheduled_tasks_changed` 全局事件）。
 *
 * 背景：任务定义不属于任何会话，落库时没有可依附的会话级事件 —— 渲染层此前只能在进入
 * 「定时任务」页 / 任务看板时重拉一次，别处改名、改 cron、启停要等下次进入才更新。
 *
 * 这里只做一件事：订阅那条事件、把版本号 +1；**具体重拉交给各视图**（定时任务页持有任务
 * 列表、看板持有「任务 id → 名称」映射，各自知道自己该拉什么，不该由同步层越俎代庖）。
 * 形状与 `tabsSync` 一致：模块级单例订阅，无后端桥（纯浏览器调试）时静默跳过。
 */
import { readonly, ref } from 'vue'
import type { Ref } from 'vue'

/** 定义变更版本号：任何增删改 / 启停后自增，视图 `watch` 它重拉自己的那份数据 */
const version = ref(0)
/** 事件订阅取消函数（模块级，仅初始化一次） */
let offEvent: (() => void) | null = null

/** 初始化（在 `App.vue` 里调用一次，与 `initTabsSync` 并列） */
export function initSchedulesSync(): void {
  if (!window.api?.backend?.onEvent || offEvent) return
  offEvent = window.api.backend.onEvent((chunk) => {
    const ev = chunk as { type?: string }
    if (ev?.type !== 'scheduled_tasks_changed') return
    version.value += 1
  })
}

/**
 * 读版本号（只读，避免视图误改）。
 * 用法：`watch(useScheduledTasksVersion(), () => void loadTasks())`
 */
export function useScheduledTasksVersion(): Readonly<Ref<number>> {
  return readonly(version)
}
