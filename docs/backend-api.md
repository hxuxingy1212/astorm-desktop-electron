# Astorm Agent — 后端 API 与实体设计文档

> 依据前端代码快照(`astorm-agent-electron` v0.1.0)反向推导整理,供后端设计与前端接入共用。
>
> 文档版本:v1.0 · 2026-08-12 · 依据 commit `8da5ed2` 附近代码

---

## 1. 现状概述

当前前端是一个纯 UI 框架阶段的应用,具备以下特征:

- **技术栈**:Electron + Vue 3 + Pinia + Arco Design Vue + vue-i18n + vue-router + vue-stream-markdown。
- **没有任何网络请求**:全仓库(`src/**`)不存在 `fetch` / `axios` / `WebSocket` / `EventSource`,无 API base URL 配置,无 `.env` 文件。
- **数据仅存 localStorage**,且只持久化**会话列表**与**应用设置**:
  - `astorm:sessions:v2` → `Session[]`(`src/renderer/src/stores/sessions.ts:7,211`)
  - `astorm:settings` → 完整 `SettingsState`(`src/renderer/src/stores/settings.ts:5,69`)
  - **消息列表是内存态,不持久化**(`stores/sessions.ts:136` 注释)。
- **唯一的后端契约**是 Agent 数据源抽象(`src/renderer/src/services/agent.ts:6-8`):

  ```ts
  export interface ChatProvider {
    stream(history: ChatMessage[]): AsyncIterable<string>
  }
  ```

  当前由 `MockAgent` 逐块吐出固定 markdown 模拟 LLM 流式输出,注释明确说明:**接入真实 Agent 只需替换 `createAgentProvider` 的实现**(`agent.ts:3-4,46-48`)。这是未来后端最核心的接口形态。
- **Electron 主进程无任何特权能力**:IPC 仅覆盖窗口控制、缩放、原生文件选择对话框(`src/main/ipc.ts`),没有数据库、没有网络、没有文件系统读写。
- 前端通过 `window.api.dialog.openFiles()` 拿到的是**本地文件路径数组 `string[]`**,不是文件内容(未来后端需要一种方式接收本地路径或上传内容)。
- 一批领域在 UI 上已「预告」但未实现:设置页的模型/智能体/插件/技能/工具、侧栏的定时任务分组、git 分支列表(mock)。右侧面板已实现 2 个工具面板(浏览器/终端),另有文件预览、网页与插件页签。

---

## 2. 通用约定

### 2.1 接口风格

- RESTful + JSON;流式对话使用 **SSE**(Server-Sent Events)。
- 基础路径:`/api/v1`(占位,最终以后端部署为准)。

### 2.2 响应信封

非流式接口统一返回:

