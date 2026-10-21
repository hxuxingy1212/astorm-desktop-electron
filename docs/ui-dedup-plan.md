# 前端复用修整方案（去重）

> **这份文档是什么**：2026-10-15 对渲染层做的两层审计（UI 层重复实现 + 逻辑层复用）的产出。
> `docs/ui-style.md` §1.7 回答"**该用什么**"；本文件回答"**现有的重复怎么收**"。
>
> **机制（历史方案）**：曾用 `pnpm run check:reuse` —— 复用基线棘轮（`frontend/scripts/ui-reuse-check.mjs` +
> `ui-reuse-baseline.json`）。它把当前已知的重复数量固化，**只允许下降、不允许上涨**；
> 新增违规会打印位置并以退出码返回数量（与 e2e 脚本同一约定）。
>
> **口径**：不追求一次清零。分四批，每批独立可交付、独立可回滚，跑 `typecheck` + 相关 e2e + 截图。
>
> ⚠️ **2026-10-15 回滚说明**：本文件记录的 P0 / P1 / P2 实施（新增共享件、收编重复、令牌层改造）
> **已全部回滚** —— 前端代码回到提交 `04a00f0`，只保留 `docs/` 与 `AGENTS.md` 的规范文本。
> 原因：其中"滚动容器右内边距归零"那条改动被机械地套用到了所有含 `overflow` 的规则上，
> 造成输入区等多个组件的内边距丢失/内容左移，用户要求整体回滚。
> **下面这些清单与方案依然有效**，重新实施时按批次逐条走，并且**每条改动都要限定在自己的选择器范围内**、逐张截图复核。
> `pnpm run check:reuse`（棘轮脚本）与 `pnpm run ui:shots`（截图巡检）也随回滚移除；需要时按本文与
> `docs/ui-style.md` §8 的描述重建。

---

## 1. 现状与量化基线

结论先说：**核心状态层的收敛度是好的**——会话/页签是单例状态机、中断提交只有一个出口（`sessions.submitInterrupt`）、
页签身份只有 `tabKey` 一个判据、卡片与输入框样式已分别压进 `cards.css` / `inputs.css`、面板宽度拖拽只有一份
`useResizeDrag`。**重复都长在"叶子层"**：格式化、错误提示、剪贴板、外部关闭、文件树 UI 状态、设置页 CSS、
内联图标。这些是复制粘贴成本最低、也最容易继续长的地方。

当时的 `check:reuse` 基线（2026-10-15）：

| 规则 | 当前 | 目标 | 批次 |
|---|---|---|---|
| `mono-font-literal` 等宽字体字面量 | 90 | ≤4（终端 / CodeMirror 两处例外） | P0 |
| `error-inline` 内联错误取值 | 62 | ≤10 | P0 |
| `inline-svg` 内联 `<svg>` | 32 | 19（剩下的全是唯一图形，见 §4 不做什么） | ✅ P1 已完成 |
| `spin-keyframes` 本地旋转动画 | 17 | 0 | P0 |
| `dark-check-inline` 深色判定复式 | 13 | 0 | P0 |
| `size-text-local` 本地体积格式化 | 8 | 0 | P0 |
| `click-outside-manual` 手写外部关闭 | 8 | **0** | ✅ P1 已完成 |
| `history-push-chat` 手写会话跳转 | 8 | **0** | ✅ P1 已完成 |
| `to-error-message-local` 各 store 的 `toErrorMessage` | 7 | 0 | P0 |
| `settings-row-style` 设置页本地行样式 | 7 | 0 | P2 |
| `clipboard-direct` 直接写剪贴板 | 5 | 0 | P0 |
| `lucide-class` 遗留 `i-lucide-*` | 2 | 0 | P1 |
| **合计** | **259** | **≤24** | |

### 1.1 P0 已实施（2026-10-15）

| 规则 | 基线 | 现状 |
|---|---|---|
| `mono-font-literal` 等宽字体字面量 | 90 | **0** |
| `error-inline` 内联错误取值 | 62 | **0** |
| `to-error-message-local` 各 store 的 `toErrorMessage` | 7 | **0** |
| `size-text-local` 本地体积格式化 | 8 | **0** |
| `clipboard-direct` 直接写剪贴板 | 5 | **0** |
| `dark-check-inline` 深色判定复式 | 13 | **0** |
| `spin-keyframes` 本地旋转动画 | 17 | **1**（`MacConfirm` 的 border 圆环，规范要求保留） |
| **合计** | **259** | **72** |

