# 长期记忆（OKF v0.2 编译管道）

把历史会话沉淀成可检索的「记忆」：`astorm-memory` crate（lib + `okf-agent` CLI）负责全部记忆
语义，`astorm-core` 只做「应用设定 → 记忆库」的集成适配，`astorm-napi` 暴露命令，前端两页浏览。

## 1. 三层架构与不变量

```
会话 JSONL ─[Parser]→ raw/sessions/*.md ─[Extractor]→ 记忆单元 ─[Compiler]→ OKF 概念
                                    ─[Merger]→ 与既有概念比对（CREATE / UPDATE / SKIP）─[Writer]→ wiki/
```

- **`raw/` 是唯一来源真相且写后不改**；编译**只从 `raw/` 读**，从不从已有 `wiki/` 文本编译
  （否则会知识漂移）。
- 全管道只有 **3 个 LLM 调用点**（提取 / 编译 / 合并判定），其余阶段纯确定性 → 可复现、可离线单测、
  可整体取消。带工具的 agent 在**管道之外**（那不是编译的一部分）。
- **写入只有一条路**：`memory` 工具是只读的。分类、去重、合并判定都在管道里，开 agent 直写会同时
  绕过这三者，并让 `raw/` 不再是唯一真相。会话里说的话本来就会被下次编译吃掉。

## 2. Bundle 布局

```
<用户目录>/memory/            # 默认 ~/astorm/memory（用户目录 = 设置里的会话空间基目录）
├── .okf-bundle.json          # 归属标记：非本管道的目录会被拒绝写入
├── .okfstate.json            # 增量状态（size/mtime 快路径 + sha256 慢路径；compiled_at 区分「归一化过」与「编译过」）
├── .okf-index/               # tantivy 全文索引（纯派生，随 Bundle 同生共死）
├── raw/sessions/<日期>-<sid8>.md
└── wiki/
    ├── index.md  log.md      # OKF 保留文件
    └── <category.dir>/{index.md, <slug>.md}
```

Concept ID = 相对 `wiki/` 去掉 `.md`（如 `decisions/use-jwt`）。**目录前缀由分类表权威决定**，
模型返回的路径只取末段当 slug —— 消除「把 Decision 写进 concepts/」这一整类错误。

**归属守卫**：目标目录已有内容但不属于本管道（无标记）时 `ensure_writable` 直接拒绝并列出证据，
绝不覆盖 —— 避免把别人的 memory 目录当自己的 Bundle。

## 2.5 raw 正文的渲染格式

`raw/sessions/*.md` 既是提取阶段的输入、也要在记忆页给用户看，故正文按「**人看简洁、模型也读得懂**」
渲染（`parser.rs::render_message`）：

- 头部 `# 会话 <标题>`；可选 `# 历史用户画像`（摘要消息的 `user_infos`）；随后 `# 对话`。
- 每条消息一个 `### [mNN] <role>` 锚点标题（`mNN` 即提取阶段 `frag_ids` 引用的锚点）。
- **工具调用**折叠进围栏代码块，每条一行 `调用 \`<name>\` \`<args, 400 字符>\``——参数里的 JSON
  不再散落正文，既省字符也避免把结构噪声混进语义。
