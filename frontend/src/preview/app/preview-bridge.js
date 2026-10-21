/**
 * GitHub Pages 预览用的 mock 桥。
 *
 * 渲染层与桌面端共用一份代码，它只认 `window.api`（Electron preload 暴露的接口）。
 * 这里在浏览器里注入一份形状相同的实现：会话/历史来自下方种子数据，发送消息后
 * 通过 `chat.onEvent` 回放流式事件（reasoning → tool_call → tool_result → text → done），
 * 因此页面上的组件、流式动画、工具收纳块、文件引用页签都是真实实现，不是静态截图。
 *
 * 所有数据只存在内存里，刷新即恢复种子；不会请求任何服务器。
 */
;(function () {
  'use strict'
  if (window.__ASTORM_PREVIEW_BRIDGE__) return
  window.__ASTORM_PREVIEW_BRIDGE__ = true

  var nowMs = Date.now()
  var nowSec = Math.floor(nowMs / 1000)
  var MIN = 60
  var HOUR = 60 * MIN
  var DAY = 24 * HOUR

  /* ------------------------------------------------------------------ *
   * 种子数据
   * ------------------------------------------------------------------ */

  var sessions = [
    {
      id: 'preview-auth',
      title: '重构鉴权中间件并补齐测试',
      titleZh: '重构鉴权中间件并补齐测试',
      titleEn: 'Refactor auth middleware and add tests',
      workspace: '/Users/you/workspace/astorm',
      model: 'DeepSeek-V3',
      sessionType: 'normal',
      createdAt: nowSec - 2 * HOUR,
      updatedAt: nowSec - 2 * MIN,
      status: 'done'
    },
    {
      id: 'preview-research',
      title: '调研 Codex 的交互与工具设计',
      titleZh: '调研 Codex 的交互与工具设计',
      titleEn: 'Research Codex interaction and tool design',
      workspace: '',
      model: 'DeepSeek-V3',
      sessionType: 'normal',
      createdAt: nowSec - 6 * HOUR,
      updatedAt: nowSec - 26 * MIN,
      status: 'done'
    },
    {
      id: 'preview-mail',
      title: '整理今天的邮件待办',
      titleZh: '整理今天的邮件待办',
      titleEn: 'Triage today mail',
      workspace: '',
      model: 'DeepSeek-V3',
      sessionType: 'normal',
      createdAt: nowSec - 1 * DAY,
      updatedAt: nowSec - 2 * HOUR,
      status: 'done'
    },
    {
      id: 'preview-release',
      title: '准备 v0.4.0 发版说明',
      titleZh: '准备 v0.4.0 发版说明',
      titleEn: 'Prepare v0.4.0 release notes',
      workspace: '',
      model: 'DeepSeek-V3',
      sessionType: 'normal',
      createdAt: nowSec - 2 * DAY,
      updatedAt: nowSec - 5 * HOUR,
      status: 'done'
    },
    {
      id: 'preview-report',
      title: '整理 Q3 增长复盘报告',
      titleZh: '整理 Q3 增长复盘报告',
      titleEn: 'Q3 growth review',
      workspace: '/Users/you/workspace/growth',
      model: 'DeepSeek-V3',
      sessionType: 'project',
      createdAt: nowSec - 1 * DAY,
      updatedAt: nowSec - 38 * MIN,
      status: 'done'
    },
    {
      id: 'preview-landing',
      title: '生成官网新版落地页',
      titleZh: '生成官网新版落地页',
      titleEn: 'New marketing landing page',
      workspace: '/Users/you/workspace/website',
      model: 'DeepSeek-V3',
      sessionType: 'project',
      createdAt: nowSec - 2 * DAY,
      updatedAt: nowSec - 3 * HOUR,
      status: 'done'
    },
    {
      id: 'preview-win',
      title: '排查 Windows 安装包启动失败',
      titleZh: '排查 Windows 安装包启动失败',
      titleEn: 'Debug Windows installer startup',
      workspace: '/Users/you/workspace/astorm',
      model: 'DeepSeek-V3',
      sessionType: 'project',
      createdAt: nowSec - 2 * DAY,
      updatedAt: nowSec - 6 * HOUR,
      status: 'error'
    }
  ]

  /**
   * 历史消息（后端后端 jsonl 的等价形状）：
   * - assistant.toolCalls 折叠为过程步骤，role=tool 的结果按 toolCallId 挂回
   * - 每条 assistant 带 metadata.started_at / ended_at（毫秒），收纳块耗时用它
   * - metadata.artifacts 是当轮交付物/变更清单
   */
  function toolCall(id, name, params) {
    return { id: id, function: { name: name, arguments: JSON.stringify(params) } }
  }

  function toolResult(id, toolCallId, content, metadata) {
    return {
      id: id,
      role: 'tool',
      content: content,
      toolCallId: toolCallId,
      seq: 0,
      createdAt: nowSec,
      metadata: Object.assign({ status: 'ok' }, metadata || {})
    }
  }

  var authDiff = [
    'diff --git a/backend/crates/astorm-core/src/auth.rs b/backend/crates/astorm-core/src/auth.rs',
    '@@ -41,9 +41,17 @@ impl AuthService {',
    ' pub fn refresh(&self, token: &str) -> Result<TokenPair> {',
    '-    let claims = self.verify(token)?;',
    '-    Ok(self.issue(claims.sub))',
    '+    let claims = self.verify_with_leeway(token, 30)?;',
    '+    let rotated = self.rotate_refresh_token(&claims.jti)?;',
    '+    self.revoke(&claims.jti)?;',
    '+    Ok(TokenPair {',
    '+        access: self.issue_access(claims.sub.clone())?,',
    '+        refresh: rotated,',
    '+    })',
    ' }'
  ].join('\n')

  function authHistory() {
    var t0 = nowMs - 2 * HOUR
    return [
      {
        id: 'auth-u1',
        role: 'user',
        content: '帮我检查一下 auth 模块的 JWT 刷新逻辑，有问题就修复并补上测试。',
        seq: 1,
        createdAt: Math.floor(t0 / 1000)
      },
      {
        id: 'auth-a1',
        role: 'assistant',
        content: '',
        reasoningContent:
          '先定位鉴权入口，确认 refresh token 的轮换、过期容忍和重放保护，再决定改哪里。',
        toolCalls: [
          toolCall('auth-t1', 'file_operation', {
            action: 'read',
            path: 'backend/crates/astorm-core/src/auth.rs'
          }),
          toolCall('auth-t2', 'file_operation', {
            action: 'grep',
            pattern: 'refresh_token',
            path: 'backend/crates/astorm-core/src'
          })
        ],
        seq: 2,
        createdAt: Math.floor(t0 / 1000) + 6,
        metadata: { started_at: t0, ended_at: t0 + 18_000 }
      },
      toolResult('auth-r1', 'auth-t1', '已读取 backend/crates/astorm-core/src/auth.rs（214 行）', {
        read_stats: { lines: 214, chars: 8123 }
      }),
      toolResult('auth-r2', 'auth-t2', '找到 3 处 refresh_token 相关实现：auth.rs:47、auth.rs:118、session.rs:203'),
      {
        id: 'auth-a2',
        role: 'assistant',
        content: '',
        toolCalls: [
          toolCall('auth-t3', 'file_operation', {
            action: 'edit',
            path: 'backend/crates/astorm-core/src/auth.rs'
          }),
          toolCall('auth-t4', 'bash', {
            command: 'cargo test -p astorm-core auth -- --nocapture'
          })
        ],
        seq: 5,
        createdAt: Math.floor(t0 / 1000) + 24,
        metadata: { started_at: t0 + 18_000, ended_at: t0 + 47_000 }
      },
      toolResult('auth-r3', 'auth-t3', '已修改 1 个文件：+27 −9', {
        file_diff: { path: 'auth.rs', additions: 27, deletions: 9, patch: authDiff, newLines: 227 }
      }),
      toolResult('auth-r4', 'auth-t4', 'running 12 tests\ntest result: ok. 12 passed; 0 failed; 0 ignored', {
        sandbox: { protected: true, evicted: [], timed_out: false, output_truncated: false }
      }),
      {
        id: 'auth-a3',
        role: 'assistant',
        content: [
          '已经修复了 JWT 刷新逻辑。核心问题是旧 refresh token 在轮换后没有立即失效，攻击者可以在有效期内重复使用它。',
          '',
          '**改动点**',
          '',
          '- 刷新时强制轮换（rotation），并立刻吊销旧的 `jti`；',
          '- 校验增加 30 秒时钟偏移容忍，避免多实例部署下的误判；',
          '- 对并发刷新做幂等处理：同一 `jti` 只允许成功一次；',
          '- 补齐 4 个边界用例：过期、并发刷新、重放、无效签名。',
          '',
          '实现：`let rotated = self.rotate_refresh_token(&claims.jti)?;`，随后 `self.revoke(&claims.jti)?`。',
          '',
          '完整改动见 `backend/crates/astorm-core/src/auth.rs:47`，测试结果：**12 passed**。'
        ].join('\n'),
        seq: 8,
        createdAt: Math.floor(t0 / 1000) + 49,
        metadata: {
          started_at: t0 + 47_000,
          ended_at: t0 + 94_000,
          artifacts: {
            deliverables: [
              {
                path: '/Users/you/workspace/astorm/backend/crates/astorm-core/src/auth.rs',
                name: 'auth.rs',
                size: 8421
              }
            ],
            changes: [
              {
                path: 'backend/crates/astorm-core/src/auth.rs',
                name: 'auth.rs',
                dir: 'backend/crates/astorm-core/src/',
                status: 'M',
                additions: 27,
                deletions: 9
              }
            ]
          }
        }
      },
      {
        id: 'auth-u2',
        role: 'user',
        content: '顺便把 README 里的鉴权说明也更新一下，别写太长。',
        seq: 9,
        createdAt: Math.floor((t0 + 2 * MIN * 1000) / 1000)
      },
      {
        id: 'auth-a4',
        role: 'assistant',
        content: [
          '已更新 `README.md:86` 的鉴权章节，压缩成 5 行：',
          '',
          '- 说明 access token 的 15 分钟有效期与 refresh token 的轮换策略；',
          '- 补一句「旧 refresh token 在一次刷新后立即失效」；',
          '- 指向完整实现在 `auth.rs:47`。',
          '',
          '没有改动其它章节。'
        ].join('\n'),
        seq: 10,
        createdAt: Math.floor((t0 + 2 * MIN * 1000) / 1000) + 20,
        metadata: {
          started_at: t0 + 2 * MIN * 1000,
          ended_at: t0 + 2 * MIN * 1000 + 21_000,
          artifacts: {
            deliverables: [],
            changes: [
              {
                path: 'README.md',
                name: 'README.md',
                dir: '',
                status: 'M',
                additions: 5,
                deletions: 2
              }
            ]
          }
        }
      }
    ]
  }

  function researchHistory() {
    var t0 = nowMs - 6 * HOUR
    return [
      {
        id: 'research-u1',
        role: 'user',
        content: '调研一下 Codex 这类产品的交互设计，重点看工具调用和过程展示，整理成可落地的建议。',
        seq: 1,
        createdAt: Math.floor(t0 / 1000)
      },
      {
        id: 'research-a1',
        role: 'assistant',
        content: '',
        reasoningContent: '先抓官方页面和两篇体验文章，再归纳「过程收纳 / 审批 / 产物」三条线索。',
        toolCalls: [
          toolCall('research-t1', 'search', { action: 'webfetch', url: 'https://openai.com/codex/' }),
          toolCall('research-t2', 'search', { query: 'Codex agent UI tool call approval design' })
        ],
        seq: 2,
        createdAt: Math.floor(t0 / 1000) + 8,
        metadata: { started_at: t0, ended_at: t0 + 26_000 }
      },
      toolResult('research-r1', 'research-t1', '已抓取 Codex 产品页：强调「delegate tasks / review results / stay in flow」。', {
        web_results: [
          {
            title: 'OpenAI Codex',
            url: 'https://openai.com/codex/',
            snippet: 'Delegate coding tasks to Codex, review results, and keep your flow.'
          }
        ]
      }),
      toolResult('research-r2', 'research-t2', '找到 8 条结果，保留 3 条高相关。', {
        web_results: [
          {
            title: 'Inside Codex: how the agent shows its work',
            url: 'https://example.com/codex-ux',
            snippet: 'Collapsed process, inline diffs, and approval checkpoints keep long tasks legible.'
          },
          {
            title: 'Designing trust for coding agents',
            url: 'https://example.com/trust-agents',
            snippet: 'Show what will run before it runs; make every side effect reviewable.'
          }
        ]
      }),
      {
        id: 'research-a2',
        role: 'assistant',
        content: [
          '## 可落地的三条结论',
          '',
          '1. **过程默认收起，但一步可展开**：用户关心的是结果，只有在长任务或异常时才需要看细节。对应我们的「已处理 48s」收纳块。',
          '2. **副作用先审后跑**：写文件、跑命令前给出明确的命令与影响范围，批准按「一次 / 本会话」分档。',
          '3. **产物要能被带走**：每轮结束固定输出交付物与变更文件，而不是埋在对话里。',
          '',
          '如果要继续，我建议先从**审批卡的信息密度**入手：现在命令 + 目标路径已经够用，可以再补一个影响文件数。'
        ].join('\n'),
        seq: 5,
        createdAt: Math.floor(t0 / 1000) + 30,
        metadata: { started_at: t0 + 26_000, ended_at: t0 + 71_000 }
      }
    ]
  }

  function simpleHistory(userText, answer) {
    var t0 = nowMs - 2 * HOUR
    return [
      {
        id: 'simple-u1',
        role: 'user',
        content: userText,
        seq: 1,
        createdAt: Math.floor(t0 / 1000)
      },
      {
        id: 'simple-a1',
        role: 'assistant',
        content: answer,
        seq: 2,
        createdAt: Math.floor(t0 / 1000) + 12,
        metadata: { started_at: t0, ended_at: t0 + 18_000 }
      }
    ]
  }

  var histories = {
    'preview-auth': authHistory(),
    'preview-research': researchHistory(),
    'preview-mail': simpleHistory(
      '把今天的邮件按「需要回复 / 仅供参考 / 可以归档」分一下，回复的都给我拟个草稿。',
      [
        '今天共有 **14 封**新邮件，我分成三组：',
        '',
        '- **需要回复（3）**：供应商合同确认、招聘面试时间、客户续费问题。草稿已放进邮箱草稿箱。',
        '- **仅供参考（6）**：周报、监控告警汇总、两份 newsletter。',
        '- **可以归档（5）**：发票、系统通知。',
        '',
        '优先建议先处理客户续费那封，涉及本周五的到期时间。'
      ].join('\n')
    ),
    'preview-release': simpleHistory(
      '根据最近的提交帮我写一份 v0.4.0 的发版说明，分「新功能 / 修复 / 破坏性变更」。',
      [
        '## v0.4.0',
        '',
        '### 新功能',
        '- 交付物页支持按类型筛选与两级折叠；',
        '- 会话目标（`/goal`）支持修改与删除；',
        '- 内置浏览器页签支持跨域站点嵌入。',
        '',
        '### 修复',
        '- 修复 Windows 安装包首次启动时的路径解析问题；',
        '- 修复流式输出结束后状态行偶发不消失。',
        '',
        '### 破坏性变更',
        '- 安全配置迁移到 `config/security.json`，旧字段启动时自动迁移。'
      ].join('\n')
    ),
    'preview-report': simpleHistory(
      '把 Q3 的增长数据整理成复盘，重点看渠道 ROI 和留存，输出一份可以直接投屏的摘要。',
      [
        '## Q3 增长复盘',
        '',
        '**大盘**：新增注册 18.4k（环比 +22%），次月留存 41%（环比 +3pt）。',
        '',
        '| 渠道 | 花费 | 注册 | ROI |',
        '| --- | ---: | ---: | ---: |',
        '| 内容搜索 | ¥82k | 7,120 | 1.84 |',
        '| 社区合作 | ¥46k | 4,980 | 2.13 |',
        '| 信息流 | ¥120k | 6,300 | 0.96 |',
        '',
        '**结论**：信息流 ROI 已低于 1，建议下季度砍掉 40% 预算，转投社区合作与内容搜索。留存的主要提升来自新引导流程，而不是渠道结构变化。',
        '',
        '完整数据在 `reports/q3-growth.xlsx`，投屏版摘要已生成。'
      ].join('\n')
    ),
    'preview-landing': simpleHistory(
      '给 Astorm 做一个新版官网落地页，要能在 GitHub Pages 上预览，最好能直接操作应用界面。',
      [
        '已经生成新版落地页，并把真实的应用界面嵌进去了：',
        '',
        '- Hero 区用一句话讲清「桌面智能体」，主 CTA 直接滚到交互预览；',
        '- 预览窗口复用当前渲染层组件，带种子会话与 mock 流式回复；',
        '- 特性区拆成「真正执行 / 任意模型 / MCP / 浏览器 / Office / 云端」六块。',
        '',
        '入口在 `site/index.html`，应用预览在 `site/app/index.html`。'
      ].join('\n')
    ),
    'preview-win': simpleHistory(
      'Windows 用户反馈安装后启动直接退出，帮我看一下日志。',
      [
        '找到原因了：安装包把 `astorm-backend.node` 打进了 asar，`createRequire` 无法从压缩包内加载原生模块，启动时直接抛错。',
        '',
        '修复方式是在 `electron-builder.yml` 里把该文件加入 `asarUnpack`：',
        '',
        '```yaml',
        'asarUnpack:',
        '  - resources/astorm-backend.node',
        '```',
        '',
        '重新打包后已在 Windows 11 上验证启动正常。'
      ].join('\n')
    )
  }

  /* ------------------------------------------------------------------ *
   * 事件总线与流式回复
   * ------------------------------------------------------------------ */

  var chatSubs = new Set()
  var backendSubs = new Set()
  var timers = new Map()
  var runSeq = 0

  function emit(ev) {
    var run = function () {
      chatSubs.forEach(function (cb) {
        try {
          cb(ev)
        } catch (e) {
          console.error('[preview] event handler failed', e)
        }
      })
    }
    setTimeout(run, 0)
  }

  function emitBackend(ev) {
    backendSubs.forEach(function (cb) {
      try {
        cb(ev)
      } catch (e) {
        console.error('[preview] backend handler failed', e)
      }
    })
  }

  function later(sessionId, delay, fn) {
    var id = setTimeout(function () {
      var list = timers.get(sessionId)
      if (list) {
        var idx = list.indexOf(id)
        if (idx >= 0) list.splice(idx, 1)
      }
      fn()
    }, delay)
    if (!timers.has(sessionId)) timers.set(sessionId, [])
    timers.get(sessionId).push(id)
    return id
  }

  function cancelRun(sessionId) {
    var list = timers.get(sessionId)
    if (list) {
      list.forEach(function (id) {
        clearTimeout(id)
      })
      timers.delete(sessionId)
    }
  }

  function sessionOf(id) {
    for (var i = 0; i < sessions.length; i += 1) {
      if (sessions[i].id === id) return sessions[i]
    }
    return null
  }

  /**
   * 后端在每次落库后都会推一条 session_info。前端靠它恢复会话状态（侧栏绿点/红点）、
   * 会话设置（执行模式 / 供应商 / 模型）。没有这条事件，未打开的会话会被当成「待办」隐藏。
   */
  function sessionInfoEvent(s, status) {
    return {
      type: 'session_info',
      session_id: s.id,
      id: s.id,
      title: s.title,
      titleZh: s.titleZh,
      titleEn: s.titleEn,
      workspace: s.workspace,
      model: s.model,
      sessionType: s.sessionType,
      provider: 'DeepSeek',
      sessionMode: 'execute',
      permissionMode: 'ask',
      reasoningEffort: 'medium',
      status: status || s.status || 'done',
      createdAt: s.createdAt,
      updatedAt: s.updatedAt,
      questionTimes: 1,
      currentRound: 1
    }
  }

  function announceSession(sessionId, status) {
    var s = sessionOf(sessionId)
    if (s) emit(sessionInfoEvent(s, status))
  }

  function touchSession(id) {
    var s = sessionOf(id)
    if (s) s.updatedAt = Math.floor(Date.now() / 1000)
  }

  function answerFor(text) {
    var t = String(text || '')
    if (/页面|官网|落地页|landing|网站/i.test(t)) {
      return {
        reasoning: '先确认目标：要一个能直接部署的静态页面，并且应用界面可交互。我按「介绍页 + iframe 预览」来拆。',
        tool: {
          name: 'bash',
          params: { command: 'pnpm -C frontend build:preview' },
          result: 'vite v7.3.6 building for production...\n✓ built in 8.42s\npreview-dist/index.html  18.2 kB',
          diff: { path: 'frontend/src/preview/index.html', additions: 186, deletions: 0 }
        },
        text: [
          '已经按「介绍页 + 真实应用预览」的结构做完了：',
          '',
          '- 介绍页保留 Hero、能力卡片和架构说明，主 CTA 直接滚到可交互预览；',
          '- 预览窗口用 iframe 挂载同一份渲染层，路由、侧栏、输入框都是真实组件；',
          '- mock 数据放在 `preview-bridge.js`，刷新即恢复，不会污染真实数据。',
          '',
          '构建产物在 `frontend/preview-dist`，推送到 GitHub 后由 Actions 自动发布。'
        ].join('\n')
      }
    }
    if (/邮件|mail|收件箱/i.test(t)) {
      return {
        reasoning: '先拉今天的邮件列表，按「需要回复 / 仅供参考 / 归档」分类，再给需要回复的拟草稿。',
        tool: {
          name: 'search',
          params: { query: 'in:inbox newer_than:1d' },
          result: '共 14 封新邮件；需要回复 3 封，其余可归档或仅阅读。'
        },
        text: [
          '今天有 **14 封**新邮件，已经分好：',
          '',
          '- **需要回复（3）**：客户续费、供应商合同、面试时间；',
          '- **仅供参考（6）**：周报、告警汇总、newsletter；',
          '- **可归档（5）**：发票与系统通知。',
          '',
          '三封回复草稿已生成，优先处理客户续费那封——周五到期。'
        ].join('\n')
      }
    }
    if (/测试|bug|修复|报错|错误|失败/i.test(t)) {
      return {
        reasoning: '先复现问题、定位到具体文件，再做最小修改并跑一遍相关测试。',
        tool: {
          name: 'bash',
          params: { command: 'cargo test -p astorm-core -- --nocapture' },
          result: 'running 12 tests\ntest result: ok. 12 passed; 0 failed',
          diff: { path: 'backend/crates/astorm-core/src/auth.rs', additions: 27, deletions: 9 }
        },
        text: [
          '问题已定位并修复：',
          '',
          '- 根因是刷新 token 轮换后旧 token 没有立即失效；',
          '- 修改集中在 `backend/crates/astorm-core/src/auth.rs:47`；',
          '- 补了 4 个边界用例，**12 个测试全部通过**。',
          '',
          '变更统计：+27 −9，已写入当前轮次的交付物。'
        ].join('\n')
      }
    }
    if (/调研|研究|对比|分析|方案/i.test(t)) {
      return {
        reasoning: '把问题拆成「现状 / 可选方案 / 取舍」三段，先收集信息，再给出明确建议。',
        tool: {
          name: 'search',
          params: { query: 'local first desktop agent architecture' },
          result: '找到 6 条结果，保留 3 条与架构取舍直接相关的资料。'
        },
        text: [
          '我按三条线整理了一下：',
          '',
          '1. **现状**：桌面智能体的差异不在模型，而在工具执行与结果审查的闭环。',
          '2. **方案**：本地执行 + 云容器双形态，数据与凭据留在同一侧；工具通过统一 schema 接入。',
          '3. **取舍**：本地能力强、隐私好；云端跨设备、可长时间运行。两者共用一套内核最省维护成本。',
          '',
          '如果要做决定，我建议先统一工具协议，再决定执行宿主。'
        ].join('\n')
      }
    }
    return {
      reasoning: '先理解目标，再快速扫一遍工作区里相关的文件和约束，然后给出可以直接执行的结论。',
      tool: {
        name: 'bash',
        params: { command: 'rg -n "TODO|FIXME" -S . --glob "!node_modules"' },
        result: '扫描完成：找到 3 处相关标记，已按优先级排序。'
      },
      text: [
        '看完了，给你一个可以直接执行的结论：',
        '',
        '- 先处理影响面最大的那一项，避免同时改多个模块；',
        '- 改动落在一个文件里，方便 review 和回滚；',
        '- 完成后跑一遍相关测试，把结果附在本轮交付物里。',
        '',
        '如果你确认，我可以直接开始改。'
      ].join('\n')
    }
  }

  function chunkText(text) {
    var chunks = []
    var i = 0
    while (i < text.length) {
      var size = 3 + Math.floor(Math.random() * 5)
      chunks.push(text.slice(i, i + size))
      i += size
    }
    return chunks
  }

  function streamReply(sessionId, userText) {
    cancelRun(sessionId)
    var runId = ++runSeq
    var startedMs = Date.now()
    var answer = answerFor(userText)
    var round = 1
    var s = sessionOf(sessionId)
    if (!s) return

    touchSession(sessionId)
    emit({
      type: 'session_info',
      session_id: sessionId,
      id: sessionId,
      title: s.title,
      titleZh: s.titleZh,
      workspace: s.workspace,
      model: s.model,
      sessionType: s.sessionType,
      provider: 'DeepSeek',
      sessionMode: 'execute',
      permissionMode: 'ask',
      reasoningEffort: 'medium',
      status: 'running',
      createdAt: s.createdAt,
      updatedAt: s.updatedAt,
      questionTimes: 1,
      currentRound: 1
    })
    later(sessionId, 120, function () {
      var reasoning = answer.reasoning
      var first = reasoning.slice(0, Math.ceil(reasoning.length / 2))
      var rest = reasoning.slice(first.length)
      emit({ type: 'reasoning', session_id: sessionId, content: first, round: round })
      later(sessionId, 260, function () {
        emit({ type: 'reasoning', session_id: sessionId, content: rest, round: round })
        later(sessionId, 240, function () {
          var toolId = 'run-' + runId + '-tool'
          emit({
            type: 'tool_call',
            session_id: sessionId,
            round: round,
            tool_calls: [{ id: toolId, name: answer.tool.name, params: answer.tool.params }]
          })
          later(sessionId, 700, function () {
            emit({
              type: 'tool_result',
              session_id: sessionId,
              id: toolId,
              round: round,
              status: 'ok',
              result: answer.tool.result,
              duration_ms: 620,
              file_diff: answer.tool.diff || null
            })
            var chunks = chunkText(answer.text)
            var cursor = 0
            var pump = function () {
              if (cursor >= chunks.length) {
                later(sessionId, 240, function () {
                  emit({
                    type: 'session_stats',
                    session_id: sessionId,
                    prompt_tokens: 1842,
                    completion_tokens: 468,
                    total_tokens: 2310
                  })
                  emit({ type: 'done', session_id: sessionId })
                  var doneSession = sessionOf(sessionId)
                  if (doneSession) doneSession.status = 'done'
                  // 回填 mock 历史：刷新预览后这一轮仍然在（否则只剩用户消息）
                  var hist = histories[sessionId] || (histories[sessionId] = [])
                  hist.push({
                    id: sessionId + '-a-' + runId,
                    role: 'assistant',
                    content: answer.text,
                    reasoningContent: answer.reasoning,
                    seq: hist.length + 1,
                    createdAt: Math.floor(Date.now() / 1000),
                    metadata: { started_at: startedMs, ended_at: Date.now() }
                  })
                  announceSession(sessionId, 'done')
                })
                return
              }
              emit({
                type: 'text',
                session_id: sessionId,
                content: chunks[cursor],
                round: round
              })
              cursor += 1
              later(sessionId, 18, pump)
            }
            pump()
          })
        })
      })
    })
  }

  /* ------------------------------------------------------------------ *
   * 文件内容（点击正文里的文件引用时，打开的是真实文件预览页签）
   * ------------------------------------------------------------------ */

  var FILE_CONTENTS = {
    'auth.rs': [
      '// backend/crates/astorm-core/src/auth.rs',
      'use anyhow::{bail, Result};',
      'use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};',
      '',
      '#[derive(Debug, Clone)]',
      'pub struct TokenPair {',
      '    pub access: String,',
      '    pub refresh: String,',
      '}',
      '',
      'impl AuthService {',
      '    /// 刷新 access token，并强制轮换 refresh token。',
      '    pub fn refresh(&self, token: &str) -> Result<TokenPair> {',
      '        let claims = self.verify_with_leeway(token, 30)?;',
      '        let rotated = self.rotate_refresh_token(&claims.jti)?;',
      '        self.revoke(&claims.jti)?;',
      '        Ok(TokenPair {',
      '            access: self.issue_access(claims.sub.clone())?,',
      '            refresh: rotated,',
      '        })',
      '    }',
      '',
      '    fn verify_with_leeway(&self, token: &str, leeway: u64) -> Result<Claims> {',
      '        let mut validation = Validation::default();',
      '        validation.leeway = leeway;',
      '        let data = decode::<Claims>(token, &self.decoding_key, &validation)?;',
      '        Ok(data.claims)',
      '    }',
      '',
      '    fn rotate_refresh_token(&self, jti: &str) -> Result<String> {',
      '        if !self.revoked.insert(jti.to_string()) {',
      '            bail!("refresh token 已被使用")',
      '        }',
      '        let claims = Claims::refresh(self.next_jti());',
      '        Ok(encode(&Header::default(), &claims, &self.encoding_key)?)',
      '    }',
      '}'
    ].join('\n'),
    'README.md': [
      '# Astorm',
      '',
      'Electron + Rust 的桌面智能体。',
      '',
      '## 鉴权',
      '',
      'Access token 有效期 15 分钟；refresh token 每次刷新后立即轮换，旧 token 立刻失效。',
      '多实例部署时允许 30 秒时钟偏移。完整实现见 `backend/crates/astorm-core/src/auth.rs:47`。',
      '',
      '## 开发',
      '',
      '```bash',
      'pnpm run dev',
      '```',
      '',
      '后端为 Rust napi dll，首次启动会自动编译并同步到 `frontend/resources`。'
    ].join('\n'),
    'index.html': [
      '<!doctype html>',
      '<html lang="zh-CN">',
      '  <head>',
      '    <meta charset="UTF-8" />',
      '    <title>Astorm</title>',
      '  </head>',
      '  <body>',
      '    <main id="hero">',
      '      <h1>让 AI 真正坐到你的电脑前</h1>',
      '      <p>桌面智能体 · 本地优先 · 开源</p>',
      '    </main>',
      '  </body>',
      '</html>'
    ].join('\n')
  }

  function fileContent(path) {
    var name = String(path || '').split(/[\\/]/).pop() || ''
    if (FILE_CONTENTS[name]) return FILE_CONTENTS[name]
    return [
      '// 预览数据：' + path,
      '// 这里是 mock 桥返回的文件内容，用于演示「点击正文里的文件引用 → 打开文件页签」。',
      '',
      'export const preview = true'
    ].join('\n')
  }

  var MOCK_TREE = [
    {
      name: 'backend',
      type: 'folder',
      children: [
        {
          name: 'crates',
          type: 'folder',
          children: [
            {
              name: 'astorm-core',
              type: 'folder',
              children: [
                {
                  name: 'src',
                  type: 'folder',
                  children: [
                    { name: 'auth.rs', type: 'file' },
                    { name: 'session.rs', type: 'file' },
                    { name: 'lib.rs', type: 'file' }
                  ]
                }
              ]
            }
          ]
        }
      ]
    },
    {
      name: 'frontend',
      type: 'folder',
      children: [
        { name: 'src', type: 'folder', children: [{ name: 'renderer', type: 'folder', children: [] }] },
        { name: 'package.json', type: 'file' }
      ]
    },
    { name: 'README.md', type: 'file' },
    { name: 'package.json', type: 'file' }
  ]

  /* ------------------------------------------------------------------ *
   * window.api
   * ------------------------------------------------------------------ */

  var storageNull = function () {
    return null
  }

  var info = {
    mode: 'local',
    endpoint: '',
    hasToken: false,
    welcome: false,
    onboarded: true
  }

  /**
   * 嵌在介绍页里时按 macOS 桌面壳渲染：应用会在侧栏品牌行为红黄绿预留 76px，
   * 外层只需把三个圆点盖上去，就是原生窗口的单层头部（不再额外画一条浏览器式标题栏）。
   * 单独打开 /app/ 时退回浏览器形态（侧栏显示 logo + 名称）。
   */
  var embedded = false
  try {
    embedded = window.self !== window.top
  } catch (e) {
    embedded = true
  }

  var api = {
    __web: !embedded,
    platform: 'darwin',
    // 预览桥是纯浏览器 mock，没有 Electron 的 astorm-icon:// 图标缓存协议
    iconProtocol: false,
    versions: { electron: 'preview', chrome: String(navigator.userAgent), node: 'preview' },

    shellConnection: {
      info: info,
      test: function () {
        return Promise.resolve({ ok: true, model: 'DeepSeek-V3', dataDir: '/Users/you/astorm' })
      },
      save: function (patch) {
        return Promise.resolve(Object.assign({}, info, patch || {}))
      },
      openWelcome: function () {
        return Promise.resolve()
      },
      closeWelcome: function () {
        return Promise.resolve()
      }
    },

    shell: {
      openExternal: function () {
        return Promise.resolve('')
      }
    },

    system: {
      appDir: function () {
        return Promise.resolve('/Users/you/Library/Application Support/Astorm')
      }
    },

    window: {
      minimize: function () {},
      toggleMaximize: function () {},
      close: function () {},
      isMaximized: function () {
        return Promise.resolve(false)
      },
      onMaximizedChange: function () {
        return function () {}
      },
      isFullscreen: function () {
        return Promise.resolve(false)
      },
      onFullscreenChange: function () {
        return function () {}
      }
    },

    zoom: {
      setFactor: function () {},
      getFactor: function () {
        return Promise.resolve(1)
      }
    },

    power: {
      setKeepAwake: function () {
        return Promise.resolve(false)
      }
    },

    permission: {
      status: function () {
        return Promise.resolve([])
      },
      request: function () {
        return Promise.resolve([])
      },
      openSettings: function () {
        return Promise.resolve(false)
      }
    },

    appearance: {
      setThemeMode: function () {}
    },

    attachment: {
      toUrl: function () {
        return ''
      }
    },

    dialog: {
      openFiles: function () {
        return Promise.resolve([])
      },
      openDirectory: function () {
        return Promise.resolve('')
      },
      openAttachments: function () {
        return Promise.resolve([])
      },
      openPptx: function () {
        return Promise.resolve('')
      },
      saveFile: function () {
        return Promise.resolve('')
      }
    },

    fs: {
      openFolder: function () {
        return Promise.resolve('')
      },
      readFileBase64: function () {
        return Promise.resolve('')
      },
      readArrayBuffer: function () {
        return Promise.resolve(null)
      },
      writeTextFile: function () {
        return Promise.resolve({ ok: true })
      },
      listDirs: function (path) {
        return Promise.resolve({
          path: path || '/Users/you/workspace',
          parent: null,
          root: '/',
          dirs: [
            { name: 'astorm', path: '/Users/you/workspace/astorm' },
            { name: 'growth', path: '/Users/you/workspace/growth' },
            { name: 'website', path: '/Users/you/workspace/website' }
          ]
        })
      },
      mkdir: function () {
        return Promise.resolve(true)
      }
    },

    git: {
      currentBranch: function () {
        return Promise.resolve('main')
      },
      isRepo: function () {
        return Promise.resolve(true)
      },
      listBranches: function () {
        return Promise.resolve(['main', 'feat/auth-rotation'])
      },
      branchState: function () {
        return Promise.resolve({
          current: 'main',
          local: ['main', 'feat/auth-rotation'],
          remotes: ['origin/main'],
          uncommitted: 2
        })
      },
      log: function () {
        return Promise.resolve({ commits: [] })
      },
      checkout: function () {
        return Promise.resolve({ ok: true })
      },
      createBranch: function () {
        return Promise.resolve({ ok: true })
      }
    },

    providers: {
      list: function () {
        return Promise.resolve({
          providers: [
            {
              name: 'DeepSeek',
              baseUrl: 'https://api.deepseek.com/v1',
              apiKey: 'sk-preview',
              apiFormat: 'chat_completions',
              order: 0,
              models: [
                { name: 'DeepSeek-V3', order: 0 },
                { name: 'DeepSeek-R1', order: 1 }
              ]
            }
          ],
          encryptionAvailable: true
        })
      },
      sync: function (providers) {
        return Promise.resolve({ providers: providers })
      },
      test: function () {
        return Promise.resolve({ ok: true, message: '预览模式：模拟连接成功' })
      }
    },

    llm: {
      ask: function () {
        return function () {}
      }
    },

    office: {
      preview: function () {
        return Promise.resolve(null)
      }
    },

    pptx: {
      unpack: function () {
        return Promise.resolve({ ok: false, message: '预览模式不支持 PPTX 解包' })
      }
    },

    browser: {
      pickElement: function () {
        return Promise.resolve(null)
      },
      cancelPick: function () {
        return Promise.resolve()
      },
      frame: function () {
        return Promise.resolve(null)
      },
      capture: function () {
        return Promise.resolve(null)
      },
      recordSave: function () {
        return Promise.resolve({ ok: false })
      },
      openDevtools: function () {
        return Promise.resolve()
      }
    },

    browserAgent: {
      onCommand: function () {
        return function () {}
      },
      commandResult: function () {
        return Promise.resolve()
      }
    },

    backend: {
      invoke: function (command, args) {
        var cmd = String(command || '')
        var path = args && (args.path || args.dir)
        if (cmd === 'read_file_content') return Promise.resolve(fileContent(path))
        if (cmd === 'list_directory') return Promise.resolve(MOCK_TREE)
        if (cmd === 'get_settings') return Promise.resolve({ locale: 'zh-CN' })
        if (cmd === 'read_global_agents') return Promise.resolve('')
        if (cmd === 'detect_local_env') return Promise.resolve({})
        if (cmd === 'memory_status') {
          return Promise.resolve({ total: 0, compiled: 0, pending: 0, lastCompiledAt: null })
        }
        if (cmd === 'usage_stats') {
          return Promise.resolve({ days: [], totals: { prompt: 0, completion: 0, total: 0 } })
        }
        if (
          cmd === 'plugin_scan' ||
          cmd === 'list_skills' ||
          cmd === 'list_agents' ||
          cmd === 'list_tools' ||
          cmd === 'list_plan_files' ||
          cmd === 'list_scheduled_tasks' ||
          cmd === 'session_deliverables' ||
          cmd === 'all_deliverables' ||
          cmd === 'list_mcp_servers' ||
          cmd === 'kb_list' ||
          cmd === 'read_audit_log' ||
          cmd === 'toolchain_allowances'
        ) {
          return Promise.resolve([])
        }
        return Promise.resolve({})
      },
      onEvent: function (cb) {
        backendSubs.add(cb)
        return function () {
          backendSubs.delete(cb)
        }
      }
    },

    chat: {
      createSession: function (request) {
        var req = request || {}
        var id = 'preview-new-' + ++runSeq
        var text = String(req.message || '新任务')
        var title = text.replace(/\s+/g, ' ').trim().slice(0, 20) || '新任务'
        var s = {
          id: id,
          title: title,
          titleZh: title,
          titleEn: title,
          workspace: String(req.workspace || ''),
          model: String(req.model || 'DeepSeek-V3'),
          sessionType: 'normal',
          createdAt: Math.floor(Date.now() / 1000),
          updatedAt: Math.floor(Date.now() / 1000),
          status: 'running'
        }
        sessions.unshift(s)
        histories[id] = [
          {
            id: id + '-u1',
            role: 'user',
            content: text,
            seq: 1,
            createdAt: Math.floor(Date.now() / 1000)
          }
        ]
        setTimeout(function () {
          emit({ type: 'title', session_id: id, title: title, title_zh: title, title_en: title })
          streamReply(id, text)
        }, 60)
        return Promise.resolve(id)
      },

      sendMessage: function (sessionId, request) {
        var req = request || {}
        var text = String(req.message || '')
        touchSession(sessionId)
        var list = histories[sessionId] || (histories[sessionId] = [])
        list.push({
          id: sessionId + '-u-' + Date.now(),
          role: 'user',
          content: text,
          seq: list.length + 1,
          createdAt: Math.floor(Date.now() / 1000)
        })
        setTimeout(function () {
          streamReply(sessionId, text)
        }, 80)
        return Promise.resolve()
      },

      startSession: function (sessionId) {
        var s = sessionOf(sessionId)
        if (s) streamReply(sessionId, s.title)
        return Promise.resolve()
      },

      cancel: function (sessionId) {
        cancelRun(sessionId)
        emit({ type: 'cancelled', session_id: sessionId })
        var s = sessionOf(sessionId)
        if (s) s.status = 'cancelled'
        announceSession(sessionId, 'cancelled')
        return Promise.resolve()
      },

      rewindToUserMessage: function () {
        return Promise.resolve()
      },

      forkSession: function (sessionId) {
        return Promise.resolve(sessionId)
      },

      summarizeSession: function () {
        return Promise.resolve(true)
      },

      updateSessionSettings: function () {
        return Promise.resolve()
      },

      updateSessionGoal: function () {
        return Promise.resolve()
      },

      interruptMessage: function (sessionId, request) {
        var text = String((request && request.message) || '')
        cancelRun(sessionId)
        emit({
          type: 'done',
          session_id: sessionId
        })
        setTimeout(function () {
          streamReply(sessionId, text)
        }, 120)
        return Promise.resolve()
      },

      submitInterrupt: function () {
        return Promise.resolve()
      },

      reopenInterrupt: function () {
        return Promise.resolve()
      },

      listSessions: function () {
        var snapshot = sessions.map(function (s) {
          return Object.assign({}, s)
        })
        // 列表回填后补推 session_info：前端据此恢复状态点与会话设置（否则未打开的会话会被当成待办隐藏）
        setTimeout(function () {
          sessions.forEach(function (s) {
            emit(sessionInfoEvent(s, s.status || 'done'))
          })
        }, 320)
        return Promise.resolve(snapshot)
      },

      listMessages: function (sessionId) {
        return Promise.resolve((histories[sessionId] || []).slice())
      },

      archivedBefore: function () {
        return Promise.resolve([])
      },

      getSystemPrompt: function () {
        return Promise.resolve(
          '你是 Astorm 的桌面智能体。当前为 GitHub Pages 交互预览环境，所有工具调用与文件系统访问均为模拟。'
        )
      },

      deleteSession: function (sessionId) {
        sessions = sessions.filter(function (s) {
          return s.id !== sessionId
        })
        delete histories[sessionId]
        return Promise.resolve()
      },

      onEvent: function (cb) {
        chatSubs.add(cb)
        return function () {
          chatSubs.delete(cb)
        }
      }
    }
  }

  // 通用兜底：少数设置页/插件页会探测更细的接口，缺一个方法不应让整页崩掉。
  var noop = function () {
    return Promise.resolve(undefined)
  }
  ;[
    'appearance',
    'attachment',
    'dialog',
    'fs',
    'git',
    'providers',
    'llm',
    'office',
    'pptx',
    'browser',
    'browserAgent',
    'permission',
    'power',
    'shell',
    'system',
    'window',
    'zoom'
  ].forEach(function (ns) {
    if (!api[ns]) return
    var keys = [
      'setFactor',
      'getFactor',
      'setKeepAwake',
      'status',
      'request',
      'openSettings',
      'openExternal',
      'appDir',
      'minimize',
      'toggleMaximize',
      'close',
      'isMaximized',
      'onMaximizedChange',
      'isFullscreen',
      'onFullscreenChange',
      'toUrl',
      'openFiles',
      'openDirectory',
      'openAttachments',
      'openPptx',
      'saveFile',
      'openFolder',
      'readFileBase64',
      'readArrayBuffer',
      'writeTextFile',
      'listDirs',
      'mkdir',
      'currentBranch',
      'isRepo',
      'listBranches',
      'branchState',
      'log',
      'checkout',
      'createBranch',
      'list',
      'sync',
      'test',
      'ask',
      'preview',
      'unpack',
      'pickElement',
      'cancelPick',
      'frame',
      'capture',
      'recordSave',
      'openDevtools',
      'onCommand',
      'commandResult'
    ]
    keys.forEach(function (key) {
      if (typeof api[ns][key] !== 'function') api[ns][key] = noop
    })
  })

  window.api = api

  /* ------------------------------------------------------------------ *
   * 预览默认偏好：浅色 + 中文；置顶两个会话，让侧栏有层级
   * ------------------------------------------------------------------ */
  try {
    if (!localStorage.getItem('astorm:settings')) {
      localStorage.setItem(
        'astorm:settings',
        JSON.stringify({
          themeMode: 'light',
          themePreset: 'blue',
          terminalStyle: 'follow',
          locale: 'zh-CN',
          zoom: 1,
          keepAwake: false
        })
      )
    }
    if (!localStorage.getItem('astorm:sessions:pins:v1')) {
      localStorage.setItem(
        'astorm:sessions:pins:v1',
        JSON.stringify({
          'preview-auth': Date.now(),
          'preview-research': Date.now() - 1000
        })
      )
    }
    if (!localStorage.getItem('astorm:projects:pins:v1')) {
      localStorage.setItem(
        'astorm:projects:pins:v1',
        JSON.stringify({ '/Users/you/workspace/astorm': Date.now() })
      )
    }
  } catch (e) {
    /* 隐私模式忽略 */
  }
})()
