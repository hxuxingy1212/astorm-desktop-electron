/**
 * Electron e2e 驱动库（零第三方依赖，除 `ws`）。
 *
 * 三件事：
 *  ① **隔离实例**：`ASTORM_DATA_DIR` + `ASTORM_CDP_PORT` 起一份独立 app —— 数据目录、单实例锁
 *     都与用户正在用的那份无关，绝不碰真实数据；退出按进程组杀（`.bin/electron` 只是启动壳，
 *     杀它不杀 app，残留实例会占着 CDP 端口把下一轮连到旧窗口上）。
 *  ② **CDP 驱动**：真实点击 / 真实输入 / 真实 DOM 断言，不做内部状态注入；窗口换壳（本机↔云端
 *     是创建期选项，只能重建窗口）会换 CDP target，`attach({ notId })` 负责重新挂上。
 *  ③ **桩服务端**：本地 http，提供 `/api/v1/status`（令牌校验 + 模型/数据目录）与一个带标记的
 *     首页 —— 云端分支的「测连接」「加载远端 UI + 令牌注入」都在它上面断言，不依赖真实容器。
 */
import { execFileSync, spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const require = createRequire(import.meta.url)
const WebSocket = require('ws')
/** 真 Electron 可执行文件路径（不是 node_modules/.bin 那个启动壳） */
const ELECTRON = require('electron')

export const FRONTEND = join(dirname(fileURLToPath(import.meta.url)), '..', '..')

/** 所有起过的实例，收尾时统一清掉（用例中途抛错也不会漏） */
const live = new Set()

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/** 杀掉占着某个数据目录的残留实例（只认自己的目录，不动用户的 app） */
export function killStale(dataDir) {
  try {
    let ps = ''
    if (process.platform === 'darwin') {
      ps = execFileSync('ps', ['-Eww', '-eo', 'pid=,command='], { encoding: 'utf8' })
    } else if (process.platform === 'linux') {
      // GNU procps 不支持 -E；eww 同样把环境变量拼进 command 便于按 dataDir 匹配
      ps = execFileSync('ps', ['eww', '-eo', 'pid=,command='], { encoding: 'utf8' })
    } else {
      // Windows 没有 ps：残留由每轮 stop() 的 taskkill /T 负责
      return
    }
    for (const line of ps.split('\n')) {
      if (!line.includes(dataDir) || line.includes('ps -E')) continue
      const pid = Number(line.trim().split(/\s+/)[0])
      if (pid && pid !== process.pid) process.kill(pid, 'SIGKILL')
    }
  } catch {
    /* 没有残留 */
  }
}

async function listTargets(port) {
  const resp = await fetch(`http://127.0.0.1:${port}/json/list`)
  return (await resp.json()).filter((t) => t.type === 'page')
}

/** 等 CDP 端点起来并出现满足条件的页面 target */
async function waitTarget(port, predicate = () => true, timeoutMs = 40000) {
  const deadline = Date.now() + timeoutMs
  let last = []
  while (Date.now() < deadline) {
    try {
      last = await listTargets(port)
      const hit = last.find(predicate)
      if (hit) return hit
    } catch {
      /* 还没起来 */
    }
    await sleep(250)
  }
  throw new Error(`等页面 target 超时（${timeoutMs}ms）；现有：${last.map((t) => t.url).join(', ')}`)
}

/** 一个 CDP 页面连接：run（求值）/ waitFor（轮询断言）/ text / shot */
class Client {
  constructor(ws, target) {
    this.ws = ws
    this.target = target
    this.targetId = target.id
    this.closed = false
    this.seq = 0
    this.pending = new Map()
    ws.on('message', (raw) => {
      const m = JSON.parse(raw.toString())
      const cb = m.id && this.pending.get(m.id)
      if (cb) {
        this.pending.delete(m.id)
        cb(m)
      }
    })
    ws.on('close', () => {
      this.closed = true
      for (const cb of this.pending.values()) cb({ __closed: true })
      this.pending.clear()
    })
  }

  send(method, params = {}, timeoutMs = 20000) {
    return new Promise((res, rej) => {
      if (this.closed) return rej(new Error('CDP 连接已断开'))
      const id = ++this.seq
      const timer = setTimeout(() => {
        this.pending.delete(id)
        rej(new Error(`CDP 超时（${timeoutMs}ms）: ${method}`))
      }, timeoutMs)
      this.pending.set(id, (m) => {
        clearTimeout(timer)
        if (m.__closed) rej(new Error('CDP 连接被关闭（页面换壳/导航？）'))
        else res(m)
      })
      this.ws.send(JSON.stringify({ id, method, params }))
    })
  }

  /** 求值（自动 await 异步体、按值返回；页面内抛错原样抛出） */
  async run(code, timeoutMs = 20000) {
    const r = await this.send(
      'Runtime.evaluate',
      { expression: `(async () => { ${code} })()`, awaitPromise: true, returnByValue: true },
      timeoutMs
    )
    if (r.result?.exceptionDetails) {
      throw new Error(
        '页面内异常: ' +
          (r.result.exceptionDetails.exception?.description ??
            r.result.exceptionDetails.text ??
            'unknown')
      )
    }
    return r.result?.result?.value
  }

  /** 轮询求值直到为真；超时抛错（附最后的值） */
  async waitFor(code, { timeoutMs = 15000, desc = code } = {}) {
    const deadline = Date.now() + timeoutMs
    let last
    while (Date.now() < deadline) {
      last = await this.run(`return !!(${code})`)
      if (last) return true
      await sleep(200)
    }
    throw new Error(`等待超时: ${desc}`)
  }

  async text(selector) {
    return this.run(
      `return document.querySelector(${JSON.stringify(selector)})?.textContent?.trim() ?? null`
    )
  }

  /** 截图（失败诊断用） */
  async shot(path) {
    const r = await this.send('Page.captureScreenshot', { format: 'png' })
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, Buffer.from(r.result.data, 'base64'))
    return path
  }

  close() {
    try {
      this.ws.close()
    } catch {
      /* 已断开 */
    }
  }
}