- **工具结果** `**工具结果** \`<tool_call_id>\`` 起头：多行或 JSON 容器 → 围栏代码块
  （JSON 标注 ` ```json `），单行纯文本 → 引用行；超 `toolResultChars`（默认 1200）截断。
- 其余角色正文保持原样 Markdown。

原则：**结构化 payload 一律折叠成代码块**（可读、可复制、模型一眼分段），**自由文本不加工**。

## 3. 分类体系（配置驱动）

`MemoryConfig.categories: [{ id, dir, type, hint }]`：`id` 是提取阶段 LLM 输出的 `event_type` 取值、
`dir` 是 `wiki/` 下的目录、`type` 是 OKF frontmatter 的 `type`、`hint` 拼进提示词的语义说明。
OKF 的 `type` 是**描述性**而非枚举性的（消费者必须容忍未知类型），故不做代码白名单：
未知 `event_type` 兜底到 `other`（不丢记忆），新增分类只改配置。

**`hint` 是分类准确率的主要杠杆**：它被注入两处 —— 提取阶段的取值集
（`extract_options` → `"procedure" (可复用的操作步骤…)`）与编译阶段的目录映射表
（`CategoryTable::prompt_lines` → `- procedure（type: Procedure）→ procedures/  —— …`）。
调整分类效果优先改 `hint`，而不是改提示词代码。

默认 9 条 → 8 个目录（`fact` / `other` 共用 `concepts/`），**顺序即引导强度**：

| 分类 | 目录 | type | 说明 |
|---|---|---|---|
| `decision` | `decisions/` | Decision | 一次性的选择及其理由 |
| `preference` | `preferences/` | Preference | 用户稳定的倾向与习惯 |
| `instruction` | `instructions/` | Instruction | 用户要求长期遵守的规则 |
| `convention` | `conventions/` | Convention | 项目约定：目录/命名/流程/工具链的固定做法 |
| `procedure` | `procedures/` | Procedure | 可复用的操作步骤、命令序列或排查流程 |
| `constraint` | `constraints/` | Constraint | 环境或外部强加的硬限制：平台、版本、不可改的接口 |
| `fact` | `concepts/` | Fact | 项目里客观成立**且不属于以上分类**的知识 |
| `relationship` | `entities/` | Entity | 命名实体及其属性与关联 |
| `other` | `concepts/` | Other | 无法归入以上分类的其他记忆 |

`convention` / `procedure` / `constraint` 排在 `fact` 之前是刻意的：真实数据审计（64 个概念）显示
`fact` 曾占 55%，其中约 12 条是代码探索笔记、5 条是操作步骤、5 条是环境限制 —— 把「步骤/限制/约定」
笼统归入 `fact` 会让它变成垃圾桶。**已删除的 `status`**（临时状态、进度与待办）与「耐久记忆」的预算
口径自相矛盾：这类内容会过期，现由提取与精选提示词的硬门槛显式拒收；存量 `type: Status` 概念仍留在
`concepts/`，按目录兜底归入 `fact` 计数。

读侧**动态扫目录**（`concept_dirs` / `concept_files`，无白名单），手工放入的自定义目录同样能被列出/校验/检索。

设置页有 4 条 footgun 提示：改 `dir` 不迁移已落盘概念；改 `type` 会让同目录旧概念成为类型冲突；
删分类不删已落盘概念；改 `id` 等于换一类。

## 4. 阈值与去噪

| 配置项 | 默认 | 作用 |
|---|---|---|
| `minConfidence` | 0.6 | 低于此置信度的单元丢弃 |
| `maxUnitsPerSession` | 40 | 单会话产出上限（按置信度排序截断） |
| `curate` | true | 第二遍精选：去噪 + 同主题合并（关掉会明显过抽） |
| `curateUnitsPerCall` | 60 | 精选批大小 |
| `maxUnitsPerCompileCall` | 20 | 单次编译批大小 |
| `rawCharBudget` | 24000 | 单次提取正文字符预算（超出按消息边界分块） |
| `toolResultChars` | 1200 | 归一化时工具结果截断（工具输出是最大噪声源） |
| `candidateMinOverlap` | 0.25 | 合并候选的主题重叠下限（Jaccard，不是裸 BM25 分数） |
| `judgeCandidates` | 8 | 合并判定候选数上限 |
| `maxTags` / `maxAppendSections` | 8 / 3 | 防膨胀：标签上限、追加段上限（超出汇总为单一「补充」段） |
| `relatedConcepts` | 3 | 正文末尾「相关」小节的概念互链条数（0 = 不生成） |
| `extractSelfCritique` | false | 提取二遍自查（成本约 2×） |

**纯寒暄类会话**会被提示词的硬门槛挡掉（`extract` / `curate` 两处都显式列了
`Greetings, pleasantries, and one-off chatter` 与 `Temporary state, progress, TODOs…`），
curate 还明确允许「return an empty array」。
结果：`raw/` 留痕（归一化是确定性阶段，无条件落盘），`wiki/` 无概念，**只花 1 次提取调用**，
且记为 `compiled_at` 后永不再跑。**注意**：这是**提示词层面的语义判断，代码里没有入口阈值**
（最小消息数 / 最小字符数都不存在），所以判断有波动；刻意的取舍是「宁可漏一条，不要塞垃圾」。

## 4.5 正文里的链接（「相关」小节）

概念正文末尾有一节**机器生成**的链接，形如：

```markdown
## 相关

