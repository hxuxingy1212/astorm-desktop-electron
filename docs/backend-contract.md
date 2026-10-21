# 后端契约:模型供应商配置

> 适用:Electron 桌面端智能体应用。供应商配置的持久化由 **Electron 主进程**负责
> (safeStorage 加密 apiKey,配置文件落 `userData/providers.json`);
> **Rust(napi)** 在应用启动/配置变更时接收解密后的完整配置注入 `AppState`,
> 每次请求携带 `provider + model`,后端按 name 从 AppState 匹配。

## 1. 总体架构

```
┌─ Electron 主进程 ────────────────┐        ┌─ Rust (napi, 与主进程同进程) ─┐
│ providers.json (safeStorage 加密)│        │ AppState { providers[] }     │
│        │ 解密(仅在 main 内部)     │        │   baseUrl/apiKey 明文驻内存   │
│        ↓                         │        │   models[] / order           │
│ init_app_state / set_providers ──┼───────▶│ 匹配: name → provider/model   │
│        ▲                         │        │  chat_stream / test_provider │
│ 渲染层触发保存 → 写文件 → 重推     │        └──────────────────────────────┘
└────────┼─────────────────────────┘
         ▲ 渲染层:只做 UI 交互;明文 apiKey 仅出现在设置表单编辑期与发送途中
```

关键安全边界:**apiKey 落盘前安全加密、解密只在主进程内存、注入 napi 也在主进程**;
渲染层拿到的明文只用于设置页表单展示/编辑(见 §6 权衡)。

## 2. 配置形状

### 2.0 目录布局（系统目录 / 用户目录）

```text
<appData>/astorm/                 # 系统目录（应用自身;mac: ~/Library/Application Support/astorm）
├── config/                       # config.json + providers.json + backend-settings.json + security.json + mcp_servers.json + mcp_oauth.json
├── data/                         # 持久化数据（sqlite.db / session/ / index/ / skill_index/）
├── log/                          # 按天滚动日志
├── plugins/                      # 插件目录 + plugins.json
├── skills/                       # 技能 / 技能包（预留）
├── cache/                        # 应用缓存（icons/ = 站点与邮件图标，7 天 TTL）
└── electron/                     # Chromium 内部文件（主进程 userData 重定向至此）

<用户目录>/                       # 用户内容;AppConfig.workspace,默认 ~/astorm,系统设置里可改
├── workspace/                    # 未指定会话空间时的任务工作区（时间戳/定时任务目录归入其下）
└── knowledge/                    # 知识库（真实文件树 + .manifest.json）
```

- 主进程经 napi `getDataPaths()`（**无需 start()**）取系统目录，并在 ready 前把
  `app.setPath('userData')` 重定向到 `<系统目录>/electron`；JSON 配置统一放 `config/`。
- 安全配置（黑白名单 / 防护开关 / 资源限制）单独落 `config/security.json`；旧版本把它
  存在 `backend-settings.json` 的 `security` 段，启动时自动迁移并从原文件删除。
- 用户目录根 = `AppConfig.workspace`（设置「用户数据目录」），相对工作区名解析到
  `<用户目录>/workspace/<名>`，知识库根 = `<用户目录>/knowledge`。

### 2.1 供应商配置

```jsonc
// config/providers.json(schema_version: 1)
{
  "schema_version": 1,
  "providers": [
    {
      "name": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "apiKey": "3f9a...",           // safeStorage 加密后转 hex;storage=plaintext 时为明文
      "storage": "safeStorage",      // "safeStorage" | "plaintext"
      "apiFormat": "chat_completions", // "chat_completions" | "responses" | "anthropic_messages"(缺省 chat_completions)
      "models": [{ "name": "GPT-4o", "order": 0 }],
      "order": 0                     // 0 起,列表第一项即「默认」
    }
  ]
}
```

napi 侧使用的解密后配置(仅存在于主进程 ↔ napi 之间):

```ts
interface AppConfig { providers: ProviderConfig[] }
interface ProviderConfig {
  name: string          // 供应商唯一标识
  baseUrl: string
  apiKey: string        // 明文,由主进程解密后注入
  apiFormat?: 'chat_completions' | 'responses' | 'anthropic_messages'
  models: Array<{ name: string; order: number }>
  order: number
}
```

## 3. 启动与同步时序

1. 应用启动:主进程读取 `providers.json` → 解密 → `init_app_state(config)` 初始化 AppState;
2. 配置变更(设置页增删/改名/排序/保存):渲染层调 `providers:sync`(全量)→ 主进程校验+加密写盘 → `set_providers(全量)` 重推;
3. 每次请求:渲染层携带 `provider + model` → 主进程/渲染层调用 `chat_stream`,Rust 从 AppState 按 name 匹配供应商与其模型列表。

匹配错误码:`config_not_initialized` / `provider_not_found` / `model_not_found` / `storage_error`。

## 4. napi 导出(Rust 侧实测导出,函数名由 napi-rs 自动转 camelCase)