落地方式：新建 `utils/errors.ts`（`errorMessage` / `isBackendUnavailable`）、`utils/format.ts`（`formatSize`）、
`utils/clipboard.ts`（`copyText`）、`theme/index.ts` 的 `useIsDark()`（以 `data-theme` 为唯一事实源，避开 store 循环依赖）、
`global.css` 的 `.spin`；**7 个 store 的 `toErrorMessage` 收编**（41 处调用点）；
设置页删掉 14 块被 `!important` 压掉的输入控件死代码；设置行字号对齐 15/14；
`#34c759` / `#ff3b30` 等 macOS 硬编码 → `--success` / `--danger`（开关按 §3.4 裁决走 `--accent-text`）。

剩余 72 处全部是 P1/P2 项（内联 `<svg>` 32、点外关闭 8、会话跳转 8、设置行样式 7、`i-lucide-*` 2…）。

另有**不能机械识别**的重复（需人工判断，见附录 A）：弹窗骨架 5 套、设置页三块重复 CSS 约 300 行、
空态 6 种命名、悬浮层定位 7 套、文件树两套状态约 120 行、中断卡片外壳三连复制约 200 行 CSS。



---

## 2. 最值得做的八项（按 收益 ÷ 风险 排序）

> **P2 进展（2026-10-15）**：设置页三块重复 CSS 已归零、分区已卡片化、文件预览面包屑已落地、
> **弹窗骨架 5 套 → 2 套**（`.fm-modal` / `.agent-modal` / `.scheduled-modal` 并入 `mac-modal` + `mac-modal--solid`，
> 三个文件共删 88 行；保留 `mail-compose` 的独立体量）。
> P2 又完成：**列表拖拽排序 3 套 → 1 个 `useListDragSort`**（支持"放下换序"与"边拖边换序"两种交互）、
> **文件树重复算法 → `utils/fileTree.ts`**（expandBranchIfMatch / folderDescendantHit / filterTree 纯函数，两面板共用）
> + `debounce()`（两处手写定时器）。
> P2 剩余：文件树**状态机**合并（两面板各自持树/展开集，业务上需要独立，算法已共用）、
> 会话派生收口（`searchableRoot` / `byRecency` / `loadHistoriesBatched` 收进 sessions store）。
> **`useStickToBottom` 暂缓**：ChatPanel（60px 阈值 + rAF 合帧）与 ChatTurnGroup（24px + MutationObserver）机制不同，
> 而**滚动跟随行为截图测不出来**、e2e 也没有滚动断言 —— 先补验证手段再动，避免盲改。

