//! Compiler：记忆单元 → OKF 概念文档。
//!
//! 关键设计：**分类是权威的**
//! - `concept_id` 的目录前缀由 `event_type` 查分类表得出（模型返回的目录被丢弃）
//! - frontmatter 的 `type` 也取分类表的 `type_name`（模型写成别的值也以分类表为准）
//! - `sources` / `generated` 由**调用方确定性写入**（provenance 不交给模型）
//!
//! 这样把「模型把 Decision 写进 concepts/」「漏写 type」「伪造来源」三类错误整类消除。
//! 产出经 [`ConceptDocument::validate_shape`] 校验，缺 `type` 等无效产出**不写入** Bundle
//! （方案文档 §5.2），只计入报告。

use tokio_util::sync::CancellationToken;

use super::concept::{build_concept_id, ConceptDocument, ConceptFrontmatter, GeneratedInfo, SourceEntry};
use super::llm::MemoryLlm;
use super::prompt;
use super::types::{CategoryTable, MemoryUnit};
use crate::config::MemoryConfig;

/// 生产者标识（写入 `generated.by`）。
pub const GENERATOR: &str = concat!("astorm-memory/", env!("CARGO_PKG_VERSION"));

/// 编译阶段产出。
#[derive(Debug, Clone, Default)]
pub struct CompileOutcome {
    pub documents: Vec<ConceptDocument>,
    pub calls: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 无效产出说明（缺概念 ID / 缺 type 等；不写入 Bundle）。
    pub invalid: Vec<String>,
    /// 告警（模型自报 type 与分类表不一致等）。
    pub warnings: Vec<String>,
}

/// 编译上下文（provenance 的真实来源）。
#[derive(Debug, Clone, Default)]
pub struct CompileSource {
    /// 相对 bundle 根的来源资源（`raw/sessions/2026-09-25-a1f9.md`）。
    pub resource: String,
    /// 来源标题（会话标题）。
    pub title: Option<String>,
    /// 来源最后修改时间（RFC3339）。
    pub last_modified: Option<String>,
    /// 锚点 → 消息时间戳：由编译产出的 `frag_ids` **确定性推导** `observed_at`
    /// （LLM 自造时间不可信）。
    pub anchors: std::collections::BTreeMap<String, i64>,
}

/// 把记忆单元编译为 OKF 概念文档（按 `max_units_per_compile_call` 分批）。
pub async fn compile_units(
    llm: &dyn MemoryLlm,
    units: &[MemoryUnit],
    categories: &CategoryTable,
    source: &CompileSource,
    cfg: &MemoryConfig,
    cancel: &CancellationToken,
) -> Result<CompileOutcome, String> {
    let mut out = CompileOutcome::default();
    if units.is_empty() {
        return Ok(out);
    }
    let system = prompt::compile_system(categories);
    let batch_size = cfg.max_units_per_compile_call.max(1);
    let model = llm.model_label();
    let generated_at = crate::parser::now_rfc3339();

    for batch in units.chunks(batch_size) {
        if cancel.is_cancelled() {
            return Err("已取消".into());
        }
        let user = prompt::compile_user(batch);
        let outcome = llm.complete_json(&system, &user, cancel.clone()).await?;
        out.calls += 1;
        out.prompt_tokens += u64::from(outcome.usage.prompt);
        out.completion_tokens += u64::from(outcome.usage.completion);

        let items = outcome
            .value
            .get("concepts")
            .and_then(|v| v.as_array())
            .or_else(|| outcome.value.as_array());
        let Some(items) = items else {
            out.invalid
                .push("编译响应缺少 concepts 数组（本批产出被跳过）".into());
            continue;
        };
        for item in items {
            match build_document(
                item,
                categories,
                source,
                &model,
                &generated_at,
                cfg.max_tags,
                &mut out.warnings,
            ) {
                Ok(doc) => match doc.validate_shape() {
                    Ok(()) => out.documents.push(doc),
                    Err(e) => out.invalid.push(e),
                },
                Err(e) => out.invalid.push(e),
            }
        }
    }

    Ok(out)
}