```ts
// 生命周期(astorm-core/src/app/config.rs 为 AppConfig,camelCase,缺省可省)
// config JSON 顶层字段:log / workspace / llm / summary / compact / goal /
//   browser{binary} / security / memory / instructions /
//   office{binDir, skillDir, chromeBin?, sofficePath?, timeoutSec?} /
//   webSearch{defaultEngine, timeoutSec} /
//   webRender{bin, script, profileDir?, concurrency?, queueMax?, idleSecs?, minIntervalMs?}。
//   office(astorm-office 套件,见 docs/office-integration.md):binDir=五 CLI 目录(统一入口 astorm-office +
//   json2docx/xlsx/pptx/pdf,`office` 工具 spawn 用);skillDir=kit skill 树(启动时复制进用户技能目录注册为 builtin 技能
//   `astorm-office`,含「桌面端运行方式」附录;资源缺失则技能与工具不可用);chromeBin/sofficePath
//   转发 CLI 环境变量 ASTORM_CHROME_BIN / ASTORM_SOFFICE_PATH;timeoutSec 单次调用超时(默认 180)。
//   webSearch.defaultEngine ∈ baidu|sogou|bing|duckduckgo|google(缺省 baidu);
//   `search(action:"websearch")` 免 key HTML 抓取,被限流/异常时按**内置顺序**
//   (百度→搜狗→必应→DuckDuckGo→Google)自动回退到下一个引擎。
//   webRender(websearch/webfetch 共用):复用 Electron 自带 Chromium 的**常驻隐藏窗口**渲染网页
//   (支持 JS + stealth 反爬)。bin=Electron 可执行文件、script=page-render.cjs;治理:
//   concurrency(并发窗口,默认 2)、queueMax(排队上限,默认 16)、idleSecs(空闲回收秒,0=常驻)、
//   minIntervalMs(渲染节流)。启动即预热;服务崩溃/超时自愈;进程 kill_on_drop 兜底。
start(config: string): boolean          // JSON 文本;幂等,已运行返回 false
shutdown(): void                        // 取消全部会话循环并复位,可再 start
isRunning(): boolean
version(): string

// 系统目录布局(纯路径计算,无需 start;主进程启动早期重定向 userData / 放置配置用)
getDataPaths(): { appDir, configDir, dataDir, logDir, pluginsDir, electronDir }

// 配置热更新(为避免 shutdown 中断会话,新增 updateConfig——供应商配置即时生效)
updateConfig(config: string): void      // JSON 文本;替换 LLM 注册表(供应商/模型/默认),
                                        // 运行中会话下一轮推理立即生效;不中断会话、不重开存储

// 右侧页签状态镜像(渲染层 → Rust,见 §4.4;IPC 命令名 `update_tabs_state`)
updateTabsState(session_id: string, state_json: string): void
// state_json = TabsState 的 JSON 文本(见 §4.4 形状);非法 JSON / session_id 为空即抛错

// 本机 Chrome Cookie 导入(设置页「自动化控制 → 浏览器数据」,见 §5.2)
// async(内部 spawn_blocking):macOS 首次读钥匙串会弹系统授权框,同步会卡死主进程。
importChromeCookies(): Promise<{
  profile: string,        // 使用的 Chrome Profile 名(如 "Default")
  profileDir: string,     // Profile 目录绝对路径
  total: number,          // Cookie 库总条数(含解不开的)
  skipped: number,        // 因加密方式不支持而跳过(Windows Chrome 127+ app-bound v20)
  failed: number,         // 解密失败
  cookies: Array<{ hostKey, name, value, path, secure, httpOnly,
                   sameSite: "unspecified"|"no_restriction"|"lax"|"strict",
                   expires: number | null }>   // expires = Unix 秒;null = 会话 cookie
}>
// 只读本机 Chrome 数据:先把 Cookies(+ -wal/-shm)复制到临时目录再只读打开,
// 不修改 Chrome 任何文件(与设置页文案一致)。整体性失败抛错(Chrome 未安装 /
// 未找到 Profile / 解密密钥不可用);单条失败不抛错,只计入 skipped/failed。
// 平台:macOS(钥匙串 `Chrome Safe Storage` + PBKDF2-HMAC-SHA1 + AES-128-CBC)、
//       Windows(Local State 的 DPAPI 主密钥 + AES-256-GCM v10);其余平台抛「不支持」。
// 明文不落盘,只经 IPC 交主进程写入 `session.defaultSession`(见 §5.2)。

// 事件流(必须先于 create_session 订阅,无回放;仅一条全局流)
subscribe(): ReadableStream<{ type, session_id, timestamp, ... }>
// type: title | plan_updated | tabs_command | scheduled_tasks_changed | loop_life(started|finished) |
//       reasoning | text | stream_reset | tool_call | tool_result | interrupt | session_stats |
//       done | cancelled | error | user_message
// 全局告警帧: 事件总线积压丢帧时降级为 { type:"error", session_id:null, message:"事件总线积压丢帧: …" },
//   订阅流本身保持存活。前端应把它当作「这一窗事件已丢」→ 节流重拉一次会话列表兜底
//   （渲染层见 stores/sessions.ts 的 scheduleCatchUp）。
// stream_reset:本轮流式输出作废重来(流中途传输中断 → provider 自动重开重发)。
//   前端**必须**丢弃本轮此前已渲染的正文/思考/工具卡,否则两次尝试的内容会被拼在一起。
//   形状: { type:"stream_reset", session_id, timestamp, round }(无 content 字段)
// title(业务层事件,astorm-core 定义):会话标题生成/替换完成。首条消息创建会话时一次 LLM
//   调用同时生成中英文标题；摘要压缩完成后按新摘要重新生成并**替换**会话标题。
//   形状: { type:"title", session_id, timestamp, title, title_zh, title_en }
//   `title` 保留中文版本兼容旧消费者；前端按当前界面语言取 `title_zh` / `title_en`。
//   旧记录没有这两个字段，前端回退 `title`（不做存量迁移）。摘要后同时广播
//   `session_info`（记录已落盘），不依赖下次列表刷新。
// plan_updated(业务层事件,astorm-core 定义):task_list 工具创建/更新(status、详情文档)/
//   删除计划后广播 → 前端即时刷新任务规划胶囊,不必等「消息变化 + 防抖」兜底刷新。
//   形状: { type:"plan_updated", session_id, timestamp, action:
//           "create"|"update_detail"|"update_nodes"|"delete", task_id }
// scheduled_tasks_changed(业务层事件,astorm-napi 定义):定时任务**定义**变更(新增/删除/
//   修改/启停/立即执行后的会话回写)后广播。任务定义不属于任何会话,没有会话级事件可依附,
//   此前前端只能在进入「定时任务」页 / 任务看板时重拉;有了它,改名/启停可即时反映到
//   定时任务列表与看板卡片上的归属徽标。形状: { type:"scheduled_tasks_changed", session_id:null,
//   timestamp }(无其他载荷 —— 定义内容由 list_scheduled_tasks 重拉)
// tabs_command(业务层事件,astorm-core 定义,见 §4.4):tabs 工具对**右侧工作区页签**下发的
//   变更命令 → 渲染层执行后回传新状态(updateTabsState)。形状:
//   { type:"tabs_command", session_id, timestamp, request_id, action:"open"|"switch"|"remove",
//     kind?, url?, path?, width?, height?, zoom?, index?, id? }
// 交付物不再经事件下发（原 artifacts 事件已移除）：`submit_deliverables` 工具（astorm-core
//   `tool::delivery`）在每轮收尾由模型调用，就地富集（存在性过滤 / git 差异统计）后写入
//   sqlite `session_deliverables` 表（按 session_id + round upsert，同轮重调覆盖），前端在
//   对话结束后经 `list_session_deliverables` 拉取渲染（见下方会话命令）。旧会话历史卡片
//   仍回退消息 metadata.artifacts（老数据只读）。

// 会话(请求对象字段为 snake_case:model, provider?, workspace?,
//      mode?, permission?, reasoning_effort?, agent_type?, message, attachments?, auto_run?)
// 注:早先的 git_branch 字段已移除——分支不进会话记录,系统提示词的「会话环境」块
//   每个用户消息对工作区**实时** `git rev-parse --abbrev-ref HEAD` 取当前分支。
// attachments(可选,本地**绝对路径**列表):按**文件内容**分两类,均把描述写进用户消息
//   `metadata.attachments`(前端缩略图 / 文件胶囊 / 冷启动重建据此恢复):
//   - 图片(魔数 JPEG/PNG/GIF/WebP):复制进 `<会话工作区>/.astorm/attachment/`,
//     作为多模态 user 消息随本条消息上送;单条 ≤8 张、单张 ≤20MB、总量 ≤30MB;
//   - 其他文件:**不读内容、不复制**,只把原始路径追加进用户消息文本
//     (「【附件】…未复制进工作区…」段),由模型按类型自行读取(PDF/Office 用 astorm-office 技能)
//     (工作区外的路径读取会走审批卡);单条 ≤20 个。
//   非法/超限项逐项忽略,并以「(以下附件未读入:…)」注明在文本尾部。
// agent_type:智能体 id(见 §4.5);缺省 = 内置默认主智能体(不限制工具/技能)。父会话 id
//   (`pid`)不在 napi 暴露:子会话只由后端 `sub_session` 工具创建。
// auto_run(可选,缺省/`true` = 现在的流程):`false` = **仅创建**——落库(idle) + 生成标题 +
//   把首条消息写进会话历史,**不起 ReAct 循环**;想跑时在该会话里再发一条消息即可。
//   调度器与 `sub_session` 一律传 `true`(后者在等子会话终态,idle 会被当作已结束)。
// mentions(可选):结构化提及,由前端从输入框 chip 收集;缺省 = 无,行为不变。
//   形状(字段 camelCase): { kind: "skill"|"agent"|"file"|"command",
//                          id: string,     // skill 名 / agentId(裸,不含 agent/ 前缀) /
//                                          // 文件**工作区相对路径** / command id
//                          token: string,  // 用户可见「原文」:文件名/路径、智能体名、'$skill' 等
//                          label?: string }[]
// elements(可选):浏览器拾取元素的围栏原文,每项一段 ```element 围栏(前端 chip 收集)。
// quotes(可选):对话正文引用(对话「添加到当前任务」选中的一段)。
//   形状: { text: string,        // 选中正文原文(完整)
//           seq?: number,         // 来源消息序号 #seq
//           role?: "assistant"|"user" }[]
//   正文只保留截断预览;完整文本 + 来源渲染成「【引用对话】」块,同 elements 一起进
//   metadata.refsNote(不渲染),喂模型 / 冷启动重建时拼回 user 消息。
// 正文只保留用户可见的「原文」(@文件→路径、$智能体→名称、元素→编号标签、/命令→/token)。
// 后端把 mentions + elements 渲染成「引用说明」(【引用指令】块 + 元素围栏),**作为消息元数据
// `metadata.refsNote` 落盘(不渲染)**,同时注入**喂模型**副本的尾部;冷启动重建历史时再拼回
// user 消息正文,使重建后的历史与当轮喂模型一致。skill → 调 `skills` 工具 load;agent → 调
// `sub_session` 委派;file → 给出相对+绝对路径并提示 `file_operation` 读取;command 暂不处理。
createSession(request): string          // 返回 UUID
sendMessage(session_id, request): void  // 非阻塞,增量走事件流
// 中断决议(与 sendMessage 并列的第二条会话输入):审批同意/拒绝、表单回答/忽略。
// **不追加用户消息**——决议就地把用户决定写回被中断的 tool 消息(决议即 tool 结果)并续跑;
// 失效/类型不符的决议在入队时即报错(前端提示,不静默丢弃)。
submitInterrupt(session_id, request): void
//   request(字段按 interruptType 取用;**入参 camelCase**——napi `#[napi(object)]`
//   字段名由 napi-rs 转 camelCase,写 snake 会被判为缺字段而整条决议提交失败):
//   { requestId: string, interruptType: "approval"|"form",
//     decision?: "allow"|"deny",   // approval
//     allowScope?: "once"|"session", // approval: 仅 allow 时;session = 记入会话内存
//                                    // 白名单(本会话内同目标不再弹卡,重启即失效;危险命令不纳入)
//     answer?: string,             // form: 表单值对象 JSON 文本
//     ignored?: boolean }          // form: 忽略(落 ignored 终态且不续跑,提问仍可 reopenInterrupt 重开)
//   注意:以上是**提交决议**的入参;中断信封/事件 payload 里的 request_id / interrupt_type
//   仍是 snake_case(两个方向形状不同,勿混)。
// 重开提问:被忽略/被取代的 chat_with_user 用原表单定义重新登记待决中断(换新 request_id),
// 并发 interrupt 事件 → 前端把原表单(空白)弹回输入框位置;仅会话空闲且无其他待决中断时允许。
reopenInterrupt(session_id, tool_call_id): void
cancelSession(session_id): void
// 撤回历史:保留前 keepUserMessages 条用户消息,删除第 N 条(0-based)及其后**全部**消息
// (内存 history 与磁盘活动日志同步截断,并作废被删区间的待决中断);语义 = 立即中断当前循环。
// - 不得跨会话摘要:目标用户消息必须在最新摘要记录(metadata.summary==true)之后,
//   否则报错「不可跨摘要撤回」(摘要前的对话已压缩丢弃,无法回退)。
// - 磁盘截断**不留备份段**(撤回=丢弃);next_seq 不回退(重发消息取更大 seq,回读排序安全)。
// - 会话运行中:同步校验后挂起 + 取消令牌,由 drain 循环在本轮结束后、发终态事件前落地,
//   避免与在飞轮次竞态(前端在 cancelled/done 时两端已一致)。
rewindToUserMessage(session_id, keepUserMessages): void
// 基于已有会话 fork 出新会话:返回新会话 id。新会话**共用源工作区**、新 id、新标题
// (`fork from <源标题|新对话>`);继承源活动日志中「最后一个无工具调用的助手消息」之前的消息
// (含 system / 最近一次摘要记录作上下文前缀)。多次摘要时活动日志本身=系统提示词+最近摘要+
// 保留消息,故天然只继承最近摘要之后的部分。before_final(可选)= 在无工具助手消息中 fork 在其
// 之前的下标(0-based,供逐条 fork);缺省=最后一条;越界报错。新会话状态 idle,后续照常发消息。
forkSession(session_id, before_final?): string
// 手动触发一次会话摘要(`/compact`):async,返回 boolean(是否完成摘要)。
// **上下文占用低于摘要阈值 50% 时 reject**(当前上下文较短,无需生成摘要);会话正在生成时
// reject(请稍后再试)。摘要走 LLM 结构化摘要,把会话历史重写为
// 「系统提示词 + 摘要消息(assistant,metadata.summary=true) + 保留尾部」,旧活动日志留作
// 备份段 message_NNN.jsonl,并递增 summaryTimes(广播 session_info → 前端重载会话)。
// 摘要完成后会按新摘要重生成并替换会话标题（title 事件广播 title_zh/title_en）。
// 历史太短 / 无可拆分点时返回 false(未做摘要)。
summarizeSession(session_id): Promise<boolean>
// 会话级设置的**异步改写**（模式 / 审批 / 思考强度 / 模型 / 供应商）：**不必发消息**。
// 这五项是会话的持久化属性（落在 SessionRecord 里），真相只有一份——后端记录。
// 写的是**内存活记录**，工具策略每个工具调用、LLM 每轮都读它 → 立即生效，
// 不需要打断当前循环；已在等待用户决定的审批卡不受影响（切到自动授权只作用于后续工具调用）。
// 缺省字段 = 保持原值（全缺省 = 无操作）；成功后广播 session_info（前端据此校正乐观值）。
// 入参 camelCase：{ mode?, permission?, reasoningEffort?, model?, provider? }；失败抛错。
updateSessionSettings(session_id, request): void
// 会话目标（`/goal`）的**异步设置 / 修改 / 删除**：不必发消息，写入内存活记录后立即
// 影响下一次目标判定 / 续跑。`goal` 非空 = 新设或修改（预算重置为 0，基线 = 当前累计用量）；
// `goal = null` / 纯空白 = 删除目标。成功后广播 `session_info`（前端据此显示 / 收起目标入口）。
// 目标判定完成或触达预算后**不删除记录**：`GoalState.status` 置为 `completed` / `stopped`，
// 前端继续展示目标与结束状态；只有用户显式删除（null）才移除。
updateSessionGoal(session_id, goal, goal_lang?): void
listSessions(): SessionRecord[]
// 读取会话消息历史(按 seq 升序)。**展示口径**:摘要后的重读会在活动日志之前拼接
// 「摘要备份段(message_NNN.jsonl)的末尾 ≤2 轮用户消息起」的原始消息(去重后的归档回填,
// 让用户仍能看到被压缩掉的前文);喂模型 / 重放 / 撤回判定仍只走活动日志
// (core 内部 load_messages 不拼接,仅 list_messages 展示路径拼接)。
// 读取会话系统提示词(按当前配置实时拼装:静态人设 + 全局/会话空间 AGENTS.md + 智能体指引;
// **不落盘**,仅摘要重写后活动日志里可能留一条可能过期的副本)。供「会话轨迹」页签展示。
getSystemPrompt(session_id): string
deleteSession(session_id): void

