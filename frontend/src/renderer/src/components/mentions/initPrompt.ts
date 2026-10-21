/**
 * `/init` 命令的预置用户提示词：让模型调研当前工作区并创建/更新 AGENTS.md。
 *
 * 本应用会把「工作区根目录的 AGENTS.md（+ 用户目录全局 AGENTS.md）」自动注入后续会话的
 * 系统提示词（见 docs/backend-contract.md §4.6.1），因此 init 的目标就是生成工作区根 AGENTS.md。
 *
 * 提示词按界面语言给出（zh* → 中文，否则英文）。**刻意不含**路径形态的 `@word` 与 `$token`，
 * 避免被 `deriveMentionRefs` 误判为提及下发。
 */

const ZH = `请为当前工作区创建或更新 AGENTS.md，作为面向后续 AI 编码会话的精简工作区说明。

目标：一份紧凑、可执行的说明，帮助后续会话快速上手、少踩坑。每条内容都要能回答：「没有它，AI 很可能做错吗？」不能，就别写。

## 先读高价值来源
- README 及根清单文件（package.json / Cargo.toml / go.mod / pyproject.toml 等）、工作区与多包配置
- 构建 / 测试 / lint / 格式化 / 类型检查 / 代码生成的配置
- CI 工作流与 pre-commit、任务运行器配置
- 已有的说明文件（AGENTS.md、CLAUDE.md、.cursor/rules、.cursorrules、.github/copilot-instructions.md）
- 仓库内的应用配置（如 opencode.json）

配置与文档读完仍不清楚架构时，再少量阅读有代表性的代码，定位真正的入口、包边界与执行流；优先读「说明系统如何接线」的文件，而非随机叶子文件。以可执行的真相来源为准（脚本、配置优先于文档）；文档与配置冲突时以可执行来源为准，只保留你验证过的内容。

## 需要提炼
- 精确的开发命令，尤其是反直觉的那些
- 如何运行单个测试、单个包或一次定向验证
- 需要固定顺序时的命令顺序（例如 lint 然后 typecheck 然后 test）
- 单仓 / 多包边界、主要目录归属、真正的应用与库入口
- 工具链怪癖：生成代码、数据库迁移、代码生成、构建产物、特殊环境变量加载、开发服务器、部署流程
- 与语言或框架默认不同的仓库约定
- 测试怪癖：夹具、集成测试前置条件、快照流程、依赖的外部服务、慢或易抖动的套件
- 已有说明文件里值得保留的重要约束

## 写作规则
- 只保留高信号、仓库特有的内容：精确命令与快捷方式、从文件名看不出的架构说明、与默认不同的约定、环境与运维坑、值得引用的现有说明来源
- 排除通用建议、长篇教程、详尽文件树、显而易见的语言约定、未经证实的说法
- 使用短段落与小标题；仓库简单就写得简单，很大就只总结真正改变工作方式的结构性事实
- 内容语言与仓库现有文档保持一致
- 若 AGENTS.md 已存在，就地更新而非整篇重写；保留其中已验证的有效内容

本应用会把工作区根目录的 AGENTS.md 自动注入后续会话，请写到工作区根目录。完成后简述你写入的主要章节。`

const EN = `Create or update AGENTS.md for the current workspace: a compact instruction file that helps future AI coding sessions ramp up quickly and avoid mistakes.

Every line should answer: "Would an agent likely miss this without help?" If not, leave it out.

## Read the highest-value sources first
- README and root manifests (package.json / Cargo.toml / go.mod / pyproject.toml ...), workspace and monorepo config
- build / test / lint / format / typecheck / codegen config
- CI workflows, pre-commit and task runner config
- existing instruction files (AGENTS.md, CLAUDE.md, .cursor/rules, .cursorrules, .github/copilot-instructions.md)
- repo-local app config such as opencode.json

If the architecture is still unclear after config and docs, inspect a small number of representative code files to find the real entrypoints, package boundaries and execution flow. Prefer files that explain how the system is wired together over random leaf files. Prefer executable sources of truth (scripts and config over prose); if docs conflict with config, trust the executable source and keep only what you can verify.

## What to extract
- exact developer commands, especially non-obvious ones
- how to run a single test, a single package, or a focused verification step
- required command order when it matters (e.g. lint then typecheck then test)
- monorepo / multi-package boundaries, ownership of major directories, real app and library entrypoints
- toolchain quirks: generated code, migrations, codegen, build artifacts, special env loading, dev servers, deploy flow
- repo-specific conventions that differ from language or framework defaults
- testing quirks: fixtures, integration-test prerequisites, snapshot workflows, required services, slow or flaky suites
- important constraints from existing instruction files worth preserving

## Writing rules
- Include only high-signal, repo-specific guidance: exact commands and shortcuts, architecture notes not obvious from filenames, conventions that differ from defaults, setup and operational gotchas, references to existing instruction sources that matter
- Exclude generic advice, long tutorials, exhaustive file trees, obvious language conventions, speculative claims
- Use short sections and bullets; if the repo is simple keep it simple, if it is large summarize only the structural facts that change how an agent works
- Match the language of the repository's existing documentation
- If AGENTS.md already exists, update it in place instead of rewriting it; preserve verified useful guidance

This app automatically injects the workspace-root AGENTS.md into future sessions, so write it at the workspace root. Finish by summarizing the main sections you wrote.`

/**
 * `/plan <需求>` 的预置提示词：先切「规划模式」，再按用户需求做任务规划。
 * 需求由调用方保证非空（空需求在 ChatInput 已拒绝）。
 */
export function planPrompt(locale: string, requirement: string): string {
  const req = requirement.trim()
  return locale.toLowerCase().startsWith('zh')
    ? `当前修改为计划模式，按照以下用户需求进行任务规划：\n\n${req}`
    : `Switching to Plan mode. Plan the task according to the following request:\n\n${req}`
}

/**
 * 按界面语言取 /init 预置提示词（zh* → 中文，否则英文）。
 * `focus`（`/init <附加约束>` 里的文字）非空时，追加到提示词末尾，要求优先满足。
 */
export function initPrompt(locale: string, focus?: string): string {
  const zh = locale.toLowerCase().startsWith('zh')
  const base = zh ? ZH : EN
  const extra = (focus ?? '').trim()
  if (!extra) return base
  const header = zh ? '用户附加的重点或约束（请优先满足）：' : 'User-provided focus or constraints (honor these):'
  return `${base}\n\n${header}\n${extra}`
}