/// 单条编译产出 → 概念文档（分类权威 + 确定性 provenance）。
fn build_document(
    item: &serde_json::Value,
    categories: &CategoryTable,
    source: &CompileSource,
    model: &str,
    generated_at: &str,
    max_tags: usize,
    warnings: &mut Vec<String>,
) -> Result<ConceptDocument, String> {
    let event_type = item
        .get("event_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let category = categories.resolve(&event_type);
    // 编译阶段回显的锚点：用于推导 observed_at 与保留消息级追溯
    let frag_ids: Vec<String> = item
        .get("frag_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // 分类权威优先：模型自报 type 与分类表不一致时以分类表为准（并告警）
    let mut frontmatter = match item.get("frontmatter").cloned() {
        Some(fm) => serde_json::from_value::<ConceptFrontmatter>(fm)
            .map_err(|e| format!("frontmatter 解析失败（event_type={event_type}）: {e}"))?,
        None => ConceptFrontmatter::default(),
    };
    let reported = frontmatter.concept_type.trim();
    if !reported.is_empty() && reported != category.type_name {
        warnings.push(format!(
            "event_type={event_type} 的产出自报 type={reported}，按分类表改为 {}",
            category.type_name
        ));
    }
    frontmatter.concept_type = category.type_name.clone();

    // tags 上限（新概念同样受限：模型常给 10+ 个 tags）
    if max_tags > 0 && frontmatter.tags.len() > max_tags {
        frontmatter.tags.truncate(max_tags);
    }

    // concept_id：只取模型给的末段作为 slug，目录由分类权威决定
    let body = item
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let raw_id = item
        .get("concept_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let seed = if raw_id.is_empty() {
        frontmatter
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| body.clone())
    } else {
        raw_id.clone()
    };
    let concept_id = build_concept_id(&category, &seed, &seed);

    // observed_at：优先合法 ISO 日期；否则由 frag_ids → 锚点时间戳**确定性推导**；
    // 再否则用来源日期兜底。绝不接受 LLM 自造时间。
    frontmatter.observed_at = frontmatter
        .observed_at
        .take()
        .map(|v| v.trim().to_string())
        .filter(|v| is_iso_date(v))
        .or_else(|| crate::parser::observed_at_from(&source.anchors, &frag_ids))
        .or_else(|| {
            source
                .last_modified
                .as_deref()
                .map(|ts| ts.split('T').next().unwrap_or(ts).to_string())
                .filter(|v| is_iso_date(v))
        });

    // 消息级追溯：frag_ids 写入**类型化**的生产者扩展字段（键名与顺序与旧版 `extra` 写法一致）。
    // 类型化后前端才能拿到它渲染「依据」并跳回原文对应行。
    frontmatter.frag_ids = frag_ids;

    // provenance：本管道自己的来源置首位（不采信模型伪造），模型发现的其它来源去重追加
    let mut sources = vec![SourceEntry {
        resource: source.resource.clone(),
        title: source.title.clone().filter(|t| !t.trim().is_empty()),
        author: None,
        usage_count: None,
        last_modified: source.last_modified.clone(),
    }];
    if let Some(extra) = frontmatter.sources.take() {
        for entry in extra {
            if !entry.resource.trim().is_empty()
                && !sources.iter().any(|s| s.resource == entry.resource)
            {
                sources.push(entry);
            }
        }
    }
    frontmatter.sources = Some(sources);
    frontmatter.generated = Some(GeneratedInfo {
        by: GENERATOR.into(),
        at: generated_at.into(),
        model: Some(model.to_string()),
    });

    Ok(ConceptDocument::new(concept_id, frontmatter, body))
}

/// 是否形如 `YYYY-MM-DD`。
fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ScriptedLlm;
    use serde_json::json;

    fn table() -> CategoryTable {
        CategoryTable::new(MemoryConfig::default().categories)
    }

    fn unit(text: &str, ty: &str) -> MemoryUnit {
        MemoryUnit {
            event_text: text.into(),
            frag_ids: vec!["m01".into()],
            event_type: ty.into(),
            confidence: 0.9,
            observed_at: Some("2025-10-09".into()),
        }
    }

    fn source() -> CompileSource {
        CompileSource {
            resource: "raw/sessions/2025-10-09-a1f9.md".into(),
            title: Some("鉴权讨论".into()),
            last_modified: Some("2025-10-09T08:55:00Z".into()),
            anchors: [("m01".to_string(), 1_760_000_100)].into_iter().collect(),
        }
    }

    async fn compile(llm: &ScriptedLlm, units: &[MemoryUnit]) -> CompileOutcome {
        compile_units(
            llm,
            units,
            &table(),
            &source(),
            &MemoryConfig::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn compiles_with_category_authoritative_dir_and_type() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [
                {"event_type": "decision",
                 "concept_id": "concepts/use-jwt",
                 "frontmatter": {"type": "Decision", "title": "用 JWT", "description": "认证选型", "tags": ["auth"], "observed_at": "2025-10-09"},
                 "body": "选 JWT 而非 session。"}
            ]}),
        );
        let out = compile(&llm, &[unit("选了 JWT", "decision")]).await;

        assert_eq!(out.documents.len(), 1);
        let doc = &out.documents[0];
        // 模型给的 concepts/ 目录被丢弃，按分类落到 decisions/
        assert_eq!(doc.concept_id, "decisions/use-jwt");
        assert_eq!(doc.frontmatter.concept_type, "Decision");
        assert_eq!(doc.frontmatter.title.as_deref(), Some("用 JWT"));
        assert_eq!(doc.frontmatter.tags, vec!["auth"]);
        assert_eq!(doc.frontmatter.observed_at.as_deref(), Some("2025-10-09"));
        assert_eq!(doc.body, "选 JWT 而非 session。");
        assert_eq!(llm.calls(super::super::llm::PromptStage::Compile), 1);
    }

    /// provenance 不采信模型：本管道来源置首位，模型伪造的同资源条目被去重。
    #[tokio::test]
    async fn provenance_is_deterministic_and_deduped() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{
                "event_type": "fact",
                "concept_id": "napi-dll",
                "frontmatter": {
                    "type": "Fact", "title": "后端形态",
                    "sources": [
                        {"resource": "raw/sessions/2025-10-09-a1f9.md", "title": "伪造标题"},
                        {"resource": "raw/sessions/other.md", "title": "模型发现的其它来源"}
                    ]
                },
                "body": "后端是 napi dll。"
            }]}),
        );
        let out = compile(&llm, &[unit("后端是 napi dll", "fact")]).await;
        let sources = out.documents[0].frontmatter.sources.as_ref().unwrap();

        assert_eq!(sources.len(), 2, "同资源应去重: {sources:?}");
        assert_eq!(sources[0].resource, "raw/sessions/2025-10-09-a1f9.md");
        assert_eq!(sources[0].title.as_deref(), Some("鉴权讨论"), "首条应是本管道的真实来源");
        assert_eq!(sources[1].resource, "raw/sessions/other.md", "模型的其它来源保留");

        let generated = out.documents[0].frontmatter.generated.as_ref().unwrap();
        assert_eq!(generated.by, GENERATOR);
        assert_eq!(generated.model.as_deref(), Some("scripted"));
    }

    /// 模型把 type 写错或漏写 → 以分类表为准（并告警）。
    #[tokio::test]
    async fn reported_type_mismatch_is_overridden_with_warning() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{
                "event_type": "decision",
                "concept_id": "x",
                "frontmatter": {"type": "Fact", "title": "T"},
                "body": "b"
            }]}),
        );
        let out = compile(&llm, &[unit("决策", "decision")]).await;
        assert_eq!(out.documents[0].frontmatter.concept_type, "Decision");
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("Fact"), "{:?}", out.warnings);
    }

    /// 未知 event_type 兜底到 other 分类（不丢记忆）。
    #[tokio::test]
    async fn unknown_event_type_falls_back_to_other_category() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{
                "event_type": "episodic",
                "concept_id": "some-thing",
                "frontmatter": {"title": "T"},
                "body": "b"
            }]}),
        );
        let out = compile(&llm, &[unit("某事", "episodic")]).await;
        assert_eq!(out.documents.len(), 1, "未知分类不得丢弃记忆");
        assert_eq!(out.documents[0].concept_id, "concepts/some-thing");
        assert_eq!(out.documents[0].frontmatter.concept_type, "Other");
    }

    #[tokio::test]
    async fn invalid_outputs_are_rejected_not_written() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [
                {"event_type": "fact", "concept_id": "", "frontmatter": {"title": ""}, "body": ""},
                {"event_type": "fact", "concept_id": "ok-one", "frontmatter": {"title": "好的"}, "body": "b"},
                {"event_type": "fact", "frontmatter": "不是对象", "body": "b"}
            ]}),
        );
        let out = compile(&llm, &[unit("a", "fact"), unit("b", "fact"), unit("c", "fact")]).await;
        // 第一条 concept_id 为空 → 以 title/body 兜底（哈希 slug），仍可写入
        assert!(out.documents.len() >= 2, "{:?}", out.documents);
        assert!(out.invalid.len() >= 1, "坏 frontmatter 应计入无效: {:?}", out.invalid);
        assert!(out.documents.iter().all(|d| d.validate_shape().is_ok()));
    }

    #[tokio::test]
    async fn missing_concepts_key_is_reported() {
        let llm = ScriptedLlm::new().with(super::super::llm::PromptStage::Compile, json!({"nope": 1}));
        let out = compile(&llm, &[unit("a", "fact")]).await;
        assert!(out.documents.is_empty());
        assert_eq!(out.invalid.len(), 1);
        assert!(out.invalid[0].contains("concepts"));
    }

    #[tokio::test]
    async fn batches_by_configured_size() {
        let llm = ScriptedLlm::new().with(super::super::llm::PromptStage::Compile, json!({"concepts": []}));
        let units: Vec<MemoryUnit> = (0..5).map(|i| unit(&format!("单元{i}"), "fact")).collect();
        let cfg = MemoryConfig {
            max_units_per_compile_call: 2,
            ..MemoryConfig::default()
        };
        let out = compile_units(&llm, &units, &table(), &source(), &cfg, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.calls, 3, "5 个单元按每批 2 个应分 3 批");
    }

    #[tokio::test]
    async fn empty_units_make_no_call() {
        let llm = ScriptedLlm::new();
        let out = compile(&llm, &[]).await;
        assert!(out.documents.is_empty());
        assert_eq!(out.calls, 0);
        assert!(llm.prompts().is_empty());
    }

    /// LLM 自造的非法日期不得被采用；应由 frag_ids → 锚点确定性推导。
    #[tokio::test]
    async fn llm_supplied_invalid_observed_at_is_replaced_by_derived_date() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{
                "event_type": "fact", "concept_id": "x", "frag_ids": ["m01"],
                "frontmatter": {"title": "T", "observed_at": "1900 年 1 月"},
                "body": "b"
            }]}),
        );
        let out = compile(&llm, &[unit("a", "fact")]).await;
        let observed = out.documents[0].frontmatter.observed_at.as_deref();
        assert_ne!(observed, Some("1900 年 1 月"), "非法值必须被丢弃");
        assert_eq!(observed, Some("2025-10-09"), "应改用 frag_ids → 锚点推导的日期");
    }

    /// 无锚点、无来源日期 → observed_at 为 None（绝不编造时间）。
    #[tokio::test]
    async fn observed_at_is_none_when_nothing_is_derivable() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{"event_type": "fact", "concept_id": "x", "frontmatter": {"title": "T"}, "body": "b"}]}),
        );
        let bare = CompileSource {
            resource: "raw/x.md".into(),
            title: None,
            last_modified: None,
            anchors: Default::default(),
        };
        let out = compile_units(
            &llm,
            &[unit("a", "fact")],
            &table(),
            &bare,
            &MemoryConfig::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.documents[0].frontmatter.observed_at, None);
    }

    /// 无锚点但来源有日期 → 用来源日期兜底（比 None 更有用且仍是确定性的）。
    #[tokio::test]
    async fn observed_at_falls_back_to_source_date() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{"event_type": "fact", "concept_id": "x", "frontmatter": {"title": "T"}, "body": "b"}]}),
        );
        let mut src = source();
        src.anchors.clear();
        let out = compile_units(
            &llm,
            &[unit("a", "fact")],
            &table(),
            &src,
            &MemoryConfig::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.documents[0].frontmatter.observed_at.as_deref(), Some("2025-10-09"));
    }

    /// 编译阶段回显的 frag_ids 写入**类型化**字段（消息级可追溯，前端据此跳原文）。
    #[tokio::test]
    async fn echoed_frag_ids_are_recorded_for_traceability() {
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{
                "event_type": "fact", "concept_id": "x", "frag_ids": ["m03", "m04"],
                "frontmatter": {"title": "T"}, "body": "b"
            }]}),
        );
        let out = compile(&llm, &[unit("a", "fact")]).await;
        assert_eq!(out.documents[0].frontmatter.frag_ids, vec!["m03", "m04"]);
        // 序列化键名与旧版 `extra` 写法一致（存量文件读回不丢字段）
        let md = out.documents[0].to_markdown().unwrap();
        assert!(md.contains("frag_ids:"), "{md}");
        assert!(out.documents[0].frontmatter.extra.is_empty(), "不应再落到 extra");
        // 无 frag_ids 时不写该字段
        let llm = ScriptedLlm::new().with(
            super::super::llm::PromptStage::Compile,
            json!({"concepts": [{"event_type": "fact", "concept_id": "y", "frontmatter": {"title": "T"}, "body": "b"}]}),
        );
        let out = compile(&llm, &[unit("a", "fact")]).await;
        assert!(out.documents[0].frontmatter.frag_ids.is_empty());
        assert!(!out.documents[0].to_markdown().unwrap().contains("frag_ids"));
    }

    #[test]
    fn iso_date_validation() {
        assert!(is_iso_date("2025-10-09"));
        assert!(!is_iso_date("2025-1-9"));
        assert!(!is_iso_date("2025/10/09"));
        assert!(!is_iso_date(""));
        assert!(!is_iso_date("2025-10-09T08:55:00Z"));
    }
}