// 会话交付物行（sqlite `session_deliverables` 表,按轮次升序;返回 JSON 数组字符串）。
// `submit_deliverables` 工具每轮收尾落库,前端在对话结束后（done / cancelled）与历史
// 装载完成时拉取,把行按当轮用户消息 join 到对话轮次渲染「交付物 / 变更 / 有用记忆」卡片。
listSessionDeliverables(session_id): string
// 元素形如: { sessionId, round, sessionTitle, userMsgSeq, userMessage,
//   deliverables: [{ path, name, size }], changes: [{ path, name, dir, status, additions, deletions }],
//   memoryIds: [string], createdAt, updatedAt }
// round = 落库时后端 question_times（本会话第几条用户消息,1 起;决议续跑与当轮共用轮次,
//   同轮重调覆盖）;userMsgSeq = 当轮用户消息 jsonl seq（前端精确 join;直发消息无 seq 时
//   回退 userMessage 文本匹配）;userMessage 截断至 2000 字符;删除会话级联清行,
//   撤回历史时清理 round > 保留条数的行。

// 单次 LLM 补全(插件 SDK `llm.chat` 通路;不创建会话;事件经全局事件总线回传)
askLlm(request, callId): void            // 启动即返回
// request 字段(入参为 camelCase): { model?, provider?, messages: [{ role, content }],
//   reasoningEffort?: "low"|"medium"|"high"|"max" }  缺省 model/provider 用默认值
// 总线事件(subscribe 流中): {"type":"llm","call_id":callId,
//   "kind":"chunk"|"reasoning"|"reset"|"end"|"error","delta"?,"usage"?,"finishReason"?,"message"?}
// 契约:chunk/reasoning 为增量;end 后无更多;error 后补一条 end
// reset:上游流中途传输中断后自动重开,订阅方应作废**已收到的**增量(老订阅方忽略该
//   kind 时行为与今天一致:两段文本被拼接,不会崩)
// 实现:astorm-napi::ask_llm → astorm-core::AppState::spawn_llm_chat
//      (复用 LlmRegistry 路由客户端与模型 → llm-provider::chat_stream → 事件入全局事件总线)