| # | 项 | 证据 | 改法 | 风险 |
|---|---|---|---|---|
| 1 | **设置页三块共享 CSS** | `.settings-row` 在 Security:639 / Memory:556 / System:359 / Session:247 **md5 完全相同**；`.section-title` 3 份字节相同 + 2 变体；`.page-footer` 5 份 | 搬进 `styles/settings.css`，页面删块（可选用 `<SettingRow>` 包一层） | 极低（纯搬迁） |
| 2 | **格式化函数** | `sizeText` 4 份字节相同 + 3 份变体（KB 用 `toFixed(1)` 还是 `Math.round` 各家不同）；`fmtTime/fmtIso` 两份字节相同；时长 3 份（`utils/time.ts` / `turnGrouping.ts` / `ChatTurnGroup.vue`） | 新建 `utils/format.ts`（`formatSize`/`formatDuration`），时间统一走 `utils/time.ts` | 极低（纯函数，注意口径：**统一时会出现可见变化**，选一个口径写进规范） |
| 3 | **错误取值与提示** | 7 个 store 各有一份**逐字节相同**的 `toErrorMessage`；内联 `e instanceof Error ? e.message : String(e)` 62 处；`BACKEND_UNAVAILABLE` 只有 memory 一家识别 | 新建 `utils/errors.ts`（`errorMessage` / `isBackendUnavailable`）+ `utils/notify.ts`（`notify.error` / `notify.copy`）；`Message.*` 显式调用不强迁 | 低（文案逐处核对） |
| 4 | **深色判定** | 同一复式 `themeMode==='dark' \|\| (themeMode==='system' && resolveSystemDark())` 在 **13 个文件**重复；`theme/terminal.ts` 还有第三套（读 `data-theme`） | `theme/index.ts` 导出 `useIsDark()`（含 `matchMedia` 监听），13 处改一行；**以 `data-theme` 为唯一事实源** | 低 |
| 5 | **等宽字体** | 去重后 **12 种**写法 / 90 处（`Cascadia Code` 系 3 种、`SF Mono` 系 4 种、`ui-monospace` 系 3 种、`Menlo` 系 2 种）+ 7 文件裸 `monospace` | `base.css` 定义 `--font-mono`，机械替换；终端与 CodeMirror 传 JS 的两处保留独立常量 | 极低 |
| 6 | **旋转动画与空态** | 18 个 `@keyframes *-spin` 全在 `rotate(360deg)`，时长 0.6–1.1s 各不相同；空态 6 种命名（`.empty-state` 2 份字节相同、`.empty-tip` / `.fe-empty` / `.bw-empty`…）+ 8 处 Arco `a-empty` | `global.css` 加 `.spin`；抽 `<EmptyState>`（图标 32 + 标题 + 描述 + 可选动作） | 低（空态有轻微视觉统一） |
| 7 | **悬浮层定位与外部关闭** | `HoverCard.vue` 与 `SessionInfoCard.vue` 的定位算法**逐行同构**；另有 `ModelEffortSelect` / `MentionInput` / `GitGraphCard` / `StatusBar` / `AppSidebar` / `BoardCard` / `MemoryView`，共 **12 处**独立实现；`document mousedown` 各写一遍 | 抽 `composables/useAnchoredPanel.ts`（定位 + 悬停/固定 + 外点关闭）与 `useClickOutside.ts`；`HoverCard` 成为唯一入口 | 中（交互回归面广，需手动过一遍菜单/气泡/提及） |
| 8 | **文件树两套状态** | `WorkspaceFilesPanel.vue` 与 `FileExplorerPanel.vue` 的 `filterTree` / `expandBranchIfMatch` / `folderDescendantHit` / 200ms 防抖**逐字节相同**，合计约 120 行；面板注释自认"两处状态各持一份" | 抽 `composables/useFileTree(loader)` + `utils/fileTree.ts` 纯函数；左栏 git 特性用可选参数保留 | 中（涉及展开/搜索/选中，需回归两侧文件树） |

**只删不抽（顺手做）**：`MemoryView.fmtTime` 与 `MemoryConceptPanel.fmtIso` 同文件直接删一份改 import；
`ScheduledView` 的 `groupByDay` 与 `dayLabel` 补零口径二选一；`ChatTurnGroup.compactDuration` 与 `utils/time.ts` 二选一；
`ModelSettings.vue:252` 的拖拽把手用错图标（`<icon-box>` → `icon-drag-dot-vertical`，规范见 §1.6）；
约 **120 行被 `!important` 压掉的输入框死代码**（各设置页的 `.arco-input-wrapper { border-radius: 6px }` 之类，
已被 `inputs.css:23-30` 覆盖）——删掉以免继续误导后续开发。

---

## 3. 分批实施

### P0｜机械替换（零行为变更，一次做完）

范围：基线里的字体字面量、错误取值、深色判定、体积格式化、剪贴板、`toErrorMessage`、本地旋转动画 + 死代码删除。

- 改动面：约 40 个文件，全部是"取值方式"变化，不改结构。
- 新增 4 个共享件：`utils/format.ts`、`utils/errors.ts`、`utils/notify.ts`、`utils/clipboard.ts`，
  加 `theme/index.ts` 的 `useIsDark()`、`global.css` 的 `.spin`。
- 验收：`pnpm run typecheck` + 按本文规则自查（原 `check:reuse` 脚本已移除）+ 出改动前后截图（体积格式化口径会变）。
- 回滚：单文件级，无副作用。

### P1｜抽 util / composable（低风险）

范围：`useClickOutside` / `useAnchoredPanel`、`useListDragSort`、`useStickToBottom`、
`utils/debounce`、会话派生（`searchableRoot` / `byRecency` / `loadHistoriesBatched` 收进 sessions store）、
`openSessionRoute`、内联 SVG 换手写图标。

- 验收：逐个替换点手动过一遍交互（菜单开合、提及弹层、队列拖拽、面板滚动贴底）；
  人工核对 `click-outside-manual` / `history-push-chat` / `inline-svg` / `lucide-class` 归零或接近（原脚本已移除）。
