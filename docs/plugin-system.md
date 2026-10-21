# 插件系统（Plugin System）

> 面向**外部插件**的本地托管 + 双向消息 + 能力复用方案。
> 插件 = 一个带 `plugin.json` 清单的本地目录（含静态网站），由应用自定义协议
> `astorm-plugin://` 离线托管，页签中 iframe 显示，经 postMessage SDK 与应用通信。
> 本版本（v1）仅支持本地目录型插件；远程 URL / 打包安装等见文末「展望」。

## 1. 架构与通信时序

```
┌───────────────────────────── 渲染层（Electron renderer） ─────────────────────────────┐
│                                                                                       │
│  插件页签（PluginSitePanel.vue）                                                       │
│  ┌──────────────── tailor ────────────────┐         postMessage 协议                    │
│  │  iframe sandbox                        │ ◄──────────────────────────────────────────► │
│  │  astorm-plugin://<id>/index.html       │   {astorm:true, kind:request/response/      │
│  │     └ window.astorm (SDK)              │     stream/event, id, method, params...}    │
│  └────────────────────────────────────────┘   origin 校验：event.source + 插件 id         │
│                                                                                       │
│  stores/plugins.ts  ⇄  window.api.llm.ask ⇄  napi:event / llm:event                    │
└───────────────────────────────────────────────────────────────────────────────────────┘
                                      │ IPC（backend:invoke / llm:ask / llm:event）
┌────────────────────────────── 主进程（Electron main） ────────────────────────────────┐
│  plugins.ts   注册表 plugins.json + userData/plugins/ 目录 + astorm-plugin:// 协议     │
│  llm.ts       llm:ask（转发 binding.askLlm 的 Web ReadableStream → llm:event）         │
│  backend.ts   插件命令路由（plugin_scan/install/remove/set_enabled/save_config）        │
└───────────────────────────────────────────────────────────────────────────────────────┘
                                      │ napi 函数调用（synchronous）
┌────────────────────────────── Rust napi 后端（astorm-backend.node） ──────────────────┐
│  astorm-napi::ask_llm ──► astorm-core::AppState::ask_llm_stream                       │
│    复用 LlmRegistry（供应商/模型配置）→ llm-provider::chat_stream → 事件流               │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

核心选择：

- **托管**：安装即复制目录到 `userData/plugins/<id>/`，`astorm-plugin://<id>/...` 静态托管
  （标准 scheme，hostname = 插件 id ⇒ 每个插件独立 origin；站点根 = 插件目录自身，与 plugin.json 同层）。
- **SDK**：`astorm-plugin://__sdk__/astorm-sdk.js`（随应用分发）注入插件页面；插件引入后
  经 `window.parent.postMessage` 与应用双向通信，无需插件方自建任何服务。
- **LLM 复用**：新增 napi `ask_llm` 单次补全，直接走应用已有的 LLM 注册表/供应商配置，
  不创建会话、不污染侧栏；事件经全局事件总线（`type:"llm"` + `call_id`）回传。

## 2. 插件目录与清单（plugin.json）

插件打包为一个目录，上传的 `entry` 等路径均相对目录根：

```
my-plugin/
├── plugin.json     # 清单（必填）
├── index.html      # 入口页面（任意相对路径）
├── style.css / app.js / icon.svg …
```

仓库内需要构建的插件，源码工程统一放仓库根 `plugins_dev/<name>/`（独立工程，
`pnpm build` 输出到 `plugins/<name>/`）；`plugins/` 只存放可直接运行/分发的**净安装包**
（内置插件随 electron-builder 打包进 `resources/plugins`，源码工程不在分发范围内）。
无需构建的手写插件（如 `demo-plugin`）可一个目录兼任源码与净包。

`plugin.json` 字段：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 必填，`^[a-z0-9][a-z0-9-_]{0,63}$`，全局唯一 |
| `name` | string | 必填，显示名 |
| `version` | string | 版本号 |
| `description` | string | 描述 |
| `entry` | string | 必填，入口页面相对路径（如 `./index.html`） |
| `icon` | string | 可选，图标（当前版本页签统一用拼图占位） |
| `permissions` | string[] | 申请的能力：`llm` / `chat`；缺省 = 仅默认能力 |
| `sdkVersion` | string | SDK 版本号（当前 `"1"`） |
| `configSchema` | array | 配置字段定义（驱动设置页动态表单） |

`configSchema` 字段结构：

```ts
interface PluginConfigField {
  key: string                 // 配置键
  label: string               // 表单标签
  type: 'text' | 'textarea' | 'password' | 'secret' | 'number' | 'boolean' | 'select'
  required?: boolean          // 必填（保存时校验）
  default?: unknown           // 默认值
  placeholder?: string
  description?: string
  options?: Array<{ label: string; value: string }>   // select 用
}
```