// 枚举(string enum):mode: plan|execute|auto ... permission: manual|auto
//   ... reasoning_effort: none|low|medium|high|max
```

**参数映射(前端 → 后端)**:mode `build→execute`;permission `ask→manual`、`allow→auto`;思考强度 0-10 → `none/low/medium/high/max`（0→none, 1-3→low, 4-6→medium, 7-8→high, 9-10→max）。

> **会话设置的所有权**:`mode` / `permission` / `reasoning_effort` / `model` / `provider`
> 是**会话的持久化属性**。`createSession` 随首条消息把它们写进会话记录；之后一律走
> `updateSessionSettings`（见上）**单独改写**。`sendMessage` / `interruptMessage` **不再需要**
> 携带这些字段——缺省 = 保持（`sync_mode_permission` 只在显式给出时才回写，兼容旧客户端与
> 定时任务等调用方）。这样排队项 / 插件通路里捕获的旧 UI 值不会把运行中的会话悄悄改回去。

> `mode` 与 `permission` 是两个**正交**的轴：`permission=auto` 只免审批弹卡（可审批项自动同意），
> 不放宽 `mode=plan` 的只读写禁令；详见 §4.2「Plan 只读与权限模式正交」。

### 4.2 中断与决议协议（安全审批 + 向用户提问，同一机制）

工具需要用户介入时（被安全策略拦截待审批 / 向用户提问）**不再引入审批工具**，
而是由工具自身返回**中断信封**并打断本轮循环。审批与提问**同构**，仅 `interrupt_type`
与专属载荷不同 —— 前端按类型渲染审批卡或提问表单，用户决定后经 `submitInterrupt`
交回，后端把决议**就地写回那条 tool 消息**（决议即 tool 结果），随后继续循环。

```jsonc
// 中断信封（= tool_result 内容 = interrupt 事件的 payload = metadata.interrupt.request）
{
  "interrupt": true,
  "interrupt_type": "approval",      // approval | form
  "request_id": "uuid",              // 提交决议时携带
  "tool_call_id": "call_…",          // 前端把卡片关联到对应工具卡
  "message": "…给模型的说明…",
  "approval": {                      // interrupt_type = approval
    "category": "file_write",        // 拒绝分类（7 值，见下）
    "source": "static_precheck",     // 分类来源 / 可信度（见下）
    "targets": ["/tmp/a"],           // 涉及目标（路径 / 主机，可为空）
    "command": "echo x > /tmp/a",    // 被拦操作原文（重放闸门 key）
    "reason": "…拦截明细…"
  }
  // interrupt_type = form 时为:
  // "form": { "message": "…问题…", "tip": null, "fields": [ …见下… ] }
}
```

`form.fields[]`（`chat_with_user` 的字段定义，由 LLM 按工具 schema 生成）：

```jsonc
{
  "name": "form_kind",          // 字段名:仅作回答回传的键,不在界面展示
  "field_type": "single_choice",// text | number | boolean | single_choice | multiple_choice
  "description": "你说的「表单」具体指哪种?",  // 问题原文:界面作为卡片标题展示(schema 中必填)
  "required": true,             // 是否必填:标题后展示「必填/选填」,必填项未答无法提交
  "options": ["HTML 网页表单", "Excel 模板"],  // 选择类字段的候选项
  "true_label": "需要",          // boolean 字段「是」选项文案;其它类型忽略
  "false_label": "不需要"        // boolean 字段「否」选项文案;其它类型忽略
}
```

**回答(`submitInterrupt` 的 `answer`,JSON 文本)**:按字段名索引的对象,值类型随字段类型 ——

| field_type | 值 |
| --- | --- |
| `text` / `number` / `single_choice` | 字符串 |
| `multiple_choice` | 字符串数组 |
| `boolean` | 布尔 `true` / `false` |

- 选择类字段在界面上额外提供一个**自由输入框**(常驻,不必先选「其他」):单选时与选项互斥,多选时作为额外一项**追加到数组末尾**(与已有选项重复时不重复追加)。自由输入的内容并入该字段的值,因此**不产生额外键**。
- 未填写的字段**不出现在对象里**;后端不校验字段类型 / `options` / `required`,也不要求 `answer`
  非空(`{}` 合法),只按字段名回读并渲染成 `用户已回答:\n- 字段: 值` 写回 tool 消息
  (`字段` 用 `name`,`值` 为原样渲染)。`required` 由前端把关(必填项未答时提交按钮禁用)。
- **`boolean` 是/否二选一,两选项初始都不选中("未选"是合法状态)**:用户没点过就不上送该键 ——
  界面不得把未选静默当成 `false`(会把「没回答」伪装成「答了否」)。必填 boolean 未选时无法提交,
  选填可整个不答;全部字段都没填时 `answer` 为 `{}`,渲染为「(用户未填写任何字段)」。

**重开被放弃的提问（`reopenInterrupt`）**：用户点「忽略」后本轮不续跑、会话落 `ignored`（侧栏灰点），
台账里的待决项也已被取走（旧 `request_id` 失效）。但中断终态仍保留 `metadata.interrupt.request.form`，
因此那次提问可以被重新打开：

- 入参：`reopenInterrupt(sessionId, toolCallId)` —— `toolCallId` 即工具卡上的 `call_…`
  （与前端的 `tool.interrupt.request.tool_call_id` 同源）。
- 行为：读回该 tool 消息的 `metadata.interrupt` → 校验（必须是 `form`；状态 ∈
  `ignored` / `superseded` / `interrupted` / `failed`；`form.fields` 非空）→ **换一个新 `request_id`**
  并沿用原 `form` 定义（`tool_call_id` 不变）→ 就地写回 `pending` 标记 → 重新登记待决 → 发 `interrupt` 事件。
- 前端据此把**原表单**（新实例，空白，不保留上次填写）弹回输入框位置；用户提交后与原回答走**完全
  相同**的链路（写回同一条 tool 消息 + 续跑），落盘标记由 `ignored` 变 `resolved`。
- 只在**会话空闲且没有其他待决中断**时允许：运行中（本轮收尾会就地改写这条 tool 消息）、
  已有待决中断（两个等待态互相覆盖）都返回错误，由前端以告警提示。
- 同轮被 `abandon_deferred_calls` 放弃的 deferred 调用**不会被重开复活**（其 `deferred` 已置 `false`），
  由模型按新获得的信息重新发起。
- 重开后的 `pending` 标记与普通待决同构，故冷启动恢复（`get_or_rehydrate` 扫描历史）同样能把它弹回来。

> 兼容：`approval.category` 之前是 `approval.kind`（`file_read` / `file_write` / `network`）。
> 已落盘的旧待决中断记录可能只有 `kind`，读取时按 `category ?? kind` 兜底。

**拒绝分类（8 值，全链路统一）**：`file_read` / `file_write` / `danger_command` / `cwd_change` / `network` / `resource` / `browser` / `mail`。
`danger_command` 覆盖危险命令黑名单命中与资源/策略硬拒。
`cwd_change` 覆盖 bash 命令里**切出会话工作区**的目录切换（硬规则）。
**P2 放宽**：`cd <工作区内路径>`（含相对子目录，`cd build && make`、`(cd sub && npm test)`）现在放行——
它是构建脚本的日常写法，而"命令始终在会话空间内"这条不变量并未被破坏（写保护由内核白名单兜底，
工作区外的写仍会被 Seatbelt 拒）。仍拒绝：目标在工作区外（`cd /tmp`、越界 `../..`）、
目标无法可靠解析（`cd $VAR` / `cd ~` / `cd -` / 通配）、`cd` 无目标，以及 `popd`（依赖目录栈，
静态推不出落点）/ `chdir`。被拒时提示改用 bash 工具的 `cwd` 参数（这也是推荐写法，因为静态分析
对相对路径的解析以 `cwd` 为基准）。它**单独成类**而非并入 `danger_command`——后者会被读成
「命中危险命令黑名单」，而 cd 只是一条工具约定。
`browser` 覆盖 `browser` 工具（内置浏览器 AI 代理操控）自身的策略拒绝：Plan 模式下拒绝网页写操作、
未暴露的危险动作（如 `eval`）、以及域名门禁；来源固定为 `static_precheck`（不经沙箱）。
`mail` 覆盖 `mail` 工具的安全门禁：`send` 走**写邮件确认中断**（`interrupt_type = mail_compose`，
非审批卡）、`move`/`trash`/`junk` 走审批卡、Plan 模式下这些动作硬拒；来源固定为 `static_precheck`。

**分类来源（`source`，表达可信度）**：

| source | 含义 | 可信度 |
|---|---|---|
| `static_precheck` | 执行前的静态预检（解析命令 + 策略比对） | 确定 |
| `sandbox_preflight` | 沙箱预检拦截（黑名单命中，命令未执行） | 确定 |
| `sandbox_runtime` | 沙箱运行期结构化事件（危险命令实时终止 / 驱逐 / 资源超限） | 确定 |
| `stderr_attribution` | 运行期内核拒绝的 stderr 归因（**历史来源**：bash 已不再产生——内核文件拒绝没有分类，按普通失败交回 LLM；枚举保留以兼容历史 `metadata.interrupt`） | **推断** |
| `exit_code` | 退出码归因（rlimit 信号：磁盘 153 / CPU 时间 152） | **推断** |

各分类的可识别性边界（前端对推断来源标注提示）：

| 分类 | 置信度 |
|---|---|
| `danger_command` | 高（预检正则 + 运行期子进程命令行匹配） |
| `cwd_change` | **确定**（`static_precheck`：命令位关键字扫描，先于一切执行） |
| `file_read` / `file_write` | **确定**，只有两个来源：① `static_precheck`——静态分析器识别出空间外读/写（质量取决于 `astorm-sandbox::analyzer` 的读写命令表：`ls`/`stat`/`cat`/`grep` 等只读命令的路径参数算读，`>`/`>>` 与写类命令算写；**漏登记只读命令曾是硬伤**：`ls -la /Users/…` 提不出读引用 → 被误判为写 → Plan 硬拒，修于只读命令表扩充）；② 沙箱的**结构化分类**（`sandbox_preflight` 文件黑名单命中 / `sandbox_runtime` 文件事件）。**内核拒绝不算来源**：越出沙箱白名单的文件访问由 Seatbelt / AppContainer 拦下，只把 EPERM/EACCES 写进进程输出，**沙箱不产生任何分类** → 按普通命令失败交回 LLM（不弹卡、不硬拒、不重跑），不做「事后用静态分析猜方向」。此时 bash 会在结果里追加一句 `[沙箱提示]` 诊断（说明可能的原因与可行动作），**只影响文案，不影响分类与放行判定** |
| `network` | 高（预检 net 黑名单 + 运行期驱逐事件，携带进/出站方向；另有**静态联网意图**这一确定来源，见下） |
| `resource` | 中：超时 / 内存驱逐 / 磁盘 / CPU 时间可归因（后两者靠退出码）；Windows Job 对象超限只有退出码，无分类 |

**网络：内核整片拒绝出站时的补位（`static_precheck`）**。白名单开启且名单为空时 profile 里是
`(deny network-outbound)`，连接在 connect/bind 阶段就被 EPERM，**不会出现在 lsof 里** → 驱逐监视
产生不了结构化事件 → `network` 分类无从产生，联网命令只能以裸 EPERM 失败。为此 bash 增加
**静态联网意图**判定（`astorm-core/tool/net_intent.rs`，只在上述配置下启用）：

- Auto：命中 → 按「按目标扩白名单、沙箱内重跑」放行，审计 `approval=auto`（明细含「静态检测联网意图」）；
- Execute/Manual：命中 → 返回 `network` 审批信封（`source=static_precheck`），会话白名单已覆盖则直接按目标扩白名单重跑；
- 判定取高置信信号：网络客户端程序（`curl`/`ssh`/`uvx`/`kubectl`…，自省参数除外）、
  `程序 + 联网子命令`（`git clone`/`npm install`/`pip install`…）、或「关注程序 + 明确网络目标（URL/IP/host:port）」。
  **宁可漏判不可误判**：命中即按目标放行（P2 起仍在沙箱内），未命中只退回普通失败。

**「自动放行」的真实语义（P2 已改，务必知情）**：Auto 模式与"用户批准"后的重跑**不再是关沙箱**，
而是**按目标扩白名单、仍在沙箱内重跑**（`tool/shell.rs` 的 `RerunExt` / `extend_protection`）：

- 静态可判定的**空间外写** → 把该写目标并入 `write_white`（`{目标}/**`）；
- **空间外读**（批准路径）→ 并入 `read_white`；
- **联网**（静态意图 / 运行期违规）→ 把命令行里识别出的主机并入 `out_white`/`in_white`
  （`domain:`/`ip:` 同语法）；**提不出具体主机**（裸 `git push`）时退化为只关掉联网白名单开关
  （内核不再按方向拒绝出站，主机级只剩黑名单）——文件、资源、危险命令三层照旧生效；
- 因此重跑那次仍是 `protected=true`；只有**用户明确批准**却仍被挡住（内核 EPERM / 驱逐 / 预检再次
  拦截，判据见 `kernel_blocked_again`）时，才**兜底整条无防护重跑**并写一条审计
  （「扩白名单后仍被内核拒绝 → 兜底无防护重跑」），此时结果里 `sandbox.protected=false` 会标出来。

**硬规则永不重跑**这一条不变：危险命令、资源超限、结构化文件分类（Auto 下硬拒）、Plan 只读、`cwd_change`。
即「自动授权」= 对可审批项免弹卡并**按目标放行**，不是关沙箱，也不是放开一切。

**沙箱读放行必须覆盖系统配置区（TLS / DNS）**：Seatbelt profile 的读白名单含 `/private/etc`
（`/etc` 的符号链接实体）与 `/private/var/run/resolv.conf`（`/etc/resolv.conf` 的实体）。缺了它们
**出站放行也白搭**：`curl`/`wget` 启动即 `fopen('/private/etc/ssl/openssl.cnf','rb')`，读不到连 TLS
都初始化不了，只留一条裸 `Operation not permitted`；DNS 也只剩 mDNSResponder 兜底（实测同一 profile
两次 curl 一成一败）。现场表现极像「网络被沙箱禁了」——会话 2026_09_14_00_22_47 正是如此，模型据此
写出「网络也被禁」的错误结论。因此 `[沙箱提示]` 的网络一句**按本次实际策略生成**：只有「联网白名单
开启且名单为空」（内核 `(deny network-outbound)`）时才说拒绝出站，否则明说「未在内核层限制出站」；
沙箱未在内核层生效（总开关关闭 / 平台降级为驱逐监视）时**不加**该提示，避免把真实系统错误说成沙箱拦截。

**Plan（只读）模式的审批语义**：Plan 下**文件写不提供临时审批**——静态预检判定出的写、
以及沙箱**结构化分类**出的写都直接硬拒，tool_result 提示「切换到 Execute 或 Auto 模式」。
理由：Plan 只读，写只有切换模式才能授权；且审批同意后走的是放行重跑（扩白名单/兜底无防护），用一次性审批绕过
只读契约自相矛盾。空间外**读**在 Plan 下仍可审批（读不属于写禁令）。

**Plan 只读与权限模式正交**：`permission = auto`（权限模式）**只免审批卡**，不解除写禁令——
自动审批下的 Plan 写同样硬拒；审批自动化的语义边界是「可审批项自动同意」，不含硬规则。
（与会话模式 `auto` 区分：那是另一个轴，代表全自动执行，本身即非 Plan。）

- `file_operation`：Plan 下**任意**写（含会话空间内）硬拒，先于 auto/台账判定——
  否则 Execute 期遗留的一次性额度可能被消费而放行写。
- `bash`：Plan 下静态分析出的**任意写目标**（含会话空间内、相对路径）硬拒，闸门先于 auto 分支——
  否则 Auto 会按静态空间外写命中做二跑（P2 起为扩白名单，仍在沙箱内），把写目标放出去（硬规则逃生口）。
  零副作用设备（`/dev/null`、`NUL` 等）已由分析器豁免，`2>/dev/null` 惯用法不受影响。
- `browser`：Plan 下写类动作硬拒（先于 auto 判定）。
- 已知残余：bash 的 Plan 写禁令是静态判定，解释器内联路径（如 `python -c` 拼路径）分析不出，
  而沙箱对工作区内写放行，故这类写仍可能落在工作区内成功（收紧沙箱写白名单会使工具缓存 /
  探测型命令大面积失败，属独立硬化议题）。

**bash 的审批卡来源（仅两个合法来源）**：
1. 静态预检的确定分类（分析得出空间外读 / 写 → 直接弹卡，命令不执行）；
2. 沙箱的结构化分类（`Event::Denied/Evicted` 或 `report.denials` 的 `Category`）。

沙箱**没有**给出分类的运行期文件拒绝（无分类，只有内核 EPERM 文本）不弹卡：那不是「分析
不出读写」，而是「根本没有依据」——按普通命令失败返回，由 LLM 自行判断。

方向**未知**时拒绝文案不谎称「写」：说明「无法判定是读还是写（含解释器内读写，保守拒绝）」
并给出出路（读白名单 / 切换模式），分类仍记 `file_write`（保守），前端按推断来源标 `?`。

**静态预检的「空间外」口径与内核读白名单同源（2026-10-08 修）**：静态读预检过去只认「会话工作区 +
安全/会话白名单 + 技能目录」，而内核 profile 明确放行系统区与已登记工具链目录，
`which git; /usr/bin/git --version`、`ls /opt/homebrew/bin` 这类命令因此白弹一张审批卡（真实会话留痕）。
现在两处共用 `astorm-sandbox/src/roots.rs`：Seatbelt profile（`macos_system_read_roots()`）与
`tool::policy::path_kernel_readable()`（工具层判定）取自同一份根清单，口径不许漂移；
工具链目录同理共用 `toolchain::read_allow_dirs()`。Windows 侧按 AppContainer「系统区默认可读」
语义给出同一组根。**只豁免读**——写仍只有会话工作区与显式白名单，判定不变。

**文件访问方向：复制/移动必须分清源与目标**：分析器对 `cp`/`copy`/`install` 取**最后一个**路径参数为写、
其余为读；`mv`/`move`/`ren`/`rename` 的源与目标都算写（源同时算读，因为内容被移走）。此前 `cp`
的全部参数按写处理，`cp /bin/echo x` 被判成「空间外写」→ Auto 模式据此二跑整条命令
（当时是关沙箱；P2 起改为扩白名单。审计留痕：「自动放行（静态检测空间外写：/bin/echo…）后…重跑」）。方向判错等于放大静态
分析的误报，因此方向表改动必须带 `analyzer` 单测（`复制的来源是读_目标是写` 等）。

**bash 工具结果新增 `sandbox` 事实块（2026-10-08）**：`{ protected, backend, evicted, denied, notes,
degraded, output_truncated }`。模型此前只看得到 `returncode`——命令被驱逐、被拦截、**根本没在沙箱里跑**
（兜底无防护重跑 / 总开关关闭 / 强制层降级）全都不可见，这正是把「沙箱拦了」误判成「命令写错了」的根源。
同一轮还改了输出呈现：stdout / stderr **分流上报**（沙箱 `Event::Chunk` 本就带 `StreamKind`，此前被合并、
stderr 恒为空），截断改为**保留头 + 尾**（错误与 `[沙箱提示]` 都在末尾，只留头会把它们切掉），
两路各 4096 字符预算、尾部各留 1024。

**沙箱开发环境（2026-10-08 P1，四项合一）**：

1. **持久 HOME（按工作区）**：`HOME` 指向 `{data_dir}/sandbox/home/<工作区名>-<路径哈希>`
   （`tool::policy::sandbox_home_dir`，FNV-1a 稳定键：同工作区恒定、不同工作区不撞车），
   沙箱创建后**命令结束不删除** → cargo/rustup/uv/npm/pip 缓存与用户级配置跨命令、跨会话复用。
   此前 HOME 是「逐命令一个临时目录、命令后删除」，工具链装完下一条命令就没了（会话 2026-10-08 最长的一段反复）。
   `TMPDIR` / `TEMP` **仍是逐命令临时**（命令结束即删），临时文件不会堆积。
   它**不是**真实 HOME（真实 `~/.ssh`、`~/.gitconfig` 仍不可读）；删 `{data_dir}/sandbox/` 即清空缓存。
2. **工作区内 exec 放行**（`ProtectionConfig::allow_workspace_exec`，默认 true；CLI `--no-workspace-exec`）：
   `.venv/bin/python`、`node_modules/.bin/*`、编译产物、自写脚本可直接执行。理由：解释器本就放行
   （`node -e` 已是任意代码执行），禁止工作区 exec 不构成安全边界，只挡住正常开发流程
   （`uv sync`、npm postinstall、`cargo build` 产物全被它挡过）。子进程仍受文件/网络沙箱约束。
   程序目录一律**别名 + 真实路径双下发**（Seatbelt 按解析后路径匹配；工作区在 `/var`、`/tmp` 下时
   只发别名会漏，实测表现为 `bad interpreter: Operation not permitted`）。
3. **uv 托管 Python 与 Xcode/CLT 开发目录**（`toolchain.rs`：`uv_python_bins` / `developer_roots`）：
   uv 解释器不在 PATH（PATH 上是 Homebrew 口径），只有 venv 符号链接指过来 → 显式登记
   `…/uv/python/*/bin`（归属 `python3`，「本地环境」页禁用即不注入）；Developer 目录按
   `xcode-select -p` → `/Applications/Xcode*.app/Contents/Developer` → `/Library/Developer/CommandLineTools`
   探测（含 Xcode-beta），**只读放行整棵子树**、exec 放行其中的 clang/ld（归属 `clang`）。
4. **开发工具链环境注入**（`toolchain::dev_toolchain_env`）：`DEVELOPER_DIR` + `SDKROOT`。
   macOS 上 **Xcode 选中时 `/usr/bin/cc` 在沙箱内必失败**：shim 会跑 `xcodebuild -sdk … -find clang`，
   而 xcodebuild 启动即加载 `CoreDevice → /System/Library/PrivateFrameworks/MobileDevice.framework/…`，
   该文件带 SIP `restricted` 标记，**Seatbelt 无法放行**（连 literal 白名单也照拒，实测）。
   因此只要有 CLT，就把 `DEVELOPER_DIR` 固定指向 CLT（`cc`/`clang`/`xcrun` 全部走 xcrun 直连，不碰 xcodebuild）
   并给出 `SDKROOT`；仅装 Xcode 时保留 Xcode 目录 + `SDKROOT`（直调 toolchain clang 可用，`cc` shim 仍不可用）。
   配套只读放行 `/Library/Preferences/com.apple.dt.Xcode.plist`（许可状态，读不到则 cc 报
   「You have not agreed to the Xcode license agreements」exit 69），以及 mach 放行
   `com.apple.trustd` / `com.apple.trustd.agent`（系统信任库 TLS 校验；用户钥匙串仍不可达）。
   **实测**（`tests/sandbox_macos_tests.rs::p1_开发链路实测`）：持久 HOME 跨命令保留、工作区脚本直接执行、
   uv python 直接执行、`cc` 编译并运行、`rustc -O` 链接并运行——会话 2026-10-08 的四道墙全部通过。

**回环（loopback）始终放行（2026-10-08）**：`bind` 归内核 `network-inbound` 管、连出归
`network-outbound` 管，而默认策略是「白名单开 + 名单为空」→ 两者一起被 `(deny …)` 禁掉，
**连 127.0.0.1 也被禁**（实测 bind 与 connect 回环都是 EPERM）。后果是"编译通过却起不了本地服务、
自测不了"（dev server / 后端 8088 / `curl healthz` 全废）。现在三层同源放行回环、只放回环：

- 内核 profile：`(allow network-outbound (remote ip "localhost:*"))` 与
  `(allow network-inbound (local ip "localhost:*"))`，**必须排在方向级 deny 之前**（Seatbelt 首条命中生效）；
- 运行期驱逐监视：`decision::decide_net` 在黑名单之后、白名单默认拒绝之前放行
  `NetAddr::is_loopback()`（否则 lsof 轮询会把整棵树 SIGKILL）；
- 静态联网意图：`net_intent` 在"有目标且**全部**是回环"时不算联网意图（`curl http://127.0.0.1:8088/healthz`
  不该弹审批卡）；混合目标仍算（`… && curl https://api.example.com` 要放行）。
