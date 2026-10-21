/**
 * 启动引导 e2e（Electron 真实跑一遍；`pnpm run test:e2e:onboarding`）。
 *
 * 覆盖：
 *   ① 首次启动 → 欢迎 → 选择（本机）→ 配置模型（预设/真实探测/落盘）→ 偏好 → 完成 → 进应用外壳
 *   ② 重启：连接方式已落盘 → 直接进应用，不再出引导
 *   ③ 菜单口径「连接方式…」→ 已配置过落在选择步 →「返回」回应用
 *   ④ 云端：填端点（裸 host:port，验归一化）+ 令牌 → 测连接 → 跳过偏好（云端不出现模型步）→ 加载远端 UI + 令牌注入
 *   ⑤ 负例：令牌错误 → 报错且「继续」不放行
 *
 * 全部在隔离实例上跑（ASTORM_DATA_DIR 指向临时目录），不碰用户数据、不依赖真实云容器
 * （云端分支用本地桩服务端）。退出码 = 失败断言数。
 */
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  caseDirs,
  click,
  clickFire,
  clickRadio,
  fill,
  launchApp,
  makeChecker,
  readConnection,
  readProviders,
  startStubCloud,
  stopAll
} from './lib.mjs'

const ROOT = join(tmpdir(), 'astorm-e2e', `run-${Date.now()}`)
const STUB_TOKEN = 'stub-token-1234'
const { check, rows, failures } = makeChecker()

/** 用例上下文：起过的实例挂在这里，异常时由 runner 统一收尾 */
const ctx = { app: null, shots: '' }

/** 引导页已挂载（每个用例起手都先等它，CDP target 出现 ≠ 页面挂载完） */
const OB_READY = `document.querySelector('.ob-card')`

/** 前置 fixture：直接落一份「已选本机」的连接配置（用例之间彼此独立，不靠上一条的结果） */
function seedLocalConfig(dataDir) {
  mkdirSync(join(dataDir, 'electron'), { recursive: true })
  writeFileSync(
    join(dataDir, 'electron', 'connection.json'),
    JSON.stringify(
      { schemaVersion: 1, mode: 'local', endpoint: '', token: null, storage: 'safeStorage' },
      null,
      2
    )
  )
}

