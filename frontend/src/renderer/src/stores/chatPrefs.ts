import { defineStore } from 'pinia'
import { useModelsStore } from './models'

/** 对话模式:计划 / 执行 / 自动 */
export type ChatMode = 'plan' | 'build' | 'auto'
/** 权限模式:手动授权 / 自动授权 */
export type ChatPermission = 'ask' | 'allow'

/**
 * **新建会话的初始设置草稿**(供应商/模型 + 模式/权限/思考强度)。
 *
 * 注意职责边界:会话级设置(模式/审批/思考强度/模型/供应商)是**会话的持久化属性**,
 * 真相在后端会话记录里(sessions store 的 `settingsOf` 读取、`setSessionSettings` 改写,
 * 运行中改动经 `chat:update-session-settings` 立即生效)。本 store 只在
 * 「会话还没创建」时有意义 —— 新建页选的这套值随 create_session 一次性落库,
 * 也是 `settingsOf` 在会话无任何记录时的最后一档回落。
 * 内存态、不持久化:未创建的会话没有可落盘的载体。
 */
export const useChatPrefsStore = defineStore('chatPrefs', {
  state: () => ({
    /** 选中路径 [供应商名, 模型名];未配置模型时为空(输入栏显示「无模型配置」) */
    selectedPath: [] as string[],
    mode: 'plan' as ChatMode,
    permission: 'ask' as ChatPermission,
    thinkingLevel: 5
  }),
  actions: {
    /**
     * 按优先级(供应商 order 顺序)把选中回落/初始化到第一个含模型的供应商+其第一个模型。
     * 当前选中仍有效时不打扰用户;选中项被删或配置变更后自动回落;无模型配置则清空。
     */
    applyDefaultSelection(): void {
      const models = useModelsStore()
      const [pName, mName] = this.selectedPath
      if (pName && mName) {
        const p = models.providers.find((x) => x.name === pName)
        if (p && p.models.some((m) => m.name === mName)) return
      }
      for (const p of models.providers) {
        if (!p.models.length) continue
        const path = [p.name, p.models[0].name]
        if (path[0] === pName && path[1] === mName) return
        this.selectedPath = path
        return
      }
      if (this.selectedPath.length) this.selectedPath = []
    }
  }
})