`secret` 类型字段由主进程 `safeStorage` 加密落盘（降级 `plain:` 前缀明文标记），
解密仅发生在主进程；`config.get` 返回的是解密后的值（供插件自身使用）。

### 示例

```json
{
  "id": "demo",
  "name": "示例插件",
  "version": "0.1.0",
  "description": "展示插件系统能力的演示插件",
  "entry": "./index.html",
  "permissions": ["llm", "chat"],
  "sdkVersion": "1",
  "configSchema": [
    { "key": "apiBase", "label": "示例 API 地址", "type": "secret", "default": "" },
    { "key": "greeting", "label": "问候语", "type": "text", "default": "你好 👋" }
  ]
}
```

完整示例见仓库根 `plugins/demo-plugin/`（内置 demo，首次运行自动安装）。
SDK（`astorm-plugin://__sdk__/astorm-sdk.js`）源文件在仓库根 `plugins/sdk/astorm-sdk.js`。

## 3. 安装 / 卸载 / 启停

| 动作 | 入口 | 说明 |
| --- | --- | --- |
| 安装 | 侧栏「插件」分组 `+`按钮 / 设置 → 插件 → 安装插件 | 选择含 `plugin.json` 的文件夹 → 校验清单 → 复制到 `userData/plugins/<id>/`，默认启用 |
| 卸载 | 设置 → 插件 → 卸载 | 删除注册表条目 + 本地目录 |
| 启停 | 设置 → 插件 → 开关 / 点击已禁用条目 | `enabled` 落盘 `plugins.json`；禁用后侧栏/工具栏隐藏、页签显示占位 |
| 配置 | 设置 → 插件 → 点卡片展开 | schema 驱动动态表单 → `plugin_save_config` |

持久化文件：

- 插件内容：`<userData>/plugins/<id>/`
- 注册表（启停 + 明文配置 + 加密 secret）：`<userData>/plugins.json`

首次运行（dev 与打包一致）：主进程把仓库根 `plugins/demo-plugin` 复制为
`userData/plugins/demo` 并注册启用。

## 4. 消息协议（postMessage）

插件 ⇄ 应用全部消息都带 `astorm: true` 前缀；插件侧由 SDK 封装，一般无需手写。

**插件 → 应用（request）**

```jsonc
{ "astorm": true, "kind": "request", "id": 1, "method": "llm.chat", "params": { /* 方法参数 */ } }
```

**应用 → 插件（response / stream / event）**

```jsonc
{ "astorm": true, "kind": "response", "id": 1, "ok": true, "result": { } }
{ "astorm": true, "kind": "response", "id": 1, "ok": false, "error": { "code": "FORBIDDEN", "message": "..." } }
{ "astorm": true, "kind": "stream", "id": 1, "chunk": { "type": "chunk", "delta": "..." } }
{ "astorm": true, "kind": "event", "event": "app.message", "data": { "from": "app", "text": "...", "ts": 0 } }
```

**方法一览**

| method | 权限 | 参数 | 返回 |
| --- | --- | --- | --- |
| `platform.info` | — | — | `{ app, sdkVersion, protoVersion, providers, sessionActive, locale }` |
| `config.get` | — | — | `{ config, schema }` |
| `chat.appendInput` | — | `{ text }` | `{ ok }`（填入当前会话发送框，不发送） |
| `chat.send` | `chat` | `{ text }` | `{ ok }`（按当前输入框模式/权限/思考强度直接发送） |
| `state.get` | — | `{ key? }` | `{ sessionId, value }`（会话级状态读，传 key 取单值） |
| `state.set` | — | `{ key, value }` | `{ ok, sessionId }`（会话级状态写，立即持久化） |
| `llm.chat` | `llm` | `{ messages[], provider?, model?, reasoningEffort? }` | 流式 `chunk/reasoning/end/error` |
| `file.openPptx` | — | — | `{ canceled }` 或 `{ canceled:false, file, deck }`（系统对话框选 .pptx，默认定位当前会话空间；宿主 json2pptx 解包后回 deck） |
| `plugin.ping` | — | — | `{ ok, ts }` |
| `plugin.messageToApp` | — | `{ text }` | `{ ok }`（应用在插件页签内展示收到消息） |

流式事件（`llm.chat`）契约与后端 `ask_llm` 一致：

```jsonc
{ "type": "chunk", "delta": "..." }
{ "type": "reasoning", "delta": "..." }
{ "type": "end", "usage": { "prompt": 0, "completion": 0, "total": 0 }, "finishReason": "stop" }
{ "type": "error", "message": "..." }   // 错误后仍会补一条 end
```