显式黑名单仍然优先（用户主动 `ip:127.0.0.1` 进黑名单时照禁）。外网侧策略完全不变。

**内置危险命令清单收敛（2026-10-08）**：原 `rm\s+-rf\s+/` 把**任何绝对路径**删除都当成
"递归删除根目录"、`rm\s+-rf\s+\*` 把 `rm -rf *.o` 也拦下。后果是 `make clean`、清 `node_modules`、
构建脚本里的 `rm -rf /abs/target` 一律硬拒；**更糟的是实时轮询对子进程命令行用同一组正则**，
命中即 SIGKILL —— 删除做了一半被杀，留下 `Directory not empty` 的半删目录（会话 2026-10-08 实测）。
现在 `rm` 类规则**精确到路径深度**：只拦根 `/`（含 `/*`、`/.`）、顶层系统目录本身、`/Users/<用户>`、
`~`/`$HOME`、以及"当前目录全部"（`*` / `.`）；`rm -rf *.o`、`rm -rf ./build`、`rm -rf build`、
`rm -rf /Users/x/proj/node_modules` 一律放行。回归矩阵见 `policy.rs` 的
`内置rm规则只拦灾难_不误伤构建清理`（20 拦 + 12 放）。清单在设置页可编辑（用户副本优先）。

**「额外可执行目录」用户白名单（2026-10-08）**：安全配置新增 `security.execWhite: string[]`
（设置页「文件访问名单」区内，每行一个目录）。用途：放行**非常规安装位置**的工具链/编译器
——自动探测只覆盖 PATH 上能解析到的工具，手动解压的编译器、独立工具链
（如 `/Library/Developer/Toolchains/swift-*.xctoolchain`）、自定义 SDK 进不了白名单，
实测以 `Operation not permitted` **静默失败**；此前**没有任何用户入口**能加 exec 目录
（`readWhite` 只给读，不给执行）。语义：目录本身 exec 放行 + 目录与**父目录**只读放行
（覆盖 bin 旁的 lib/Frameworks，与工具链探测同构）；父目录是 `/` 或 `$HOME` 时**不**放行
（否则一条配置就把根或整个家目录读开）。解析：`astorm_sandbox::toolchain::extra_dirs`；
CLI 对应 `sbox run --exec-white-add <dir>`。回归用例（含反证：不配置时同一命令被内核拒）
见 `tests/sandbox_macos_tests.rs::额外可执行目录放行实测`。

**轻量执行审计（P2）**：每次 bash 执行落一条 `type=bash_exec` 记录——此前审计**只在"有决策"时落盘**
（审批/硬拒/自动放行），普通命令不留痕，事后取证只能靠工具结果本身（2026-10-08 排查一个会话时，
应用日志与审计都查不到执行事实，最后靠 npm 自己的日志才定案）。记录内容：exit 码、是否受防护
（`protected=true` 或「无防护」）、耗时、`cwd`，以及 `timed_out` / `evicted` 摘要；`approval=none`、
不带分类，命令原文进 `command` 字段。审计页类型筛选与标签同步（`settings.audit.typeBashExec`）。

**工具卡沙箱事实标注（P2）**：`ShellTool::result_meta` 把结果 JSON 里的 `sandbox` 事实
（`protected` / `evicted` / `timed_out` / `output_truncated`）并入 tool_result 元数据
（**denial 形状不变**，只加字段；非 JSON 结果静默跳过）。前端 `SandboxBadge` 在工具卡上标注
「无防护执行 / 被驱逐（原因进 tooltip）/ 输出截断」，一切正常时不渲染——用户据此一眼看出
"这条命令到底在不在沙箱里跑"。

**硬拒绝（不打断循环）的分类载体**：危险命令 / 资源限制 / Plan 模式拒写 / 技能目录只读等直接拒绝路径，工具以 `Err(拒绝信封)` 返回（纯 JSON，含 `denied: true`、`message`、`denial`），
经 `Tool::result_meta` 转成结果元数据：

```jsonc
// tool_result 事件（status 由工具显式给出，前端不再按文本推断）
{ "type": "tool_result", "session_id": "…", "id": "call_…", "name": "bash",
  "result": "工具 'bash' 执行失败: {\"denied\":true,\"message\":\"…\",\"denial\":{…}}",
  "status": "error",
  "denial": { "category": "danger_command", "source": "sandbox_runtime", "detail": "…", "targets": [] } }
```

同一份元数据写入 tool 消息 `metadata.{status, denial}`，**重启读历史仍可恢复分类**
（决议重放路径同样携带，metadata 合并而非整体替换）。

随后引擎发出中断事件（**tool 结果已照常成对落库**，不会产生孤立 tool_call）：

```jsonc
{ "type": "interrupt", "session_id": "…", "timestamp": 1, "tool_name": "bash",
  "payload": { /* 即上面的中断信封 */ } }
```

决议（`submitInterrupt`）的处理：

| 类型 | 决议 | 后端动作 | 是否续跑 |
|---|---|---|---|
| approval | `decision=allow` | 审计 agree → 台账一次性放行 → **重放**被拦工具 → 写真实结果（`approved`/`failed`） | 是 |
| approval | `decision=deny` | 审计 deny → 写拒绝说明（`denied`） | 是 |
| form | `answer` | 写「用户已回答：\n- 字段: 值」（按表单字段顺序，`resolved`） | 是 |
| form | `ignored` | 写「用户忽略了本次提问」（`ignored`） | **否**（省 token） |
| 任一 | 请求已失效 / 类型不符 | 入队即报错（前端提示），不写审计 | — |
| 任一 | 普通用户消息到达 | 该会话待决中断全部置 `superseded`（不执行） | — |

**时限（勿混淆）**：**待决中断本身不设 TTL / 不过期**——登记后一直有效，直到被决议
（agree / deny / answer / ignore）或被新用户消息取代；进程重启后由 `metadata.interrupt`
重建台账，仍可继续决议。带 60s 窗口的是**决议之后**的两个审批闸门：同意后的「一次性
无防护重放」额度、以及拒绝后的「同命令短路」（防止同一被拒操作反复撞审批）。二者都是
一次性 + 进程内存，重启即失效。

**持久化与恢复契约**（中断态载体是消息流，不新增会话字段）：

- tool 消息 `metadata.interrupt = { request_id, interrupt_type, status, updated_at, request }`，
  `request` 保存**完整信封**（冷启动据此重建台账、前端重绘卡片，重启后待审批/待回答不丢）。
- `status` 状态机：
  - approval：`pending → replaying → approved|failed`；`pending → denied|superseded`；
    `replaying → interrupted`（进程在重放中退出，保守报「结果未知」，**不重跑**非幂等操作）
  - form：`pending → resolved`；`pending → ignored|superseded`（无副作用，崩溃只停在 `pending` 可重新回答）；
    `ignored|superseded|interrupted|failed → pending`（仅 `reopenInterrupt`：换新 `request_id` 重开，见 §4.2）
- tool 消息用**确定性 id**`<session_id>:tool:<tool_call_id>`，就地更新 = 追加同 id、同 seq 的新记录；
  `load_messages` **按 id 去重（后写覆盖先写）**后按 seq 排序，历史回读即最新版本，
  且不会把同一 tool_call 的响应重复喂给模型（上游会 400）。
- 冷启动（`get_or_rehydrate`）扫描历史：`pending` → 重建台账；`replaying` → 覆盖为 `interrupted`。

`tool_result` 事件可选携带 `status`（`ok` / `error`）：决议改写由业务侧补发结果，
拒绝/忽略的文风不统一，用显式状态免去前端按文本推断。引擎路径自本次起也**总是**给出
`status`（`ok` = 执行成功，`error` = 工具返回 `Err`），前端无需再按结果文本正则推断。

