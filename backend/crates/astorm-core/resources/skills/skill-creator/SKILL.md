---
name: skill-creator
description: 为 astorm 创建、修改和优化技能（SKILL.md），并把做好的技能导入应用完成注册。当用户想把某个工作流固化为可复用的技能、修改已有技能的指令、排查技能未正确触发，或在对话中提到"创建技能""写个技能""把这个做成技能""技能模板""导入技能""注册技能"时使用。
version: 1.1.0
author: astorm
metadata:
  category: 研发效能
  tags: skill, creator, template, register
---

# 技能创建指南

指导你为 astorm 创建高质量的技能，并通过 `skills` 工具注册到应用。

## 什么是技能

技能是 AI 在对话运行时按需加载的指令集。本质上是一个包含 YAML frontmatter 的 Markdown 文件（SKILL.md），注册到应用后，AI 通过 `skills` 工具加载其完整指令，按照预定义的指令行事。

技能在本应用中的核心机制：

1. **注册**：技能文件放入应用技能目录并建立索引（`skills` 工具 `register` 动作，或「设置 → 技能」页导入）
2. **启用**：用户在「设置 → 技能」页管理技能；新注册的技能默认启用
3. **注入**：会话可用技能的 name + description 摘要注入环境提示词，AI 据此判断何时使用某个技能
4. **加载**：AI 调用 `skills` 工具 `load` 动作加载技能的完整指令并执行

`skills` 工具的三个动作：

| 动作 | 参数 | 说明 |
|------|------|------|
| `search` | `query`（关键词）、`size`（返回条数，默认 5）、`threshold`（相关度阈值，默认 0.5） | 对 SKILL.md 全文检索（名称/描述 + 正文双路），返回匹配的技能名称和描述 |
| `load` | `skills`（技能名称列表） | 返回指定技能的完整 SKILL.md 指令 |
| `register` | `path`（技能路径，支持 `$SESSION_WORKSPACE` 变量） | 校验目录结构后，把技能复制到应用技能目录并注册元数据，幂等可重复 |

---

## SKILL.md 格式详解

技能文件为标准的 "YAML frontmatter + Markdown 正文" 格式：

```markdown
---
name: skill-name
description: 简短但精确的触发描述
version: 1.0.0
author: your-name
metadata:
  category: 研发效能
  tags: python, automation
---

# 技能标题

完整的 Markdown 指令内容
```

### Frontmatter 字段

| 字段 | 必填 | 说明 |
|------|------|------|
| `name` | 是 | 技能唯一标识。kebab-case（小写字母、数字、连字符），对应注册后的技能目录名 |
| `description` | 是 | 触发描述。用于全文检索与提示词注入，写清**技能做什么 + 何时触发** |
| `version` | 否 | 语义化版本号，默认 `1.0.0` |
| `author` | 否 | 作者 |
| `metadata.category` | 否 | 分类，用于设置页筛选，取值见下方分类规范 |
| `metadata.tags` | 否 | 标签，逗号分隔，用于全文检索 |

注意：除上述字段外的 frontmatter 字段（如 license、compatibility）**不会被解析**，重要信息请写进正文。

### 分类规范

`metadata.category` 从六个固定领域中选一个（与「设置 → 技能」页的分类筛选一致）：

`办公自动化` / `数据分析` / `内容创作` / `研发效能` / `安全合规` / `个人助理`

更细粒度的信息（子场景、职业、技术栈）放到 `tags`，如 `tags: git, commit, conventional-commits`。

### 正文写作要点

- 正文即注入给 AI 的指令（`prompt`），建议控制在 **500 行以内**；接近此限时把详细内容拆分到 `references/` 目录，在正文中引用
- 正文之前以空行与 frontmatter 分隔
- 完整的 Markdown 格式，AI 会严格按照你写的指令执行

### 完整示例

```markdown
---
name: astorm-commit-helper
description: 帮助用户生成规范的 Git 提交信息。根据暂存区的变更自动分析变更类型（feat/fix/docs/refactor 等）并生成符合 Conventional Commits 格式的提交信息。当用户执行 git commit 或询问如何编写提交信息时触发。
version: 1.0.0
author: astorm
metadata:
  category: 研发效能
  tags: git, commit, conventional-commits
---

# 提交信息助手

你是一个专业的 Git 提交信息助手。请遵循以下规则：

1. 分析用户提供的 git diff 输出，判断变更类型
2. 使用 Conventional Commits 格式：`<type>(<scope>): <description>`
3. 如果提交包含破坏性变更，在说明末尾添加 `BREAKING CHANGE: <description>`
4. 正文可选，但如果有应说明"为什么"而不是"做了什么"
```

---

## 技能目录组织

```text
# 最小结构
<skill-name>/
└── SKILL.md                 # 必需

# 完整结构
<skill-name>/
├── SKILL.md                 # 必需
├── scripts/                 # 可选：辅助代码片段（纯引用，不自动执行）
│   └── template.py
├── references/              # 可选：按需加载的参考文档
│   └── api-docs.md
└── assets/                  # 可选：模板、示例文件等资源
    └── report-template.docx
```

- `register` 导入时会连同 `scripts/` `references/` `assets/` 三个子目录一并复制到应用技能目录，其余同级文件不会复制
- `references/` 中的参考文档按需加载，超过 300 行应加目录；技能正文中要清晰指引 AI"下一步该读哪个文件"
- `scripts/` 存放可复用代码片段，当多个场景重复编写相同代码时应提取到这里

应用从以下目录自动发现技能（启动时扫描，需重启生效）：