**应用 → 插件事件**：
- `event: "app.message"`，`data: { from, text, ts }`。触发来源：插件页签工具栏「发送消息给插件」、插件加载时自动推送的欢迎消息。
- `event: "app.session.change"`，`data: { sessionId, ts }`。页签全局共享、切会话 iframe 存活时推送，插件据此重新读取新会话自己的状态。

**权限门控**：`llm.chat` 需清单 `permissions` 含 `llm`；`chat.send` 需含 `chat`。
非授权方法返回 `{ code: "FORBIDDEN" }`。`chat.appendInput / config.get / state.* /
app.onMessage / platform.info / plugin.*` 为默认能力。

**origin 校验（宿主侧）**：仅接受 `event.source === 当前插件 iframe.contentWindow`
且 `event.origin === astorm-plugin://<插件id>` 的消息。

### 会话级状态（state.get / state.set）

插件数据按 **会话 × 插件** 作用域保存（应用 localStorage，key `astorm:plugin-state:<会话id>:v1`，
与页签的按会话持久化同构）：

- `state.set(key, value)` 写入**当前会话**下本插件的键值，立即落盘；
- `state.get(key?)` 读取当前会话本插件数据，返回 `{ sessionId, value }`；
- **页签按会话绑定**：切换会话时页签换挡、插件 iframe 重建（不做跨会话保活），
  插件每次加载后主动 `state.get` 读到**该会话自己**的数据——「按会话存、切换恢复」
  由"每会话一份数据 + 插件加载时拉取"完成，不依赖 iframe 存活；
- SDK 仍提供 `app.session.change` 订阅（插件在自身存活期间收到会话切换通知的扩展能力，
  绑定模式下重建场景不需要它）；
- 无绑定会话（新建页）时不保存；同会话内不同插件互不可见（以插件 id 隔离）。

## 5. 插件 SDK（astorm-sdk.js）

插件页面引入（由应用协议托管）：

```html
<script src="astorm-plugin://__sdk__/astorm-sdk.js"></script>
```

```js
const { sdk } = window.astorm            // 亦可直接 window.astorm.xxx
await sdk.ready                          // 就绪（连接探测完成）

const { config } = await sdk.config.get()

await sdk.chat.appendInput('文本')        // 填入发送框
await sdk.chat.send('文本')               // 直接发送

const { sessionId, value } = await sdk.state.get('memo')   // 会话级状态读
await sdk.state.set('memo', '只有本会话可见')               // 会话级状态写

const res = await sdk.llm.chat({
  messages: [{ role: 'user', content: '你好' }],
  provider: undefined,                    // 缺省用默认供应商
  model: undefined,                       // 缺省用默认模型
  onChunk: (chunk) => { /* chunk.type: chunk|reasoning|end|error */ console.log(chunk.delta) }
})
// res: { text, reasoning, usage, finishReason }

sdk.app.onMessage((data) => { /* data.from / data.text / data.ts */ })
sdk.app.onSessionChange(({ sessionId }) => { /* 切会话:重读该会话状态 */ })
await sdk.plugin.sendToApp({ text: '在吗？' })
const info = await sdk.platform.info()
```

- 零依赖、UMD：暴露 `window.astorm`（含版本号/协议号）。
- 非应用内打开（独立浏览器）时所有请求立即 reject（`NOT_EMBEDDED`），不会静默挂起。
- 请求超时：普通方法 30s，`llm.chat` 180s。

## 6. LLM 复用（后端 ask_llm）

`llm.chat` 的完整链路：

1. 渲染层 `services/llm.ts::askLlm` → `window.api.llm.ask`（preload）
2. preload 触发 `llm:ask`（`{ callId, request }`），并订阅全局事件流 `napi:event`，按 `type:"llm"` + `call_id` 过滤
3. 主进程 `llm.ts::registerLlmIpc`：`llm:ask` → `napiCall('askLlm', request, callId)`，启动即返回
4. Rust `astorm-napi::ask_llm`（`src/session/ask_llm.rs`）→ `astorm-core::AppState::spawn_llm_chat`
   （`src/app/ask_llm.rs`）：
   - 复用 `LlmRegistry` 路由客户端与模型：`client_for(provider)`（缺省默认供应商）、
     `resolve_model_name` / `default_model`
   - `client.chat_stream(...)` 消费 `LlmEvent`，逐条转为 `{"type":"llm","call_id":..,"kind":..}`
     发布到**全局事件总线**（与 `subscribe()` 同一条流），前端按 call_id 路由到对应插件会话

与聊天会话完全独立：不创建会话、不产生侧栏条目、不进入会话协议事件（`type` 恒为 `llm`，
会话订阅方忽略未知类型）。事件走总线而非为每次调用创建 napi ReadableStream，
避免原生流在 tokio worker 线程析构时的 GC 竞态 panic。