**审计日志**（`{log_dir}/audit/{session_id}.jsonl`，逐行 JSON）记录 `category`（`DenialCategory`）
与 `source`（`DenialSource`，分类来源 / 可信度），两者在非拦截记录缺省不写；
与 `approval`（`agree` / `deny` / `auto` / `none`）配合，审计页可按「分类 × 处理方式」筛选，
并按 `source` 区分「确定 / 疑似」（前端不再自行伪造来源）：

```jsonc
{ "session_id": "…", "time": 1750000000000, "type": "filewrite", "approval": "none",
  "approved_at": null, "detail": "命令试图写入会话空间外路径：/tmp/a",
  "command": "echo x > /tmp/a", "category": "file_write", "source": "static_precheck" }
```

### 4.3 会话状态（后端唯一持有）

会话级运行状态**落盘**在 `SessionRecord` 上，随 `listSessions()` 返回、并随每条
`session_info` 事件广播。前端**不再自造终态**（此前侧栏图标只依赖内存中的流式集合与
任务标记，应用重启后全部丢失；history 回读也恢复不了 `done` / `error` / `cancelled`，
因为终态事件是纯广播、不落消息记录）。

`SessionRecord` 新增字段（camelCase 为 napi 侧命名）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `status` | `SessionStatus` | 会话运行状态，见状态机 |
| `statusUpdatedAt` | `number` | 状态最后一次变更的 unix 秒；0 表示升级前的老记录 |

两个字段均 `#[serde(default)]`：升级前的 JSONL / sqlite 旧记录照常反序列化，
缺省即 `idle`（**无图标，与升级前行为一致，不是回归**）；该会话跑过一次后即持有状态。

```jsonc
// SessionStatus（snake_case 字符串枚举，napi 与 common 同名同值）
"idle" | "running" | "awaiting_approval" | "awaiting_form"
     | "done" | "error" | "cancelled" | "interrupted" | "ignored"
```

**状态机**（以下为写入点；`set_status` 幂等——状态未变则不写盘、不广播）：

| 时机 | 状态 |
|---|---|
| 创建会话（首请求入队即 spawn） | `running` |
| 创建会话但 `auto_run = false`（仅创建：不 spawn、不起循环） | `idle` |
| `sendMessage` / `submitInterrupt`（审批与表单决议）入队 | `running` |
| 每轮循环出队分派 | `running` |
| 中断登记（审批 / 表单待用户决定） | `awaiting_approval` / `awaiting_form` |
| 表单决议「忽略」（不续跑，提问仍可经 `reopenInterrupt` 重开） | `ignored` |
| 提问被重开（`reopenInterrupt` 重新登记待决） | `awaiting_form` |
| 本轮「跑完了」落终态（**仅当该会话没有未决中断**） | `done` / `cancelled` / `error` |
| 本轮结束且队列为空（会话仍可继续对话） | `idle` |

⚠️ 停在中断待答（审批 / 表单）的轮次：引擎因中断退出，编排器同样走「本轮结束」分支，
但**不落终态**——`awaiting_*` 是登记中断时写下的，比「这一轮跑完了」更权威。否则落盘状态被
覆盖成 `done`，重启后侧栏由「等你处理」橙点退化成绿点（前端只能等打开会话、从消息级
`metadata.interrupt` 重建橙点）。对应地，`done` 事件在应用侧也不当作「任务完成」：
有待决中断的会话不推完成通知、不自动派发排队消息（派发会以新消息顶掉待决中断）。

**冷启动修正**：`listSessions()` 合并内存注册表——内存中存在且 `running == true` 的会话报
`running`；否则磁盘上的 `running` 视为**陈旧**，返回 `interrupted`（进程已重启，不可能仍在跑）。
该修正是**纯返回值**，不回写磁盘（原记录保持 `running`，避免把「同一进程内暂时未注册」
误判成崩溃；只有应用重启后读到才会呈现为 `interrupted`）。

分层对应关系：会话级 `status` 与消息级 `metadata.interrupt`（审批卡 / 提问表单）、
`metadata.status`（工具卡 `ok` / `error`）**互不替代**，各管一层。

### 4.3.1 轮次预算与上下文 token

`SessionRecord` 另有两个 token / 轮次相关字段（同样 `#[serde(default)]`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `currentRound` | `number` | **累计** LLM 轮次（跨用户消息累加，只用于展示） |
| `contextTokens` | `number` | **当前上下文**占用的真实 token = 上一轮请求的 `prompt_tokens`（provider 未返回时为 0） |

- **轮次上限按「每条用户消息」计时**：引擎对单条用户消息（含审批决议续跑）最多跑
  `MAX_ROUNDS = 100` 轮（`react-loop` 的 `turn_round`）；`currentRound` 只是累计展示值，
  **不再参与上限判定**——否则整条会话累计满 100 轮后，新消息会静默不推理（历史 bug）。
  撞上限只是本轮结束，用户再发消息即恢复。
- **重复工具调用防护**：每次引擎运行独立观察新回复里的全部工具调用，不重扫历史、不依赖轮次或工具调用 ID。最近 12 次调用中，同一工具与规范化 JSON 参数出现 3 次时，在下一次请求的摘要处理之后注入临时警告（不落历史）；达到 6 次则拒绝执行整个当前批次，补齐工具响应并返回明确错误。滑动窗口允许较早的调用过期；正常轮询等重复操作也受此保守限制。
- **上下文容量**：前端会话信息底部「上下文容量」进度条 = `contextTokens` / 系统设置里的
  摘要阈值 `summary.tokenThreshold`（分母同源，到 100% 即摘要触发点）。
- **摘要触发**：每轮 LLM 前检查（`loop_start`），阈值判定**真实 `contextTokens` 优先**，
  provider 不返回 usage 时回退字符估算（`(content + 工具参数)/4`）。阈值 / 保留尾部
  配置经 `updateConfig` 热更，保存后运行中会话下一轮即按新值判定。
- **摘要即活动日志重写（持久化语义）**：摘要把「**系统提示词 + 摘要消息 + 保留的消息**」
  组装成**新的活动日志**（`<session>.jsonl`）——活动日志**就是会话历史**，回读只读它。
  替换前的活动日志留作**备份段** `<session>/message_NNN.jsonl`（NNN = `summaryTimes`），
  仅供备份与 `search` 检索，**不再参与回读**（否则被摘要掉的旧对话会被重新喂给模型）。
  摘要消息为 `role=assistant` + `metadata.summary=true`；备份先行落盘、活动文件以
  临时文件 + 原子改名替换（任一步崩溃都不会留下「活动文件缺失 / 只写一半」的会话）。
  压缩写回时消息 `seq` 从会话计数器**连续分配**，回读按 `seq` 排序即得
  `系统提示词 → 摘要 → 保留消息 → 新消息`。
  摘要后 `contextTokens` 归零并立即广播 `session_info`（真实值由同轮 `after_llm` 回填）——
  否则回读后仍按压缩前的峰值判定，第一条消息就会再次触发摘要。
  前端同步：`summaryTimes` 变大即重载该会话消息（旧对话不再展示，只显示摘要 + 保留消息；
  `role=system` 始终不渲染）。
- **摘要切分安全**：保留尾部起点不得落在 `assistant(tool_calls)` 与其 `tool` 响应之间
  （否则上送报 400）；切分点会回退到该组 tool 的 assistant。引擎另在构造请求前剔除
  无前置 `tool_calls` 的孤立 `tool` 消息作协议兜底。
- **消息 `seq` 单调递增**：压缩写回与后续新消息都取会话内单调递增计数器
  （`Session.next_seq`，冷启动从活动日志最大 `seq` + 1 续接），不随内存历史压缩回退。

### 4.4 工作区页签工具（`tabs`）与状态镜像

模型可以用内置工具 `tabs` 操控**右侧工作区的页签**（打开/列出/切换/移除）。页签状态
**只存在于渲染层**（纯运行期内存，不落磁盘，重启即空），而 Rust → 渲染层只有**单向异步事件**
（无返回值），因此这里是一个「渲染层上报镜像 + 工具下发命令」的闭环：

```
渲染层（tabs store）── updateTabsState(session_id, TabsState) ──▶ Rust 每会话镜像（内存）
        ▲                                                            │
        └── tabs_command 事件（open/switch/remove）── 广播 ──────────┘  （list 同步读镜像）
```

- **按会话寻址**：镜像与命令都以 `session_id` 为键，与「当前正在看哪个会话」无关。
  对非显示会话下发命令只改它的内存快照，用户切回该会话时即可见。
- **命令是异步的**：`open`/`switch`/`remove` 只广播事件并立即返回「已下发命令（命令号 …）」；
  执行结果由渲染层随下一次上报回传（`last_command` 字段），`list` 会据此显示 ⚠️ 告警，
  故工具描述里提示模型「可用 `tabs list` 确认结果」。
- **免审批**：四个 action 均不弹审批卡，Plan 模式下同样可用（`open` 在 Rust 侧做同步校验：
  url 必须 http(s)，文件路径必须存在且非目录）。插件页签与终端页签不暴露给模型，
  `kind` 只支持 `browser` / `file`。
- **zoom 语义** = **页面缩放百分比**：`1366×800 50%` = 视口 1366×800 + 缩放 50%
  （对应 UI 的「视口尺寸 + 页面缩放」两个控件），不是「取 1366×800 的 50%」。

| action | 参数 | 说明 |
|---|---|---|
| `open` | `kind`=`browser`\|`file`；browser: `url`（必填）、`width`(默认 1366)、`height`(默认 800)、`zoom`(默认 50，百分比)、`new_tab`(默认 false)；file: `path`（必填，绝对路径） | 打开页签；`kind=browser` 默认**复用**该会话现有浏览器页签并**就地导航 + 覆盖视口**，`new_tab=true` 时**新建**一个浏览器页签（一个会话可开多个浏览器页签） |
| `list` | — | 读镜像，输出「序号 + 类型 + 参数 + 标题 + 当前页签」 |
| `switch` | `index` 或 `id` | 切换激活页签；`index` **从 1 开始**，与 `list` 输出的序号一致，越界时错误里回显当前清单 |
| `remove` | `index` 或 `id` | 关闭页签；关掉激活项时接位到同索引右邻居 |

`tabs_command` 事件的 `open` body 增加 `new_tab: boolean`（缺省 false；渲染层 `applyTabCommand` 据此决定复用还是新建）。

镜像与上报形状（`serialize` 后即 `updateTabsState` 的 `state_json`）：

```jsonc
{
  "tabs": [                       // 顺序 = 页签栏顺序；kind: browser|file|tool|plugin
    { "id": "…", "kind": "browser", "tool_id": "browser", "url": "https://…",
      "title": "…",               // 页面加载后由渲染层回写，可能为空
      "viewport": { "mode": "custom", "w": 1366, "h": 800, "zoom_percent": 50 },
      "active": true },
    { "id": "…", "kind": "file", "path": "/abs/path.md", "name": "path.md", "active": false }
  ],
  "active_index": 2,              // 1 基；无激活页签为 0
  "collapsed": false,             // 右侧面板是否收起（list 会额外提示）
  "last_command": { "request_id": "…", "ok": false, "message": "页签不存在(共 2 个)" }  // 可选
}
```

