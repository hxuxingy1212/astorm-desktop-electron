//! `memory` 工具：长期记忆（OKF v0.2 Bundle）的**只读**检索。
//!
//! 记忆是跨会话沉淀的结论（决策 / 偏好 / 约定 / 事实），由编译管道在会话结束后写入。
//! 本工具只读三件事：`search` / `read` / `list`。
//!
//! **为什么没有 `status`**：库概览（概念数 / 待编译 / 上次编译）是**诊断信息**，记忆页已经
//! 完整呈现；模型在会话里查它只会多一轮无产出调用。分类表则由 `list` 的空结果提示动态给出
//! （[`available_categories`]），不必额外开一个动作让模型去发现。
//!
//! **为什么没有写入动作**：写入只有一条路（会话 → `raw/` → 编译 → `wiki/`），分类、去重与
//! 合并判定都在管道里；开一条 agent 直写路径会同时绕过这三者，并让 `raw/` 不再是唯一来源真相。
//! 会话里说的话本来就会被下一次编译吃掉，`remember` 是冗余的。
//!
//! 所有读取都走 [`crate::facade::MemoryImpl`]（不直接触碰 Bundle 路径），其中 `read` 的
//! 概念 id 由**模型**填写，故门面侧使用带准入校验的路径接口（挡住 `../` 穿越）。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::app::App;
use crate::facade::memory::ConceptSummary;
use crate::tool::ToolContext;
use crate::util::text::truncate_chars;

/// 检索默认/上限条数。
const SEARCH_DEFAULT: usize = 8;
const SEARCH_MAX: usize = 20;
/// 列表默认/上限条数（只作用于**分类页**的条目；目录页不受 limit 影响）。
const LIST_DEFAULT: usize = 40;
const LIST_MAX: usize = 200;
/// 目录页每类样例条数。
const OVERVIEW_SAMPLES: usize = 3;
/// 目录页样例标题截断（比条目视图更短：目录页只用来判断「值不值得进去」）。
const SAMPLE_TITLE_CHARS: usize = 40;
/// 单条摘要字段截断（避免标题/描述把工具结果撑爆）。
const TITLE_CHARS: usize = 80;
const DESC_CHARS: usize = 120;
/// `read` 返回正文的字符上限（概念正文通常几百字符，超长时截断并标注）。
const BODY_MAX_CHARS: usize = 6_000;

/// `memory` 工具。
pub struct MemoryTool;

#[async_trait]
impl crate::tool::Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "长期记忆检索工具：查跨会话沉淀下来的决策、偏好、指令、项目约定、可复用步骤、环境限制与事实（只读）。三种操作：\n 1. search — 按关键词检索记忆，返回概念 id / 类型 / 标题 / 摘要，可用 read 取全文\n 2. read — 按概念 id（如 decisions/use-jwt）读取完整正文与来源\n 3. list — 浏览记忆库，**分两级**：不带 category 先给目录页（分类 / 概念数 / 样例），带上 category 再列该类条目\n\
        使用建议：当用户提到「之前定过 / 上次说好的 / 我们不是决定过」或在同一项目上继续工作时，先 search 一下过往结论，避免重复讨论或与既有决策冲突；不确定库里有哪几类可用 list 看目录页。\n\
         记忆是只读的：新的结论由应用在会话结束后自动沉淀，不要试图通过本工具写入记忆。"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "read", "list"],
                    "description": "操作类型：search=关键词检索，read=读单个概念，list=两级浏览（目录页 → 分类页）"
                },
                "query": {
                    "type": "string",
                    "description": "检索关键词（search 使用；中文可直接检索，BM25 分词已支持）"
                },
                "concept_id": {
                    "type": "string",
                    "description": "概念 id（read 使用），形如 decisions/use-jwt 或 concepts/xxx"
                },
                "category": {
                    "type": "string",
                    "description": "分类过滤（list 使用）：传了才列该类条目，可用分类 id（如 decision / procedure / convention）、type 名（如 Procedure）或目录名（如 decisions）；不传则返回分类目录页"
                },
                "limit": {
                    "type": "integer",
                    "description": "最大返回条数（search 默认 8、上限 20；list 默认 40、上限 200；目录页不受此参数影响）"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<String, String> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let app = App::try_global()
            .ok_or_else(|| "App 未初始化，无法访问长期记忆".to_string())?;
        let memory = app.memory();

        // 未启用时给一句可读说明而非报错：模型可以继续用当前上下文作答
        if !memory.config().enabled {
            return Ok("长期记忆当前未启用（用户可在「设置 → 记忆」中开启）。\
                       本次无法检索记忆，请依据当前会话上下文作答。"
                .to_string());
        }

        match action.as_str() {
            "search" => execute_search(memory, &params).map(|(text, _)| text),
            "read" => execute_read(memory, &params),
            "list" => execute_list(memory, &params),
            other => Err(format!(
                "不支持的操作: {other}（可用: search / read / list）"
            )),
        }
    }

    /// 附带结构化结果元数据（`memory_results`）：前端「相关记忆」直接读它，不再解析文本。
    async fn execute_with_meta(
        &self,
        params: Value,
        ctx: &ToolContext,
    ) -> Result<(String, Option<Value>), String> {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
        if action != "search" {
            return self.execute(params, ctx).await.map(|text| (text, None));
        }
        let app = App::try_global()
            .ok_or_else(|| "App 未初始化，无法访问长期记忆".to_string())?;
        let memory = app.memory();
        if !memory.config().enabled {
            return self.execute(params, ctx).await.map(|text| (text, None));
        }
        let (text, hits) = execute_search(memory, &params)?;
        let meta = (!hits.is_empty()).then(|| {
            json!({
                "memory_results": hits
                    .iter()
                    .map(|c| json!({
                        "concept_id": c.concept_id,
                        "title": c.title,
                        "snippet": c.description,
                        "observed_at": c.observed_at,
                    }))
                    .collect::<Vec<_>>()
            })
        });
        Ok((text, meta))
    }
}