```json
{
  "code": 0,
  "message": "ok",
  "data": { }
}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `code` | number | `0` 成功;非 0 见错误码表 |
| `message` | string | 人类可读信息(对应前端 i18n 文案) |
| `data` | any | 业务数据,分页时为分页结构 |

分页结构:

```json
{ "list": [], "total": 0, "page": 1, "pageSize": 20 }
```

### 2.3 错误码

| code | 含义 | 备注 |
| --- | --- | --- |
| 0 | 成功 | |
| 40001 | 参数校验失败 | 前端展示 `message` |
| 40004 | 资源不存在 | 如会话/消息/工作区不存在 |
| 40100 | 未登录 / 凭证失效 | 规划中,前端暂无登录 |
| 40300 | 无权限 | 规划中 |
| 42900 | 请求过于频繁 | 流式对话限流 |
| 50000 | 服务端内部错误 | |
| 50300 | Agent 上游不可用 | 模型服务超时/鉴权失败 |

### 2.4 ID 与时间

- **前端现状**:`uid()` 生成 base36 短 ID(`Date.now().toString(36) + 随机 base36`,`src/renderer/src/utils/index.ts:1-4`),非 UUID;时间戳全部为 **epoch 毫秒数**(`Date.now()`)。
- **后端建议**:服务端生成 **UUID(字符串)** 作为主键,客户端可选传入 `clientId` 做幂等;`createdAt`/`updatedAt` 统一 epoch 毫秒,与前端类型 `createdAt: number` 对齐,避免前端转换。

### 2.5 鉴权(规划中)

前端当前无任何用户/登录概念,仅主进程设置 `app.setAppUserModelId('com.astorm.agent')`(`src/main/index.ts:66`)。设计上预留 `Authorization: Bearer <token>` 头,所有接口(除登录/注册)默认要求鉴权,实体表统一带 `userId` 字段。

---

## 3. 实体设计

### 3.1 已实现实体(前端代码直接支撑)

#### Session 会话

前端定义:`src/renderer/src/types/index.ts:1-11`;存储:`stores/sessions.ts`(localStorage `astorm:sessions:v2`)。

| 字段 | 类型 | 必填 | 说明 | 前端出处 |
| --- | --- | --- | --- | --- |
| `id` | string (UUID) | ✅ | 主键 | `types/index.ts:2` |
| `title` | string | ✅ | 会话标题,允许重名;新建时自动生成「任务 N / 项目 N」 | `stores/sessions.ts:162-166` |
| `kind` | enum `task \| project` | ✅ | 会话分类;旧数据缺失时兜底为 `task` | `types/index.ts:6`、`stores/sessions.ts:19` |
| `workspace` | string | ❌ | 所属工作区(mock 数据) | `types/index.ts:8` |
| `branch` | string | ❌ | 关联 git 分支(欢迎页创建时选择) | `types/index.ts:10`、`stores/sessions.ts:20` |
| `createdAt` | number (epoch ms) | ✅ | 创建时间,当前唯一排序依据 | `types/index.ts:3` |
| `updatedAt` | number (epoch ms) | ✅* | 最后活动时间(*后端补充字段,用于列表排序) | — |

补充说明:

- 侧栏按 `kind` 分为「任务 / 项目」两组(`stores/sessions.ts:143-145`),并按 `workspace` 聚合成项目节点(`AppSidebar.vue` 的 `SessionTreeNode`)。后端列表接口应按 `kind`、`workspace` 分组返回或支持过滤。
- 会话操作菜单(UI 已有,未实现):`pin` 置顶、`rename` 重命名、`archive` 归档、`wiki` 生成项目 Wiki、`reveal` 定位、`copyPath` 复制路径、`copyId` 复制 ID(`StatusBar.vue:45-53`)。其中 `pin`/`archive` 需要实体字段支撑,`rename` 需要更新接口。
- **mock 种子数据**(`stores/sessions.ts:28-82`)展示了 `workspace` 的取值风格:`astorm-desktop`、`astorm-agent-electron`、`astorm-web`。

#### ChatMessage 消息

前端定义:`src/renderer/src/types/index.ts:13-20`;当前**仅内存态**(`stores/sessions.ts:137`),通过 `messages[activeId]` 按会话分组。

| 字段 | 类型 | 必填 | 说明 | 前端出处 |
| --- | --- | --- | --- | --- |
| `id` | string (UUID) | ✅ | 主键 | `types/index.ts:14` |
| `sessionId` | string | ✅ | 所属会话(*后端补充;前端靠内存 Map 隐含关联) | — |
| `role` | enum `user \| assistant` | ✅ | 消息角色;**暂无 `system` 角色**(系统提示词如何注入由后端决定) | `types/index.ts:15` |
| `content` | string (markdown) | ✅ | 消息正文;助手消息由流式 chunk 累积而成 | `types/index.ts:16`、`stores/sessions.ts:197-199` |
| `createdAt` | number (epoch ms) | ✅ | 发送时间 | `types/index.ts:17` |
| `streaming` | boolean | ❌ | **瞬态标志,不落库**:标记助手消息正在流式输出 | `types/index.ts:19`、`stores/sessions.ts:182-189` |

补充说明:

- 前端发送一条消息会**同时创建两条记录**:用户消息 + 一条 `content: ''`、`streaming: true` 的空助手消息,随后由流式输出逐块填充(`stores/sessions.ts:177-193`)。后端流式接口需返回**助手消息 ID**,供前端填充对应占位消息。
- 消息与文件的关联规划中(`Attachment.sessionId` 或 `Attachment.messageId`)。

#### PanelTab 面板页签

前端定义:`src/renderer/src/types/index.ts:217-233`;状态在 `stores/tabs.ts`,**纯运行期内存**(不落磁盘)。

四种形态由可选字段判别(`file` / `web` / `pluginId` / `toolId`):

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 页签实例 id(仅运行期存在,不对外暴露) |
| `toolId` | enum `browser \| terminal` | 工具页签标识,同一工具只开一个页签(`stores/tabs.ts` `openTab` 去重) |
| `pluginId` | string | 插件页签(一个插件最多一个页签) |
| `file` | `FileTabState` | 文件页签:按 `filePath` 去重,内容懒加载 |
| `web` | `WebTabState` | 网页页签:按 `toolId='browser'` 去重的**全局单例**,含 url/title/favicon 与视口 `viewport` |
| `label` | string | 自定义标题(如终端 = 会话目录名) |

归属:面板页签属于**本地 UI 运行期状态**,按会话在内存中换挡(切会话保留、重启清空),
不落磁盘也不需要后端表;`toolId` 同时是工具 API 的命名空间(见 4.6)。

### 3.2 规划中实体(前端已预告,标注出处与设计建议)

以下实体在 UI/类型上已有占位,后端可提前设计表结构:

#### User 用户(规划中)

前端无任何出处(仅主进程 `appUserModelId` 常量 `com.astorm.agent`)。建议字段:`id`、`username`、`email`、`avatar`、`createdAt`、`updatedAt`。

#### Workspace 工作区(规划中)

出处:`Session.workspace`(`types/index.ts:8`)、种子数据三个工作区(`stores/sessions.ts:28-82`)、会话操作菜单 `copyPath`/`reveal`(定位工作区路径)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `name` | string | 工作区名,如 `astorm-desktop` |
| `path` | string | 本地磁盘路径(会话 `copyPath`/`reveal` 需要) |
| `userId` | string | 归属用户 |

#### GitBranch 分支(规划中)

出处:`Session.branch`(`types/index.ts:10`)、`FileBranchBar.vue:22-27` 的 mock 分支列表(`main`、`develop`、`feature/login-fix`、`release/1.0`,注释写明「placeholder, to be replaced with real git later」)、`BRANCH_POOL`(`stores/sessions.ts:10`)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `workspaceId` | string | 所属工作区 |
| `name` | string | 分支名 |
| `isDefault` | boolean | 是否为默认分支(main) |

#### ScheduledTask 定时任务(规划中)

出处:`AppSidebar.vue` 侧栏第三个分组「定时任务」,恒为空列表(`list: [] as Session[]`,`AppSidebar.vue:74-76`)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `title` | string | 任务名 |
| `cron` / `schedule` | string | 触发规则 |
| `sessionId` | string | 关联会话(可选) |
| `enabled` | boolean | 是否启用 |
| `lastRunAt` | number | 上次执行时间 |

#### Model 模型(规划中)

出处:设置页注册表 `models`(`components/settings/registry.ts:17`,目前渲染占位页)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `provider` | string | 提供商(OpenAI / Anthropic / 本地 Ollama 等) |
| `name` | string | 模型名,如 `gpt-4o` |
| `baseUrl` | string | 自定义网关地址(可选) |
| `apiKey` | string | 凭证(加密存储) |
| `isDefault` | boolean | 默认模型 |
| `enabled` | boolean | 是否启用 |
| `capabilities` | string[] | 能力标签(规划:function calling / vision 等) |

#### Agent 智能体(规划中)

出处:设置页注册表 `agents`(`registry.ts:18`)。对应前端的 `ChatProvider` 抽象 —— 未来可让每个智能体提供独立的流式入口(见 4.3 的 `agentId` 参数)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `name` | string | 智能体名 |
| `modelId` | string | 绑定的模型 |
| `systemPrompt` | string | 系统提示词(消息 role 暂无 `system`,由智能体承载) |
| `toolIds` | string[] | 启用的工具 |
| `temperature` | number | 采样温度 |

#### Plugin / Skill / Tool 插件、技能、工具(规划中)

出处:设置页注册表 `plugins` / `skills` / `tools`(`registry.ts:19-21`);`toolId` 枚举见 `panels/index.ts:26-31`。

| 实体 | 建议字段 | 说明 |
| --- | --- | --- |
| `Plugin` | `id`、`name`、`version`、`enabled`、`settings`(JSON) | 插件市场/本地插件 |
| `Skill` | `id`、`name`、`description`、`prompt` | 可复用技能模板 |
| `Tool` | `id`、`name`、`namespace`、`enabled`、`config`(JSON) | `namespace` 对应面板 `toolId`:`browser`、`terminal`、`code-review`、`file-preview`、`project-wiki`、`database` |

#### Attachment 文件/附件(规划中)

出处:当前附件只是**本地路径字符串**——`window.api.dialog.openFiles()` 返回 `string[]`(`src/preload/index.d.ts:13`、`src/main/ipc.ts:34-42`),`FileBranchBar.vue` 的 `files: string[]`(`FileBranchBar.vue:10-11`),发送时拼进用户消息文本(`src/renderer/src/views/NewChatView.vue:21-27`)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `sessionId` / `messageId` | string | 归属 |
| `name` | string | 文件名 |
| `localPath` | string | 本地绝对路径(桌面端独有,Electron 可读) |
| `size` | number | 字节数 |
| `mimeType` | string | 类型 |
| `uploadedAt` | number | 上传时间 |

> 桌面端设计权衡:Electron 主进程可直接读本地文件,**不一定需要上传**;后端可按「本地路径 + 读取通道(IPC 或服务端映射)」设计。

#### WikiPage 项目 Wiki(规划中)

出处:`project-wiki` 面板(`panels/index.ts:30`)、会话操作菜单 `wiki` 项(`StatusBar.vue:45-53`)。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 主键 |
| `workspaceId` / `sessionId` | string | 归属 |
| `title` | string | 页面标题 |
| `content` | string (markdown) | 正文 |
| `updatedAt` | number | 更新时间 |

### 3.3 实体关系总览

```
User 1 ──── N Workspace 1 ──── N GitBranch
User 1 ──── N Session 1 ──── N ChatMessage
Session N ──── N Attachment(经 sessionId/messageId)
Workspace 1 ──── N WikiPage
User 1 ──── N ScheduledTask
User 1 ──── N Model / Agent / Plugin / Skill / Tool
Agent N ──── M Tool(agent.toolIds)
Agent N ──── 1 Model
```

---

## 4. API 设计

> 标注「✅ 现有契约」的接口由前端代码直接推导;**「🧩 规划中」**为 UI 已预告、需后端提前设计的接口。

### 4.1 会话 API

对应前端:`stores/sessions.ts` 的 `init` / `createSession` / `setActive` / `persist`。

| 方法 | 路径 | 说明 | 状态 |
| --- | --- | --- | --- |
| GET | `/api/v1/sessions?kind=task\|project&workspace=xxx&page=1&pageSize=20` | 会话列表,支持按 `kind`/`workspace` 过滤与分页;侧栏任务/项目分组、按工作区聚合依赖此接口 | ✅ 现有契约 |
| POST | `/api/v1/sessions` | 创建会话,body:`{ "title"?, "kind": "task"\|"project", "workspace"?, "branch"? }`;`title` 缺省时后端可自动生成「任务 N」 | ✅ 现有契约 |
| GET | `/api/v1/sessions/{id}` | 会话详情 | ✅ 现有契约 |
| PATCH | `/api/v1/sessions/{id}` | 更新,body 支持:`title`、`workspace`、`branch`、`pinned`、`archived`(对应会话菜单 pin/rename/archive) | 🧩 规划中(菜单已预告) |
| DELETE | `/api/v1/sessions/{id}` | 删除会话(级联删除消息) | ✅ 现有契约 |

创建请求示例:

```json
{
  "title": "优化首页加载速度",
  "kind": "task",
  "workspace": "astorm-agent-electron",
  "branch": "feature/performance"
}
```

### 4.2 消息 API

对应前端:消息列表当前为内存态(`stores/sessions.ts:137`),切换会话后即丢失——后端接入后由接口恢复。

| 方法 | 路径 | 说明 | 状态 |
| --- | --- | --- | --- |
| GET | `/api/v1/sessions/{id}/messages` | 按 `createdAt` 升序返回会话全部消息 | ✅ 现有契约(前端需要) |
| POST | `/api/v1/sessions/{id}/messages` | 追加一条消息(body:`{ "role": "user"\|"assistant", "content": "..." }`),返回完整消息对象(含 `id`);仅用于非流式场景,常规对话走 4.3 | ✅ 现有契约 |

### 4.3 流式对话(核心,对应 `ChatProvider`)

对应前端:`ChatProvider.stream(history: ChatMessage[]) → AsyncIterable<string>`(`services/agent.ts:6-8`),消费方 `stores/sessions.ts:194-206`(`runStream` 逐块追加 `msg.content`,`stopFlag` 中断)。

**请求**

```http
POST /api/v1/sessions/{id}/chat/stream
Content-Type: application/json
Accept: text/event-stream
```

```json
{
  "messages": [
    { "role": "user", "content": "登录页在 1280 以下宽度时表单会溢出,帮我定位一下问题。" }
  ],
  "agentId": "agent_xxx",
  "modelId": "model_xxx",
  "attachments": ["file_xxx"]
}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `messages` | ✅ | **完整历史消息数组**(与前端 `ChatProvider` 输入一致),由后端负责裁剪上下文;未传时后端取会话已有消息 |
| `agentId` / `modelId` | ❌ | 指定智能体/模型,缺省用默认 |
| `attachments` | ❌ | 附件 id 列表(规划中) |