- 渲染层在**任何页签变化**（用户点击 / 命令执行后）与**切换会话**时上报；上报串行化，
  避免并发覆盖镜像。
- `deleteSession` 会同时清理该会话的镜像（内存 Map，进程内即失效）。
- 上报是**幂等覆盖**：Rust 不合并、不推断，只存最后一次收到的整体快照。

### 4.5 智能体（agent）管理与子会话（`sub_session`）

**智能体卡片**（设置页「智能体」管理，落盘在 sqlite `agents` 表）：

```ts
// 智能体 CRUD（napi;函数名由 napi-rs 转 camelCase,前端命令名为 snake_case:
//   list_agents / get_agent / create_agent / update_agent / remove_agent）
listAgents(): AgentCard[]
getAgent(agent_id: string): AgentCard | null
createAgent(request: CreateAgentRequest): AgentCard   // 名称必填且唯一
updateAgent(agent_id: string, updates: AgentUpdate): AgentCard
removeAgent(agent_id: string): void                   // 内置卡片拒绝

// AgentCard(出参 camelCase)
// { agentId, name, description, guidance, toolIds[], skillIds[], mode, permission,
//   builtin, createdAt, updatedAt }
// CreateAgentRequest: { name, description?, guidance?, toolIds?, skillIds?, mode?, permission? }
// AgentUpdate:        { name?, description?, guidance?, toolIds?, skillIds?, mode?, permission? }
//   —— 缺省字段 = 保持原值；给出空数组 = 清空该集合；guidance 空串 = 清空指引
// mode（执行模式，缺省 inherit）: read_only | read_write | auto | inherit
// permission（权限模式，缺省 inherit）: manual | auto | inherit
//   二者在**创建会话时**直接决定 SessionRecord 的 sessionMode / permissionMode：
//   read_only → plan（工具层硬拒写，请求写 execute 也会被压回 plan）
//   read_write → execute ｜ auto → auto ｜ inherit → 沿用请求 / 父会话（子会话）/ 缺省
//   permission：manual → manual ｜ auto → auto ｜ inherit → 同上
```

**语义（单一真源；会话的 `tool_list` / `skill_list` 由卡片物化）**：

| 会话的 `agent_type` | 工具集（**强制过滤**） | 技能集（**仅提示词**） | 会话模式 / 权限 | 系统提示词 |
|---|---|---|---|---|
| 缺省 → 内置默认主智能体（`builtin-main`） | 全量内置工具（`schedule_task` / `mail` / `browser` 除外，经 `tool_load` 加载） | 不列技能（`skills` 工具仍可加载任意技能） | 卡片为 `inherit`：沿用请求 / 缺省 | 静态提示词 + 约定文件（AGENTS.md，见 §4.6.1） |
| 自定义智能体（如 `builtin-explore`） | `toolIds` ∩ 注册中心（**空 = 无工具**：请求不带 `tools`） | 环境提示词按 `skillIds` 列「可用技能 + 技能摘要」（**空 = 不列**） | 由卡片 `mode` / `permission` 决定（见上）；`inherit` 时沿用请求 / 父会话 | 静态提示词 + 约定文件 + `\n\n` + `guidance` |
| 指定但卡片不存在 | 创建会话直接报错（附可用智能体列表） | — | — |
| 运行中卡片被删除 | 冻结为会话记录里的快照（不清空 / 不放宽） | 同左 | 指引消失（活取卡片） |

- `guidance` **必须拼进同一条 system 消息**（摘要写回只保留第一条 system；约定文件同理，
  见 §4.6.1）；卡片编辑后**下一轮**即生效（复用「系统提示词按版本刷新」路径）。
- 工具集在会话创建与冷启动自愈时物化进 `SessionRecord.tool_list`，循环按它过滤
  （`ToolRegistry::tool_map_for`）；技能集不影响 `skills` 工具加载，只是引导。
- **动态工具加载（`tool_load`）**：`schedule_task` / `mail` / `browser` 三工具（`agent::dynamic_tools::DYNAMIC_TOOLS`，
  清单驱动可持续追加）初始**不下发** LLM；模型按上下文调用 `tool_load` 工具显式加载，
  写入 `SessionRecord.loaded_tool_list`（持久化，重启不丢；fork 继承），**下一次回复**起携带其
  schema（`loop_start` hook 每轮按 `tool_list ∪ loaded_tool_list` 重建循环工具集；本轮不重建）。
  `unload` 移除加载态、`status` 查询；系统提示词「可加载工具」段只列**未加载**的动态工具
  （名称 + 一句话用途，含 `tool_load` 用法提示）；常驻工具不在提示词罗列（契约在 API tools schema）。
  工具目录（`list_tools`）的 `ToolInfo` 带 `dynamic` 标记（清单真源仍在
  `DYNAMIC_TOOLS`），设置页据此把动态工具独立分组展示。
- 内置卡片（`builtin=true`）不可编辑 / 删除；环境提示词的「可用智能体」块列出全部卡片
  （id / 名称 / 描述）供模型填 `sub_session` 的 `agentType`。内置卡片内容由代码内嵌、
  **随版本刷新**（仿内建技能 `skill-creator`；用户不可编辑内置卡，故刷新不覆盖用户数据）。
- 启动时播种两个内置卡片：

| id | 名称 | 工具集 | 说明 |
|---|---|---|---|
| `builtin-main` | 默认主智能体 | 不限制（全量内置工具；动态三工具经 `tool_load` 加载） | 会话未指定 `agentType` 时使用 |
| `builtin-explore` | Explore | `file_operation` / `search` / `read_images` / `memory` | 快速只读探索代理：查找文件、搜索关键字、解答代码库问题。`mode=read_only`（会话落 plan：**写操作被工具层硬拒**）+ `permission=auto`（探索期读空间外不弹卡）；工具集不含 shell（`memory` 亦为只读检索） |
| `builtin-general` | General | 全部内置工具，**除** `task_list`（任务规划）与 `sub_session` | 通用执行代理：可改文件、跑命令、驱动浏览器与页签，适合**并行铺开多个工作单元**（父会话一次委派多个）。`mode=inherit` / `permission=inherit`：写权限与审批行为随会话（子会话继承父记录）。不给 `sub_session` 是因为子会话不能再开子会话 |

**子会话与主会话共享页签 / 浏览器**：`tabs` 与 `browser` 都按**根会话**归属
（`astorm-core/src/tool/mod.rs::root_session_id`，子会话 → 父会话一层归一化）。
子会话 `tabs open` 下发的 `tabs_command` 带父会话 id，前端据此落到父会话的页签集上；
`browser` 按父会话读端点，因此子会话与主会话**共用同一个浏览器实例**（同一 daemon / 桥）。
例外：浏览器**截图落盘**留在子会话自己的工作区（`.astorm/browser-snapshot/`），
策略 / 审批 / 审计仍按子会话自身记录（权限继承自父会话）。

### 4.6 长期记忆（`memory` 工具 + 会话侧注入）

跨会话沉淀的结论存于 OKF v0.2 Bundle（默认为 `<用户目录>/memory`，见 `docs/memory.md`）。
会话侧只有**只读**通路：

- **概念正文的链接**：写入时由管道确定性生成结尾的 `## 相关` 小节（概念互链 + 原文链接，
  只链接**已存在**的概念 ⇒ 永不悬空，幂等）。`validate` 的「越出 Bundle 根」按 **Bundle 根**
  判（`raw/` 属于 Bundle），指向 Bundle 之外才报警。详见 `docs/memory.md` §4.5。
- **详情 DTO** 带 `fragIds`（权威依据锚点）：前端据此渲染可点「依据」并跳回原文对应行；
  概念正文里的相对链接由前端拦截为站内跳转（概念 → 切换详情；`raw/` → 打开底稿）。
- **`memory` 工具**：`action` = `search` / `read` / `list`。`search` 为 BM25 检索
  （索引不可用时退化为同分词器的关键词扫描）；`read` 按 `concept_id` 取正文；
  `list` **分两级**：不带 `category` 返回目录页（分类 id + 计数 + 每类 ≤3 条样例，含空分类），
  带 `category`（分类 id / type 名 / 目录名三种写法皆可）才列该类条目。
  这是 OKF 分层索引的等价物，但**每次从概念文件现场算**、不读派生的 `index.md`。
  **没有 `status` 动作**：库概览属诊断信息（记忆页已有），分类表由目录页动态给出，
  工具描述**不写死分类清单**（写死必然随配置漂移）。
  **无写入动作**：写入只有「会话 → `raw/` → 编译 → `wiki/`」一条路（分类 / 去重 / 合并判定
  都在管道里，开直写会同时绕过三者）。记忆未启用时工具返回可读说明而非报错。
- **每轮注入**（`agent/prompt.rs`）：能力声明在**系统提示词「可用能力」段**
  （`build_capability_block`，随卡片 / 配置版本刷新）；`ensure_user_prompt` 用**当前用户消息**
  做一次检索，把命中摘要追加成 `【相关记忆】` 块，只落在**最后一条用户消息**尾部
  （易变区），system 前缀与更早历史不变 —— 前缀缓存不受影响。

三个不变量（改动时务必保持）：

1. **注入文本不得进入 `raw/`（防自我喂养）**：注入只改**请求消息**，落盘的 `message.jsonl`
   不含注入块（实测确认）；`astorm-memory` 归一化时另按 `【会话环境】` / `【相关记忆】`
   两个标记取**最早切点**整体剥离作为防御层（覆盖外部导入文本与将来若改为持久化注入）。
2. **`concept_id` / raw stem 走读路径准入**：它们可能由模型填写，故门面用
   `concept_path_checked` / `raw_file_checked`（段校验 + 落点蕴含检查）。`concept_path` /
   `raw_file` 保留给写入侧自造 id，**不得**直接用于外部输入。
3. **注入必须 fail-soft**：任何错误（索引被编译占用、索引未建、无命中）一律不注入，
   绝不让它影响会话。

注入相关配置：`memory.injectMemories`（默认 `true`）、`memory.injectedConcepts`（默认 `5`，0 = 关闭）。

### 4.6.1 约定文件（AGENTS.md 合并注入）

会话运行时把两份 AGENTS.md **拼进同一条 system 消息**（与静态提示词、智能体 guidance 共用一条，
原因见 §4.5 的「只保留第一条 system」不变量）：

| 顺序 | 来源 | 路径 |
|---|---|---|
| ① | 静态系统提示词 | 代码常量 `SYSTEM_PROMPT`（`agent/prompt.rs`；含 shell 约束 / 技能 / 长期记忆 / 任务规划的使用说明） |
| ② | **全局约定** | `<用户目录>/AGENTS.md` |
| ③ | **会话空间约定** | `<会话工作区>/AGENTS.md` |
| ④ | 智能体指引 | `AgentRecord.guidance`（`agent_type` 实时取卡片） |
| ⑤ | **可用能力**（动态段） | 技能（≤10 个，名称+描述） / 可加载工具（未加载的动态工具,名称+一句话用途） / MCP 服务（已连接服务器 + 工具名，连接即下发） / 长期记忆能力行 / 可用智能体（根会话）——`build_capability_block`;常驻工具的契约由 API tools schema 承载,不在提示词罗列 |

