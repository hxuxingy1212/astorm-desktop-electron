import { defineStore } from 'pinia'

/**
 * 非持久化的 UI 状态。
 * 视图切换(会话/新建/设置)已迁移到 vue-router,见 router/index.ts。
 */
export const useUiStore = defineStore('ui', {
  state: () => ({
    /** 全局搜索弹窗 */
    searchVisible: false
  }),
  actions: {
    openSearch(): void {
      this.searchVisible = true
    },
    closeSearch(): void {
      this.searchVisible = false
    }
  }
})