**SSE 响应事件**

| 事件 | data 载荷 | 说明 |
| --- | --- | --- |
| `meta` | `{ "messageId": "msg_xxx" }` | 会话开始,返回**助手消息 id**(前端填充流式占位消息,`stores/sessions.ts:182-189`) |
| `chunk` | `{ "content": "string 增量" }` | 文本增量块,前端逐块拼接(`runStream` 循环体) |
| `done` | `{ "messageId": "msg_xxx", "usage"?: { "inputTokens": n, "outputTokens": n } }` | 流结束 |
| `error` | `{ "code": 42900, "message": "..." }` | 中途出错(含模型上游错误),发送后关闭流 |

事件格式:

```text
event: chunk
data: {"content":"已经分析了你的需求"}

event: done
data: {"messageId":"msg_xxx"}
```

**停止生成**:前端通过 `stopFlag` 本地中断(`stores/sessions.ts:207-209`),只需客户端断开连接;后端检测到连接关闭即终止生成、释放模型调用。规划中可补充 `POST /api/v1/chat/stop { requestId }`。

**幂等/限流**:客户端可选传 `requestId` 防重复提交;同会话并发流式请求限流(错误码 42900)。

### 4.4 工作区与分支 API

对应前端:`FileBranchBar.vue` 的 mock 分支列表(`FileBranchBar.vue:22-27`,注释「待接入真实 git」)与 `Session.workspace`/`branch` 字段。

