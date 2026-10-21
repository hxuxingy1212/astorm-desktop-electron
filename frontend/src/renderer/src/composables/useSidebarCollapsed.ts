import { computed } from 'vue'
import type { ComputedRef } from 'vue'
import { useSettingsStore } from '@/stores/settings'

/**
 * 侧栏折叠的生效状态(AppLayout / Workspace / StatusBar 共用同一规则):
 * - 应用启动后侧栏始终展开(首页需看到完整菜单,不受窗口宽度与上次折叠记忆影响);
 * - 运行期内只认显式值:null = 未指定(展开),true/false = 用户伸缩或页签自动折叠写入的值;
 * - 重启应用即回到展开态(手动折叠不跨启动记忆,持久化值在 loadPersistedSettings 里丢弃)。
 *
 * 这里必须**直接读 store**。此前用「组件内 userToggled 标记 + watch 变更」实现「启动即展开」,
 * 有两个致命副作用:
 *  1. 每个调用点各持一份标记,而按钮写入的是「当前生效态的反值」——一旦 store 里已经是该值
 *     (例如上次退出时持久化了 true),写回同一个值不产生变更,标记永不翻转,点击毫无反应;
 *  2. 晚挂载的实例标记仍为 false,于是同一个 store 在不同组件里算出不同折叠态,
 *     按钮显隐与真实状态错位(折叠态下甚至没有任何可用的展开入口)。
 * 「启动即展开」现在由持久化层负责丢弃历史值,不再需要运行期标记。
 */
export function useSidebarCollapsed(): ComputedRef<boolean> {
  const settings = useSettingsStore()
  return computed(() => settings.sidebarCollapsed ?? false)
}
