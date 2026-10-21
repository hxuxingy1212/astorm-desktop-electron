import { defineStore } from 'pinia'
import { watch } from 'vue'
import type {
  FileTabState,
  PanelTab,
  TabCommandResult,
  TabInfoPayload,
  TabsCommand,
  TabsStatePayload,
  WebTabState,
  WebViewportState,
  SourceItem,
  MemorySource
} from '@/types'
import { useSessionsStore } from '@/stores/sessions'
import { useFilesStore } from '@/stores/files'
import { useSettingsStore } from '@/stores/settings'
import { uid } from '@/utils'
import { BROWSER_DESKTOP_1280, clampZoomPercent } from '@/utils/browserViewport'

/** 某会话的页签集合(当前会话取 live 字段,其他会话取内存快照) */
interface TabSet {
  tabs: PanelTab[]
  activeId: string
  collapsed: boolean
  lastBrowser: WebTabState | null
}

/** 页签定位:index 为 1 基(与后端 list 输出一致),id 优先 */
interface TabTarget {
  index?: number
  id?: string
}

/** 一次页签操作的结果(失败时会作为回执回传给后端) */
interface TabOpResult {
  ok: boolean
  message?: string
}

/**
 * 终端页签关闭 → 显式断开该会话的 PTY。
 * 切换页签/切换会话**不断连**（shell 服务端持久，重开终端复用同一 shell，
 * cwd/环境保持）；只有显式关闭页签才断开。
 */
function closeTerminalPty(chatSessionId: string | undefined): void {
  if (!chatSessionId) return
  const sessions = useSessionsStore()
  const backendId = sessions.sessions.find((s) => s.id === chatSessionId)?.backendId
  if (!backendId) return
  void window.api?.backend
    ?.invoke('pty_close', { session_id: backendId })
    .catch(() => {})
}

/**
 * 单个会话的页签快照(切换会话时换挡)。**只存元数据**:
 * 文件页签剥离已读内容(content 置空)→ 切走即释放文件内存,切回由 FilePreviewPanel 懒加载;
 * 网页/插件/工具页签保留下次恢复所需的最小字段。
 */
interface SessionTabSnapshot {
  tabs: PanelTab[]
  activeId: string
  collapsed: boolean
  /** 浏览器最后一次浏览状态(关闭/全部关闭后重开仍还原;按会话隔离) */
  lastBrowser: WebTabState | null
}

/**
 * 清理历史版本遗留的页签落盘数据:早期实现按会话写 localStorage(`astorm:tabs:session:<id>:v1`)。
 * 页签现已改为**纯运行期状态**(仅内存快照,不落磁盘),旧键不再被读取,
 * 启动时一次性清掉,避免磁盘残留、以及删除会话后留下的孤儿键。
 */
function purgeLegacyTabStorage(): void {
  try {
    const prefix = 'astorm:tabs:session:'
    const keys: string[] = []
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i)
      if (k?.startsWith(prefix)) keys.push(k)
    }
    for (const k of keys) localStorage.removeItem(k)
  } catch {
    /* 存储不可用时忽略 */
  }
}

/** 键前缀构造:与 `tabKey` 同一套规则(open* 新建前用它先算目标键,避免到处手写前缀) */
const tabKeys = {
  file: (filePath: string) => `file:${filePath}`,
  web: (url: string) => `web:${url.trim()}`,
  tool: (toolId: string) => `tool:${toolId}`,
  plugin: (pluginId: string) => `plugin:${pluginId}`,
  session: (sessionId: string) => `session:${sessionId}`,
  memoryConcept: (conceptId: string) => `memory-concept:${conceptId}`,
  memoryRaw: (stem: string) => `memory-raw:${stem}`
}

/** 网页页签键:有 URL 按 URL(同 URL 复用);空白(无 URL / about:blank)按唯一令牌 → 永不复用 */
export function webTabKey(url: string | undefined, id: string): string {
  const u = (url ?? '').trim()
  return u && u !== 'about:blank' ? tabKeys.web(u) : `web:blank:${id}`
}

