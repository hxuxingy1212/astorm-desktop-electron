/**
 * 摘要替换标题 e2e（Electron 真实跑一遍；`pnpm run test:e2e:summarytitle`）。
 *
 * 覆盖：
 *   ① 首轮消息生成初始双语标题（侧栏显示中文版本）；
 *   ② 第三轮前触发**自动摘要压缩**（把后端摘要阈值调到极小，不依赖真实长上下文）——
 *      摘要完成后按新摘要重生成标题，侧栏标题被替换；
 *   ③ 重载应用后标题仍为摘要后的版本（说明已随会话记录落盘，不只是内存事件）。
 *
 * 桩 LLM 按提示词分流：标题调用回双语标题；摘要调用回结构化摘要 JSON；普通轮次回正文。
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

const ROOT = join(tmpdir(), 'astorm-e2e', `summary-title-${Date.now()}`)
const STUB_TOKEN = 'stub-token-summary-title'
const { check, rows, failures } = makeChecker()

/** 摘要桩：必填两字段 + 其余 null（与 SessionSummary 反序列化对齐） */
const SUMMARY_JSON = JSON.stringify({
  user_requirement: '测试摘要替换标题',
  current_task: '验证摘要后标题更新',
  pending_tasks: null,
  history_process: null,
  key_decisions: null,
  resource_files: null,
  user_infos: null
})

/** 标题桩：摘要调用的标题源以「用户需求：」开头；首轮标题源是用户原话 */
function titleReply(prompt) {
  return prompt.includes('用户需求：')
    ? JSON.stringify({ title_zh: '摘要后的标题', title_en: 'After Summary' })
    : JSON.stringify({ title_zh: '初始标题', title_en: 'Initial Title' })
}

/** 侧栏任务行标题（各分组共用 `.task-title`） */
const TITLE_SEL = `[...document.querySelectorAll('.task-title')].map((el) => el.textContent.trim())`

async function main() {
  const dirs = caseDirs(ROOT, 'summary-title')
  mkdirSync(dirs.shots, { recursive: true })
  const stub = await startStubCloud({
    token: STUB_TOKEN,
    turnReply: '收到。',
    titleReply,
    summaryReply: SUMMARY_JSON
  })
  const { userDir } = seedLocalApp({
    dataDir: dirs.dataDir,
    baseUrl: stub.llmBaseUrl,
    apiKey: STUB_TOKEN
  })
  // 后端配置在 start 时读取：把摘要阈值调到「三轮内必触发」，其余保持默认
  writeFileSync(
    join(dirs.dataDir, 'config', 'backend-settings.json'),
    JSON.stringify(
      {
        workspace: userDir,
        summary: {
          token_threshold: 1,
          min_messages: 4,
          keep_token_threshold: 1,
          keep_tail_rounds: 1
        }
      },
      null,
      2
    )
  )

  const app = await launchApp({ dataDir: dirs.dataDir, cdpPort: 9618, fresh: false })
  const c = await app.attachReady({
    ready: `document.querySelector(".mention-editor[contenteditable='true']")`,
    timeoutMs: 60000
  })

  check(
    '① 直接进应用外壳，落在新建对话页',
    await c.run(`return location.hash === '#/newChat'`),
    await c.run(`return location.hash`)
  )

  // ---- 第一轮：创建会话 + 生成初始标题 ----
  await sendChat(c, '第一轮：开始任务')
  check(
    '① 会话已创建',
    await c
      .waitFor(`location.hash.startsWith('#/chat/')`, { timeoutMs: 30000, desc: '会话路由' })
      .catch(() => false),
    await c.run(`return location.hash`)
  )
  check(
    '① 首轮生成初始标题（侧栏）',
    await c
      .waitFor(`${TITLE_SEL}.includes('初始标题')`, { timeoutMs: 30000, desc: '初始标题' })
      .catch(() => false),
    await c.run(`return ${TITLE_SEL}.join(' | ')`)
  )

  // ---- 第二 / 第三轮：历史达到阈值，第三轮 loop_start 触发自动摘要并替换标题 ----
  await c.waitFor(`document.querySelectorAll('.markdown-body').length >= 1`, {
    timeoutMs: 30000,
    desc: '第一轮正文'
  })
  await sendChat(c, '第二轮：继续')
  await c.waitFor(`document.querySelectorAll('.markdown-body').length >= 2`, {
    timeoutMs: 30000,
    desc: '第二轮正文'
  })
  await sendChat(c, '第三轮：再继续')

  check(
    '② 摘要压缩后会话标题被替换（侧栏）',
    await c
      .waitFor(`${TITLE_SEL}.includes('摘要后的标题')`, {
        timeoutMs: 40000,
        desc: '摘要后的标题'
      })
      .catch(() => false),
    await c.run(`return ${TITLE_SEL}.join(' | ')`)
  )
  check(
    '② 真的走了结构化摘要请求',
    stub.bodies.some((b) => JSON.stringify(b.messages ?? '').includes('user_requirement（文本，必填）')),
    stub.bodies
      .map((b) => JSON.stringify(b.messages ?? '').slice(0, 80))
      .join(' || ')
  )

  // ---- 重载：标题从落盘记录恢复（验证上行 upsert 而非仅广播）----
  await c.run(`location.reload(); return true`)
  check(
    '③ 重载后仍显示摘要后的标题（已落盘）',
    await c
      .waitFor(`${TITLE_SEL}.includes('摘要后的标题')`, {
        timeoutMs: 30000,
        desc: '重载后的标题'
      })
      .catch(() => false),
    await c.run(`return ${TITLE_SEL}.join(' | ')`)
  )

  if (failures()) {
    const shot = join(dirs.shots, 'summary-title.png')
    await c.shot(shot)
    console.log(`\n失败截图: ${shot}\napp 日志尾部:\n${app.tail(20)}`)
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
