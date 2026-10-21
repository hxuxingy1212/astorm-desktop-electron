/**
 * 会话目标（`/goal`）e2e（Electron 真实跑一遍；`pnpm run test:e2e:goal`）。
 *
 * 覆盖完整链路：
 *   ① `/goal <目标>` 下发后，目标芯片出现在对话框工具行（计划右侧、上下文用量左侧）；
 *   ② 一轮结束后判定器返回未完成 → `goal_continue` 注入续跑消息（用户气泡 + 新一轮回复）；
 *   ③ 下一次判定完成 → 后端清除目标，芯片自动消失；
 *   ④ 重新设定目标后，悬浮卡内「修改」立即改写目标（后端 session_info 回执）；
 *   ⑤ 卡内「删除」清除目标，芯片消失。
 *
 * 桩 LLM 分流：普通轮次回正文；目标判定按提示词里的目标文本返回 done / 下一步指令。
 * 跑的是构建产物，渲染层改动需先 `pnpm -C frontend build`。退出码 = 失败断言数。
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
  stopAll
} from './lib.mjs'

const ROOT = join(tmpdir(), 'astorm-e2e', `goal-${Date.now()}`)
const STUB_TOKEN = 'stub-token-goal'
const { check, rows, failures } = makeChecker()
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/** 目标判定：第一目标第一次未完成、第二次完成；第二目标一直未完成（等用例删除） */
let firstGoalJudgements = 0
function goalJudgeReply(prompt) {
  if (prompt.includes('第一目标')) {
    firstGoalJudgements += 1
    return firstGoalJudgements === 1
      ? JSON.stringify({ done: false, message: '继续执行第二步' })
      : JSON.stringify({ done: true, message: '' })
  }
  return JSON.stringify({ done: false, message: '继续推进第二阶段' })
}

/** 目标芯片：footer-icons 里带 data-goal 的按钮（内容 = 目标全文） */
const goalChip = (text) =>
  `[...document.querySelectorAll('[data-goal]')].find((el) => (el.getAttribute('data-goal') ?? '').includes(${JSON.stringify(text)}))`