/** 一个隔离的 app 实例 */
export class App {
  constructor({ dataDir, cdpPort, child }) {
    this.dataDir = dataDir
    this.cdpPort = cdpPort
    this.child = child
    this.log = []
    this.client = null
    child.stdout.on('data', (d) => this.log.push(String(d)))
    child.stderr.on('data', (d) => this.log.push(String(d)))
    live.add(this)
  }

  /** 日志尾部（失败诊断） */
  tail(n = 25) {
    return this.log
      .join('')
      .split('\n')
      .filter(Boolean)
      .slice(-n)
      .join('\n')
  }

  /** 挂上当前（或下一个满足条件的）页面 */
  async attach({ predicate, notId, timeoutMs = 40000 } = {}) {
    const target = await waitTarget(
      this.cdpPort,
      (t) => (notId ? t.id !== notId : true) && (predicate ? predicate(t) : true),
      timeoutMs
    )
    return this.connect(target)
  }

  /**
   * 挂上「已经就绪」的页面：CDP target 出现 ≠ 页面挂载完（刚建窗口时文档还是空的），
   * 页面**重载**（如回欢迎页）还会把旧连接掐断 —— 这里反复「连上 → 探一次 → 不行就换下一个」，
   * 直到某个页面的 DOM 满足 `ready`。`notId` 用于已知会换 target 的场景（换壳重建窗口）。
   */
  async attachReady({ ready, notId, timeoutMs = 45000 } = {}) {
    const deadline = Date.now() + timeoutMs
    let lastErr = '（无 target）'
    while (Date.now() < deadline) {
      let targets = []
      try {
        targets = await listTargets(this.cdpPort)
      } catch (e) {
        lastErr = e.message
      }
      for (const t of targets) {
        if (notId && t.id === notId) continue
        let client
        try {
          client = await this.connect(t)
          const ok = ready ? await client.run(`return !!(${ready})`) : true
          if (ok) return client
          lastErr = 'DOM 未就绪'
        } catch (e) {
          lastErr = e.message
        }
        client?.close()
      }
      await sleep(250)
    }
    throw new Error(`等页面就绪超时（${timeoutMs}ms）: ${ready ?? '(任意页面)'}；最后错误：${lastErr}`)
  }

  async connect(target) {
    const ws = new WebSocket(target.webSocketDebuggerUrl, { perMessageDeflate: false })
    await new Promise((res, rej) => {
      ws.on('open', res)
      ws.on('error', rej)
    })
    const client = new Client(ws, target)
    this.client?.close()
    this.client = client
    return client
  }

