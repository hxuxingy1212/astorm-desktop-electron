import { ipcMain } from 'electron'
import {
  napiCancelSession,
  napiCreateSession,
  napiDeleteSession,
  napiForkSession,
  napiGetSystemPrompt,
  napiInterruptMessage,
  napiArchivedMessagesBefore,
  napiListMessages,
  napiListSessions,
  napiReopenInterrupt,
  napiRewindToUserMessage,
  napiSendMessage,
  napiStartSession,
  napiSubmitInterrupt,
  napiSummarizeSession,
  napiUpdateSessionGoal,
  napiUpdateSessionSettings
} from './napi'

/**
 * 聊天 IPC:转发渲染层请求到 Rust napi 后端。
 * 请求形状与后端 #[napi(object)] CreateSessionRequest 一致(camelCase,serde rename_all):
 * { model, provider?, workspace?, mode?, permission?, reasoningEffort?, message,
 *   attachments?, autoRun? }
 * autoRun:缺省/true = 创建即起首轮循环;false = 仅创建(落 idle + 生成标题 + 首条消息进历史,不起循环)。
 *
 * 注意:会话级设置(模式/审批/思考强度/模型/供应商)是**会话的持久化属性**,
 * 创建时随首条消息写入,之后经 chat:update-session-settings 单独改写;
 * 普通发消息不再携带这些字段(后端缺省 = 保持),避免 UI 里的旧值把会话改回去。
 */
export function registerChatIpc(): void {
  ipcMain.handle('chat:create-session', (_e, request: unknown): string => napiCreateSession(request))

  ipcMain.handle(
    'chat:send-message',
    (_e, args: { session_id?: string; request?: unknown }): void => {
      if (args?.session_id) napiSendMessage(args.session_id, args.request)
    }
  )

  // 启动:把「仅创建」(autoRun:false)落下、尚未执行的指令跑起来。
  // 不追加新的用户消息——指令早在创建时就写进了会话历史,这里只驱动循环。
  ipcMain.handle('chat:start-session', (_e, sessionId?: string): void => {
    if (sessionId) napiStartSession(sessionId)
  })

  // 立即发送:打入后端打断消息队列并取消当前循环,当前循环收尾后优先执行
  ipcMain.handle(
    'chat:interrupt-message',
    (_e, args: { session_id?: string; request?: unknown }): void => {
      if (args?.session_id) napiInterruptMessage(args.session_id, args.request)
    }
  )

  // 中断决议:审批同意/拒绝、表单回答/忽略(不追加用户消息,就地写回 tool 结果并续跑)
  ipcMain.handle(
    'chat:submit-interrupt',
    (_e, args: { session_id?: string; request?: unknown }): void => {
      if (args?.session_id) napiSubmitInterrupt(args.session_id, args.request)
    }
  )

  ipcMain.handle('chat:cancel', (_e, sessionId?: string): void => {
    if (sessionId) napiCancelSession(sessionId)
  })

  // 撤回历史:保留前 keepUserMessages 条用户消息,删除其后全部(内存+磁盘),并中断当前循环
  ipcMain.handle(
    'chat:rewind',
    (_e, args: { session_id?: string; keepUserMessages?: number }): void => {
      if (args?.session_id && typeof args.keepUserMessages === 'number') {
        napiRewindToUserMessage(args.session_id, args.keepUserMessages)
      }
    }
  )

  // 重开提问:被忽略/被取代的 chat_with_user 用原表单定义重新登记待决中断(表单卡重新弹出)
  ipcMain.handle(
    'chat:reopen-interrupt',
    (_e, args: { session_id?: string; tool_call_id?: string }): void => {
      if (args?.session_id && args.tool_call_id) {
        napiReopenInterrupt(args.session_id, args.tool_call_id)
      }
    }
  )

  // 会话级设置:不必发消息即可改写,后端立即写活记录(下一个工具调用/下一轮即生效)
  ipcMain.handle(
    'chat:update-session-settings',
    (_e, args: { session_id?: string; request?: unknown }): void => {
      if (args?.session_id) napiUpdateSessionSettings(args.session_id, args.request)
    }
  )

  // 会话目标(/goal):新设/修改/删除;不必发消息,后端写活记录并广播 session_info
  ipcMain.handle(
    'chat:update-session-goal',
    (
      _e,
      args: { session_id?: string; goal?: string | null; goal_lang?: string }
    ): void => {
      if (args?.session_id) {
        napiUpdateSessionGoal(args.session_id, args.goal ?? null, args.goal_lang)
      }
    }
  )

  // 基于已有会话 fork 出新会话(共用工作区),返回新会话 id
  ipcMain.handle(
    'chat:fork-session',
    (_e, args: { session_id?: string; beforeFinal?: number }): string => {
      if (!args?.session_id) return ''
      return napiForkSession(args.session_id, args.beforeFinal)
    }
  )

  ipcMain.handle('chat:list-sessions', (): unknown[] => napiListSessions())

  // 手动触发一次会话摘要(/compact);async,返回是否完成摘要
  ipcMain.handle('chat:summarize', (_e, sessionId?: string): Promise<boolean> => {
    if (!sessionId) return Promise.resolve(false)
    return napiSummarizeSession(sessionId)
  })

  ipcMain.handle('chat:list-messages', (_e, sessionId?: string): unknown[] => {
    if (!sessionId) return []
    return napiListMessages(sessionId)
  })

  // 归档分页(「加载更早」):取摘要备份段中更早一轮批次的原始消息(seq 游标,空数组=已到起点)
  ipcMain.handle(
    'chat:archived-before',
    (_e, sessionId?: string, beforeSeq?: number, maxUserTurns?: number): unknown[] => {
      if (!sessionId || typeof beforeSeq !== 'number') return []
      return napiArchivedMessagesBefore(sessionId, beforeSeq, maxUserTurns ?? 5)
    }
  )

  // 会话系统提示词(按当前配置实时拼装,不落盘;供「会话轨迹」展示)
  ipcMain.handle('chat:system-prompt', (_e, sessionId?: string): string => {
    if (!sessionId) return ''
    return napiGetSystemPrompt(sessionId)
  })

  ipcMain.handle('chat:delete-session', (_e, sessionId?: string): void => {
    if (sessionId) napiDeleteSession(sessionId)
  })
}