- `<用户目录>` = `AppConfig.workspace`（前端设置页「默认会话空间基目录」，缺省 `~/astorm`），
  后端按 `app::paths::user_dir` 推导；`<会话工作区>` 按 `tool::resolve_workspace` 推导
  （与 `.astorm/plan` 同源）。**路径均由后端推导，前端不传路径**（前端设置页用
  `join(默认会话空间基目录, 'AGENTS.md')` 得到同一路径来编辑该文件）。
- 每块内容前**固定拼一段说明**（标明来源与效力范围）；顺序即优先级，**冲突时以会话空间约定为准**，
  智能体指引最后、最具体。
- 每份上限 `AGENTS_MAX_CHARS = 8000` 字符（超出按**字符**截断并附省略号），读取前另有
  `AGENTS_MAX_BYTES = 1 MiB` 字节守卫；**全程 fail-soft**：缺失 / 非文件 / 空 / 超大 / 读取失败
  一律跳过。无文件时会话行为与改动前逐字节一致。
- 配置：`instructions.injectGlobalAgents`、`instructions.injectWorkspaceAgents`（默认均 `true`，
  `update_config` 热更，下一轮生效）。会话内编辑 AGENTS.md 会经「系统提示词按版本刷新」路径生效
  （`ensure_system_prompt` 检测到内容变化即就地改写并记日志）。
- 全局那份是**用户目录下的真实文件**（设置页「全局约定」编辑，Node 侧 `read_global_agents` /
  `write_global_agents`）；会话空间那份是工作区里的普通文件，不进设置页。

**子会话工具 `sub_session`**（模型可调用的内置工具）：

```
参数: { task: string(必填, 作为子会话首条用户消息),
        agent_type: string(必填, 子会话使用的智能体 id) }
```

- 子会话是**普通会话**（同一条落盘 / 事件 / 侧栏链路），只是 `pid` 指向父会话；
- **工作区** = `父会话工作区/.astorm/subagent_{子会话id}/`；
- **继承**父会话的模型 / 供应商 / 模式 / 权限 / 思考强度（由 `create_session` 在 `pid` 存在时统一填充）；
- **禁止嵌套**（深度 = 1；已是子会话的会话调用即报错）；
- **同步等待**：工具订阅全局事件总线等子会话终态，把其最后一条 assistant 文本作为
  tool result 返回；子会话等审批（`interrupt`）时继续等待（用户在其会话卡片上处理），
  父会话取消则一并取消子会话；事件滞后时按落盘状态兜底判定。

## 5. Electron IPC 命令(本仓库 `src/main/providers.ts`)

| 命令 | 参数 | 返回 | 说明 |
|---|---|---|---|
| `providers:list` | — | `{ providers: ProviderView[], encryptionAvailable }` | 返回解密明文供设置表单展示 |
| `providers:sync` | `{ providers: ProviderInput[] }` | `{ providers: ProviderView[] }` | 全量替换:校验+加密写盘;运行中走 `updateConfig` 热更新,未运行则 `start`(不再 shutdown 中断会话) |
| `providers:test` | `{ name, model }` | `{ ok, message }` | 按协议发一次图文能力探测（文字指令 + 739 图 OCR）；`vision_unsupported` = 模型不支持文本+图片 |

`ProviderInput`:`{ name, baseUrl, apiKey, models }`(明文,由渲染层持有);
`order` 由主进程按数组顺序强制一致(数组顺序 = 排序 = 默认)。

## 5.1 知识库(napi `kb*` 系列 + IPC `kb_*` 命令)

数据 = **真实文件系统**:根目录 `<userData>/knowledge/`(见 `AppDir.knowledge_dir`),
文件夹 = 目录、文档 = 文件(缺扩展名自动补 `.md`)。展示元数据(图标/颜色/标签)
集中存于根下 `.manifest.json`(相对路径 → 元数据),由后端在重命名/删除时同步迁移/清理。
根目录缺失时后端自动写入演示树(仅首次)。条目 id = 相对路径(`/` 分隔)。

napi 导出(函数名由 napi-rs 自动转 camelCase,参数对象字段为 camelCase):

```ts
kbList(): KbItem[]
kbCreate(req): KbItem        // req = { parent: string(空串=根层), name, kind: "folder"|"document",
                             //        icon?, color?, tags?, content?(文档初始内容) }
kbRename(path, newName): KbItem
kbDelete(path): void         // 文件夹递归删除;幂等
kbUpdateMeta(meta): KbItem   // meta = { path, icon?, color?, tags?(空数组=清空) }
kbRead(path): string         // >1MB 拒绝
kbReadBuffer(path): Buffer   // 文档字节(前端预览用,>50MB 拒绝)
kbWrite(path, content): void
kbImport(parent, paths): KbItem[]  // 导入本地文件/目录(目录递归拷贝)
```

`KbItem`(返回,camelCase):`{ id, nodeType: "folder"|"document", name, parentId?,
icon, color, tags, size(文档字节;文件夹 0), createdAt, updatedAt(ms), absPath }`。

Electron IPC 命令(`src/main/backend.ts` `NAPI_COMMANDS`,渲染层经 `invoke()`):

| 命令 | 参数 | 说明 |
|---|---|---|
| `kb_list` | — | 整棵树(文件夹在前、名称排序) |
| `kb_create` | `{ req }` | 新建;父目录缺失时报错 |
| `kb_rename` | `{ path, new_name }` | 重命名(清单同步迁移) |
| `kb_delete` | `{ path }` | 删除(递归;清理元数据) |
| `kb_update_meta` | `{ meta }` | 仅更新展示元数据 |
| `kb_read` / `kb_write` | `{ path }` / `{ path, content }` | 文档内容读写 |

前端 `KnowledgeView.vue` 为唯一消费方;点击文档经 `kb_read_buffer` 取字节,
中心区用 open-file-viewer(@open-file-viewer/vue)渲染预览。

## 5.2 内置浏览器数据(设置页「自动化控制」)

内置浏览器是渲染层里的**跨域 `<iframe>`**,与宿主 UI 共用 `session.defaultSession`
(无 partition)。因此 Cookie 只能由主进程写进该会话:Rust 负责读 Chrome 的 SQLite
并解密(`astorm-core/src/browser/`,见 §4),`src/main/browser-data.ts` 负责写入/清理。

Electron IPC 命令(`src/main/backend.ts` `NODE_COMMANDS`,渲染层经 `invoke()`):

| 命令 | 参数 | 返回 | 说明 |
|---|---|---|---|
| `import_chrome_cookies` | — | `{ profile, profileDir, total, skipped, decryptFailed, imported, writeFailed }` | 调 `importChromeCookies`(napi)→ 逐条 `cookies.set` 写入默认会话 |
| `clear_browser_cache` | — | `{ ok: true }` | `clearCache()` + `clearStorageData({ storages: [cachestorage, serviceworkers, shadercache] })`,保留 Cookie |
| `clear_browser_data` | — | `{ sites, cookies }` | 删除 Cookie + 站点数据 + 缓存,不可撤销 |

两处刻意的实现约束(改动前务必先读代码注释):

- **导入强制 `SameSite=None + Secure`**:站点自身的 Lax/Strict 会话 Cookie 在跨域
  iframe 里不会被带上,不改写则导入等于白导(站点仍报"无登录信息")。代价是
  host-only 语义保留但 secure 被提升。
- **清理必须按站点 origin 限定**:默认会话与宿主 UI 共用,无条件 `clearStorageData()`
  会把应用自己的 localStorage(设置、页签布局)一起清掉。范围由会话内已有 Cookie 的
  域名反推,并排除宿主 UI 的 hostname(开发态 `ELECTRON_RENDERER_URL`);代价是没有
  Cookie 的站点数据清不到。

## 4.9 LLM 流中途中断(自动重试契约)

**症状**:长回答/长思考途中突然整轮失败,错误形如
`LLM stream 错误: [E005] Stream error: error decoding response body，底层原因: error reading a body from connection → connection reset（url=…/chat/completions，http_version=HTTP/2.0，elapsed=…s，received=370083B，sse_lines=1222，…）`。

**成因**:SSE 已正常推送很久之后,对端或中间设备把 TCP 连接 RST 了(裸 `connection reset`,
不是 HTTP/2 的 `GOAWAY`/`RST_STREAM`,故更可能是边缘网关按连接时长/空闲回收)。**不是**鉴权、
首包或 SSE 解析问题——`sse_lines` 已计数 1222 条合法帧。

**契约**:

| 环节 | 行为 |
|---|---|
| 首包前失败(0 字节) | `llm-provider` 内部整体重发一次(`STREAM_OPEN_RETRY_TIMES`) |
| **收到字节后中断** | 自动重开重发一次(`STREAM_MID_RETRY_TIMES`),重试前投递 `LlmEvent::Restart` / 事件 `stream_reset` |
| 消费端义务 | 收到 `Restart`/`stream_reset` **必须丢弃本轮此前已累积的正文/思考/工具调用**,否则两次尝试会被拼接(引擎、结构化输出、`llm_util`、插件 `llm.chat` 的 `kind:"reset"` 都已按此实现) |
| 重试耗尽 | 上报富化 `E005`(含 `url`/`http_version`/`elapsed`/`received`/`sse_lines`/编码头)+ 文案「已自动重试 N 次」;同时写 app 日志 |
| 落库 | 失败轮**不落库**半截助手消息(截断内容不进后续上下文);自动重试成功的轮只落最后一次尝试的正文 |
| 传输层加固 | 默认 client 显式设置 `tcp_keepalive(15s)` / `http2_keep_alive_interval(15s)` / `read_timeout(180s,大于引擎 120s 看门狗)` / `Accept: text/event-stream`(见 `llm-provider/src/client.rs::default_http_client`) |
| 非传输类失败 | body 正常读到 EOF 但零 `data:` 报文(网关乱码/错误页)**不**重试,直接空流诊断 |

> 前端表现:收到 `stream_reset` 清空本轮已渲染的半截内容并显示「生成中断(连接被重置),
> 正在自动重试…」;重试成功后内容从头重画。

## 6. 安全说明与权衡

- **落盘加密**:`safeStorage.isEncryptionAvailable()` 为真 → `encryptStringAsync/decryptStringAsync` → hex;不可用(如 Linux 无钥匙串)降级**明文 + `storage: "plaintext"` 标志**,渲染层可据此提示。
- **解密失败**(密钥轮换/环境变更):`decryptString` 抛错 → 返回空串,不静默丢数据;可扩展为 `storage_error` 让前端提示重填。
- **渲染层明文接触**:设置表单需要展示/编辑现有密钥,故 `providers:list` 返回明文。这是本阶段取舍;后续可改为「返回 `keyHint` 掩码 + 单独 reveal」,表单只允许覆盖写入(恢复「后端永远不出明文」的严格边界)。
- **明文 apiKey 所在位置**:主进程内存、napi AppState、渲染层表单编辑期;均不落盘。

## 7. 迁移

旧版本使用渲染层 `localStorage[astorm:models:v3]`(明文)。首次接入后建议:

- 方案 A(推荐):一次性把 localStorage 数据经 `providers:sync` 推给主进程(明文→加密落盘),之后 localStorage 不再作为事实源;
- 方案 B:直接丢弃(与早期「恢复初始状态」一致)。

渲染层 `services/providers.ts` 已实现:Electron 下走 IPC,纯浏览器调试(Vite)自动降级 localStorage,切换无感。