/* ---------------- ① 首次启动：本机分支走完四步（含「配置模型」） ---------------- */
async function caseLocalFresh(dirs) {
  const stub = await startStubCloud({ token: STUB_TOKEN })
  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9611, fresh: true })
  ctx.app = app
  const c = await app.attachReady({ ready: OB_READY, timeoutMs: 60000 })

  check('① 首次启动落在欢迎步', await c.run(`return !!document.querySelector('[data-ob=start]')`))
  check('① 欢迎步标题', (await c.text('.ob-title')) === '欢迎使用 Astorm', await c.text('.ob-title'))
  check(
    '① 步骤点：第 1 步激活',
    (await c.run(
      `return [...document.querySelectorAll('.ob-dot')].findIndex((d) => d.classList.contains('active'))`
    )) === 0
  )
  check(
    '① 欢迎页没有「已有配置，直接进入」',
    (await c.run(`return !document.querySelector('[data-ob=enter]')`)) === true
  )

  await click(c, 'start', 800)
  check('① 进入选择步', await c.waitFor(`document.querySelector('[data-ob=card-local]')`, {
    desc: '选择步（本机卡）'
  }))
  check(
    '① 默认选中本机',
    await c.run(`return !!document.querySelector('[data-ob=card-local].active')`)
  )

  await click(c, 'card-local')
  await click(c, 'next', 900)

  // ---- 模型步（仅本机分支）----
  check(
    '① 本机分支进入「配置模型」步',
    await c.waitFor(`document.querySelector('[data-ob=model-base-url]')`, { desc: '模型步' })
  )
  check(
    '① 步骤点变 4 个（本机多一步）',
    (await c.run(`return document.querySelectorAll('.ob-dot').length`)) === 4
  )
  check(
    '① 预设胶囊填好地址/协议/模型',
    await c.run(`
      document.querySelector('[data-ob=preset-openai]').click()
      await new Promise((r) => setTimeout(r, 250))
      return document.querySelector('[data-ob=model-base-url] input').value === 'https://api.openai.com/v1'
        && document.querySelector('[data-ob=model-name] input').value === 'gpt-4o-mini'
    `)
  )
  // 换成桩 LLM；「保存并继续」会自动跑一次文本+图片能力探测
  await fill(c, '[data-ob=model-base-url] input', stub.llmBaseUrl)
  await fill(c, '[data-ob=model-api-key] input', STUB_TOKEN)
  await fill(c, '[data-ob=model-name] input', 'stub-model')
  await click(c, 'model-next', 1500)
  check(
    '① 保存并继续自动探测通过并进入偏好步',
    await c.waitFor(`document.querySelector('[data-ob=theme-mode]')`, {
      timeoutMs: 25000,
      desc: '自动探测后进入偏好步'
    })
  )
  {
    const probe = stub.bodies.find((b) => JSON.stringify(b).includes('PINEAPPLE'))
    const serialized = JSON.stringify(probe ?? {})
    check(
      '① 探测请求同时包含文本指令与图片',
      !!probe && serialized.includes('data:image/png;base64'),
      serialized.slice(0, 300)
    )
  }
  check(
    '① 探测确实打到 /v1/chat/completions',
    stub.requests.some((r) => r.includes('/chat/completions')),
    stub.requests.filter((r) => r.includes('completions')).join(' | ')
  )

  // 回退链：偏好 → 模型（配置带出来）→ 选择（本机保持）→ 再往前
  await click(c, 'back', 300)
  check(
    '① 上一步回到模型步且带出已存配置',
    await c.waitFor(
      `document.querySelector('[data-ob=model-base-url] input')?.value === ${JSON.stringify(stub.llmBaseUrl)}`,
      { desc: '模型步（配置回填）' }
    )
  )
  await click(c, 'back', 300)
  check(
    '① 再上一步回到选择步且保持本机选中',
    await c.waitFor(
      `document.querySelector('[data-ob=card-local].active') && !document.querySelector('[data-ob=model-base-url]')`,
      { desc: '选择步（本机仍选中）' }
    )
  )
  await click(c, 'next', 900)
  await click(c, 'model-next', 1500) // 已回填，直接保存并继续（会再探一次）
  check(
    '① 再次进入偏好步',
    await c.waitFor(`document.querySelector('[data-ob=theme-mode]')`, { timeoutMs: 25000, desc: '偏好步' })
  )

  // 偏好：深色 + English + 保持电脑运行（主题色设置已移除）
  await clickRadio(c, 'theme-mode', '深色')
  check(
    '① 偏好步不再提供主题色设置',
    (await c.run(`return !document.querySelector('[data-ob=accent]')`)) === true
  )
  await clickRadio(c, 'locale', 'English')
  await click(c, 'keep-awake', 500)
  check(
    '① 明暗即时生效',
    (await c.run(`return document.documentElement.getAttribute('data-theme')`)) === 'dark',
    String(await c.run(`return document.documentElement.getAttribute('data-theme')`))
  )
  check(
    '① 语言即时生效（标题转英文）',
    (await c.text('.ob-title')) === 'Preferences',
    await c.text('.ob-title')
  )

  // 完成 → 进应用（欢迎页与本机同属「本机形态」，不换窗口，只是重载同一 target）
  clickFire(c, 'finish')
  const appClient = await app.attachReady({
    ready: `document.querySelector('.app-shell')`,
    timeoutMs: 60000
  })
  check(
    '① 完成 → 进入应用外壳',
    await appClient.waitFor(`document.querySelector('.app-shell')`, { timeoutMs: 40000, desc: '应用外壳' })
  )
  check(
    '① 引导页已退场',
    (await appClient.run(`return !!document.querySelector('.ob-card')`)) === false
  )
  const conn = readConnection(dirs.dataDir)
  check(
    '① 连接方式落盘 = local 且记录引导完成标记',
    conn?.mode === 'local' && conn?.onboarded === true,
    JSON.stringify(conn)
  )
  const st = JSON.parse(
    (await appClient.run(`return localStorage.getItem('astorm:settings')`)) || '{}'
  )
  check(
    '① 偏好已持久化（明暗/语言/电源）',
    st.themeMode === 'dark' && st.locale === 'en-US' && st.keepAwake === true,
    JSON.stringify({
      themeMode: st.themeMode,
      locale: st.locale,
      keepAwake: st.keepAwake
    })
  )
  const prov = readProviders(dirs.dataDir)
  const p0 = prov?.providers?.[0]
  check(
    '① 供应商已落盘（地址/协议/模型，密钥走 safeStorage 非明文）',
    p0?.baseUrl === stub.llmBaseUrl &&
      p0?.apiFormat === 'chat_completions' &&
      p0?.models?.[0]?.name === 'stub-model' &&
      !!p0?.apiKey &&
      p0.apiKey !== STUB_TOKEN &&
      p0.storage === 'safeStorage',
    JSON.stringify({ baseUrl: p0?.baseUrl, model: p0?.models?.[0]?.name, storage: p0?.storage })
  )
  await app.stop()
  await stub.close()
}