  async stop() {
    this.client?.close()
    if (process.platform === 'win32') {
      try {
        execFileSync('taskkill', ['/PID', String(this.child.pid), '/T', '/F'], {
          stdio: 'ignore'
        })
      } catch {
        /* 已退出 */
      }
    } else {
      try {
        process.kill(-this.child.pid, 'SIGKILL')
      } catch {
        try {
          this.child.kill('SIGKILL')
        } catch {
          /* 已退出 */
        }
      }
    }
    live.delete(this)
    await sleep(300)
  }
}

/**
 * 起一份隔离实例。
 * @param dataDir  数据目录（`<userData>` 会是它的 `electron/` 子目录）；传 fresh=true 先清空
 */
export async function launchApp({ dataDir, cdpPort, fresh = false, env = {} }) {
  killStale(dataDir)
  if (fresh) rmSync(dataDir, { recursive: true, force: true })
  const child = spawn(
    ELECTRON,
    [
      '.',
      // 测试窗口常常被别的窗口挡住：被遮挡的窗口 Chromium 会节流定时器，Vue 的 out-in 过渡
      // （靠 transitionend/时长兜底定时器）可能迟迟不结束，页面就卡在「空白卡片」上。
      // 这三个开关只影响这一份隔离实例。
      '--disable-background-timer-throttling',
      '--disable-backgrounding-occluded-windows',
      '--disable-renderer-backgrounding'
    ],
    {
      cwd: FRONTEND,
      detached: true,
      env: { ...process.env, ASTORM_DATA_DIR: dataDir, ASTORM_CDP_PORT: String(cdpPort), ...env },
      stdio: ['ignore', 'pipe', 'pipe']
    }
  )
  const app = new App({ dataDir, cdpPort, child })
  await app.attach({ timeoutMs: 60000 })
  return app
}

/**
 * 按 OpenAI 兼容 SSE 逐片回一段正文（真实会话轮走的就是这条流；分包推送顺带覆盖流式渲染）。
 * 收尾补一帧 `finish_reason: "stop"` + `[DONE]`，与真实网关一致。
 */
function sseReply(res, text, model, { chunk = 12, delayMs = 60 } = {}) {
  res.writeHead(200, {
    'content-type': 'text/event-stream; charset=utf-8',
    'cache-control': 'no-cache',
    connection: 'keep-alive'
  })
  const frame = (delta, finish = null) =>
    `data: ${JSON.stringify({
      id: 'stub-stream',
      object: 'chat.completion.chunk',
      created: Math.floor(Date.now() / 1000),
      model,
      choices: [{ index: 0, delta, finish_reason: finish }]
    })}\n\n`
  const parts = text.match(new RegExp(`[\\s\\S]{1,${chunk}}`, 'g')) ?? []
  let i = 0
  const timer = setInterval(() => {
    if (i < parts.length) {
      res.write(frame({ content: parts[i++] }))
      return
    }
    clearInterval(timer)
    res.write(frame({}, 'stop'))
    res.write('data: [DONE]\n\n')
    res.end()
  }, delayMs)
  res.on('close', () => clearInterval(timer))
}

/** 收尾：杀掉本库起过的所有实例 */
export async function stopAll() {
  for (const app of [...live]) await app.stop()
}

/** 读连接配置（`<dataDir>/electron/connection.json`） */
export function readConnection(dataDir) {
  try {
    return JSON.parse(readFileSync(join(dataDir, 'electron', 'connection.json'), 'utf8'))
  } catch {
    return null
  }
}

/** 读供应商配置（`<dataDir>/config/providers.json`；密钥为 safeStorage 密文） */
export function readProviders(dataDir) {
  try {
    return JSON.parse(readFileSync(join(dataDir, 'config', 'providers.json'), 'utf8'))
  } catch {
    return null
  }
}

/**
 * 桩云服务端 + 桩 LLM：`/api/v1/status`（Bearer 校验）+ 一次最小补全（chat/completions、
 * responses、anthropic messages 三种路径都认）+ 带标记的首页（回读 localStorage 里的令牌）。
 * 断言「测连接」「云端壳子加载远端 UI + 令牌注入」「模型探测」用，不依赖真实容器/真实模型。
 *
 * `turnReply` 非空时，**流式**补全请求（真实会话轮：`stream: true`）改回它——按 SSE 逐片推送，
 * 于是能在不发真实模型的前提下跑完整一轮对话（会话标题那次调用仍回 JSON 标题，见下）。
 * 也可传函数 `(baseUrl) => string`：正文里要引用「桩服务端自己」的页面时用它
 * （端口要等 listen 之后才知道）。
 */