- [JWT 密钥轮换](../concepts/jwt-rotation.md) — 共同标签：auth、jwt
- 原文：[2026-09-20 会话](../../raw/sessions/2026-09-20-cddff183.md)
```

**为什么要管道生成、而不是让模型写**：编译是**单次调用、无工具**的，模型看不到 Bundle 里
有哪些概念，任何跨概念链接都是猜的（真实首次全量编译就产出过 `./approval-tool-call-id.md`
这种悬空链接，随后被 `repair_relative_links` 降级成纯文本）。所以：

- 候选来自 `Merger::candidates`（BM25 + 本轮 overlay），**写链接前再确认目标文件存在**
  ⇒ 结构上不可能产生悬空链接（`related_section_links_existing_concepts_and_is_stable` 钉住）
- 相关性门槛：**有共同标签**或词重叠 ≥ `0.1`（比合并下限 0.25 宽松 —— 链接只要求「值得互看」，
  不要求「可并入同一概念」）。门槛不能是 0：候选池非空时 BM25 总会返回点什么，
  0 门槛会把无关概念也链上（曾出现「偏好中文注释」链到「认证方案用 JWT」）
- 小节按标题**整段替换**并固定落在正文末尾 ⇒ 重复编译逐字稳定（幂等）；
  判等时 `documents_equivalent` 会先剔除这一节，否则 `--force` 重跑会被判成「内容有变化」
- 原文链接用 `sources[].resource`（`raw/sessions/x.md`），从 `wiki/<dir>/` 出发退两级；
  **`validate` 的「越出 Bundle 根」按 Bundle 根判**（`raw/` 也是 Bundle 的一部分），
  只有指向 Bundle 之外（`../../../../etc/passwd`）才 Warning

设置页可调条数（`relatedConcepts`，0 = 关闭）。**存量概念不会自动回填**：小节在「本轮写入过
的概念」上重建，老概念要等它下次被更新（或另行全量重编译）。

## 5. 触发与增量

- 手动「增量编译」/「强制全量」（`memory_compile`，异步 + 进度事件）
- `autoCompile`（默认**关**）：会话**真正收尾**（正常结束且无待决中断）时跑一次增量；
  子会话收尾不唤醒（子代理完成很频繁，白白烧额度）
- 增量：指纹未变**且**编译过 → 跳过（不花 token）。「待编译 N」与编译**共用同一来源集合**
  （子会话被排除），否则那个数字永远归不了零
- **子会话（`SessionType::Child`）整体不进记忆**：那是委派执行的内部轨迹，结论已回到父会话

## 6. 命令与 CLI

napi 命令（`memory_*`）：`memoryRoot` / `memoryStatus` / `memoryCompile` / `memoryCancel` /
`memoryReindex` / `memoryListConcepts` / `memoryShowConcept` / `memorySearch` / `memoryValidate` /
`memoryReadLog` / `memoryListRawSources` / `memoryReadRawSource`。
进度与结果经全局事件总线（`type: "memory"`，`session_id` 为空）。

CLI（独立于 Electron，可对任意会话目录运行）：

```bash
cd backend
cargo run --bin okf-agent -- self-test              # 离线自检闭环（含幂等复跑）
cargo run --bin okf-agent -- status                 # 概念数 / 来源数 / 待编译
cargo run --bin okf-agent -- compile --force        # 全量重编译（需 LLM 凭据）
cargo run --bin okf-agent -- search 桌面整理
cargo run --bin okf-agent -- validate
```

凭据来自 `backend/.env`（`LLM_BASE_URL` / `LLM_MODEL` / `LLM_API_KEY`，**该文件 gitignore**）
或同名环境变量。Bundle 根可用 `--bundle` 指定（默认 `~/astorm/memory`）。

## 7. agent 集成（会话侧）

- **`memory` 工具**（只读）：`search` / `read` / `list`，**渐进式加载**共 4 级：

  | 级 | 载体 | 预算 | 触发 |
  |---|---|---|---|
  | L0 | 能力行 `- 长期记忆：N 个概念（用 memory 工具检索：…）` | ~20 token | 每轮自动 |
  | L1 | `【相关记忆】` 块（BM25 top-5，标题 60 / 描述 90 字符截断） | ≤1200 字符 | 每轮自动，无命中不注入 |
  | L2 | `search`（≤8 条摘要） | 摘要级 | 模型按需 |
  | L3 | `read`（**单个**概念正文，含 frontmatter/sources） | ≤6000 字符 | 模型按需 |

  `list` 是 L2 的**浏览路径**，自己再分两级：不带 `category` 给**目录页**
  （分类 id + 计数 + 每类 ≤3 条样例，**含 0 计数的空分类**），带 `category` 才列该类条目
  （id / type / 标题 / 描述 / 观察日期 + 分组计数）。过滤值三种写法都收：
  分类 id / type 名 / 目录名。
  这是 OKF「根 `index.md` → 分类 `index.md` → 概念」那套分层索引的**等价物**，区别是
  **每次从概念文件现场算**，不读磁盘上那份派生索引 —— 分层只用来控制呈现成本，
  不引入第二份真相（否则会多出「索引陈旧 ⇒ 模型看到已删概念」这类失效模式）。
  实测（64 概念的真实 Bundle）：目录页 **1.6 KB ≈ 500 token 且完整**（9 类含空分类），
  而旧的「一次列 40 条」是 **11 KB ≈ 3.7k token 且已截断 24 条** —— 约 7 倍差价。

  **没有 `status` 动作**：库概览（概念数 / 待编译 / 上次编译）是诊断信息，记忆页已完整呈现，
  模型在会话里查它只会多一轮无产出调用；分类表由目录页与空结果提示动态给出。工具描述里
  因此**不写死分类清单**（写死必然随配置漂移）。
  `search` 走 BM25，索引不可用时退化为**同一分词器**的关键词扫描（不能拿整串查询当子串匹配：
  「我们之前认证方案是怎么定的」不可能作为子串出现在任何正文里）。
  在 `builtin-main`（全量工具）与 `builtin-explore` / `builtin-general` 的工具集中。
- **记忆页的跳转**（`MemoryView.vue`）：来源行可点开原文底稿；「依据」从 frontmatter 的
  `frag_ids`（**权威**依据集，`observed_at` 即由它推导）渲染成可点小丸，点击会跨来源会话找到
  含该锚点的那一份、打开底稿并 `scrollIntoView` 定位高亮 `### [mNN]`；正文里的相对链接
  （`./x.md` / `../concepts/x.md` / `../../raw/sessions/x.md`）被点击拦截 → 概念走站内切换、
  原文走底稿抽屉，不让渲染器导航离开应用。