| 方法 | 路径 | 说明 | 状态 |
| --- | --- | --- | --- |
| GET | `/api/v1/workspaces` | 工作区列表(会话新建页/欢迎页展示) | 🧩 规划中 |
| GET | `/api/v1/workspaces/{id}/branches` | 分支列表,替换 `FileBranchBar` mock | 🧩 规划中 |
| GET | `/api/v1/workspaces/{id}/tree` | 文件树(规划中,`reveal`/`copyPath` 定位需要) | 🧩 规划中 |

### 4.5 文件 API

对应前端:当前只有本地路径数组(`preload/index.d.ts:13`)。后端按「路径直读」或「上传」二选一设计,建议同时支持:

| 方法 | 路径 | 说明 | 状态 |
| --- | --- | --- | --- |
| POST | `/api/v1/files` | 上传文件(multipart),返回 `Attachment` | 🧩 规划中 |
| POST | `/api/v1/files/from-path` | body:`{ "path": "D:/xxx/a.ts" }`,登记本地路径(桌面端直读,不传内容) | 🧩 规划中 |
| GET | `/api/v1/files/{id}/content` | 读取文件内容(流式/分块,供 Agent 与面板使用) | 🧩 规划中 |
| GET | `/api/v1/files/{id}` / DELETE | 附件详情 / 删除 | 🧩 规划中 |