export async function startStubCloud({
  token,
  model = 'stub-model',
  dataDir = '/stub/data',
  turnReply = null,
  /** 标题调用返回内容（字符串或 `(prompt) => string`）；缺省用文件引用用例的双语标题 */
  titleReply = null,
  /** 摘要调用返回内容（字符串或 `(prompt) => string`）；缺省不识别，按普通正文处理 */
  summaryReply = null,
  /** 目标判定调用返回内容（字符串或 `(prompt) => string`）；缺省不识别，按普通正文处理 */
  goalJudgeReply = null
}) {
  const requests = []
  /** 补全请求体全文（真会话轮 / 标题调用 / 探测都记）：断言提示词与工具面真的发给了模型 */
  const bodies = []
  /** listen 之后才有（供 `turnReply` 函数形态拼来源 URL） */
  let baseUrl = ''
  const server = createServer((req, res) => {
    const url = req.url ?? ''
    requests.push(`${req.method} ${url}`)
    const json = (status, body) => {
      res.writeHead(status, { 'content-type': 'application/json' })
      res.end(JSON.stringify(body))
    }
    // 供应商探测 / 会话轮 / 标题生成：providers:test 按协议 POST 到 baseUrl + 路径
    if (req.method === 'POST' && /\/(chat\/completions|responses|messages)$/.test(url)) {
      let raw = ''
      req.on('data', (c) => (raw += c))
      req.on('end', () => {
        const auth = req.headers.authorization ?? ''
        const key = req.headers['x-api-key'] ?? ''
        if (auth !== `Bearer ${token}` && key !== token) {
          json(401, { error: { message: 'invalid api key (stub)' } })
          return
        }
        let body = null
        try {
          body = JSON.parse(raw || '{}')
        } catch {
          /* 非 JSON 请求体：按探测处理 */
        }
        if (body) bodies.push(body)
        const stream = body?.stream === true
        const prompt = JSON.stringify(body?.messages ?? body?.input ?? '')
        // 模型能力探测（图文）：回固定答案让「文本+图片识别」断言通过
        if (/PINEAPPLE/i.test(prompt)) {
          json(200, {
            id: 'stub-capability',
            choices: [
              {
                index: 0,
                message: { role: 'assistant', content: 'PINEAPPLE 739' },
                finish_reason: 'stop'
              }
            ]
          })
          return
        }
        // 摘要调用（summary.rs 的结构化摘要提示）：用例可回一份合法摘要 JSON
        if (
          summaryReply &&
          prompt.includes('#工作流程') &&
          prompt.includes('user_requirement')
        ) {
          const text =
            typeof summaryReply === 'function' ? summaryReply(prompt) : summaryReply
          if (stream) return sseReply(res, text, model)
          json(200, {
            id: 'stub-summary',
            choices: [{ index: 0, message: { role: 'assistant', content: text }, finish_reason: 'stop' }]
          })
          return
        }
        // 目标判定（goal.rs 的完成判定器提示）：用例可回 {"done":bool,"message":string}
        // 只匹配判定器**系统提示**的专属开头——/goal 的启动提示词里也有「完成判定器」字样，
        // 泛匹配会把标题生成请求误当判定请求。
        if (
          goalJudgeReply &&
          (prompt.includes('你是本会话「目标推进」的完成判定器') ||
            prompt.includes('You are the completion judge for this session'))
        ) {
          const text =
            typeof goalJudgeReply === 'function' ? goalJudgeReply(prompt) : goalJudgeReply
          if (stream) return sseReply(res, text, model)
          json(200, {
            id: 'stub-goal-judge',
            choices: [{ index: 0, message: { role: 'assistant', content: text }, finish_reason: 'stop' }]
          })
          return
        }
        // 标题生成（title.rs 的静态系统提示）：回一份双语标题 JSON，别把正文回复串到标题上
        if (prompt.includes('会话标题')) {
          const text = titleReply
            ? typeof titleReply === 'function'
              ? titleReply(prompt)
              : titleReply
            : JSON.stringify({
                title_zh: '文件引用核对',
                title_en: 'File Reference Check'
              })
          if (stream) return sseReply(res, text, model)
          json(200, {
            id: 'stub-title',
            choices: [{ index: 0, message: { role: 'assistant', content: text }, finish_reason: 'stop' }]
          })
          return
        }
        if (stream && turnReply) {
          const text = typeof turnReply === 'function' ? turnReply(baseUrl) : turnReply
          return sseReply(res, text, model)
        }
        json(200, {
          id: 'stub-completion',
          choices: [{ index: 0, message: { role: 'assistant', content: 'pong' }, finish_reason: 'stop' }]
        })
      })
      return
    }
    if (url.startsWith('/api/v1/status')) {
      const auth = req.headers.authorization ?? ''
      if (auth !== `Bearer ${token}`) {
        json(401, { code: 401, message: 'unauthorized' })
        return
      }
      json(200, { code: 0, data: { llm: { defaultModel: model }, dataDir } })
      return
    }
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end(
      `<!doctype html><html><head><title>stub-cloud</title></head><body>
<div id="marker">STUB CLOUD UI</div>
<script>
  const m = document.getElementById('marker')
  m.dataset.token = localStorage.getItem('astorm:web:token') || ''
  m.dataset.api = typeof window.api
</script></body></html>`
    )
  })
  await new Promise((res) => server.listen(0, '127.0.0.1', res))
  const port = server.address().port
  baseUrl = `http://127.0.0.1:${port}`
  return {
    port,
    url: baseUrl,
    /** 供应商探测用的 baseUrl（补全路径由主进程按协议拼） */
    llmBaseUrl: `http://127.0.0.1:${port}/v1`,
    requests,
    /** 补全请求体（同上，供断言提示词 / 工具面） */
    bodies,
    close: () => new Promise((res) => server.close(res))
  }
}