- **正文链接只用真实 `.md`**：编译提示词明令禁止模型自造伪链接方案（`session:` / `/session:`
  / `astorm:` / 只有日期的目标），来源会话只能以**纯文本**提及（如「依据：2026-09-27 的会话」）——
  正确的相对「原文」链接由 writer 确定性追加（见 §4.5）。否则这类伪链接会落到前端被当成外链弹窗。
- **每轮注入**：环境块加一行能力声明；`ensure_user_prompt` 用**当前用户消息**检索、把命中摘要
  追加成 `【相关记忆】` 块。只落在最后一条用户消息尾部（易变区）→ system 前缀不动、前缀缓存不受影响。
  无命中就不注入（宁缺勿噪）；任何错误一律不注入（fail-soft）。
  配置：`injectMemories`（默认 true）、`injectedConcepts`（默认 5，0 = 关闭）。

### 三个必须保持的不变量

1. **注入文本不得进入 `raw/`（防自我喂养）**。当前有两层保障：
   - **第一层（结构）**：注入只改**请求消息**（`ctx.chat_request` / 内存 history），
     落盘的 `message.jsonl` 里没有注入块 —— 实测确认（真会话的 JSONL 与真 Bundle 的 19 篇 raw
     都不含 `【会话环境】` / `【相关记忆】`）。
   - **第二层（防御）**：`astorm-memory` 归一化时按两个标记取**最早切点**整体剥离，
     覆盖「外部导入的转写文本本身带这类块」以及「将来若改成持久化注入」的情况。
   ⚠️ 两处标记字符串必须成对修改（`agent/prompt.rs` 写入、`astorm-memory/src/parser.rs` 剥离）。
2. **`concept_id` / raw stem 是读路径准入**：可能来自模型/用户输入，必须走
   `concept_path_checked` / `raw_file_checked`（段校验 + 落点蕴含检查）。`concept_path` /
   `raw_file` 留给写入侧自造 id。
3. **注入 fail-soft**：索引被编译占用、索引未建、检索报错 → 不注入，绝不影响会话。

## 8. 明确不做

agent 写记忆（`remember`）、向量/语义检索（检索用 BM25）、外部 agent JSONL 导入、
交互式「记忆整理会话」（处理「该合未合 / 近似重复」的正解，属管道之外的阶段）。

## 9. 测试

```bash
cd backend
cargo test -p astorm-memory                      # 217 单测：格式层 / 索引 / 归一化 / 管道幂等
cargo test -p astorm-core --test memory_auto_compile       # 自动编译钩子（真装配，假供应商零额度）
cargo test -p astorm-core --test memory_agent_integration  # 工具 3 动作（含 list 两级）+ 越权拦截 + 注入门禁
```

真实 API 的验证方式：`ASTORM_DATA_DIR`（应用根）+ `workspace`（用户目录）**双临时目录**隔离，
会话用只读 Explore 智能体 + 一句无关紧要的提示，可完整跑通「会话 done → 钩子 → 编译 → 概念落盘」
而几乎不消耗额度。