/// 必填字符串参数。
fn required_str(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少必填参数 {key}"))
}

/// 条数参数：缺省取 `default`，钳制到 `max`。
fn limit_of(params: &Value, default: usize, max: usize) -> usize {
    params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|n| *n > 0)
        .unwrap_or(default)
        .min(max)
}

/// search：BM25 检索 + 单条摘要。返回（文本, 命中概念）——命中供元数据通道使用。
fn execute_search(
    memory: &crate::facade::MemoryImpl,
    params: &Value,
) -> Result<(String, Vec<ConceptSummary>), String> {
    let query = required_str(params, "query")?;
    let limit = limit_of(params, SEARCH_DEFAULT, SEARCH_MAX);
    let hits = memory.search(&query, limit)?;
    if hits.is_empty() {
        return Ok((
            format!(
                "没有检索到与「{}」相关的长期记忆。可能是尚未沉淀，或关键词差异较大（可换同义词，或用 list 浏览记忆库）。",
                truncate_chars(&query, 40)
            ),
            Vec::new(),
        ));
    }
    let mut out = format!(
        "检索到 {} 条与「{}」相关的记忆（相关度降序）：\n",
        hits.len(),
        truncate_chars(&query, 40)
    );
    for hit in &hits {
        out.push_str(&summary_line(hit));
    }
    out.push_str("\n（要看完整正文用 read + concept_id）");
    Ok((out, hits))
}

/// 单条摘要行：`- id [Type] 标题 — 描述`。
fn summary_line(c: &ConceptSummary) -> String {
    let title = truncate_chars(&c.title, TITLE_CHARS);
    let mut line = format!("- {} [{}] {}", c.concept_id, c.type_name, title);
    if !c.description.trim().is_empty() {
        line.push_str(&format!(
            " — {}",
            truncate_chars(c.description.trim(), DESC_CHARS)
        ));
    }
    if let Some(at) = c.observed_at.as_deref().filter(|s| !s.is_empty()) {
        line.push_str(&format!("（观察于 {at}）"));
    }
    line.push('\n');
    line
}

/// read：单个概念的 frontmatter 与正文。
///
/// 直接返回 Markdown 原文（含 frontmatter）：`type` / `tags` / `observed_at` / `sources`
/// 对模型判断「这条记忆什么时候、从哪个会话来的」是有用信息，剥掉反而少线索（人在记忆页
/// 看的是剥掉后的版本，两侧取向不同）。
fn execute_read(memory: &crate::facade::MemoryImpl, params: &Value) -> Result<String, String> {
    let concept_id = required_str(params, "concept_id")?;
    let detail = memory.show_concept(&concept_id)?;
    let body = truncate_chars(&detail.markdown, BODY_MAX_CHARS);
    Ok(format!(
        "概念 {}（{}）\n\n{}",
        detail.concept_id, detail.type_name, body
    ))
}