/* ---------------- ② 重启：直接进应用 ---------------- */
async function caseRestart(dirs) {
  seedLocalConfig(dirs.dataDir)
  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9612 })
  ctx.app = app
  const c = await app.attachReady({
    ready: `document.querySelector('.app-shell')`,
    timeoutMs: 60000
  })
  check('② 直接进应用外壳', await c.run(`return !!document.querySelector('.app-shell')`))
  check('② 不再出引导页', (await c.run(`return !!document.querySelector('.ob-card')`)) === false)
  check(
    '② 旧配置自动补写 onboarded 标记',
    readConnection(dirs.dataDir)?.onboarded === true,
    JSON.stringify(readConnection(dirs.dataDir))
  )

  // ③ 回欢迎页（菜单「连接方式…」走的是同一个 IPC）；同窗口重载，旧 CDP 连接会被掐断。
  //    已配置过时**直接落在选择步**，左上角「返回」回当前连接形态。
  await c.run(`window.api.shellConnection.openWelcome(); return true`).catch(() => {})
  const w = await app.attachReady({
    ready: `document.querySelector('[data-ob=card-local]')`,
    timeoutMs: 45000
  })
  check(
    '③ 切换连接方式才打开引导页：已配置过直接落在选择步',
    await w.run(
      `return !!document.querySelector('[data-ob=card-local]') && !document.querySelector('[data-ob=enter]') && !!document.querySelector('[data-ob=exit]')`
    )
  )
  clickFire(w, 'exit')
  const back = await app.attachReady({
    ready: `document.querySelector('.app-shell')`,
    timeoutMs: 60000
  })
  check(
    '③ 「返回」回到应用外壳',
    await back.run(`return !!document.querySelector('.app-shell')`)
  )
  await app.stop()
}

/* ---------------- ④ 云端分支 ---------------- */
async function caseCloud(dirs) {
  const stub = await startStubCloud({ token: STUB_TOKEN })
  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9613, fresh: true })
  ctx.app = app
  const c = await app.attachReady({ ready: OB_READY, timeoutMs: 60000 })

  await click(c, 'start', 800)
  await click(c, 'card-cloud')
  check(
    '④ 选云容器后展开表单',
    await c.waitFor(`document.querySelector('[data-ob=cloud-form]')`, { desc: '云端表单' })
  )
  // 端点填**裸 host:port**（顺带验归一化），令牌填对
  await fill(c, '[data-ob=endpoint] input', `127.0.0.1:${stub.port}`)
  await fill(c, '[data-ob=token] input', STUB_TOKEN)
  await click(c, 'test', 1200)
  const line = await c.waitFor(`document.querySelector('.ob-test-text')`, { desc: '测试连接结果' }).then(
    () => c.text('.ob-test-text')
  )
  check(
    '④ 测连接成功（模型 / 数据目录来自服务端）',
    /已连接|Connected/.test(line) && line.includes('stub-model') && line.includes('/stub/data'),
    line
  )
  check(
    '④ 探测命中桩服务端 /api/v1/status',
    stub.requests.some((r) => r.includes('/api/v1/status')),
    stub.requests.join(' | ')
  )

  await click(c, 'next', 900)
  check(
    '④ 通过后进入偏好步（云端**不出现**「配置模型」步）',
    await c.waitFor(
      `document.querySelector('[data-ob=theme-mode]') && !document.querySelector('[data-ob=model-base-url]')`,
      { desc: '偏好步（无模型步）' }
    )
  )
  check(
    '④ 步骤点 3 个（云端不插模型步）',
    (await c.run(`return document.querySelectorAll('.ob-dot').length`)) === 3
  )

  const before = c.targetId
  clickFire(c, 'skip') // 跳过偏好 = 直接完成；随后换壳加载远端
  const remote = await app.attachReady({
    notId: before,
    ready: `document.getElementById('marker')`,
    timeoutMs: 60000
  })
  check('④ 壳子加载远端 UI（桩首页标记）', true)
  check(
    '④ 令牌已注入远端 localStorage（免登录遮罩）',
    (await remote.run(`return document.getElementById('marker').dataset.token`)) === STUB_TOKEN
  )
  check(
    '④ 云端页不暴露 window.api（web 桥才不被顶掉）',
    (await remote.run(`return document.getElementById('marker').dataset.api`)) === 'undefined',
    String(await remote.run(`return document.getElementById('marker').dataset.api`))
  )
  const conn = readConnection(dirs.dataDir)
  check(
    '④ 连接方式落盘 = cloud 且端点已归一化',
    conn?.mode === 'cloud' && conn?.endpoint === `http://127.0.0.1:${stub.port}`,
    JSON.stringify({ mode: conn?.mode, endpoint: conn?.endpoint })
  )
  check('④ 令牌已存储', !!conn?.token, `storage=${conn?.storage}`)
  await app.stop()
  await stub.close()
}