/**
 * 落一份「本机 + 已配好模型」的最小配置，让隔离实例直接进应用外壳、第一轮对话就跑桩 LLM：
 * - `electron/connection.json`：本机形态（不再出引导）；
 * - `config/providers.json`：桩 LLM 作为唯一供应商（密钥明文，免 safeStorage 依赖）；
 * - `config/backend-settings.json`：**用户目录**（工作区 / 知识库的基目录）指到临时目录——
 *   不配的话默认是真实 `~/astorm`，会话工作区会写进用户的家目录。
 * 返回用户目录供用例往里放文件（会话工作区 = `<userDir>/workspace/<时间戳>`）。
 */
export function seedLocalApp({ dataDir, baseUrl, apiKey, model = 'stub-model' }) {
  const userDir = join(dataDir, 'userdir')
  mkdirSync(join(dataDir, 'electron'), { recursive: true })
  mkdirSync(join(dataDir, 'config'), { recursive: true })
  mkdirSync(userDir, { recursive: true })
  writeFileSync(
    join(dataDir, 'electron', 'connection.json'),
    JSON.stringify(
      { schemaVersion: 1, mode: 'local', endpoint: '', token: null, storage: 'safeStorage' },
      null,
      2
    )
  )
  writeFileSync(
    join(dataDir, 'config', 'providers.json'),
    JSON.stringify(
      {
        schemaVersion: 1,
        providers: [
          {
            name: 'stub',
            baseUrl,
            apiKey,
            storage: 'plaintext',
            apiFormat: 'chat_completions',
            models: [{ name: model, order: 0 }],
            order: 0
          }
        ]
      },
      null,
      2
    )
  )
  writeFileSync(
    join(dataDir, 'config', 'backend-settings.json'),
    JSON.stringify({ workspace: userDir }, null, 2)
  )
  return { userDir }
}

/** 会话工作区目录：`<userDir>/workspace/<时间戳>`（等后端建出来；超时返回空数组） */
export async function waitWorkspaces(userDir, { timeoutMs = 15000 } = {}) {
  const root = join(userDir, 'workspace')
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const dirs = readdirSync(root, { withFileTypes: true })
        .filter((d) => d.isDirectory())
        .map((d) => join(root, d.name))
      if (dirs.length) return dirs
    } catch {
      /* 目录还没建 */
    }
    await sleep(200)
  }
  return []
}