/// 当前生效的分类 id 列表（**动态取自配置**）。
///
/// 故意不把分类清单写进 `parameters_schema` 的描述串：分类表是配置驱动的，写死必然漂移
/// （加 `procedure` / 删 `status` 之后，那份清单就会误导模型去查一个不存在的分类）。
fn available_categories(memory: &crate::facade::MemoryImpl) -> String {
    memory
        .categories()
        .all()
        .iter()
        .map(|c| c.id.clone())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// list：**两级浏览**（渐进式加载）。
///
/// - 不带 `category` ⇒ **目录页**：分类 id（含空分类）+ 概念数 + 每类少量样例。
///   等价于 OKF 的根 `index.md`（`分类 → 计数 → 链接`），但**每次从概念文件现场算**，
///   不依赖磁盘上那份派生索引，因而不会陈旧。
/// - 带 `category` ⇒ **分类页**：该分类下的条目（id / type / 标题 / 描述 / 观察日期）。
/// - 条目详情交给 `read`：模型只为真正要看的那一条付正文的钱。
fn execute_list(memory: &crate::facade::MemoryImpl, params: &Value) -> Result<String, String> {
    let limit = limit_of(params, LIST_DEFAULT, LIST_MAX);
    let filter = params
        .get("category")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let all = memory.list_concepts()?;

    // 第一跳：目录页（分类 + 计数 + 样例）
    if filter.is_none() {
        return Ok(list_overview(memory, &all));
    }

    let matched: Vec<&ConceptSummary> = all
        .iter()
        .filter(|c| {
            let f = filter.as_deref().unwrap_or_default();
            // 分类 id / type 名 / **目录名** 三种写法都收：模型看到 `decisions/xxx` 这样的概念 id，
            // 很自然会拿目录名当过滤值；三种都认，就不必让它先撞一次空结果才学会。
            c.category_id.eq_ignore_ascii_case(f)
                || c.type_name.to_ascii_lowercase() == f
                || c.dir.eq_ignore_ascii_case(f)
        })
        .collect();
    if matched.is_empty() {
        let f = filter.as_deref().unwrap_or_default();
        // 命中为空时给出**当前生效**的分类 id：分类表是配置驱动的，写死在描述里必然漂移
        return Ok(format!(
            "分类「{f}」下没有记忆概念。当前分类：{}（也可用 type 名过滤；不带 category 再调一次 list 可看目录页）。",
            available_categories(memory)
        ));
    }

    let mut out = format!("分类「{}」下有 {} 个概念", filter.as_deref().unwrap_or_default(), matched.len());
    if matched.len() > limit {
        out.push_str(&format!("，下面列出前 {limit} 个"));
    }
    out.push_str("：\n");

    // 分组计数对**全部命中**计数（不是被 limit 截断后的切片）
    let groups = group_counts(&matched);
    let mut current = String::new();
    for c in matched.iter().take(limit) {
        let group = group_key(c);
        if group != current {
            let (_, type_name, count) = groups
                .iter()
                .find(|(k, _, _)| *k == group)
                .expect("分组计数与归组键同源");
            out.push_str(&format!("\n## {group}（{type_name}）{count} 条\n"));
            current = group;
        }
        out.push_str(&summary_line(c));
    }
    Ok(out)
}

/// 目录页：分类（**含空分类**）+ 概念数 + 每类少量样例。
///
/// 分类顺序取自**当前配置表**（顺序即设计意图），末尾补上「未归入配置分类」的兜底组，
/// 使各类计数之和等于概念总数（不会出现「总数对不上」的目录页）。
fn list_overview(memory: &crate::facade::MemoryImpl, all: &[ConceptSummary]) -> String {
    if all.is_empty() {
        return "长期记忆库目前是空的（可能还没编译过，或还没有沉淀出概念）。".to_string();
    }
    let table = memory.categories();
    let mut out = format!(
        "长期记忆库共 {} 个概念，目录如下（再用 list 并传 category=<分类 id 或 type 名> 进入某一类）：\n",
        all.len()
    );
    for cat in table.all() {
        let items: Vec<&ConceptSummary> = all
            .iter()
            .filter(|c| c.category_id.eq_ignore_ascii_case(&cat.id))
            .collect();
        out.push_str(&format!("\n## {}（{}）{}\n", cat.id, cat.type_name, items.len()));
        for c in items.iter().take(OVERVIEW_SAMPLES) {
            out.push_str(&sample_line(c));
        }
    }
    // 目录不在分类表里的老概念（用户改过分类表）：按目录名兜底，避免从目录页上消失
    let leftover: Vec<&ConceptSummary> = all.iter().filter(|c| c.category_id.is_empty()).collect();
    if !leftover.is_empty() {
        out.push_str(&format!("\n## 未归入配置分类（{} 个，按目录）\n", leftover.len()));
        for c in leftover.iter().take(OVERVIEW_SAMPLES) {
            out.push_str(&sample_line(c));
        }
    }
    out
}

/// 归组键：分类 id（落不到任何分类时退化为目录名）。
fn group_key(c: &ConceptSummary) -> String {
    if c.category_id.is_empty() {
        c.dir.clone()
    } else {
        c.category_id.clone()
    }
}

/// 每个分组的 `(归组键, type 名, 条数)`（按首次出现顺序）。
fn group_counts(matched: &[&ConceptSummary]) -> Vec<(String, String, usize)> {
    let mut groups: Vec<(String, String, usize)> = Vec::new();
    for c in matched {
        let key = group_key(c);
        match groups.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, _, n)) => *n += 1,
            None => groups.push((key, c.type_name.clone(), 1)),
        }
    }
    groups
}

/// 目录页样例行：`- id 标题`（比条目视图更短 —— 目录页只用来判断「值不值得进去」）。
fn sample_line(c: &ConceptSummary) -> String {
    format!(
        "- {} {}\n",
        c.concept_id,
        truncate_chars(&c.title, SAMPLE_TITLE_CHARS)
    )
}
