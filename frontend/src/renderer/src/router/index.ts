import { createRouter, createWebHashHistory } from 'vue-router'
import ChatView from '@/views/ChatView.vue'
import NewChatView from '@/views/NewChatView.vue'
import ScheduledView from '@/views/ScheduledView.vue'
import TaskBoardView from '@/views/TaskBoardView.vue'
import KnowledgeView from '@/views/KnowledgeView.vue'
import MemoryView from '@/views/MemoryView.vue'
import MailView from '@/views/MailView.vue'
import IconsView from '@/views/IconsView.vue'
import SettingsView from '@/components/settings/SettingsView.vue'

/**
 * 路由(与参考工程一致):
 * - /chat/:id?  会话主页(聊天 + 页签面板)
 * - /newChat    新建对话页
 * - /scheduled  定时任务页
 * - /board      任务看板(顶部工具条的看板按钮进入/退出)
 * - /knowledge  我的文件(卡片文件管理器)
 * - /memory     长期记忆(OKF 概念浏览 + 编译触发)
 * - /icons      图标总览(demo:全部图标 + 亮/暗主题对照;不进侧边栏,#/icons 直达)
 * - /settings/:key?  设置页(侧栏设置菜单驱动)
 * 使用 hash 模式,兼容生产环境 file:// 加载。
 */
const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', redirect: '/newChat' },
    { path: '/chat/:id?', component: ChatView },
    { path: '/newChat', component: NewChatView },
    // /scheduled 列任务卡片; /scheduled/:taskId 为任务详情(执行记录), 同一组件按参数切换布局
    { path: '/scheduled/:taskId?', component: ScheduledView },
    { path: '/board', component: TaskBoardView },
    { path: '/knowledge', component: KnowledgeView },
    { path: '/memory', component: MemoryView },
    { path: '/mail', component: MailView },
    { path: '/icons', component: IconsView },
    { path: '/settings/:key?', component: SettingsView }
  ]
})

export default router