### 4.6 工具面板 API(规划中)

对应前端:`PANEL_REGISTRY` 的 2 个 `toolId`(`panels/index.ts:21-24`),两者均已实现真实面板
(浏览器 `BrowserPanel.vue`、终端 `TerminalPanel.vue`)。`toolId` 即 API 命名空间:

| toolId | 路径 | 当前/规划能力 |
| --- | --- | --- |
| `browser` | `/api/v1/tools/browser/*` | 已实现内置浏览器(导航/截图/元素拾取);规划 DOM 提取等 |
| `terminal` | `/api/v1/tools/terminal/*` | 已实现终端面板;规划命令执行 WS 流式输出 |

> 早期文档列出的 `code-review` / `file-preview` / `project-wiki` / `database` 四个占位面板已移除。

通用工具注册/启停接口(对应设置页 `tools` 与 `Agent.toolIds`):

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/tools` | 工具列表与启用状态 |
| PATCH | `/api/v1/tools/{toolId}` | 启用/禁用、配置 |
| POST | `/api/v1/tools/{toolId}/invoke` | 工具调用(Agent function calling 回调) |

### 4.7 设置域 API(规划中)

对应前端:设置页注册表 7 项(`components/settings/registry.ts:16-24`),除 `system`(本地 UI 偏好,不走后端)与 `about` 外均需后端:

| 方法 | 路径 | 对应实体 | 说明 |
| --- | --- | --- | --- |
| GET/POST/PATCH/DELETE | `/api/v1/models` | `Model` | 模型管理与凭证(加密) |
| GET/POST/PATCH/DELETE | `/api/v1/agents` | `Agent` | 智能体 CRUD、绑定模型与工具 |
| GET/POST/PATCH/DELETE | `/api/v1/plugins` | `Plugin` | 插件管理 |
| GET/POST/PATCH/DELETE | `/api/v1/skills` | `Skill` | 技能管理 |

> `system` 设置项(`themeMode`/`themePreset`/`locale`/`zoom`/`sidebarWidth`/`panelWidth`,`stores/settings.ts:19-27`)属于**本地 UI 偏好**,建议继续存 localStorage,不进后端。

### 4.8 其他规划接口

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET/POST/PATCH/DELETE | `/api/v1/tasks/scheduled` | 定时任务(侧栏「定时任务」分组,`AppSidebar.vue:73-77`) |
| POST | `/api/v1/auth/login` / `/api/v1/auth/register` | 登录/注册(规划中,见 2.5) |

---

## 5. 前端映射表

前端「类型/常量/行为 → 后端实体或接口」速查:

| 前端位置(文件:行号) | 前端内容 | 后端对应 |
| --- | --- | --- |
| `src/renderer/src/types/index.ts:1-11` | `Session` 接口 | 实体 `Session`(3.1) |
| `src/renderer/src/types/index.ts:13-20` | `ChatMessage` 接口 | 实体 `ChatMessage`(3.1) |
| `src/renderer/src/types/index.ts:22-25` | `PanelTab` 接口 | 本地状态;`toolId` 枚举 → 工具 API 命名空间(4.6) |
| `src/renderer/src/services/agent.ts:6-8` | `ChatProvider.stream(history)` | `POST /chat/stream` SSE(4.3) |
| `src/renderer/src/services/agent.ts:46-48` | `createAgentProvider()` 替换点 | 后端接入的切换位置 |
| `src/renderer/src/stores/sessions.ts:150-172` | `init` / `createSession` / `setActive` | 会话列表/创建接口(4.1) |
| `src/renderer/src/stores/sessions.ts:177-206` | `sendMessage` / `runStream` | 消息追加 + 流式对话(4.2、4.3) |
| `src/renderer/src/stores/sessions.ts:210-212` | `persist()` 写 localStorage | 后端接入后改为 API 调用 |
| `src/renderer/src/stores/settings.ts:19-27` | `DEFAULT_SETTINGS` | 本地偏好,不进后端(4.7) |
| `src/renderer/src/panels/index.ts:21-24` | `PANEL_REGISTRY` 2 个 toolId | 工具 API 命名空间(4.6) |
| `src/renderer/src/components/settings/registry.ts:16-24` | `SETTING_REGISTRY` 7 项 | 设置域 API(4.7) |
| `src/renderer/src/components/chat/FileBranchBar.vue:22-27` | mock 分支列表 | `GET /workspaces/{id}/branches`(4.4) |
| `src/renderer/src/views/NewChatView.vue:21-27` | 附件路径拼入用户消息 | `Attachment` 实体 + 文件 API(4.5) |
| `src/renderer/src/layouts/AppSidebar.vue:74-76` | 定时任务分组(恒空) | `ScheduledTask` 实体 + 定时任务 API(4.8) |
| `src/renderer/src/layouts/StatusBar.vue:46-52` | 会话菜单 pin/rename/archive/wiki/reveal/copyPath/copyId | `PATCH /sessions/{id}` + 工作区/Wiki 接口 |
| `src/renderer/src/components/chat/FileBranchBar.vue:10-11`、`src/renderer/src/views/NewChatView.vue:21-27` | `files: string[]` 附件路径 | `Attachment` 实体 + 文件 API(4.5) |
| `src/preload/index.d.ts:13`、`src/main/ipc.ts:34-42` | `dialog.openFiles()` → `string[]` | 文件「from-path」通道(4.5) |
| `src/renderer/src/utils/index.ts:1-4` | `uid()` base36 短 ID | 后端改用 UUID(2.4) |
| `src/renderer/src/stores/sessions.ts:10` | `BRANCH_POOL` mock 分支池 | `GitBranch` 实体种子数据参考 |

---

## 6. 附录:关键文件清单

| 文件 | 作用 |
| --- | --- |
| `src/renderer/src/types/index.ts` | 全部业务类型(Session / ChatMessage / PanelTab) |
| `src/renderer/src/services/agent.ts` | Agent 数据源抽象与 Mock 实现(后端接入点) |
| `src/renderer/src/stores/sessions.ts` | 会话/消息状态、localStorage 持久化、流式消费 |
| `src/renderer/src/stores/settings.ts` | 应用设置状态 |
| `src/renderer/src/stores/tabs.ts` | 面板页签状态 |
| `src/renderer/src/panels/index.ts` | 工具面板注册表(toolId 枚举) |
| `src/renderer/src/components/settings/registry.ts` | 设置页注册表(模型/智能体/插件/技能/工具) |
| `src/renderer/src/components/chat/FileBranchBar.vue` | 附件路径与分支选择(待接真实 git) |
| `src/renderer/src/views/NewChatView.vue` | 欢迎页(文件/分支选择 + 发送) |
| `src/renderer/src/layouts/AppSidebar.vue` | 会话树(任务/项目/定时任务分组) |
| `src/renderer/src/layouts/StatusBar.vue` | 会话操作菜单、工具栏入口 |
| `src/preload/index.d.ts`、`src/main/ipc.ts` | IPC 桥与主进程处理(当前仅窗口/对话框能力) |