- 注意：`HoverCard` 语义要收敛到一处——`SessionInfoCard` 直接改用它，而不是复制第二份 `useAnchoredPanel`。

### P2｜抽组件 / 状态（中风险，需回归）

范围：`<SettingRow>` / `<EmptyState>` / `<InterruptBar>` 外壳（审批/表单/邮件三类卡共用，**只共外壳不共内容**）、
`useFileTree`、弹窗骨架收敛为 2 套（`mac-modal` 玻璃 + solid 变体，删 `fm-modal` / `agent-modal` / `scheduled-modal`
三个前缀骨架约 200 行）。

- 验收：设置页 17 个页面过一遍；审批卡/表单卡/邮件卡各触发一次；两侧文件树展开/搜索/选中；
  弹窗（新建技能、新建智能体、知识库编辑、定时任务）各开一次并截图。
- 回滚：按项独立。

### P3｜结构收敛（触核心路径，单独排期）

范围：`PANEL_REGISTRY` 扩成 `kind → component` 全量映射（`PanelContainer.vue` 的 10 分支 if 链退化为查表）；
`tabs.openTarget()` 收敛 `PanelTabBar` 的 file/tool 分支；扩展名分类抽 `utils/fileKinds.ts`
（现在 `panels/index.ts` / `FilePreviewPanel.kindOf` / `TurnArtifacts` / `DeliverablesPanel` / `fileRefs.ts` **五处各写一份**）。

- 验收：页签恢复、宽度档位、宽文件类型判定、交付物打开——这几条正是 e2e 覆盖的路径，跑 `test:e2e:fileref` + 手动开各类文件。
- 风险：页签是核心路径，**必须排独立批次**，不要和别的改动混在一起。

---

## 4. 明确不做（避免过度抽象）

审计确认这些**看起来能抽但不该抽**，写下来是为了防止后来的人"顺手统一"：

- **数据可视化与专属图形**：`GitGraphCard` 提交图谱、`UsageStatsSettings` 甜甜圈、`MacFileIcon` 文件夹造型、
  `WindowControls` 交通灯（11px 微字形）——它们是唯一图形，不是图标。
- **富文本编辑器的对齐 / 列表 / 荧光笔 / 格式刷图标**：手写图标集里没有对应字形，且是照参考图定的造型。
- **composer 的实心发送箭头**：与 `send.svg`（描边纸飞机）是两种造型语言，硬换会破坏观感。
- **`MailCompose` 的大标题骨架**：标题 15px、内边距更宽松，是有意的"撰写"体量差异。
- **`ElementPill` / `QuotePill` / `SkillAttachment`**：走 Arco `a-popover`，三处已经一致，不必再包一层。
- **`ChatPanel` 的错误浮条**：全应用唯一一处带 `role=alert` 的浮条，没有第二处可抽。
- **卡片容器**：`cards.css` 已覆盖 19 个类名，现状就是"加类名进名单"，不要再抽组件。

---

## 5. 验收与防回流

1. **每批结束**：`pnpm run typecheck` + 该批涉及的 e2e（`test:e2e:onboarding` / `test:e2e:fileref`）+ 截图对照。
2. **复用自查**：原 `check:reuse` 棘轮脚本已随回滚移除；重新实施时按 §1 的基线人工核对，或按需重建脚本。
3. **写代码前先查** `docs/ui-style.md` §1.7：共享层没有的，**先建共享件、再在页面里用**，不要先在页面里写。
4. **PR/改动自问**：这段样式/逻辑要是在下一个页面出现，我会不会复制粘贴？会的话现在就抽。

---

## 附录 A：完整重复清单

### A1 UI 层

**内联 `<svg>`（32 处 / 14 文件，RichTextEditor 一家占 17）**