/**
 * 页签身份的**唯一算法**:key 相同 = 同一个页签(复用/激活),key 不同 = 必须是两个页签。
 *
 * 每个 `open*` 都先按此键找同键页签:命中则激活,否则新建。键由页签内容**实时算出**
 * (不落盘、不需迁移),所以页签一关闭键即消失——「关闭后重开同键」自然得到新页签。
 *
 * 各类型键:
 * - 文件页签     `file:<绝对路径>`        —— 同一路径一个页签(既有意图)
 * - 网页页签     `web:<url>`              —— 同一 URL 复用;**不同 URL 各自成页**(多浏览器页签)
 * - 空白网页页签 `web:blank:<页签 id>`    —— 无 URL / about:blank 时键带**唯一令牌**
 *   (页签 id 即新建时 uid()),故「手动新建空白浏览器页签」每次都是新页签,不会被任何去重吞掉
 * - 工具页签     `tool:<toolId>`          —— **单例**:终端/代码审查/工作区文件/会话轨迹
 *   (浏览器例外:它不是单例,见上一条)
 * - 插件页签     `plugin:<pluginId>`      —— 一个插件一个页签
 * - 子会话页签   `session:<sessionId>`    —— 「未开始」委派的合成 id 得到 `session:pending:<toolCallId>`
 * - 记忆概念     `memory-concept:<conceptId>` / 记忆原文 `memory-raw:<stem>`
 * - 相关资料/相关记忆结果页:无业务身份、每次打开都是新内容 → 键带唯一令牌(`sources:<id>` /
 *   `memory-results:<id>`),从不去重(沿用现状)
 */
export function tabKey(tab: PanelTab): string {
  if (tab.session) return tabKeys.session(tab.session.sessionId)
  if (tab.file) return tabKeys.file(tab.file.filePath)
  if (tab.web) return webTabKey(tab.web.url, tab.id)
  if (tab.pluginId) return tabKeys.plugin(tab.pluginId)
  if (tab.memory) return tabKeys.memoryConcept(tab.memory.conceptId)
  if (tab.memoryRaw) return tabKeys.memoryRaw(tab.memoryRaw.stem)
  if (tab.sources) return `sources:${tab.id}`
  if (tab.memoryResults) return `memory-results:${tab.id}`
  if (tab.toolId) return tabKeys.tool(tab.toolId)
  return `tab:${tab.id}`
}

/** 在给定页签集合里按显式键找同键页签(`tabKey` 是唯一判据) */
function findTabByKey(list: PanelTab[], key: string): PanelTab | undefined {
  return list.find((t) => tabKey(t) === key)
}

/**
 * 页签面板状态:由状态栏工具栏触发开/关;**状态绑定当前会话,每个会话独立一份**。
 * - 插件页签:pluginId 固定面板,一个插件最多一个页签
 * - 工具页签:toolId 固定面板,一个工具最多一个页签;**浏览器页签是多页签例外**——按
 *   `tabKey` 的 `web:<url>` 复用同 URL、不同 URL 各自成页,空白页签每次新建(见 tabKey 注释)
 * - 文件页签:项目工作区中点击文件产生,按文件路径去重,标题 = 文件名
 *
 * **生命周期:纯运行期**——页签只在内存里按会话换挡(切走存快照、切回还原,含折叠态),
 * **不落磁盘**,应用重启/关闭即清空(包括浏览器最后浏览状态)。
 * 切换会话时 `bindSessions` 保存离开会话的**元数据快照**(剥离文件内容,成本受控);
 * 面板(iframe/终端等)随换挡重建,不做跨会话保活。
 */