## 7. 安全模型

- 插件页面运行在 **iframe + sandbox**（`allow-scripts allow-same-origin allow-forms`），
  无 Node 能力、无 Electron IPC；唯一通道是被 origin + source 双重校验的 postMessage。
- 插件不能读取主进程 / 渲染层其他数据；能调用的能力由 `permissions` 门控。
- `secret` 配置由 `safeStorage` 加密落盘；磁盘上不存明文（降级标记 `plain:`）。
- 安装来自用户主动选择的本地目录；协议路径经规范化防目录穿越（403）。
- 此模型下插件等同于「用户手动信任的本地脚本」——安装/删除均应来自可信源。

## 8. Demo 插件

内置 `plugins/demo-plugin/`，首次运行自动安装为 id `demo`。在应用内：

1. 侧栏切换「插件」分组 → 点击「示例插件」打开插件页签（或右侧工具栏拼图按钮）
2. 设置 → 插件 → 修改问候语 / 主题色 / API 地址并保存，回到页签刷新可见
3. 页签内逐项体验：
   - `sdk.config.get()` 配置回显
   - `sdk.llm.chat()` 调用 LLM（需先在「模型设置」配置供应商；流式输出 + usage）
   - `sdk.chat.appendInput()` 把文本放进发送框、「直接发送」进入会话链路
   - `sdk.app.onMessage()` 接收「发送消息给插件」推送；`sdk.plugin.sendToApp()` 让应用在插件页签内展示

## 8.1 内置插件 ppt-inspector（PPT 元素拾取器）

`plugins/ppt-inspector/` 为随应用分发的**净安装包**（plugin.json + 构建产物，随
electron-builder extraResources 打包；首次运行由 `ensureDemoPlugin` 自动安装启用），
源码工程在 `plugins_dev/ppt-inspector/`（独立 Vite 工程，与 `plugins/` 分仓存放，
`pnpm build` 输出到 `plugins/ppt-inspector/` 净包目录）。

功能链路：页签内点「导入 PPTX」→ 宿主系统对话框（默认落在**当前会话空间目录**）选成品
`.pptx` → 主进程 spawn 随包分发的 `json2pptx-cli`（`frontend/resources/bin/`，
构建见 `scripts/sync-pptx-cli.mjs`，需 `AI_PPT_ROOT` 指向 json2pptx 源码仓库）解包 →
`file.openPptx` 响应回传 deck JSON（`{width,height,meta?,theme?,slides}`）→ 渲染幻灯片。
点击画布元素即固定并自动 `chat.appendInput` 其 CLI 路径（`/slide[N]/type[K]`，同元素去重），
供会话配合 json2pptx edit 精准修改；图片/图表等媒体元素在页签内渲染为占位框（拾取不受影响）。

**权限**：无需额外权限（`file.openPptx` 为默认能力）；清单仅声明 `chat`。
**开发迭代**：改 `plugins_dev/ppt-inspector` 后 `pnpm build` 刷新净包；dev 环境需删除
`<userData>/plugins/ppt-inspector` 后重启应用以重装内置副本。

## 9. 文档与代码索引

- 协议宿主侧实现：`frontend/src/renderer/src/panels/PluginSitePanel.vue`
- 插件 store：`frontend/src/renderer/src/stores/plugins.ts`
- 主进程注册表/协议：`frontend/src/main/plugins.ts`；补全：`frontend/src/main/llm.ts`
- 前端 SDK 服务：`frontend/src/renderer/src/services/llm.ts`
- 设置页：`frontend/src/renderer/src/components/settings/pages/PluginsSettings.vue`、
  动态表单 `components/plugins/PluginConfigForm.vue`
- 后端：`backend/crates/astorm-napi/src/session/ask_llm.rs`、
  `backend/crates/astorm-core/src/app/ask_llm.rs`（事件契约见其 `ask_llm_stream` 注释）
- SDK：`plugins/sdk/astorm-sdk.js`（协议 `__sdk__` 动态托管）
- Demo：`plugins/demo-plugin/`
- 内置页签插件：净包 `plugins/ppt-inspector/`、源码工程 `plugins_dev/ppt-inspector/`、解包 CLI 同步 `scripts/sync-pptx-cli.mjs`

## 10. 展望（非本期范围）

- 远程 URL 型插件（entry 为 `https://…`；第三方域名受限时需自带 SDK 文件）
- `zip` 包在线安装 / 插件市场目录
- Electron `<webview>` + 每插件 preload 注入（插件免引 SDK）
- MCP 作为插件后端；插件间消息；会话事件按需转发给插件（设计上已预留
  `app.message` 事件通道与 `messages.receive` 权限位）