| 位置 | 内容 | 处置 |
|---|---|---|
| `AppSidebar.vue:561`、`StatusBar.vue:354` | 侧栏伸缩按钮 | **换** `panel-left` 字形（需先注册） |
| `StatusBar.vue:567` | 页签伸缩按钮 | **换** `panel-right` 字形 |
| `FileTreeNode.vue:78` | 树展开箭头 | **换** `<icon-right>` |
| `QuotePill.vue:41`、`RichTextEditor.vue:439`、`mentions/chip.ts:113,129` | 引号图形三处各抄一份 | **换** 新增 `quote.svg` |
| `mentions/chip.ts:170,191,260,267-330` | 手抄 feather 路径表（star/undo/plan/fork… 多为已注册字形的重画） | **换** 已有字形 |
| `RichTextEditor.vue:382,394,421,333,401` | caret / 橡皮 / 插图 | **换** `chevron-down` / `eraser` / `image` |
| `RichTextEditor.vue:345,348` | 撤销 / 重做 | 撤销可换，重做**缺字形** → 保留或补字形 |
| `RichTextEditor.vue:78-93,322,388,411,414` | 对齐 / 格式刷 / 荧光笔 / 列表 | **保留**（无对应字形） |
| `ChatInput.vue:829` | 实心发送箭头 | **保留** |
| `MacFileIcon.vue:65,86`、`GitGraphCard.vue:169`、`UsageStatsSettings.vue:343`、`SessionInfoCard.vue:135` | 文件夹造型 / 图谱 / 甜甜圈 / 进度环 | **保留** |
| `MailAvatar.vue:130`、`WindowControls.vue:30,40,64` | 品牌 logo（生成物）/ 交通灯 | **保留** |

**弹窗骨架（5 套 → 收敛 2 套）**

| 骨架 | 定义 | 差异 | 处置 |
|---|---|---|---|
| `mac-modal` | `common/mac-modal.css:23-133` | 14px + 半透明毛玻璃；标题栏/底栏有底 + 发丝线；按钮 28px/7px | 基准（玻璃） |
| `fm-modal` | `KnowledgeView.vue:1689-1752` | 12px、实底、阴影更重、header 无边框、footer 无分割线 | **并入 solid 变体** |
| `agent-modal` | `AgentFormModal.vue:420-530` | 与 fm 同套 + close 热区 + 表单态 | **并入 solid 变体** |
| `scheduled-modal` | `ScheduledView.vue:1899-2009` | 与 agent-modal 归一后 **md5 相同** | **删除** |
| `mail-compose` | `MailCompose.vue:480-518` | 标题 15px、内边距更宽松 | **保留差异** |

**设置页重复 CSS**：`.settings-row`（5 份，md5 相同）、`.section-title`（3 份相同 + 2 变体）、
`.page-footer`（5 份）；最重的 5 个文件：Security(20 处行选择器) / Memory(19) / System(15) / Session(10) / Instruction(9)。

**空态 / 加载**：`a-empty` 8 处（ToolsSettings ×3、SkillsSettings、MemoryView ×2、DeliverablesPanel）；
自绘空态 7 处（`.empty-state` 2 份字节相同 + `.empty-tip` / `.fe-empty` / `.bw-empty` 等）；`a-spin` 8 处；
18 个本地 `@keyframes *-spin`（`CodeReviewPanel` / `BrowserPanel` / `SessionTracePanel` / `FileExplorerPanel` /
`AuditSettings` / `LocalEnvSettings` / `ChildSessionsList` / `ToolCallList` / `ChatTurnGroup` / `ChatPanel` ×2 /
`TaskPlanButtons` / `ChatStepList` / `RichTextEditor` / `MailCompose` / `DeliverablesPanel` / `GitGraphCard` /
`TaskBoardView` / `AppSidebar`），唯一不同形态是 `MacConfirm` 的 border 圆环（保留）。

**悬浮层定位（12 处）**：`HoverCard.vue:44-93`（基准）、`SessionInfoCard.vue:98-121`（逐行同构）、
`ModelEffortSelect.vue:206-240`、`MentionInput.vue:295-314` + `MentionPopover.vue:96`、`GitGraphCard.vue:100-116`、
`StatusBar.vue:36-52`、`AppSidebar.vue:505-523`、`MemoryView.vue:451-452`、`BoardCard.vue:197-209`、
`RichTextEditor.vue:48-54`、`BranchMenu`（受控，保留）。

**未走 token 的 hover / 选中**：`StatusBar.vue:685-721` 与 `AppSidebar.vue:985-998,1583-1588`（窗口按钮 hover 手写双层 rgba）；
`TurnArtifacts.vue:542-560`（亮色硬编码 `#1d1d1f`，暗色下刺眼——**这是 bug 级**）；
选中配方 6 处自绘（`MemoryView:1281`、`KnowledgeView:1324`、`MailList:286`、`ModelSettings:464`、`ChatInput:920`、`ChildSessionsList:186`），
而 `cards.css:60-65` 只声明了两种。

**拖拽**：`useResizeDrag` 已被 3 处复用（正面样板）；列表排序有 **3 套同构**（`PendingQueue.vue:26-35`、
`ModelSettings.vue:150-156,244-249`）；把手图标 `ModelSettings.vue:252` 用错。

