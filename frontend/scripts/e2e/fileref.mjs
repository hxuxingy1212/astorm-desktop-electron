/**
 * 正文引用 e2e（Electron 真实跑一遍；`pnpm run test:e2e:fileref`）——文件引用 + 网页引用。
 *
 * 覆盖（走真实链路：真窗口 + 真输入 + 真 SSE 流式回复 + 真点击，只把模型换成桩）：
 *   ① 正文里的 `` `auth.py:47` `` / `` `auth.py (line 12)` `` / `[auth.py#L30](auth.py#L30)`
 *      渲染为文件引用 chip（文件名 + 行号）；
 *   ② 非文件的行内代码（`pnpm run dev` / `src/renderer/src`）保持普通行内代码，不误判；
 *   ③ 网页引用 `[1](url)` / `[[3]](url)` 渲染为编号角标；同一处多来源并排显示，分别打开各自来源；
 *   ④ 点文件 chip → 右侧打开 auth.py 页签，滚到该行并高亮（行号列高亮 + 代码行高亮带）；
 *   ⑤ 再点同一文件的另一行 → 复用同一页签（不重复开），定位切到新行。
 *   另断言系统提示里两种引用的书写约定真的随请求下发（桩 LLM 记录请求体）。
 *
 * 隔离实例：`ASTORM_DATA_DIR` 临时目录 + 临时**用户目录**（工作区基目录，见 seedLocalApp），
 * 桩 LLM 按 SSE 回一段带引用的正文；跑的是构建产物，渲染层改动需先 `pnpm -C frontend build`。
 * 退出码 = 失败断言数。
 */
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  caseDirs,
  launchApp,
  makeChecker,
  seedLocalApp,
  sendChat,
  startStubCloud,
  stopAll,
  waitWorkspaces
} from './lib.mjs'

const ROOT = join(tmpdir(), 'astorm-e2e', `fileref-${Date.now()}`)
const STUB_TOKEN = 'stub-token-fileref'
const { check, rows, failures } = makeChecker()

/** 会话工作区里放的真实文件（80 行，每行一个标记，便于断言「定位到哪一行」） */
const AUTH_LINES = Array.from({ length: 80 }, (_, i) => `# L${i + 1}`)
const AUTH_PY = `${AUTH_LINES.join('\n')}\n`

/**
 * 桩模型这一轮的正文（来源地址用桩服务端自己的页面，点击时不必联网）：
 * 三种文件引用形态 + 两种**不该**被当成引用的行内代码 + 三种网页引用角标形态。
 */
function buildReply(stubUrl) {
  return [
    '## 引用核对',
    '',
    '登录入口在 `auth.py:47`，校验分支在 `auth.py (line 12)`。',
    '',
    '签名流程见[1](' + stubUrl + '/doc/sign)，打包见 [2](' + stubUrl + '/doc/pkg)。',
    '',
    '重复引用同一来源仍写 [1](' + stubUrl + '/doc/sign)，带方括号的写法也算 [[3]](' + stubUrl + '/doc/auto)。',
    '',
    '综合结论[1](' + stubUrl + '/doc/sign)[2](' + stubUrl + '/doc/pkg)[3](' + stubUrl + '/doc/auto)。',
    '',
    '另见 [登录逻辑](auth.py#L30)。下面这些不是文件：`pnpm run dev`、`src/renderer/src`。'
  ].join('\n')
}

/** 聊天区就绪：助手正文渲染出来（含引用的那一段） */
const REPLY_READY = `document.querySelector('.fr-chip')`