/* ---------------- ⑤ 负例：令牌错误不放行 ---------------- */
async function caseWrongToken(dirs) {
  const stub = await startStubCloud({ token: STUB_TOKEN })
  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9614, fresh: true })
  ctx.app = app
  const c = await app.attachReady({ ready: OB_READY, timeoutMs: 60000 })

  await click(c, 'start', 800)
  await click(c, 'card-cloud')
  await fill(c, '[data-ob=endpoint] input', stub.url)
  await fill(c, '[data-ob=token] input', 'wrong-token')
  await click(c, 'test', 1200)
  await c.waitFor(`document.querySelector('.ob-test-line')`, { desc: '错误提示' })
  const line = await c.text('.ob-test-text')
  check('⑤ 令牌错误给出可读提示', /401/.test(line), line)
  check(
    '⑤ 错误态样式（非 ok）',
    (await c.run(`return !!document.querySelector('.ob-test-line:not(.ok)')`)) === true
  )

  await click(c, 'next', 1500) // 继续：必须先连通，不该放行
  check(
    '⑤ 「继续」不放行（仍在选择步）',
    await c.run(
      `return !!document.querySelector('[data-ob=card-cloud]') && !document.querySelector('[data-ob=theme-mode]')`
    )
  )
  check('⑤ 未落盘连接方式（保持首次启动态）', readConnection(dirs.dataDir) === null)
  await app.stop()
  await stub.close()
}

/* ---------------- runner ---------------- */

const CASES = [
  ['首次启动：欢迎 → 选择（本机）→ 配置模型 → 偏好 → 完成', caseLocalFresh],
  ['重启直接进应用 + 切换连接方式打开引导页', caseRestart],
  ['云端分支：测连接 → 跳过偏好 → 加载远端 UI', caseCloud],
  ['负例：令牌错误不放行', caseWrongToken]
]

console.log(`\n引导 e2e（沙箱 ${ROOT}）\n`)
let failed = 0
for (const [name, fn] of CASES) {
  const dirs = caseDirs(ROOT, name.replace(/[^\w\u4e00-\u9fa5]+/g, '-').slice(0, 24))
  ctx.app = null
  ctx.shots = dirs.shots
  console.log(`\n▶ ${name}`)
  const before = failures()
  try {
    await Promise.race([
      fn(dirs),
      new Promise((_, rej) => setTimeout(() => rej(new Error('用例超时（150s）')), 150000))
    ])
  } catch (err) {
    check(`${name}（用例异常）`, false, err?.message ?? String(err))
    try {
      const shot = join(dirs.shots, 'fail.png')
      await ctx.app?.client?.shot(shot)
      console.log(`  ↳ 现场截图 ${shot}`)
      console.log('  ↳ app 日志尾部:\n' + (ctx.app?.tail(15) ?? '(无)').replace(/^/gm, '    '))
    } catch {
      /* 截图失败就算了 */
    }
  } finally {
    await ctx.app?.stop()
  }
  const delta = failures() - before
  failed += delta
  console.log(delta === 0 ? '  ── 通过' : `  ── 失败 ${delta} 条`)
}

await stopAll()
if (failed === 0) rmSync(ROOT, { recursive: true, force: true })
else console.log(`\n沙箱保留（含失败截图）: ${ROOT}`)
console.log(
  `\n${failed === 0 ? '全部通过' : `失败 ${failed} 条`}（断言 ${rows.length} 条，失败 ${failed}）\n`
)
process.exit(failed === 0 ? 0 : 1)