**其他**：`.editor-pane` + `:deep(.md-editor)` 14 行块 ×4（AgentFormModal / SkillFormModal / InstructionSettings / ScheduledView）；
`::-webkit-scrollbar` 手写 12 文件（FilePreviewPanel 一文件 9 条）；macOS 绿色开关 `#34c759 !important` 9 处
（其中 7 处在 `settings.css:101-112` 已覆盖的范围内，纯冗余）。

### A2 逻辑层

| 项 | 证据 | 处置 |
|---|---|---|
| **页签二次分发** | `PANEL_REGISTRY` 只有 5 个工具页签；真正的 kind→组件分派在 `PanelContainer.vue:30-49` 的 10 分支 if 链；`tabs.ts:262-284` 特判 browser、`:290` 包装 review；`PanelTabBar.vue:140-146` 自己再判 file/tool | P3：扩 registry，if 链查表化 |
| **扩展名分类 5 套** | `panels/index.ts:34`、`FilePreviewPanel.vue:232-240`、`TurnArtifacts.vue:105-116`、`DeliverablesPanel.vue:246-250`、`utils/fileRefs.ts:33-53` | P3：`utils/fileKinds.ts` |
| **`Message.*` 173 处** | success 50 / error 80 / warning 25 / info 17；最高频三类：内联错误取值 28 + `(e as Error).message` 12 + catch 紧跟 error 45 | P0：`notify.error`（不强制迁移显式调用） |
| **剪贴板 5 份** | `AuditSettings:245` / `ChatMessageItem:100` / `CodeBlock:105` 三份**完全相同**；`MemoryView:303` 变体；`MarkdownLink:77-90` **失败静默**且无卸载清理 | P0：`utils/clipboard.ts` |
| **时间/体积** | 见 §2 第 2 项；`SessionTracePanel:169` 硬编码 `zh-CN`（英文界面不符）；`ScheduledView` 同文件两种补零口径 | P0 |
| **会话派生** | 最近活动倒序 3 份（`AppSidebar:70-77,227`、`TaskBoardView:86`、`SearchModal:106`）；根会话过滤口径已漂移（侧栏排 scheduled / 搜索排两者 / 看板只排 child）；"标题+正文"搜索 2 套；历史批量补拉 2 套（并发都是 3） | P1：收进 sessions store |
| **文件树双份状态** | 见 §2 第 8 项 | P2 |
| **面板保活** | `PanelContainer.vue:79-95` 浏览器与终端各一套遍历；`ChatPanel.vue:386-400` 还有第二套"KeepAlive 滚动补偿" | P2：store 暴露 `terminalSessionIds`，保活策略表集中到 `panels/index.ts` |
| **中断流** | 三类卡无共享状态机（各自 emit，统一经 `submitInterrupt`）；`ChatPanel.vue:914-933` 的互斥条件写了两遍；三份 ~70 行材质 CSS 各抄一遍 | P2：`<InterruptBar>` 外壳 |
| **其他** | 手写 debounce 14 处（无 `utils/debounce`）；`onEvent` 订阅样板 5 处；`fileName(path)` 取末段 3 处；`router.push('/chat/…')` 8 处 | P1 |

### A3 已有但没被充分复用的件

| 已有件 | 现状 | 别处在造的轮子 |
|---|---|---|
| `utils/time.ts` `formatDateTime` | 仅 1 个消费者 | 6 处裸 `toLocaleString` + AuditSettings 手写格式化 |
| `utils/time.ts` `formatDuration` | 仅 1 个消费者 | `turnGrouping.ts:225` + `ChatTurnGroup.vue:112` 各一份 |
| `stores/memory.ts` `backendMissing` | 仅 memory 一家识别 `BACKEND_UNAVAILABLE` | 其余 34 个 invoke 调用方把原始错误串直接抛给用户 |
| `stores/files.ts`（文件树单例） | 右侧面板刻意绕过（面板注释自认） | 整个树状态自建 |
| `utils/index.ts` `todayKey` | 几乎无人用 | 日期键在多处重写 |
| `PANEL_REGISTRY` | 只覆盖 5 个工具页签 | 见 A2 |
| `useResizeDrag` / `cards.css` / `inputs.css` | 已被充分复用 | — **这是正面样板，新增共享件向它们看齐** |