async function main() {
  const dirs = caseDirs(ROOT, 'fileref')
  mkdirSync(dirs.shots, { recursive: true })
  const stub = await startStubCloud({ token: STUB_TOKEN, turnReply: buildReply })
  const { userDir } = seedLocalApp({
    dataDir: dirs.dataDir,
    baseUrl: stub.llmBaseUrl,
    apiKey: STUB_TOKEN
  })

  // fresh 必须为 false：数据目录是本轮唯一的临时目录，但要保留上面刚落的种子配置
  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9617, fresh: false })
  const c = await app.attachReady({
    ready: `document.querySelector(".mention-editor[contenteditable='true']")`,
    timeoutMs: 60000
  })

  check('① 直接进应用外壳（本机形态），落在新建对话页', await c.run(`return location.hash === '#/newChat'`), await c.run(`return location.hash`))

  // ---- 发一条消息：真会话 + 真一轮（桩 LLM 流式回带引用的正文）----
  await sendChat(c, '帮我核对文件引用')
  const routed = await c
    .waitFor(`location.hash.startsWith('#/chat/')`, { timeoutMs: 30000, desc: '会话路由' })
    .catch(() => false)
  check(
    '① 会话已创建（新对话页切到会话页）',
    routed,
    await c.run(`return location.hash + ' | 输入框: ' + (document.querySelector('.mention-editor')?.textContent ?? '(无)')`)
  )

  // 会话工作区已落盘（create_session 同步建目录）→ 往里放被引用的文件
  const workspaces = await waitWorkspaces(userDir)
  check('① 会话工作区已创建', workspaces.length > 0, workspaces.join(' | '))
  const ws = workspaces[0]
  if (ws) writeFileSync(join(ws, 'auth.py'), AUTH_PY)

  check(
    '② 正文渲染出文件引用 chip',
    await c.waitFor(REPLY_READY, { timeoutMs: 40000, desc: '引用 chip' }),
    await c.run(`return [...document.querySelectorAll('.fr-chip')].map((el) => el.textContent).join(' / ')`)
  )
  check(
    '② 这一轮真的走了 LLM 流式接口',
    stub.requests.some((r) => r.includes('/chat/completions')),
    stub.requests.filter((r) => r.includes('completions')).join(' | ')
  )
  // 提示词侧约定：系统提示里必须有「路径:行号」这条书写规范，模型才会稳定写出可点的引用
  const systemOfTurn = stub.bodies
    .flatMap((b) => (Array.isArray(b.messages) ? b.messages : []))
    .filter((m) => m.role === 'system')
    .map((m) => String(m.content ?? ''))
    .find((t) => t.includes('角色定位'))
  check(
    '② 系统提示含两种引用的书写约定（随请求下发）',
    !!systemOfTurn?.includes('路径:行号') &&
      !!systemOfTurn?.includes('编号角标 `[n](完整URL)`') &&
      !!systemOfTurn?.includes('同一处正文引用多个来源时，连续写多个独立角标'),
    systemOfTurn ? `系统提示 ${systemOfTurn.length} 字符` : '没抓到系统提示'
  )
  // 流式回复分段落到达：三处引用（末段的链接形态最后到）都渲染完再断言
  const chipsReady = await c
    .waitFor(`document.querySelectorAll('.fr-chip').length === 3`, {
      timeoutMs: 30000,
      desc: '三处引用 chip'
    })
    .catch(() => false)

  const chips = await c.run(`
    return [...document.querySelectorAll('.fr-chip')].map((el) => ({
      text: el.textContent,
      title: el.getAttribute('title'),
      hasIcon: !!el.querySelector("img")
    }))
  `)
  check(
    '② 三种引用形态共 3 个 chip（行内代码带行号 / 带 line 文案 / 相对路径链接）',
    chipsReady,
    JSON.stringify(chips.map((x) => x.text))
  )
  check(
    '② chip 带文件图标',
    chips.every((x) => x.hasIcon)
  )
  check(
    '② chip 文案 = 文件名 + 行号',
    chips[0]?.text === 'auth.py(第 47 行)' && chips[1]?.text === 'auth.py(第 12 行)',
    chips.map((x) => x.text).join(' / ')
  )
  check(
    '② chip 悬浮提示给绝对路径',
    (chips[0]?.title ?? '').endsWith('/auth.py'),
    chips[0]?.title
  )

  // 末段（两个负例所在段）最后流到，且**流式期间行内代码会先以半截文本出现**
  // （如 `src/renderer/`）——等到两段文本都完整再断言，别拿半截值当结果
  const codeSel = `[...document.querySelectorAll('.markdown-body code[data-stream-markdown="inline-code"]')]`
  const plainOk = await c
    .waitFor(
      `(() => {
        const t = ${codeSel}.map((el) => el.textContent)
        return t.length === 2 && t[0] === 'pnpm run dev' && t[1] === 'src/renderer/src'
      })()`,
      { timeoutMs: 20000, desc: '非引用行内代码（完整）' }
    )
    .catch(() => false)
  const plain = await c.run(`return ${codeSel}.map((el) => el.textContent)`)
  check('② 非文件行内代码保持普通行内代码（不误判为引用）', plainOk, JSON.stringify(plain))

  // ---- ⑥ 标题：首轮生成中英双标题，界面语言切换后即时换用另一语言 ----
  const titleSel = `[...document.querySelectorAll('.task-title')].map((el) => el.textContent.trim())`
  const zhTitleReady = await c
    .waitFor(`${titleSel}.includes('文件引用核对')`, {
      timeoutMs: 30000,
      desc: '侧栏中文标题'
    })
    .catch(() => false)
  check('⑥ 首轮生成中文标题（侧栏）', zhTitleReady, await c.run(`return ${titleSel}.join(' | ')`))

  // ---- ③ 网页引用角标：渲染 + 点击在应用内浏览器打开来源 ----
  // 正文共七处角标：[1] 出现三次复用同一来源；综合结论连续标注三个来源。
  const badgesReady = await c
    .waitFor(`document.querySelectorAll('.wr-badge').length === 7`, {
      timeoutMs: 30000,
      desc: '七处网页引用角标'
    })
    .catch(() => false)
  const badges = await c.run(`
    return [...document.querySelectorAll('.wr-badge')].map((el) => ({
      text: el.textContent,
      title: el.getAttribute('title'),
      href: el.getAttribute('href')
    }))
  `)
  check(
    '③ 网页引用渲染成编号角标（[1](url) / [[3]](url)，同一来源复用同一编号）',
    badgesReady && badges.map((b) => b.text).join(',') === '1,2,1,3,1,2,3',
    JSON.stringify(badges.map((b) => b.text))
  )
  check(
    '③ 同一来源复用编号和目标地址',
    badges.filter((b) => b.text === '1').length === 3 &&
      badges.filter((b) => b.text === '1').every((b) => b.href === `${stub.url}/doc/sign`)
  )
  check(
    '③ 角标悬浮提示给来源域名',
    (badges[0]?.title ?? '').includes('来源：127.0.0.1') &&
      (badges[0]?.title ?? '').includes('/doc/sign'),
    badges[0]?.title
  )
  check(
    '③ 同一处三个来源独立并排显示',
    await c.run(`
      const group = [...document.querySelectorAll('.markdown-body p')].find(el => el.textContent.startsWith('综合结论'))
      const refs = [...(group?.querySelectorAll('.wr-badge') ?? [])]
      return refs.length === 3 && refs.map(el => el.textContent).join(',') === '1,2,3'
        && refs.every(el => Math.abs(el.getBoundingClientRect().top - refs[0].getBoundingClientRect().top) < 1)
    `)
  )
  for (const [index, path] of ['/doc/sign', '/doc/pkg', '/doc/auto'].entries()) {
    await c.run(`
      const group = [...document.querySelectorAll('.markdown-body p')].find(el => el.textContent.startsWith('综合结论'))
      group.querySelectorAll('.wr-badge')[${index}].click()
      return true
    `)
    check(
      '③ 并排角标 ' + (index + 1) + ' 打开各自来源',
      await c.waitFor(
        `[...document.querySelectorAll('iframe.bw-frame')].some(el => el.src === ${JSON.stringify(`${stub.url}${path}`)} && el.getBoundingClientRect().width > 0)`,
        { timeoutMs: 15000, desc: '浏览器载入来源 ' + path }
      ).catch(() => false),
      await c.run(`return [...document.querySelectorAll('iframe.bw-frame')].filter(el => el.getBoundingClientRect().width > 0).map(el => el.getAttribute('src')).join(', ') || '(无浏览器面板)'`)
    )
  }

  // ---- ④ 点文件 chip → 文件页签 + 定位到行 ----
  await c.run(`
    [...document.querySelectorAll('.fr-chip')].find((el) => el.textContent.includes('47')).click()
    await new Promise((r) => setTimeout(r, 400))
    return true
  `)
  check(
    '④ 打开文件页签（标题 = 文件名）',
    await c.waitFor(
      `[...document.querySelectorAll('.tabbar-tab')].some((el) => el.textContent.includes('auth.py'))`,
      { timeoutMs: 15000, desc: 'auth.py 页签' }
    )
  )
  check(
    '④ 页签内容读到文件（含第 47 行标记）',
    await c.waitFor(
      `(() => {
        const code = document.querySelector('.fp-code')
        return !!code && code.textContent.includes('# L47')
      })()`,
      { timeoutMs: 15000, desc: '第 47 行内容' }
    )
  )
  check(
    '④ 行号列高亮第 47 行',
    (await c.run(`return document.querySelector('.cv-ln-on')?.textContent`)) === '47',
    await c.run(`return document.querySelector('.cv-ln-on')?.textContent`)
  )
  check(
    '④ 已滚到该行（高亮带出现，且该行落在可视区内）',
    await c.run(`
      const band = document.querySelector('.cv-reveal')
      const ln = document.querySelector('.cv-ln-on')
      if (!band || !ln) return false
      const box = ln.getBoundingClientRect()
      // 文件 80 行、面板放不下 —— 第 47 行在可视区内即证明「确实滚到了它」
      return parseFloat(band.style.top) > 0 && box.top >= 0 && box.top <= window.innerHeight - 40
    `),
    await c.run(`
      const ln = document.querySelector('.cv-ln-on')
      return JSON.stringify({ top: Math.round(ln.getBoundingClientRect().top), vh: window.innerHeight })
    `)
  )

  // ---- ⑤ 再点同一文件的另一行 → 复用页签，定位切换 ----
  await c.run(`
    [...document.querySelectorAll('.fr-chip')].find((el) => el.textContent.includes('12')).click()
    await new Promise((r) => setTimeout(r, 400))
    return true
  `)
  check(
    '⑤ 复用同一页签（没有开出第二个）',
    (await c.run(
      `return [...document.querySelectorAll('.tabbar-tab')].filter((el) => el.textContent.includes('auth.py')).length`
    )) === 1
  )
  check(
    '⑤ 定位切到第 12 行',
    await c
      .waitFor(`document.querySelector('.cv-ln-on')?.textContent === '12'`, {
        timeoutMs: 8000,
        desc: '第 12 行高亮'
      })
      .catch(() => false),
    await c.run(`return document.querySelector('.cv-ln-on')?.textContent`)
  )

  // 切换界面语言（持久化偏好 + 重载，等同设置页切换）→ 同一份会话记录应显示英文标题
  await c.run(`
    const st = JSON.parse(localStorage.getItem('astorm:settings') || '{}')
    st.locale = 'en-US'
    localStorage.setItem('astorm:settings', JSON.stringify(st))
    location.reload()
    return true
  `)
  const enTitleReady = await c
    .waitFor(`${titleSel}.includes('File Reference Check')`, {
      timeoutMs: 30000,
      desc: '侧栏英文标题'
    })
    .catch(() => false)
  check(
    '⑥ 切换界面语言后标题改用英文版本',
    enTitleReady,
    await c.run(`return ${titleSel}.join(' | ')`)
  )

  if (failures()) {
    const shot = join(dirs.shots, 'fileref.png')
    await c.shot(shot)
    console.log(`\n失败截图: ${shot}\napp 日志尾部:\n${app.tail(15)}`)
  }
}

main()
  .catch(async (err) => {
    console.error('用例异常:', err?.message ?? err)
    rows.push({ name: '用例执行', ok: false, detail: String(err?.message ?? err) })
  })
  .finally(async () => {
    await stopAll()
    // 失败时留着沙箱：截图与数据目录是排查依据（成功才清）
    if (!failures()) rmSync(ROOT, { recursive: true, force: true })
    else console.log(`\n沙箱保留在: ${ROOT}`)
    console.log(`\n${rows.length - failures()}/${rows.length} 断言通过`)
    process.exit(failures() ? 1 : 0)
  })