```
<应用数据目录>/skills/<name>/SKILL.md     # 用户导入的技能（register 的目标位置）
~/.config/astorm/skills/<name>/SKILL.md   # 全局技能（跨项目可用）
<项目目录>/.astorm/skills/<name>/SKILL.md # 项目级技能
```

其中应用数据目录为 macOS `~/Library/Application Support/astorm`、Windows `%APPDATA%\astorm`。

---

## 技能创建流程

### 第一步：捕捉意图

与用户沟通，明确以下问题：

- 这个技能应该让 AI 具备什么能力？
- 什么时候应该触发？（用户说的哪些话、哪些场景下应该激活这个技能）
- 期望的输出格式是什么？（代码、文本、结构化数据、文件操作等）
- 这个技能的客观可验证性如何？有明确对错输出的技能适合做测试用例；主观性技能（写作风格、创意）不需要测试

### 第二步：调研现有技能

用 `skills` 工具 `search` 查看已注册的技能，避免重复造轮子，同时了解现有技能的命名风格、分类方式、写作风格：

```json
{"action": "search", "query": "<功能关键词>", "size": 5}
```

### 第三步：编写 SKILL.md

按上述格式编写技能文件，写入磁盘。推荐写到会话工作区（`$SESSION_WORKSPACE/<skill-name>/SKILL.md`）或用户指定的目录，便于后续迭代修改：

**命名规范：**
- kebab-case，不用下划线或驼峰（如 `code-reviewer`、`excel-report-helper`）
- 名称需与现有技能不重复（重复 register 会覆盖同名技能）

**描述编写（最关键的部分）：**
- 描述是触发机制——告诉 AI 何时使用这个技能
- AI 倾向于"欠触发"（不太主动使用技能），所以描述要略"pushy"
- 包含具体场景和上下文提示词，而不仅是功能描述
- 例如：
  - ❌ `"数据分析工具"`
  - ✅ `"分析 CSV/Excel 数据并生成可视化报表。当用户提到销售数据、财务报表、数据透视表、或者想让 AI 帮忙"看一下数据"时使用，即使用户没有明确说'分析'这个词"`

### 第四步：写作指南

**指令风格：**
- 使用**祈使句**直接告诉 AI 做什么
- **解释为什么**比用 MUST/SHOULD 强压更有效——当前的大语言模型理解能力强，知道原理后能做得更好
- 举例子比列规则更有用。示例格式如下：
  ```markdown
  ## 输出格式
  始终使用以下模板：
  # [标题]
  ## 摘要
  ## 关键发现
  ## 建议
  ```

**渐进式披露（Progressive Disclosure）**——技能采用三层加载：

| 层级 | 内容 | 何时加载 | 建议大小 |
|------|------|----------|----------|
| 1 | name + description | 始终在环境提示词中 | ~100 词 |
| 2 | SKILL.md 正文 | `load` 触发时加载 | ~500 行 |
| 3 | 附属资源（references/、scripts/、assets/） | AI 按需读取 | 不限 |

- 二级内容（正文）不要超过 500 行
- 超过 300 行的参考文件应包含目录
- 在正文中清晰指引 AI"下一步该读什么"

**避免过拟合：**
- 不要让技能只对测试示例有效
- 思考用户的真实意图和通用模式
- 与其加更多 MUST 约束，不如改变隐喻或工作方式

### 第五步：register 注册与测试

技能文件写好后，用 `skills` 工具 `register` 导入应用：

```json
{"action": "register", "path": "$SESSION_WORKSPACE/<skill-name>"}
```

- `path` 可以是：单个技能目录（含 SKILL.md）、SKILL.md 文件本身、或包含多个 `<技能名>/SKILL.md` 子目录的集合目录
- register 会校验目录结构与 frontmatter（缺少 frontmatter、YAML 畸形等会逐条报错），修正后重试
- 成功后技能**默认启用**，提示用户可在「设置 → 技能」页确认管理
- 用 2-3 个真实的用户测试 prompt 发起对话，验证触发与执行

**迭代：** 修改源文件后重新 register 即可（幂等覆盖，附属目录同步更新），无需删除重建。注意重复 register 会把技能重置为启用状态。

**测试时的观察要点：**
- AI 是否正确地触发了这个技能？（有没有在不需要时触发？）
- AI 是否按照指令执行了？
- 输出质量是否符合预期？
- AI 是否被指令约束得过于死板？

### 第六步：描述优化

当技能内容稳定后，优化 trigger description：

1. 编写一组 should-trigger 用例（AI 应该触发技能的 prompt）和 should-not-trigger 用例（AI 不应触发但容易误触发的 prompt，即近义词陷阱）
2. should-not-trigger 要选择真正有挑战的边界案例——明显的无关请求（如"写一个斐波那契函数"作为 PDF 技能的负例）没有测试价值
3. 迭代调整 description，使两组用例的命中率最优
4. 最佳描述通常不是最长也不是最短的

**构建测试用例的原则：**
- should-trigger 用例要覆盖不同的表达方式：正式、口语、间接表达
- should-not-trigger 用例要选择"看起来相关但其实不需要"的场景
- 所有用例应该是**真实的**——真实用户会说的话

---

## 更新已有技能

当用户要求修改已有技能时：

1. **保留原名**：不要改变 `name` 字段（改名等于创建新技能，旧的还在）
2. **user 来源**：修改源文件后重新 `register` 覆盖，或在「设置 → 技能」页直接编辑
3. **builtin 来源**（如本 skill-creator）：随应用分发，不可编辑或删除
4. **避免破坏现有行为**：增量改进而非推倒重来
