/**
 * git 分支变更广播。
 *
 * 有些面板的内容取决于**当前分支**（代码审查页签的 diff、文件树的变更标记），但它们各自
 * 按自己的目录来源加载，不会自动知道分支被切走了。这里只放一个自增计数器：
 * 需要跟分支联动的面板 `watch(() => git.rev, reload)` 即可——比引一条事件总线轻得多。
 */
import { defineStore } from 'pinia'

export const useGitStore = defineStore('git', {
  state: () => ({
    /** 分支变更代次（每次 checkout / create-branch 成功后 +1） */
    rev: 0
  }),
  actions: {
    /** 分支已变更：通知依赖当前分支的面板重载 */
    bumpRev(): void {
      this.rev += 1
    }
  }
})