export const useTabsStore = defineStore('tabs', {
  state: () => ({
    tabs: [] as PanelTab[],
    activeId: '',
    /** 页签面板伸缩:绑定会话,各会话独立记忆 */
    collapsed: true,
    /** 各会话独立的页签快照(内存态,切换会话时换挡) */
    perSession: {} as Record<string, SessionTabSnapshot>,
    /** 当前绑定会话 id(空 = 未绑定/新建页;此时页签清空) */
    currentSessionId: '',
    /** 当前会话浏览器最后浏览状态(网址/标题/视口,供重开还原;纯内存,重启即空) */
    lastBrowser: null as WebTabState | null,
    /** 各会话最近一条页签命令的回执(随上报回给后端,供 `tabs list` 显示失败告警) */
    tabCommandResults: {} as Record<string, TabCommandResult>,
    /** 审查页签聚焦请求:自增序号 + 目标文件路径(对话交付物卡片跳转时置入,面板消费后清空) */
    reviewFocusSeq: 0,
    reviewFocusPath: ''
  }),
  getters: {
    hasTabs: (state) => state.tabs.length > 0,
    activeTab: (state) => state.tabs.find((t) => t.id === state.activeId) ?? null,
    /** 全部浏览器页签(一个会话可有多个,每个对应一个 iframe/OOPIF) */
    browserTabs: (state) => state.tabs.filter((t) => !!t.web)
  },
  actions: {
    /**
     * 绑定会话切换:跟随 sessions.activeId,离开会话时保存其页签快照(元数据,
     * 剥离文件内容)、进入会话时还原其内存快照(未访问过的会话 = 空页签)。
     * 需在应用启动会话列表就绪后调用一次;同时清理历史版本遗留的落盘数据。
     */
    bindSessions(): void {
      purgeLegacyTabStorage()
      const sessions = useSessionsStore()
      watch(
        () => sessions.activeId,
        (id, prev) => {
          if (prev && prev !== id) this.saveSession(prev)
          this.loadSession(id || '')
        },
        { immediate: true }
      )
      // 当前会话页签清空 → 右侧页签面板自动收起（兜底所有关闭路径：单关 / 全关 / 后端
      // `tabs remove`；按会话操作的非当前会话在 removeTabFor 里单独处理）。
      watch(
        () => this.tabs.length,
        (n) => {
          if (n === 0) this.collapsed = true
        }
      )
    },
    /** 保存指定会话的当前页签快照:仅存元数据(文件内容剥离开,切走后内存即释放) */
    saveSession(sessionId: string): void {
      this.perSession[sessionId] = {
        tabs: this.tabs.map((t) => stripTabContent(t)),
        activeId: this.activeId,
        collapsed: this.collapsed,
        lastBrowser: this.lastBrowser
      }
    },
    /**
     * 还原指定会话页签:有内存快照用之(含激活项与折叠态,快照保留 id 故 activeId 直接可用);
     * 未访问过的会话 → 空页签。**不读磁盘**:页签是纯运行期状态,重启后一律为空。
     */
    loadSession(sessionId: string): void {
      this.currentSessionId = sessionId
      const snap = sessionId ? this.perSession[sessionId] : undefined
      if (snap) {
        this.tabs = snap.tabs
        this.activeId = snap.activeId
        this.collapsed = snap.collapsed
        this.lastBrowser = snap.lastBrowser
        return
      }
      this.tabs = []
      this.activeId = ''
      this.collapsed = true
      this.lastBrowser = null
    },
    /** 删除会话的页签内存快照(删除会话时由 sessions store 调用) */
    clearSession(sessionId: string): void {
      delete this.perSession[sessionId]
      delete this.tabCommandResults[sessionId]
    },
    /**
     * 激活指定页签。页签点击 / 切换到已打开页签都走这里;
     * 状态只在内存中,由会话切换时的快照承载,无需落盘。
     */
    activateTab(id: string): void {
      if (!this.tabs.some((t) => t.id === id)) return
      this.activeId = id
    },
    /** 打开工具页签:已打开则激活,未打开则新建;同时展开面板。
     *  label 可选:自定义页签标题(如终端 = 会话目录名);再次打开会更新标题。
     *  工具页签是单例(键 `tool:<toolId>`);**浏览器页签不是**——每次新建都开新页签(见下)。 */
    openTab(toolId: string, label?: string): void {
      this.expand()
      // 浏览器页签:多页签,不能按 toolId 复用。仅当本会话还没有浏览器页签时,
      // 用最后浏览状态还原网址(关闭后重开续看);已有则新建**空白页签**——
      // 空白页签键 = `web:blank:<id>`(唯一令牌),故「先开一个、再手动新建」必得两个页签。
      if (toolId === 'browser') {
        this.openBrowserTabFor(this.currentSessionId, '', BROWSER_DESKTOP_1280, {
          newTab: true,
          restoreLast: true
        })
        return
      }
      const existing = findTabByKey(this.tabs, tabKeys.tool(toolId))
      if (existing) {
        if (label !== undefined) existing.label = label
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = { id: uid(), toolId, label }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /**
     * 打开代码审查页签并（可选）聚焦某个变更文件。
     * 交付物卡片里的变更文件点击走这里：先置聚焦请求，再打开 / 激活审查页签；
     * 面板在挂载或收到请求后展开该文件的 diff（懒加载仍由面板自身按需拉取）。
     */
    openReviewTab(path?: string): void {
      if (path) {
        this.reviewFocusPath = path
        this.reviewFocusSeq += 1
      }
      this.openTab('code-review')
    },
    /** 审查页签消费完聚焦请求后清空（避免切回页签时再次自动展开） */
    clearReviewFocus(): void {
      this.reviewFocusPath = ''
    },
    /** 更新页签标题(终端按会话目录切换时由面板调用) */
    setTabLabel(id: string, label: string): void {
      const tab = this.tabs.find((t) => t.id === id)
      if (tab) tab.label = label
    },
    /** 打开插件页签:已打开则激活,未打开则新建(键 `plugin:<pluginId>`,一个插件最多一个页签;随会话内存快照换挡)。 */
    openPluginTab(pluginId: string, label?: string): void {
      this.expand()
      const existing = findTabByKey(this.tabs, tabKeys.plugin(pluginId))
      if (existing) {
        if (label !== undefined) existing.label = label
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = { id: uid(), pluginId, label }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /**
     * 打开**未开始的子智能体委派页签**：子会话尚未创建，页签只展示任务文本（用户消息）。
     * 子会话创建后由 [`upgradePendingTabs`] 原地升级为实时子会话页签。
     * 键 = `session:pending:<toolCallId>`（合成会话 id，天然唯一 → 同一次委派只一个页签）。
     */
    openPendingSubSessionTab(
      toolCallId: string,
      label: string,
      task: string,
      parentBackendId: string,
      callIndex: number
    ): void {
      this.expand()
      const syntheticId = `pending:${toolCallId}`
      const existing = findTabByKey(this.tabs, tabKeys.session(syntheticId))
      if (existing) {
        existing.session!.label = label
        existing.label = label
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = {
        id: uid(),
        session: {
          sessionId: syntheticId,
          label,
          pendingTask: task,
          pendingToolCallId: toolCallId,
          pendingParentId: parentBackendId,
          pendingCallIndex: callIndex
        },
        label
      }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /**
     * 子会话创建后，把对应「未开始」页签**原地升级**为实时子会话页签：按
     * （父会话 id, 调用下标）对齐已创建子会话，替换 sessionId 并清掉 pending 标记。
     */
    upgradePendingTabs(parentBackendId: string, children: Array<{ id: string; title: string }>): void {
      for (const tab of this.tabs) {
        const s = tab.session
        if (!s?.pendingTask || s.pendingParentId !== parentBackendId) continue
        const idx = s.pendingCallIndex ?? -1
        if (idx < 0 || idx >= children.length) continue
        const child = children[idx]
        s.sessionId = child.id
        s.label = child.title || s.label
        tab.label = s.label
        delete s.pendingTask
        delete s.pendingToolCallId
        delete s.pendingParentId
        delete s.pendingCallIndex
      }
    },
    /**
     * 打开**子会话页签**:按会话 id 去重(键 `session:<sessionId>`,已打开则激活),新子会话新建页签。
     *
     * 子会话由后端 `sub_session` 工具创建,入口是父会话规划胶囊里的子会话列表;
     * 面板渲染它的实时对话(无输入框 / 无规划胶囊,审批与表单照常)。
     */
    openSessionTab(sessionId: string, label?: string): void {
      this.expand()
      const existing = findTabByKey(this.tabs, tabKeys.session(sessionId))
      if (existing) {
        if (label !== undefined) {
          existing.session!.label = label
          existing.label = label
        }
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = {
        id: uid(),
        session: { sessionId, label },
        label
      }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /**
     * 打开文件预览页签:按绝对路径去重(键 `file:<absolutePath>`,已打开则激活),新文件新建页签。
     * 标题 = 文件名;内容由 FilePreviewPanel 懒加载(fileTabState 中央持有)。
     * `state.targetLine` = 正文文件引用带来的行号(面板定位并通过 `revealSeq` 判断是否重新滚动)。
     */
    openFileTab(file: { path: string; name: string }, state: Partial<FileTabState> = {}): void {
      this.expand()
      const existing = findTabByKey(this.tabs, tabKeys.file(file.path))
      if (existing) {
        // 已打开同一路径：来源可"升级"——交付物点开时把已有页签标成 deliverable，
        // 激活时随即按宽档应用宽度（否则从工作区文件面板先开过的会一直是窄档）。
        if (state.origin === 'deliverable' && existing.file && existing.file.origin !== 'deliverable') {
          existing.file = { ...existing.file, origin: 'deliverable' }
        }
        // 带行号的引用重复点开：重新定位（序号自增，面板据此重新滚动，见 FileTabState.revealSeq）
        if (existing.file && state.targetLine !== undefined) {
          existing.file.targetLine = state.targetLine
          existing.file.revealSeq = (existing.file.revealSeq ?? 0) + 1
        }
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = {
        id: uid(),
        file: {
          filePath: file.path,
          fileName: file.name,
          loading: false,
          content: null,
          ...state,
          // 新页签的首次定位序号固定为 1（后续重复点开自增）
          ...(state.targetLine !== undefined
            ? { targetLine: state.targetLine, revealSeq: 1 }
            : {})
        }
      }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /**
     * 更新网页浏览状态:url/title/favicon 由 BrowserPanel 在导航/加载后回写,
     * viewport(视口/缩放)在用户调整时回写。状态只在内存中,随会话快照换挡,不落盘。
     */
    setWebState(tabId: string, state: Partial<WebTabState>): void {
      const tab = this.tabs.find((t) => t.id === tabId && t.web)
      if (tab?.web) Object.assign(tab.web, state)
    },
    /** 更新文件页签的加载状态(内容 / 错误 / 是否二进制),保持与其他 tab 响应式解耦 */
    setFileState(tabId: string, state: Partial<FileTabState>): void {
      const tab = this.tabs.find((t) => t.id === tabId && t.file)
      if (tab?.file) Object.assign(tab.file, state)
    },
    /** 面板展开(打开任何页签时自动展开)。同时按左侧菜单栏状态收缩侧栏:
     *  文件树状态(浏览文件)打开页签不收缩侧栏,留文件树可见;
     *  任务区状态打开页签(含文件预览)收缩侧栏,把宽度让给页签。 */
    expand(): void {
      this.collapsed = false
      this.maybeCollapseSidebar()
    },
    /** 非文件树状态打开页签 → 收缩侧栏腾出宽度;文件树状态保持侧栏展开 */
    maybeCollapseSidebar(): void {
      const files = useFilesStore()
      // 文件树浏览模式(侧栏处于文件树视图)打开页签不收缩侧栏,留文件树可见
      if (files.active) return
      useSettingsStore().setSidebarCollapsed(true)
    },
    /** 页签面板伸缩切换 */
    toggleCollapsed(): void {
      this.collapsed = !this.collapsed
    },
    closeTab(id: string): void {
      const idx = this.tabs.findIndex((t) => t.id === id)
      if (idx < 0) return
      const closed = this.tabs[idx]
      this.tabs.splice(idx, 1)
      if (this.activeId === id) {
        this.activeId = this.tabs[Math.min(idx, this.tabs.length - 1)]?.id ?? ''
      }
      // 终端页签关闭:显式断开 PTY(切换页签不断连,仅显式关闭断连)
      if (closed.toolId === 'terminal') closeTerminalPty(this.currentSessionId)
      // 浏览器页签关闭:记录本会话最后浏览状态(网址/标题/视口),重开据此还原而非空白页
      if (closed.toolId === 'browser' && closed.web?.url) {
        this.lastBrowser = {
          url: closed.web.url,
          title: closed.web.title,
          favicon: closed.web.favicon,
          viewport: copyViewport(closed.web.viewport)
        }
      }
      // 关掉最后一个页签 → 右侧页签面板自动收起（无内容无谓占宽）
      if (this.tabs.length === 0) this.collapsed = true
    },
    closeAll(): void {
      // 全部关闭前:若有终端页签,断开 PTY(与 closeTab 同语义)
      if (this.tabs.some((t) => t.toolId === 'terminal')) closeTerminalPty(this.currentSessionId)
      // 全部关闭前保留浏览器页签的本会话浏览状态(同上)
      const browserWeb = this.tabs.find((t) => t.toolId === 'browser' && t.web?.url)?.web
      this.tabs = []
      this.activeId = ''
      if (browserWeb) {
        this.lastBrowser = {
          url: browserWeb.url,
          title: browserWeb.title,
          favicon: browserWeb.favicon,
          viewport: copyViewport(browserWeb.viewport)
        }
      }
      // 全部关闭 → 面板自动收起
      this.collapsed = true
    },

    /* ───────── 按会话操作(后端 `tabs` 工具用;与「当前在看哪个会话」无关) ─────────
     * 页签状态是二元结构:当前会话用顶层 live 字段,其他会话用 `perSession` 内存快照。
     * 下面统一通过 `tabSetOf`/`writeTabSet` 读写,于是**非当前显示会话**也能被操作
     * ——改的是它的快照,用户之后切回该会话即可看到。 */

    /** 读某会话的页签集合(不回写;快照不存在时返回空集合,不建快照) */
    tabSetOf(sessionId: string): TabSet {
      if (sessionId === this.currentSessionId) {
        return {
          tabs: this.tabs,
          activeId: this.activeId,
          collapsed: this.collapsed,
          lastBrowser: this.lastBrowser
        }
      }
      const snap = this.perSession[sessionId]
      if (!snap) return { tabs: [], activeId: '', collapsed: true, lastBrowser: null }
      return {
        tabs: snap.tabs,
        activeId: snap.activeId,
        collapsed: snap.collapsed,
        lastBrowser: snap.lastBrowser
      }
    },

    /** 写回某会话的页签集合(当前会话写 live 字段,其他会话写快照——不存在则新建) */
    writeTabSet(sessionId: string, set: TabSet): void {
      if (sessionId === this.currentSessionId) {
        this.tabs = set.tabs
        this.activeId = set.activeId
        this.collapsed = set.collapsed
        this.lastBrowser = set.lastBrowser
        // 当前会话:沿用既有交互(展开面板 + 按侧栏状态决定是否让位)
        if (!set.collapsed) this.maybeCollapseSidebar()
        return
      }
      this.perSession[sessionId] = {
        tabs: set.tabs,
        activeId: set.activeId,
        collapsed: set.collapsed,
        lastBrowser: set.lastBrowser
      }
    },

    /** 打开「相关资料」页签：列出本轮的**网页**检索结果卡片。
     *  每次打开都是**新页签**（键 `sources:<id>` 带唯一令牌，从不去重）；面板不展开复用。 */
    openSourcesTab(items: SourceItem[], label?: string): void {
      this.expand()
      const tab: PanelTab = { id: uid(), sources: { items }, label }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /** 打开「相关记忆」页签：列出本轮的**记忆**检索结果卡片。
     *  每次打开都是**新页签**（键 `memory-results:<id>` 带唯一令牌，从不去重）。 */
    openMemoryResultsTab(items: MemorySource[], label?: string): void {
      this.expand()
      const tab: PanelTab = { id: uid(), memoryResults: { items }, label }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /** 打开记忆概念页签：渲染该记忆文档的 markdown 预览（键 `memory-concept:<conceptId>`，同一概念去重/激活） */
    openMemoryConceptTab(conceptId: string, label?: string): void {
      this.expand()
      const existing = findTabByKey(this.tabs, tabKeys.memoryConcept(conceptId))
      if (existing) {
        if (label !== undefined) existing.label = label
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = { id: uid(), memory: { conceptId, label }, label }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /** 打开记忆原文页签：展示 raw 会话底稿（键 `memory-raw:<stem>`，同一 stem 去重/激活） */
    openMemoryRawTab(stem: string, label?: string): void {
      this.expand()
      const existing = findTabByKey(this.tabs, tabKeys.memoryRaw(stem))
      if (existing) {
        if (label !== undefined) existing.label = label
        this.activateTab(existing.id)
        return
      }
      const tab: PanelTab = { id: uid(), memoryRaw: { stem, label }, label }
      this.tabs.push(tab)
      this.activateTab(tab.id)
    },
    /**
     * 按会话打开浏览器页签（键 `web:<url>`，见 tabKey）：
     * - 默认**按 URL 复用**：已打开同 URL 页签 → 只激活（同键即同一页，不重新导航、不清标题）；
     * - 不同 URL / `opts.newTab = true`（弹窗转页签、`tab_new`、主进程 createTarget）→ 新建页签；
     * - `opts.restoreLast`（手动「新建浏览器页签」入口）且该会话**没有已开浏览器页签**时，
     *   用本会话最后浏览状态还原网址/图标/视口（关闭后重开续看），否则新建**空白页签**——
     *   空白页签键 = `web:blank:<id>`（唯一令牌），故每次手动新建都自成一页，不会被去重吞掉。
     */
    openBrowserTabFor(
      sessionId: string,
      url: string,
      _viewport: WebViewportState,
      opts?: { newTab?: boolean; restoreLast?: boolean }
    ): string {
      // 统一视口：桌面 1280（1280×800）+ 50%。忽略调用方传入，保证所有入口一致
      const viewport = BROWSER_DESKTOP_1280
      const set = this.tabSetOf(sessionId)
      const target = url.trim()
      // 只有「显式新开」或「空白请求」才不复用；有 URL 时按 URL 键找同 URL 页签
      const existing =
        opts?.newTab || !target ? undefined : findTabByKey(set.tabs, tabKeys.web(target))
      let id: string
      if (existing?.web) {
        // 同键（同 URL）页签已打开 → 复用：只激活；页面不重新加载，标题/网址保持原值
        existing.web.viewport = { ...viewport }
        set.activeId = existing.id
        id = existing.id
      } else {
        // 续看仅限「本会话还没有浏览器页签」时，避免新页签与已开页签同 URL（同键重复）
        const last = opts?.restoreLast && !set.tabs.some((t) => !!t.web) ? set.lastBrowser : null
        const tab: PanelTab = {
          id: uid(),
          toolId: 'browser',
          web: {
            url: last?.url ?? target,
            title: '',
            favicon: last?.favicon,
            viewport: copyViewport(last?.viewport) ?? { ...viewport }
          }
        }
        set.tabs.push(tab)
        set.activeId = tab.id
        id = tab.id
      }
      set.collapsed = false
      this.writeTabSet(sessionId, set)
      return id
    },

    /**
     * 按会话新建一个浏览器页签(多页签;主进程 `createTarget` / 弹窗转页签用)。
     * **总是新建**(`newTab: true`,即使已有同 URL 页签)且不续看 lastBrowser:url 为空 = 空白页签。
     * 视口统一为桌面 1280 + 50%。返回新页签 id(作为 CDP targetId)。
     */
    createBrowserTabFor(sessionId: string, url: string): string {
      return this.openBrowserTabFor(sessionId, url, BROWSER_DESKTOP_1280, { newTab: true })
    },

    /** 按会话激活某个浏览器页签(主进程 `activateTarget` 用) */
    activateBrowserTabFor(sessionId: string, id: string): TabOpResult {
      return this.switchTabFor(sessionId, { id })
    },

    /** 按会话关闭某个浏览器页签(主进程 `closeTarget` 用) */
    closeBrowserTabFor(sessionId: string, id: string): TabOpResult {
      return this.removeTabFor(sessionId, { id })
    },

    /** 按会话导航某个浏览器页签并激活(地址栏/页内导航回写) */
    navigateBrowserTabFor(sessionId: string, id: string, url: string): void {
      const set = this.tabSetOf(sessionId)
      const tab = set.tabs.find((t) => t.id === id && t.web)
      if (!tab?.web) return
      tab.web.url = url
      tab.web.title = ''
      set.activeId = tab.id
      this.writeTabSet(sessionId, set)
    },

    /** 按会话打开文件预览页签:按绝对路径去重(键 `file:<absolutePath>`,已打开则激活) */
    openFileTabFor(sessionId: string, path: string, name: string): void {
      const set = this.tabSetOf(sessionId)
      const existing = findTabByKey(set.tabs, tabKeys.file(path))
      if (existing) {
        set.activeId = existing.id
      } else {
        const tab: PanelTab = {
          id: uid(),
          file: { filePath: path, fileName: name, loading: false, content: null }
        }
        set.tabs.push(tab)
        set.activeId = tab.id
      }
      set.collapsed = false
      this.writeTabSet(sessionId, set)
    },

    /** 定位页签:index(1 基,与后端 list 输出一致)或 id(id 优先) */
    findTabIndex(set: TabSet, target: TabTarget): number {
      if (target.id) return set.tabs.findIndex((t) => t.id === target.id)
      if (target.index && target.index >= 1) return target.index - 1
      return -1
    },

    /** 按会话切换激活页签 */
    switchTabFor(sessionId: string, target: TabTarget): TabOpResult {
      const set = this.tabSetOf(sessionId)
      if (set.tabs.length === 0) return { ok: false, message: '当前会话没有页签' }
      const i = this.findTabIndex(set, target)
      if (i < 0 || i >= set.tabs.length) {
        return { ok: false, message: `页签不存在(共 ${set.tabs.length} 个)` }
      }
      set.activeId = set.tabs[i].id
      set.collapsed = false
      this.writeTabSet(sessionId, set)
      return { ok: true }
    },

    /** 按会话关闭页签(激活项被关时接位到同索引的右邻居,与 closeTab 同语义) */
    removeTabFor(sessionId: string, target: TabTarget): TabOpResult {
      const set = this.tabSetOf(sessionId)
      if (set.tabs.length === 0) return { ok: false, message: '当前会话没有页签' }
      const i = this.findTabIndex(set, target)
      if (i < 0 || i >= set.tabs.length) {
        return { ok: false, message: `页签不存在(共 ${set.tabs.length} 个)` }
      }
      const closed = set.tabs[i]
      set.tabs = set.tabs.filter((_, k) => k !== i)
      if (set.activeId === closed.id) {
        set.activeId = set.tabs[Math.min(i, set.tabs.length - 1)]?.id ?? ''
      }
      // 关掉浏览器页签时留下最后浏览状态(重开可还原)
      if (closed.toolId === 'browser' && closed.web?.url) {
        set.lastBrowser = {
          url: closed.web.url,
          title: closed.web.title,
          favicon: closed.web.favicon,
          viewport: copyViewport(closed.web.viewport)
        }
      }
      // 关掉最后一个页签 → 该会话面板自动收起（与 closeTab 同语义，非当前会话也生效）
      if (set.tabs.length === 0) set.collapsed = true
      this.writeTabSet(sessionId, set)
      return { ok: true }
    },

    /** 记录某会话最近一条页签命令的执行结果(随上报回传给后端,供 `list` 显示告警) */
    setTabCommandResult(sessionId: string, result: TabCommandResult): void {
      this.tabCommandResults[sessionId] = result
    },

    /**
     * 应用后端 `tabs` 工具下发的一条命令。
     * 放在 store 里(而不是事件订阅处),使「UI 触发的页签操作」与「按会话的前端操作」共用同一套实现。
     * `sessionId` 缺省 = 当前显示会话;后端命令会显式传入目标会话(与用户在哪个会话无关)。
     */
    applyTabCommand(cmd: TabsCommand, sessionId?: string): TabOpResult {
      const sid = sessionId ?? this.currentSessionId
      if (!sid) return { ok: false, message: '前端未绑定会话' }
      switch (cmd.action) {
        case 'open': {
          if (cmd.kind === 'browser') {
            const url = (cmd.url ?? '').trim()
            if (!url) return { ok: false, message: 'open(browser) 缺少 url' }
            this.openBrowserTabFor(
              sid,
              url,
              {
                mode: 'custom',
                w: cmd.width ?? 1366,
                h: cmd.height ?? 800,
                // 尺寸显式给出,故不属于任何预设档位
                presetIndex: -1,
                zoomPercent: clampZoomPercent(cmd.zoom ?? 50)
              },
              { newTab: cmd.new_tab === true }
            )
            return { ok: true }
          }
          if (cmd.kind === 'file') {
            const path = (cmd.path ?? '').trim()
            if (!path) return { ok: false, message: 'open(file) 缺少 path' }
            this.openFileTabFor(sid, path, path.split(/[\\/]/).pop() ?? path)
            return { ok: true }
          }
          return { ok: false, message: `未知的 kind: ${cmd.kind ?? '(空)'}` }
        }
        case 'switch':
          return this.switchTabFor(sid, { index: cmd.index, id: cmd.id })
        case 'remove':
          return this.removeTabFor(sid, { index: cmd.index, id: cmd.id })
        default:
          return { ok: false, message: `未知的 action: ${cmd.action}` }
      }
    },

    /**
     * 产出上报给后端的可序列化快照(供 `tabs` 工具的 `list`)。
     * viewport 里的档位索引换算成**百分比**(后端不必维护档位表)。
     */
    snapshotForSync(sessionId: string): TabsStatePayload {
      const set = this.tabSetOf(sessionId)
      const tabs: TabInfoPayload[] = set.tabs.map((t) => {
        const kind = t.session ? 'session' : t.file ? 'file' : t.web ? 'browser' : t.pluginId ? 'plugin' : 'tool'
        const vp = t.web?.viewport
        return {
          id: t.id,
          kind,
          tool_id: t.toolId,
          plugin_id: t.pluginId,
          label: t.label,
          url: t.web?.url,
          title: t.web?.title,
          path: t.file?.filePath,
          name: t.file?.fileName,
          viewport: vp
            ? {
                mode: vp.mode,
                w: vp.w,
                h: vp.h,
                zoom_percent: vp.zoomPercent
              }
            : undefined,
          active: t.id === set.activeId
        }
      })
      const activeIndex = set.tabs.findIndex((t) => t.id === set.activeId)
      return {
        tabs,
        active_index: activeIndex < 0 ? 0 : activeIndex + 1,
        collapsed: set.collapsed,
        last_command: this.tabCommandResults[sessionId]
      }
    }
  }
})

/** 深拷一份视口/缩放:快照与运行时不能共享同一对象,否则一侧改动会串到另一侧 */
function copyViewport(v?: WebViewportState): WebViewportState | undefined {
  return v ? { ...v } : undefined
}

/** 快照用:剥离文件内容等重字段,只留恢复所需元数据(文件内容切走即释放,回来懒加载)。 */
function stripTabContent(tab: PanelTab): PanelTab {
  if (tab.file) {
    return {
      id: tab.id,
      file: {
        filePath: tab.file.filePath,
        fileName: tab.file.fileName,
        loading: false,
        content: null,
        // 引用行号随快照保留:切回会话高亮仍在;定位序号一并保留,避免换挡时又滚一次
        targetLine: tab.file.targetLine,
        revealSeq: tab.file.revealSeq
      }
    }
  }
  if (tab.web) {
    // 视口/缩放属于运行期状态,随快照保留(否则切会话即丢)
    return {
      id: tab.id,
      toolId: tab.toolId,
      web: {
        url: tab.web.url,
        title: tab.web.title,
        favicon: tab.web.favicon,
        viewport: copyViewport(tab.web.viewport)
      }
    }
  }
  return { ...tab }
}