/** 往聊天输入框（contenteditable 提及编辑器）输入并回车发送：真实插入文本 + 真实 Enter 键 */
export async function sendChat(client, text) {
  const editor = `.mention-editor[contenteditable='true']`
  const sel = `document.querySelector(${JSON.stringify(editor)})`
  const key = {
    key: 'Enter',
    code: 'Enter',
    windowsVirtualKeyCode: 13,
    nativeVirtualKeyCode: 13,
    text: '\r'
  }
  await client.waitFor(sel, { timeoutMs: 20000, desc: '聊天输入框' })

  // 焦点没落上 / 编辑器刚被重挂载时 Input.insertText 会插空，故「插完先验文本，真进了才回车」；
  // 连续三次都没插进去就报错（比默默发不出消息好排查）
  for (let attempt = 0; attempt < 3; attempt++) {
    await client.run(`
      ${sel}.focus()
      await new Promise((r) => setTimeout(r, 200))
      return true
    `)
    await client.send('Input.insertText', { text })
    await client.run(`await new Promise((r) => setTimeout(r, 250)); return true`)
    const landed = await client.run(`return (${sel}?.textContent ?? '').includes(${JSON.stringify(text)})`)
    if (landed) {
      await client.send('Input.dispatchKeyEvent', { type: 'keyDown', ...key })
      await client.send('Input.dispatchKeyEvent', { type: 'keyUp', ...key })
      return
    }
    // 插了半截/没插上：清空重来（编辑器会把提交后的内容自行清空，这里只处理失败态）
    await client.run(`
      ${sel}.innerHTML = ''
      ${sel}.dispatchEvent(new Event('input', { bubbles: true }))
      await new Promise((r) => setTimeout(r, 150))
      return true
    `)
  }
  throw new Error('聊天输入框里没能插入消息文本（尝试 3 次）')
}

/* ---------------- 页面交互小工具（真实点击，不做状态注入） ---------------- */

/** 填输入框：走原生 setter + input 事件（Vue 的 v-model 才认），并等一拍让 DOM 追上禁用态 */
export function fill(client, selector, value) {
  return client.run(`
    const el = document.querySelector(${JSON.stringify(selector)})
    if (!el) throw new Error('找不到输入框: ' + ${JSON.stringify(selector)})
    const d = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')
    d.set.call(el, ${JSON.stringify(value)})
    el.dispatchEvent(new Event('input', { bubbles: true }))
    await new Promise((r) => setTimeout(r, 150))
    return el.value
  `)
}

/** 点单选项（Arco `type=button` 的单选组渲染成 label，按文案点） */
export async function clickRadio(client, group, text) {
  await client.waitFor(`document.querySelector('[data-ob=${group}]')`, {
    timeoutMs: 8000,
    desc: `单选组 [data-ob=${group}]`
  })
  return client.run(`
    const el = [...document.querySelectorAll('[data-ob=${group}] label')]
      .find((n) => (n.textContent || '').includes(${JSON.stringify(text)}))
    if (!el) throw new Error('找不到单选: ${group} / ' + ${JSON.stringify(text)})
    el.click()
    await new Promise((r) => setTimeout(r, 300))
    return true
  `)
}

/** 点一个 data-ob 控件：先等它出现（步骤切换有 out-in 过渡，旧步骤还在退场），再点并等一拍 */
export async function click(client, ob, waitMs = 400) {
  await client.waitFor(`document.querySelector('[data-ob=${ob}]')`, {
    timeoutMs: 8000,
    desc: `控件 [data-ob=${ob}]`
  })
  return client.run(`
    const el = document.querySelector('[data-ob=${ob}]')
    el.click()
    await new Promise((r) => setTimeout(r, ${waitMs}))
    return true
  `)
}

/**
 * 点一个「会换壳」的控件（完成 / 跳过 / 直接进入）：点击后主进程会销毁本窗口，
 * 别等返回值（等就可能被销毁连带 reject），点完就走，改由 attachReady 等新窗口。
 */
export function clickFire(client, ob) {
  client
    .run(`document.querySelector('[data-ob=${ob}]')?.click(); return true`, 3000)
    .catch(() => {})
}

/* ---------------- 断言收集 ---------------- */

export function makeChecker() {
  const rows = []
  const check = (name, ok, detail = '') => {
    rows.push({ name, ok: !!ok, detail: detail === undefined ? '' : String(detail) })
    console.log(`${ok ? '  ✓' : '  ✗'} ${name}${detail ? `  — ${detail}` : ''}`)
    return !!ok
  }
  return { check, rows, failures: () => rows.filter((r) => !r.ok).length }
}

/** 每个用例独立的沙箱目录（含数据目录与失败截图目录） */
export function caseDirs(root, name) {
  const dir = join(root, name)
  mkdirSync(dir, { recursive: true })
  return { dataDir: join(dir, 'data'), shots: join(dir, 'shots') }
}