async function main() {
  const dirs = caseDirs(ROOT, 'goal')
  mkdirSync(dirs.shots, { recursive: true })
  const stub = await startStubCloud({
    token: STUB_TOKEN,
    turnReply: '收到。',
    goalJudgeReply
  })
  const { userDir } = seedLocalApp({
    dataDir: dirs.dataDir,
    baseUrl: stub.llmBaseUrl,
    apiKey: STUB_TOKEN
  })
  // 第二目标期间判定器持续返回未完成：把迭代预算调高，避免用例操作卡片时被预算自动清除
  writeFileSync(
    join(dirs.dataDir, 'config', 'backend-settings.json'),
    JSON.stringify(
      {
        workspace: userDir,
        goal: { max_tokens: 10000000, max_iterations: 100 }
      },
      null,
      2
    )
  )

  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9621, fresh: false })
  const c = await app.attachReady({
    ready: `document.querySelector(".mention-editor[contenteditable='true']")`,
    timeoutMs: 60000
  })
  check(
    '① 直接进应用外壳，落在新建对话页',
    await c.run(`return location.hash === '#/newChat'`),
    await c.run(`return location.hash`)
  )

  // ---- 第一目标：设定 → 续跑 → 完成清除 ----
  await sendChat(c, '/goal 第一目标：验证完整链路')
  check(
    '① /goal 后目标芯片出现（对话框工具行）',
    await c
      .waitFor(`!!${goalChip('第一目标')}`, { timeoutMs: 30000, desc: '目标芯片' })
      .catch(() => false),
    await c.run(`return [...document.querySelectorAll('.foot-icon-btn')].map((el) => el.getAttribute('data-goal') || el.getAttribute('title')).join(' | ')`)
  )
  check(
    '① 目标芯片位于上下文用量左侧、计划右侧',
    await c.run(`
      const goal = ${goalChip('第一目标')}
      const usage = document.querySelector('.session-info-wrap')
      if (!goal || !usage) return false
      return goal.closest('.footer-icons') === usage.closest('.footer-icons') &&
        !!(goal.compareDocumentPosition(usage) & Node.DOCUMENT_POSITION_FOLLOWING)
    `)
  )
  check(
    '② 判定器未完成 → 注入续跑用户消息',
    await c
      .waitFor(`document.body.innerText.includes('继续执行第二步')`, {
        timeoutMs: 40000,
        desc: 'goal_continue 用户气泡'
      })
      .catch(() => false),
    await c.run(`return document.body.innerText.includes('继续执行第二步')`)
  )
  check(
    '② 判定请求真的发给了模型',
    stub.bodies.some((b) =>
      JSON.stringify(b.messages ?? '').includes('你是本会话「目标推进」的完成判定器')
    ),
    stub.bodies
      .map((b) => JSON.stringify(b.messages ?? '').slice(0, 60))
      .join(' || ')
  )
  check(
    '③ 判定完成后目标仍保留（图标进入已完成态）',
    await c
      .waitFor(
        `!!${goalChip('第一目标')} && !!document.querySelector('[data-goal] .goal-icon-completed')`,
        { timeoutMs: 40000, desc: '已完成目标图标' }
      )
      .catch(() => false),
    await c.run(`return !!${goalChip('第一目标')}`)
  )
  await c.run(`${goalChip('第一目标')}.click(); return true`)
  check(
    '③ 卡片显示“已完成”状态',
    await c
      .waitFor(`document.querySelector('.goal-state')?.textContent.includes('已完成')`, {
        timeoutMs: 10000,
        desc: '已完成状态'
      })
      .catch(() => false),
    await c.run(`return document.querySelector('.goal-state')?.textContent.trim()`)
  )
  await c.run(`${goalChip('第一目标')}.click(); return true`)

  // ---- 第二目标：卡内修改 / 删除 ----
  await sendChat(c, '/goal 第二目标：验证卡片操作')
  check(
    '④ 重新设定目标后芯片出现',
    await c
      .waitFor(`!!${goalChip('第二目标')}`, { timeoutMs: 30000, desc: '第二目标芯片' })
      .catch(() => false),
    await c.run(`return [...document.querySelectorAll('.foot-icon-btn')].map((el) => el.getAttribute('data-goal') || el.getAttribute('title')).join(' | ')`)
  )
  check(
    '④ 新目标替代已完成目标（一个会话只有一个 goal）',
    await c.run(`
      const els = [...document.querySelectorAll('[data-goal]')]
      return els.length === 1 &&
        (els[0].getAttribute('data-goal') ?? '').includes('第二目标')
    `),
    await c.run(`return [...document.querySelectorAll('[data-goal]')].map((el) => el.getAttribute('data-goal')).join(' | ')`)
  )
  await c.run(`location.reload(); return true`)
  check(
    '④ 重载后目标仍显示（已随会话记录落盘）',
    await c
      .waitFor(`!!${goalChip('第二目标')}`, { timeoutMs: 30000, desc: '重载后的目标芯片' })
      .catch(() => false),
    await c.run(`return document.body.innerText.slice(0, 160)`)
  )

  await c.run(`${goalChip('第二目标')}.click(); return true`)
  check(
    '④ 点击目标打开卡片（全文 + 操作）',
    await c
      .waitFor(`document.querySelector('.goal-card')`, { timeoutMs: 10000, desc: '目标卡片' })
      .catch(() => false),
    await c.run(`return document.querySelector('.goal-card')?.innerText ?? '(无卡片)'`)
  )
  check(
    '④ 卡片显示目标状态（进行中 / 已暂停）',
    await c.run(
      `return ['进行中', '已暂停'].includes(document.querySelector('.goal-state')?.textContent.trim() ?? '')`
    ),
    await c.run(`return document.querySelector('.goal-state')?.textContent.trim()`)
  )
  check(
    '④ 推进次数与操作按钮同一行，修改/删除右对齐',
    await c.run(`
      const card = document.querySelector('.goal-card')
      const meta = card?.querySelector('.goal-meta')
      const actions = card?.querySelector('.goal-actions')
      if (!card || !meta || !actions) return false
      const m = meta.getBoundingClientRect()
      const a = actions.getBoundingClientRect()
      const c = card.getBoundingClientRect()
      return Math.abs((m.top + m.bottom) / 2 - (a.top + a.bottom) / 2) <= 2 &&
        Math.abs(c.right - 14 - a.right) <= 2
    `)
  )
  await c.run(`
    const btn = [...document.querySelectorAll('.goal-card .goal-act')].find((el) => el.textContent.trim() === '修改')
    btn?.click()
    return true
  `)
  check(
    '④ 卡片进入内联编辑态',
    await c.waitFor(`document.querySelector('.goal-edit')`, { timeoutMs: 8000, desc: '目标编辑框' }).catch(() => false)
  )
  await c.run(`
    const el = document.querySelector('.goal-edit')
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set
    setter.call(el, '修改后的目标：继续验证')
    el.dispatchEvent(new Event('input', { bubbles: true }))
    return true
  `)
  await c.run(`
    const btn = [...document.querySelectorAll('.goal-card .goal-act')].find((el) => el.textContent.trim() === '保存')
    btn?.click()
    return true
  `)
  check(
    '④ 修改立即生效（卡片与芯片同步新目标）',
    await c
      .waitFor(
        `document.querySelector('.goal-text')?.textContent.includes('修改后的目标') && !!${goalChip('修改后的目标')}`,
        { timeoutMs: 15000, desc: '修改后的目标' }
      )
      .catch(() => false),
    await c.run(`return (document.querySelector('.goal-text')?.textContent ?? '') + ' | ' + [...document.querySelectorAll('.foot-icon-btn')].map((el) => el.getAttribute('data-goal') || el.getAttribute('title')).join(',')`)
  )

  await c.run(`
    const btn = [...document.querySelectorAll('.goal-card .goal-act')].find((el) => el.textContent.trim() === '删除')
    btn?.click()
    return true
  `)
  check(
    '⑤ 删除目标后芯片与卡片消失',
    await c
      .waitFor(`!${goalChip('修改后的目标')} && !document.querySelector('.goal-card')`, {
        timeoutMs: 15000,
        desc: '目标已删除'
      })
      .catch(() => false),
    await c.run(`return document.body.innerText.includes('修改后的目标')`)
  )

  // ---- ⑥ 命令菜单路径：选中 /goal chip 后**直接**输入目标（中间不打空格）----
  // 回归：chip 序列化曾紧贴成 `/goal目标`，导致命令识别失败、目标没下发。
  const editor = `document.querySelector('.mention-editor[contenteditable="true"]')`
  await c.run(`
    const el = ${editor}
    el.focus()
    el.innerHTML = ''
    el.dispatchEvent(new Event('input', { bubbles: true }))
    return true
  `)
  await c.send('Input.insertText', { text: '/goal' })
  check(
    '⑥ 输入 /goal 弹出命令菜单',
    await c.waitFor(`document.querySelector('.mention-popover')`, { timeoutMs: 8000, desc: '命令菜单' }).catch(() => false)
  )
  const enterKey = {
    key: 'Enter',
    code: 'Enter',
    windowsVirtualKeyCode: 13,
    nativeVirtualKeyCode: 13,
    text: '\r'
  }
  await c.send('Input.dispatchKeyEvent', { type: 'keyDown', ...enterKey })
  await c.send('Input.dispatchKeyEvent', { type: 'keyUp', ...enterKey })
  await sleep(300)
  await c.send('Input.insertText', { text: '第三目标：菜单直接输入' })
  await sleep(200)
  await c.send('Input.dispatchKeyEvent', { type: 'keyDown', ...enterKey })
  await c.send('Input.dispatchKeyEvent', { type: 'keyUp', ...enterKey })
  check(
    '⑥ 选中命令后直接输入目标也能识别（无需手动空格）',
    await c
      .waitFor(`!!${goalChip('第三目标')}`, { timeoutMs: 30000, desc: '菜单路径目标芯片' })
      .catch(() => false),
    await c.run(`return document.body.innerText.slice(0, 300)`)
  )

  await c.shot(join(dirs.shots, 'goal.png'))
  if (failures()) {
    console.log(`\n失败截图: ${join(dirs.shots, 'goal.png')}\napp 日志尾部:\n${app.tail(20)}`)
  }
}

main()
  .catch(async (err) => {
    console.error('用例异常:', err?.message ?? err)
    rows.push({ name: '用例执行', ok: false, detail: String(err?.message ?? err) })
  })
  .finally(async () => {
    await stopAll()
    if (!failures()) rmSync(ROOT, { recursive: true, force: true })
    else console.log(`\n沙箱保留在: ${ROOT}`)
    console.log(`\n${rows.length - failures()}/${rows.length} 断言通过`)
    process.exit(failures() ? 1 : 0)
  })
