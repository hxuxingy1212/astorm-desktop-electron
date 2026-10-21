//! Merger：新概念与已有概念的语义比对 → CREATE / UPDATE / SKIP。
//!
//! ## 候选召回
//! BM25（[`MemoryIndex`]）**∪** 批内 overlay（本次运行已写出的概念）。
//!
//! 为什么需要 overlay：tantivy 的 reader 只看得见**已 commit** 的文档，而管道为保持
//! 「末尾 commit 一次」的原子性不会每个概念都 commit —— 于是同一次运行内后产出的概念
//! 无法通过 BM25 召回先产出的同批概念，而 LLM 提取很容易吐出近似重复单元。overlay 用
//! 确定性打分（标签/标题/描述的词重叠）补上这块，实现**批内去重**。
//!
//! ## 判定语义（保守、不丢信息）
//! - **无候选 ⇒ 直接 CREATE 且不调用 LLM**（省 token）
//! - LLM 只返回 `UPDATE` / `SKIP`；`CREATE` 由「无候选」或「`decisions` 为空」判定
//! - `type` 冲突（同目录但 frontmatter `type` 不同）⇒ **不自动合并**，告警并另建文件
//! - 判定失败 ⇒ **降级为 CREATE** 并告警（宁可多一个文件，不可丢记忆）
//! - UPDATE 只在 body **追加**带来源标记的小节，**不重写已有正文**（防知识漂移）

use std::collections::{HashMap, HashSet};

use tokio_util::sync::CancellationToken;

use super::concept::ConceptDocument;
use super::index::MemoryIndex;
use super::llm::MemoryLlm;
use super::prompt;
use super::types::CandidateHit;
use crate::config::MemoryConfig;

/// 合并动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    /// 新建概念文件。
    Create,
    /// 合并进已有概念。
    Update,
    /// 已被完全覆盖，跳过。
    Skip,
}

impl MergeAction {
    /// `log.md` 中记录的动作名。
    pub fn as_log_str(&self) -> &'static str {
        match self {
            MergeAction::Create => "CREATE",
            MergeAction::Update => "UPDATE",
            MergeAction::Skip => "SKIP",
        }
    }
}

/// 单个新概念的合并决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeDecision {
    pub action: MergeAction,
    /// 命中的已有概念 ID（`Update` / `Skip` 有值）。
    pub target: Option<String>,
    /// 判定理由（写入日志）。
    pub reason: String,
    /// UPDATE 时要追加的声明。
    pub merge_notes: String,
    /// 告警（类型冲突、判定失败降级等）。
    pub warnings: Vec<String>,
    /// 本次判定消耗（用于报告里的成本可见性）。
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// 合并时的量约束（由 [`MemoryConfig`] 归一化而来）。
#[derive(Debug, Clone, Copy)]
pub struct ConceptLimits {
    /// tags 上限（原有优先保留）。
    pub max_tags: usize,
    /// 追加段上限（超出后汇总为单一「补充」段）。
    pub max_append_sections: usize,
}

impl Default for ConceptLimits {
    fn default() -> Self {
        Self {
            max_tags: 8,
            max_append_sections: 3,
        }
    }
}

impl ConceptLimits {
    pub fn from_config(cfg: &MemoryConfig) -> Self {
        Self {
            max_tags: cfg.max_tags,
            max_append_sections: cfg.max_append_sections,
        }
    }
}

/// 无限制（测试与「不限制」场景）。
pub const NO_LIMITS: ConceptLimits = ConceptLimits {
    max_tags: 0,
    max_append_sections: 0,
};

/// 合并器：持有本次运行的 **overlay**（已写出概念），实现批内去重。
#[derive(Default)]
pub struct Merger {
    overlay: HashMap<String, ConceptDocument>,
}

impl Merger {
    pub fn new() -> Self {
        Self::default()
    }

    /// 本次运行已处理的概念（批内去重用）。
    pub fn overlay(&self) -> &HashMap<String, ConceptDocument> {
        &self.overlay
    }

    /// 记录一个已落盘的概念（管道在写入成功后调用）。
    pub fn track(&mut self, doc: ConceptDocument) {
        self.overlay.insert(doc.concept_id.clone(), doc);
    }

    /// 从 overlay 中取已有概念。
    pub fn overlay_get(&self, concept_id: &str) -> Option<&ConceptDocument> {
        self.overlay.get(concept_id)
    }

    /// 候选召回：BM25 ∪ overlay 确定性打分（按分数降序、去重后取 top `limit`）。
    pub fn candidates(
        &self,
        index: &MemoryIndex,
        doc: &ConceptDocument,
        limit: usize,
        min_overlap: f32,
    ) -> Result<Vec<CandidateHit>, String> {
        let mut best: HashMap<String, CandidateHit> = HashMap::new();
        for hit in index.candidates(doc, limit)? {
            // 主题重叠下限：拦掉「只因共享项目名/高频词」而互相命中的候选，
            // 否则不同主题的碎片会被 LLM 判定为 UPDATE 并全部合并进同一个概念。
            if min_overlap > 0.0 && hit_overlap_score(doc, &hit) < min_overlap {
                continue;
            }
            best.insert(hit.concept_id.clone(), hit);
        }
        for (id, existing) in &self.overlay {
            if id == &doc.concept_id {
                continue;
            }
            let score = overlap_score(doc, existing);
            if min_overlap > 0.0 && score < min_overlap {
                continue;
            }
            let entry = CandidateHit {
                concept_id: id.clone(),
                score,
                type_name: existing.frontmatter.concept_type.clone(),
                title: existing.frontmatter.title.clone().unwrap_or_default(),
                description: existing.frontmatter.description.clone().unwrap_or_default(),
                tags: existing.frontmatter.tags.clone(),
                body_excerpt: existing.body.chars().take(800).collect(),
            };
            // 同 ID 取更高分
            match best.get(id) {
                Some(current) if current.score >= score => {}
                _ => {
                    best.insert(id.clone(), entry);
                }
            }
        }
        let mut hits: Vec<CandidateHit> = best.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.concept_id.cmp(&b.concept_id))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// 决策：检索候选 → （必要时）LLM 判定 → CREATE / UPDATE / SKIP。
    ///
    /// 不做任何文件 I/O；落盘由管道负责（保证「先写文件成功、再更新索引」的顺序）。
    pub async fn decide(
        &self,
        llm: &dyn MemoryLlm,
        index: &MemoryIndex,
        doc: &ConceptDocument,
        cfg: &MemoryConfig,
        cancel: &CancellationToken,
        load_existing: &(dyn Fn(&str) -> Option<ConceptDocument> + Sync),
    ) -> Result<MergeDecision, String> {
        // ① 同 ID 已存在 → 直接判定（无需判定调用）
        if let Some(existing) = self.overlay_get(&doc.concept_id) {
            return Ok(same_id_decision(existing, doc, "同 ID 已存在（本批次内）"));
        }
        // 优先读**磁盘真实内容**：索引只存摘要（正文为空），拿它做等价判定必然误判为「有变化」
        if let Some(existing) = load_existing(&doc.concept_id) {
            return Ok(same_id_decision(&existing, doc, "同 ID 已存在（磁盘）"));
        }
        if let Some(existing) = load_indexed_concept(index, &doc.concept_id)? {
            return Ok(same_id_decision(&existing, doc, "同 ID 已存在（索引）"));
        }

        // ② 候选召回
        let candidates = self.candidates(index, doc, cfg.judge_candidates, cfg.candidate_min_overlap)?;
        if candidates.is_empty() {
            // 无候选 → CREATE，且**不调用 LLM**
            return Ok(MergeDecision {
                action: MergeAction::Create,
                target: None,
                reason: "无相关已有概念".into(),
                merge_notes: String::new(),
                warnings: Vec::new(),
                prompt_tokens: 0,
                completion_tokens: 0,
            });
        }

        // ③ LLM 判定
        let system = prompt::judge_system();
        let user = prompt::judge_user(doc, &candidates);
        let outcome = match llm.complete_json(&system, &user, cancel.clone()).await {
            Ok(outcome) => outcome,
            Err(e) => {
                // 判定失败 → 降级为 CREATE（宁可多一个文件，不可丢记忆）
                return Ok(MergeDecision {
                    action: MergeAction::Create,
                    target: None,
                    reason: "判定失败，降级为新建".into(),
                    merge_notes: String::new(),
                    warnings: vec![format!("合并判定失败，已降级为新建独立概念: {e}")],
                    prompt_tokens: 0,
                    completion_tokens: 0,
                });
            }
        };
        let mut decision = interpret_judgement(&outcome.value, &candidates, doc);
        decision.prompt_tokens = u64::from(outcome.usage.prompt);
        decision.completion_tokens = u64::from(outcome.usage.completion);
        Ok(decision)
    }

}

/// 从索引取回已有概念（索引只存摘要，正文为空；仅用于同 ID 的类型/等价判定）。
fn load_indexed_concept(index: &MemoryIndex, concept_id: &str) -> Result<Option<ConceptDocument>, String> {
    let Some(summary) = index.get(concept_id)? else {
        return Ok(None);
    };
    Ok(Some(ConceptDocument::new(
        concept_id,
        crate::concept::ConceptFrontmatter {
            concept_type: summary.type_name,
            title: Some(summary.title),
            description: Some(summary.description),
            tags: summary.tags,
            ..Default::default()
        },
        String::new(),
    )))
}

/// 同 ID 判定：类型冲突 → 另建；内容完全一致 → SKIP（保证 `--force` 重跑幂等）；
/// 否则 UPDATE。
fn same_id_decision(existing: &ConceptDocument, incoming: &ConceptDocument, reason: &str) -> MergeDecision {
    if existing.frontmatter.concept_type != incoming.frontmatter.concept_type {
        return conflict_decision(incoming, &existing.concept_id, &existing.frontmatter.concept_type);
    }
    if documents_equivalent(existing, incoming) {
        return MergeDecision {
            action: MergeAction::Skip,
            target: Some(existing.concept_id.clone()),
            reason: format!("{reason}，且内容未变"),
            merge_notes: String::new(),
            warnings: Vec::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
        };
    }
    MergeDecision {
        action: MergeAction::Update,
        target: Some(existing.concept_id.clone()),
        reason: reason.into(),
        merge_notes: String::new(),
        warnings: Vec::new(),
        prompt_tokens: 0,
        completion_tokens: 0,
    }
}

/// 正文里的追加段数（`### 追加` / `### 补充` 段标题计数）。
fn count_append_sections(body: &str) -> usize {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("### 追加") || t.starts_with("### 补充")
        })
        .count()
}

/// 把多个追加段收敛为单一「补充」段：**只去掉段标题，保留每条内容与其来源标记**。
pub fn consolidate_appends(body: &str) -> String {
    // 切分点必须是**最早的**追加段标题或来源标记，不能只看来源标记：
    // 本函数输出的 `### 补充` 标题在来源标记之前，若只按来源标记切分，
    // 该标题会被并入「原始正文」而在下次汇总时保留下来 → 标题层叠累积（真实触发过）。
    let idx = ["<!-- source:", "### 追加", "### 补充"]
        .iter()
        .filter_map(|needle| body.find(needle))
        .min();
    let Some(idx) = idx else {
        return body.to_string();
    };
    let original = body[..idx].trim_end();
    let kept: Vec<&str> = body[idx..]
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("### 追加") || t.starts_with("### 补充"))
        })
        .collect();
    let count = kept
        .iter()
        .filter(|l| l.trim_start().starts_with("<!-- source:"))
        .count();
    let header = if count == 0 {
        "### 补充".to_string()
    } else {
        format!("### 补充（累计 {count} 条）")
    };
    format!("{original}\n\n{header}\n\n{}", kept.join("\n").trim())
}

/// 两份概念在**产出层面**是否等价（忽略 `generated` / `sources` 等每次都会变的元数据）。
///
/// 用于「同 ID 且内容未变 ⇒ SKIP」：否则 `--force` 每跑一次都会给 body 追加一次来源标记，
/// wiki 会持续膨胀且不再幂等。
pub fn documents_equivalent(a: &ConceptDocument, b: &ConceptDocument) -> bool {
    if a.concept_id != b.concept_id {
        return false;
    }
    // 正文按「抹平相对 .md 链接」后比较：悬空链接修复会改动磁盘正文，若逐字比较则
    // `--force` 重跑永远被判为有变化 → 重复追加。链接的有无不算新信息。
    // 同理，「相关」小节是每轮**重新生成**的派生内容（检索结果会变），必须先整段剔除。
    let a_body = super::writer::without_related_section(a.body.trim());
    let b_body = super::writer::without_related_section(b.body.trim());
    let body_a = super::concept::strip_relative_md_links(a_body.trim());
    let body_b = super::concept::strip_relative_md_links(b_body.trim());
    if body_a != body_b {
        return false;
    }
    let norm = |v: &Option<String>| v.as_deref().map(str::trim).unwrap_or("").to_string();
    let mut ta = a.frontmatter.tags.clone();
    let mut tb = b.frontmatter.tags.clone();
    ta.sort();
    tb.sort();
    if norm(&a.frontmatter.title) != norm(&b.frontmatter.title)
        || norm(&a.frontmatter.description) != norm(&b.frontmatter.description)
        || a.frontmatter.concept_type.trim() != b.frontmatter.concept_type.trim()
        || ta != tb
    {
        return false;
    }
    // 新来源未被记录 ⇒ 不算等价：同一结论在**另一个会话**被再次确认时，应把新来源并进去
    // （provenance 累积），而不是静默 SKIP。
    let known: HashSet<&str> = a
        .frontmatter
        .sources
        .iter()
        .flatten()
        .map(|s| s.resource.as_str())
        .collect();
    b.frontmatter
        .sources
        .iter()
        .flatten()
        .all(|s| known.contains(s.resource.as_str()))
}

/// 类型冲突：不自动合并，另建文件并告警。
fn conflict_decision(doc: &ConceptDocument, target: &str, existing_type: &str) -> MergeDecision {
    MergeDecision {
        action: MergeAction::Create,
        target: None,
        reason: format!("类型冲突（已有 {target} 的 type 为 {existing_type}），另建独立文件"),
        merge_notes: String::new(),
        warnings: vec![format!(
            "概念 {} 与已有 {} 的类型冲突（{} vs {}），已另建独立文件（方案文档 §5.3）",
            doc.concept_id, target, doc.frontmatter.concept_type, existing_type
        )],
        prompt_tokens: 0,
        completion_tokens: 0,
    }
}

/// 判定器单条结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct JudgeDecision {
    existing_concept_id: String,
    action: String,
    reason: String,
    merge_notes: String,
}

/// 解释 LLM 判定：只认 `UPDATE` / `SKIP`，其余（含空数组）→ CREATE。
fn interpret_judgement(
    value: &serde_json::Value,
    candidates: &[CandidateHit],
    doc: &ConceptDocument,
) -> MergeDecision {
    let decisions = parse_decisions(value);
    let known: HashSet<&str> = candidates.iter().map(|c| c.concept_id.as_str()).collect();
    // 只接受指向候选列表内的决策（防模型编造 ID）
    let valid: Vec<&JudgeDecision> = decisions
        .iter()
        .filter(|d| known.contains(d.existing_concept_id.as_str()))
        .collect();

    // 优先 UPDATE（冲突必须 UPDATE，不得 SKIP）
    if let Some(found) = valid
        .iter()
        .find(|d| d.action.eq_ignore_ascii_case("update"))
    {
        let candidate = candidates.iter().find(|c| c.concept_id == found.existing_concept_id);
        if let Some(candidate) = candidate {
            if candidate.type_name != doc.frontmatter.concept_type {
                return conflict_decision(doc, &candidate.concept_id, &candidate.type_name);
            }
        }
        return MergeDecision {
            action: MergeAction::Update,
            target: Some(found.existing_concept_id.clone()),
            reason: found.reason.clone(),
            merge_notes: found.merge_notes.clone(),
            warnings: Vec::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
        };
    }

    if let Some(found) = valid.iter().find(|d| d.action.eq_ignore_ascii_case("skip")) {
        return MergeDecision {
            action: MergeAction::Skip,
            target: Some(found.existing_concept_id.clone()),
            reason: found.reason.clone(),
            merge_notes: String::new(),
            warnings: Vec::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
        };
    }

    // CREATE 由上层判定：无有效决策（空数组 / 全指向未知 ID）⇒ 新建
    MergeDecision {
        action: MergeAction::Create,
        target: None,
        reason: "判定未命中已有概念".into(),
        merge_notes: String::new(),
        warnings: Vec::new(),
        prompt_tokens: 0,
        completion_tokens: 0,
    }
}

/// 解析 `{"decisions": [...]}`（容忍直接给数组）。
fn parse_decisions(value: &serde_json::Value) -> Vec<JudgeDecision> {
    let array = value
        .get("decisions")
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());
    let Some(array) = array else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| {
            let existing_concept_id = item.get("existing_concept_id")?.as_str()?.trim().to_string();
            if existing_concept_id.is_empty() {
                return None;
            }
            Some(JudgeDecision {
                existing_concept_id,
                action: item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                reason: item
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                merge_notes: item
                    .get("merge_notes")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            })
        })
        .collect()
}

/// 词重叠打分（Jaccard）：ASCII 单词 + CJK 字符二元组。
fn overlap_score(a: &ConceptDocument, b: &ConceptDocument) -> f32 {
    jaccard(&feature_tokens(a), &feature_tokens(b))
}

/// 与候选摘要（标题/描述/标签）的主题重叠度。
///
/// `pub(crate)`：管道生成「相关」链接时复用同一套打分（避免两处各写一份重叠度定义）。
pub(crate) fn hit_overlap_score(doc: &ConceptDocument, hit: &CandidateHit) -> f32 {
    let a = feature_tokens(doc);
    let mut text = String::new();
    text.push_str(&hit.title);
    text.push(' ');
    text.push_str(&hit.description);
    text.push(' ');
    text.push_str(&hit.tags.join(" "));
    jaccard(&a, &tokenize_features(&text))
}

/// Jaccard 相似度。
fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 { 0.0 } else { inter / union }
}

/// 概念的特征 token：标题 + 描述 + 标签（正文不计，避免长文噪声掩盖主题差异）。
fn feature_tokens(doc: &ConceptDocument) -> HashSet<String> {
    let mut text = String::new();
    if let Some(t) = &doc.frontmatter.title {
        text.push_str(t);
        text.push(' ');
    }
    if let Some(d) = &doc.frontmatter.description {
        text.push_str(d);
        text.push(' ');
    }
    text.push_str(&doc.frontmatter.tags.join(" "));
    tokenize_features(&text)
}

/// ASCII 单词（小写）+ CJK 二元组。
fn tokenize_features(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    let mut out = HashSet::new();
    let mut word = String::new();
    let mut prev_cjk: Option<char> = None;
    for ch in lower.chars() {
        if is_cjk(ch) {
            if !word.is_empty() {
                out.insert(std::mem::take(&mut word));
            }
            if let Some(prev) = prev_cjk {
                out.insert(format!("{prev}{ch}"));
            }
            out.insert(ch.to_string());
            prev_cjk = Some(ch);
        } else if ch.is_alphanumeric() {
            word.push(ch);
            prev_cjk = None;
        } else {
            if !word.is_empty() {
                out.insert(std::mem::take(&mut word));
            }
            prev_cjk = None;
        }
    }
    if !word.is_empty() {
        out.insert(word);
    }
    out
}

/// 是否 CJK 表意文字（用于二元组切分）。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

/// 合并两份概念文档：frontmatter 字段级合并 + body **追加**（不重写已有正文）。
///
/// - 新值覆盖旧值（title / description / status / stale_after / type）
/// - `tags` 取并集并排序
/// - `sources` 按 `resource` 去重，同资源保留 `last_modified` 最新者
/// - `observed_at` 保留**最早**（首次观察）
/// - `generated` 用新值
/// - body 追加带来源标记的小节，**已有正文原样保留**
pub fn merge_documents(
    existing: &ConceptDocument,
    incoming: &ConceptDocument,
    merge_notes: &str,
    source_resource: &str,
    at: &str,
    limits: ConceptLimits,
) -> ConceptDocument {
    let mut frontmatter = existing.frontmatter.clone();

    if incoming.frontmatter.title.as_deref().map(str::trim).is_some_and(|t| !t.is_empty()) {
        frontmatter.title = incoming.frontmatter.title.clone();
    }
    if incoming
        .frontmatter
        .description
        .as_deref()
        .map(str::trim)
        .is_some_and(|d| !d.is_empty())
    {
        frontmatter.description = incoming.frontmatter.description.clone();
    }
    if incoming.frontmatter.status.is_some() {
        frontmatter.status = incoming.frontmatter.status.clone();
    }
    if incoming.frontmatter.stale_after.is_some() {
        frontmatter.stale_after = incoming.frontmatter.stale_after.clone();
    }
    if !incoming.frontmatter.concept_type.trim().is_empty() {
        frontmatter.concept_type = incoming.frontmatter.concept_type.clone();
    }
    frontmatter.generated = incoming.frontmatter.generated.clone();

    // tags：**原有优先**（保持稳定），再补新出现的，最后按上限截断 —— 纯并集会让高频概念
    // 的 tags 无限膨胀（真实数据出现过 29 个 tags 的概念，已退化成关键词垃圾堆）。
    let mut tags: Vec<String> = Vec::new();
    for t in frontmatter.tags.iter().chain(incoming.frontmatter.tags.iter()) {
        let t = t.trim().to_string();
        if !t.is_empty() && !tags.iter().any(|x| x == &t) {
            tags.push(t);
        }
    }
    if limits.max_tags > 0 {
        tags.truncate(limits.max_tags);
    }
    frontmatter.tags = tags;

    // sources 去重：同 resource 保留 last_modified 最新者
    let mut merged_sources: Vec<super::concept::SourceEntry> =
        frontmatter.sources.take().unwrap_or_default();
    for entry in incoming.frontmatter.sources.iter().flatten() {
        match merged_sources.iter_mut().find(|s| s.resource == entry.resource) {
            Some(current) => {
                let newer = match (&current.last_modified, &entry.last_modified) {
                    (Some(a), Some(b)) => b > a,
                    (None, Some(_)) => true,
                    _ => false,
                };
                if newer {
                    current.last_modified = entry.last_modified.clone();
                }
                if current.title.is_none() {
                    current.title = entry.title.clone();
                }
            }
            None => merged_sources.push(entry.clone()),
        }
    }
    merged_sources.sort_by(|a, b| a.resource.cmp(&b.resource));
    frontmatter.sources = if merged_sources.is_empty() {
        None
    } else {
        Some(merged_sources)
    };

    // observed_at 保留最早（ISO 日期可直接按字典序比较）
    frontmatter.observed_at = match (&frontmatter.observed_at, &incoming.frontmatter.observed_at) {
        (Some(a), Some(b)) => Some(a.min(b).clone()),
        (Some(a), None) => Some(a.clone()),
        (None, b) => b.clone(),
    };

    // body：追加（绝不重写已有正文）
    let addition = if merge_notes.trim().is_empty() {
        incoming.body.trim()
    } else {
        merge_notes.trim()
    };
    let date = at.split('T').next().unwrap_or(at);
    let mut body = existing.body.trim_end().to_string();
    // 幂等守卫：同样的追加内容已在正文里（重复强制编译的典型情形）→ 不再重复追加，
    // 否则每跑一次 `--force` 都会多出一段「追加」，wiki 会无限膨胀。
    // （注意：悬空链接的修复后处理会改动正文，因此“磁盘内容 ≠ LLM 原始输出”是常态，
    //   不能只靠 documents_equivalent 兜住。）
    let already_present = !addition.is_empty() && body.contains(addition);
    if addition.is_empty() || already_present {
        if !already_present || addition.is_empty() {
            body.push_str(&format!("\n\n<!-- source: {source_resource} @ {at} -->"));
        }
    } else if limits.max_append_sections > 0
        && count_append_sections(&body) >= limits.max_append_sections
    {
        // 追加段数已达上限：把历史追加汇总为**单一「补充」段**（保留每条的来源标记，
        // 只收敛段标题），新内容并入同一段。否则高频概念的正文会无限累加
        // （真实数据出现过 9 个追加段、3801 字符的概念）。
        body = consolidate_appends(&body);
        body.push_str(&format!("\n\n<!-- source: {source_resource} @ {at} -->\n\n{addition}"));
    } else {
        body.push_str(&format!(
            "\n\n<!-- source: {source_resource} @ {at} -->\n### 追加（{date}）\n\n{addition}"
        ));
    }

    ConceptDocument::new(existing.concept_id.clone(), frontmatter, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concept::{ConceptFrontmatter, GeneratedInfo, SourceEntry};
    use crate::config::MemoryConfig;
    use crate::llm::{PromptStage, ScriptedLlm};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp_index(tag: &str) -> (PathBuf, MemoryIndex) {
        let dir = std::env::temp_dir().join(format!(
            "astorm_mem_merge_{tag}_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.clone(), MemoryIndex::new(&dir).unwrap())
    }

    fn doc(id: &str, type_name: &str, title: &str, desc: &str, tags: &[&str], body: &str) -> ConceptDocument {
        ConceptDocument::new(
            id,
            ConceptFrontmatter {
                concept_type: type_name.into(),
                title: Some(title.into()),
                description: Some(desc.into()),
                tags: tags.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            body,
        )
    }

    async fn decide(
        merger: &Merger,
        index: &MemoryIndex,
        llm: &ScriptedLlm,
        doc: &ConceptDocument,
    ) -> MergeDecision {
        merger
            .decide(
                llm,
                index,
                doc,
                &MemoryConfig::default(),
                &CancellationToken::new(),
                &|_| None,
            )
            .await
            .unwrap()
    }

    /// 无候选 → CREATE 且**不调用 LLM**。
    #[tokio::test]
    async fn empty_index_creates_without_calling_llm() {
        let (dir, index) = tmp_index("create");
        let merger = Merger::new();
        let llm = ScriptedLlm::new(); // 严格模式：任何调用都会报错
        let decision = decide(&merger, &index, &llm, &doc("decisions/x", "Decision", "K8s 选型", "容器编排", &[], "选 k3s")).await;

        assert_eq!(decision.action, MergeAction::Create);
        assert!(decision.target.is_none());
        assert!(llm.prompts().is_empty(), "无候选时不应调用 LLM");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn judge_update_targets_matching_candidate() {
        let (dir, index) = tmp_index("update");
        let existing = doc("decisions/use-jwt", "Decision", "认证方案选型", "选择无状态令牌", &["auth"], "用 JWT");
        index.upsert(&existing, 1).unwrap();
        index.commit().unwrap();

        let merger = Merger::new();
        let llm = ScriptedLlm::new().with(
            PromptStage::Judge,
            json!({"decisions": [{
                "existing_concept_id": "decisions/use-jwt",
                "action": "UPDATE",
                "reason": "同一决策的补充",
                "merge_notes": "补充：还要支持多端无状态鉴权"
            }]}),
        );
        let decision = decide(&merger, &index, &llm, &doc("decisions/auth-2", "Decision", "认证方案选型补充", "无状态令牌", &["auth"], "继续用 JWT")).await;

        assert_eq!(decision.action, MergeAction::Update);
        assert_eq!(decision.target.as_deref(), Some("decisions/use-jwt"));
        assert!(decision.merge_notes.contains("多端无状态"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn judge_skip_is_respected() {
        let (dir, index) = tmp_index("skip");
        let existing = doc("decisions/use-jwt", "Decision", "认证方案选型", "选择无状态令牌", &["auth"], "用 JWT 而非 session");
        index.upsert(&existing, 1).unwrap();
        index.commit().unwrap();

        let merger = Merger::new();
        let llm = ScriptedLlm::new().with(
            PromptStage::Judge,
            json!({"decisions": [{"existing_concept_id": "decisions/use-jwt", "action": "SKIP", "reason": "已完全覆盖"}]}),
        );
        let decision = decide(&merger, &index, &llm, &doc("decisions/same", "Decision", "认证方案选型", "选择无状态令牌", &["auth"], "用 JWT 而非 session")).await;

        assert_eq!(decision.action, MergeAction::Skip);
        assert_eq!(decision.target.as_deref(), Some("decisions/use-jwt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空 decisions → CREATE（CREATE 不由 LLM 输出）。
    #[tokio::test]
    async fn empty_decisions_means_create() {
        let (dir, index) = tmp_index("empty");
        let existing = doc("decisions/use-jwt", "Decision", "认证方案选型", "无状态令牌", &["auth"], "用 JWT");
        index.upsert(&existing, 1).unwrap();
        index.commit().unwrap();

        let merger = Merger::new();
        let llm = ScriptedLlm::new().with(PromptStage::Judge, json!({"decisions": []}));
        let decision = decide(&merger, &index, &llm, &doc("decisions/new", "Decision", "认证方案选型补充", "无状态令牌", &["auth"], "另一条")).await;
        assert_eq!(decision.action, MergeAction::Create);

        // 模型编造候选 ID → 同样视为未命中 → CREATE
        let llm = ScriptedLlm::new().with(
            PromptStage::Judge,
            json!({"decisions": [{"existing_concept_id": "不存在的概念", "action": "UPDATE"}]}),
        );
        let decision = decide(&merger, &index, &llm, &doc("decisions/new2", "Decision", "认证方案选型补充", "无状态令牌", &["auth"], "另一条")).await;
        assert_eq!(decision.action, MergeAction::Create, "未知 ID 的决策应被忽略");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 判定调用失败 → 降级 CREATE 并告警（宁可多一个文件，不可丢记忆）。
    #[tokio::test]
    async fn judge_failure_degrades_to_create_with_warning() {
        let (dir, index) = tmp_index("degrade");
        index.upsert(&doc("decisions/use-jwt", "Decision", "认证方案选型", "无状态令牌", &["auth"], "用 JWT"), 1).unwrap();
        index.commit().unwrap();

        let merger = Merger::new();
        let llm = ScriptedLlm::new(); // 严格模式：判定调用直接报错
        let decision = decide(&merger, &index, &llm, &doc("decisions/x", "Decision", "认证方案选型", "无状态令牌", &["auth"], "用 JWT 而非 session")).await;

        assert_eq!(decision.action, MergeAction::Create);
        assert_eq!(decision.warnings.len(), 1);
        assert!(decision.warnings[0].contains("降级"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 类型冲突 → 不自动合并，另建文件并告警。
    #[tokio::test]
    async fn type_conflict_creates_separate_file_with_warning() {
        let (dir, index) = tmp_index("conflict");
        index.upsert(&doc("concepts/same-slug", "Fact", "认证方案选型", "无状态令牌", &["auth"], "旧事实"), 1).unwrap();
        index.commit().unwrap();

        let merger = Merger::new();
        let llm = ScriptedLlm::new().with(
            PromptStage::Judge,
            json!({"decisions": [{"existing_concept_id": "concepts/same-slug", "action": "UPDATE", "reason": "看起来相关"}]}),
        );
        // 同 ID 但 type 不同
        let decision = decide(&merger, &index, &llm, &doc("concepts/same-slug", "Decision", "认证方案选型", "无状态令牌", &["auth"], "新决策")).await;

        assert_eq!(decision.action, MergeAction::Create, "类型冲突不得自动合并");
        assert_eq!(decision.warnings.len(), 1);
        assert!(decision.warnings[0].contains("类型冲突"), "{:?}", decision.warnings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同 ID 且类型一致 → 直接 UPDATE，不调用 LLM。
    #[tokio::test]
    async fn same_id_updates_without_llm_call() {
        let (dir, index) = tmp_index("sameid");
        index.upsert(&doc("decisions/use-jwt", "Decision", "旧标题", "旧描述", &[], "旧正文"), 1).unwrap();
        index.commit().unwrap();

        let merger = Merger::new();
        let llm = ScriptedLlm::new();
        let decision = decide(&merger, &index, &llm, &doc("decisions/use-jwt", "Decision", "新标题", "新描述", &[], "新正文")).await;

        assert_eq!(decision.action, MergeAction::Update);
        assert_eq!(decision.target.as_deref(), Some("decisions/use-jwt"));
        assert!(llm.prompts().is_empty(), "同 ID 无需判定调用");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 批内去重：索引里没有（未 commit），但 overlay 有 → 仍能召回候选。
    #[tokio::test]
    async fn overlay_enables_intra_batch_dedup() {
        let (dir, index) = tmp_index("overlay");
        // 索引为空（模拟本次运行尚未 commit）
        let mut merger = Merger::new();
        merger.track(doc("decisions/use-jwt", "Decision", "认证方案选型", "选择无状态令牌", &["auth"], "用 JWT"));
        assert!(!merger.overlay().is_empty());

        // 新概念与 overlay 中的概念高度相似 → 应召回候选并调用判定
        let llm = ScriptedLlm::new().with(
            PromptStage::Judge,
            json!({"decisions": [{"existing_concept_id": "decisions/use-jwt", "action": "UPDATE", "reason": "同批重复", "merge_notes": "合并"}]}),
        );
        let decision = decide(&merger, &index, &llm, &doc("decisions/auth-extra", "Decision", "认证方案选型", "选择无状态令牌", &["auth"], "用 JWT")).await;

        assert_eq!(decision.action, MergeAction::Update);
        assert_eq!(decision.target.as_deref(), Some("decisions/use-jwt"));
        assert_eq!(llm.calls(PromptStage::Judge), 1, "overlay 命中应触发判定");

        // 不相关的新概念 → overlay 不打分 → 无候选 → CREATE
        let llm = ScriptedLlm::new();
        let decision = decide(&merger, &index, &llm, &doc("decisions/k8s", "Decision", "k8s 集群选型", "容器编排", &[], "选 k3s")).await;
        assert_eq!(decision.action, MergeAction::Create);
        assert!(llm.prompts().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同 ID 命中 overlay 也走 UPDATE（不重复判定）。
    #[tokio::test]
    async fn overlay_same_id_updates_directly() {
        let (dir, index) = tmp_index("overlaysame");
        let mut merger = Merger::new();
        merger.track(doc("decisions/use-jwt", "Decision", "认证方案选型", "描述", &[], "第一版"));
        let llm = ScriptedLlm::new();
        let decision = decide(&merger, &index, &llm, &doc("decisions/use-jwt", "Decision", "认证方案选型", "描述", &[], "第二版")).await;
        assert_eq!(decision.action, MergeAction::Update);
        assert!(decision.reason.contains("本批次"), "{}", decision.reason);
        assert!(llm.prompts().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- merge_documents（纯合并规则） ----------

    fn existing_doc() -> ConceptDocument {
        ConceptDocument::new(
            "decisions/use-jwt",
            ConceptFrontmatter {
                concept_type: "Decision".into(),
                title: Some("旧标题".into()),
                description: Some("旧描述".into()),
                tags: vec!["zeta".into(), "auth".into()],
                observed_at: Some("2025-10-09".into()),
                sources: Some(vec![
                    SourceEntry {
                        resource: "raw/a.md".into(),
                        title: Some("A".into()),
                        author: None,
                        usage_count: None,
                        last_modified: Some("2025-10-09T00:00:00Z".into()),
                    },
                    SourceEntry {
                        resource: "raw/b.md".into(),
                        title: None,
                        author: None,
                        usage_count: None,
                        last_modified: Some("2025-10-10T00:00:00Z".into()),
                    },
                ]),
                generated: Some(GeneratedInfo {
                    by: "old".into(),
                    at: "2025-10-09T00:00:00Z".into(),
                    model: Some("old-model".into()),
                }),
                ..Default::default()
            },
            "第一版正文，必须原样保留。",
        )
    }

    #[test]
    fn merge_unions_tags_and_keeps_earliest_observed_at() {
        let mut incoming = doc("decisions/use-jwt", "Decision", "新标题", "新描述", &["beta", "auth"], "");
        incoming.frontmatter.observed_at = Some("2025-10-12".into());
        incoming.frontmatter.generated = Some(GeneratedInfo {
            by: "astorm-memory/0.1.0".into(),
            at: "2025-10-12T00:00:00Z".into(),
            model: Some("new-model".into()),
        });

        let merged = merge_documents(&existing_doc(), &incoming, "新增声明", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);

        assert_eq!(merged.concept_id, "decisions/use-jwt");
        assert_eq!(merged.frontmatter.title.as_deref(), Some("新标题"), "新值覆盖旧值");
        assert_eq!(
            merged.frontmatter.tags,
            vec!["zeta", "auth", "beta"],
            "tags 应「原有优先 + 补新 + 去重」（不再是字母序并集）"
        );
        assert_eq!(
            merged.frontmatter.observed_at.as_deref(),
            Some("2025-10-09"),
            "observed_at 保留最早"
        );
        assert_eq!(
            merged.frontmatter.generated.as_ref().unwrap().model.as_deref(),
            Some("new-model"),
            "generated 用新值"
        );
    }

    /// tags 上限：原有优先保留，新词挤不进来（防止高频概念的 tags 无限膨胀）。
    #[test]
    fn merge_caps_tags_keeping_existing_first() {
        let mut existing = existing_doc(); // tags = ["zeta", "auth"]
        existing.frontmatter.tags = vec!["a".into(), "b".into(), "c".into()];
        let mut incoming = doc("decisions/use-jwt", "Decision", "T", "D", &["d", "e", "f", "a"], "");
        incoming.frontmatter.tags = vec!["d".into(), "e".into(), "f".into(), "a".into()];

        let merged = merge_documents(
            &existing,
            &incoming,
            "",
            "raw/x.md",
            "2025-10-12T03:00:00Z",
            ConceptLimits {
                max_tags: 4,
                max_append_sections: 0,
            },
        );
        assert_eq!(
            merged.frontmatter.tags,
            vec!["a", "b", "c", "d"],
            "应保留原有 3 个 + 第 1 个新词，超出的新词被截断"
        );
    }

    /// 追加段上限：超过后收敛为**单一「补充」段**，且每条的来源标记都要保留。
    #[test]
    fn merge_consolidates_appends_beyond_limit() {
        let limits = ConceptLimits {
            max_tags: 0,
            max_append_sections: 2,
        };
        let base = existing_doc();
        // 第 1、2 次追加 → 正常生成 2 个「追加」段
        let a = merge_documents(&base, &base, "第一条补充", "raw/1.md", "2025-10-01T00:00:00Z", limits);
        let b = merge_documents(&a, &base, "第二条补充", "raw/2.md", "2025-10-02T00:00:00Z", limits);
        assert_eq!(count_append_sections(&b.body), 2, "{}", b.body);

        // 第 3 次追加触发汇总：段标题收敛为 1 个「补充」，但内容与来源标记都在
        let c = merge_documents(&b, &base, "第三条补充", "raw/3.md", "2025-10-03T00:00:00Z", limits);
        assert_eq!(count_append_sections(&c.body), 1, "应汇总为单段: {}", c.body);
        assert!(c.body.contains("### 补充（累计"), "{}", c.body);
        for needle in ["第一条补充", "第二条补充", "第三条补充"] {
            assert!(c.body.contains(needle), "内容不得丢失（{needle}）: {}", c.body);
        }
        for src in ["raw/1.md", "raw/2.md", "raw/3.md"] {
            assert!(c.body.contains(src), "来源标记不得丢失（{src}）: {}", c.body);
        }
        // 原始正文仍原样保留
        assert!(c.body.starts_with("第一版正文，必须原样保留。"));

        // 再追加多次：段数**有界**（在 1..=max 之间摆动，绝不无限增长）
        let mut prev = c;
        for i in 4..=12 {
            let next = merge_documents(
                &prev,
                &base,
                &format!("第{i}条补充"),
                &format!("raw/{i}.md"),
                &format!("2025-10-{i:02}T00:00:00Z"),
                limits,
            );
            let n = count_append_sections(&next.body);
            assert!(
                n <= limits.max_append_sections,
                "追加段数应被限制在 {} 以内，实际 {n}: {}",
                limits.max_append_sections,
                next.body
            );
            assert!(next.body.contains("第一条补充"), "早期内容必须始终保留");
            assert!(next.body.contains(&format!("第{i}条补充")), "新内容必须在");
            prev = next;
        }
    }

    /// 候选主题重叠过滤：只因共享项目名而命中的候选应被拦掉（过度合并的根因）。
    #[test]
    fn candidate_overlap_filter_blocks_project_name_only_matches() {
        let (dir, index) = tmp_index("overlapfilter");
        // 已有概念：**同样提到 agent-browser**（这样 BM25 一定会互相命中），但主题是「认证」
        index
            .upsert(
                &doc(
                    "concepts/agent-browser-auth",
                    "Fact",
                    "agent-browser 的认证方式",
                    "使用 JWT 令牌访问网关",
                    &["agent-browser", "认证"],
                    "认证通过 JWT 令牌完成",
                ),
                1,
            )
            .unwrap();
        index.commit().unwrap();

        // 新概念：同样提到 agent-browser，但主题是「空闲超时」→ 仅共享项目名
        let unrelated = doc(
            "concepts/agent-browser-timeout",
            "Fact",
            "agent-browser 的空闲超时",
            "守护进程空闲超时与序列化",
            &["agent-browser", "超时"],
            "空闲超时为 30 分钟",
        );
        let merger = Merger::new();

        // 不过滤 → BM25 会因共享「agent-browser」而命中（这就是过度合并的根源）
        let unfiltered = merger.candidates(&index, &unrelated, 8, 0.0).unwrap();
        assert!(
            unfiltered.iter().any(|c| c.concept_id == "concepts/agent-browser-auth"),
            "BM25 应因共享项目名而命中（证明过滤是必要的）: {unfiltered:?}"
        );
        // 加主题重叠下限 → 该候选被拦掉
        let blocked = merger.candidates(&index, &unrelated, 8, 0.25).unwrap();
        assert!(
            blocked.iter().all(|c| c.concept_id != "concepts/agent-browser-auth"),
            "仅共享项目名的候选应被重叠过滤拦掉: {blocked:?}"
        );

        // 真正相关的概念仍能通过
        let related = doc(
            "concepts/agent-browser-auth-2",
            "Fact",
            "agent-browser 的认证方式补充",
            "JWT 令牌访问网关",
            &["agent-browser", "认证"],
            "令牌可续期",
        );
        let kept = merger.candidates(&index, &related, 8, 0.25).unwrap();
        assert!(
            kept.iter().any(|c| c.concept_id == "concepts/agent-browser-auth"),
            "相关候选不应被拦: {kept:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：反复「汇总 + 追加」不得让 `### 补充` 标题层叠累积。
    #[test]
    fn repeated_consolidation_does_not_stack_headers() {
        let limits = ConceptLimits {
            max_tags: 0,
            max_append_sections: 1,
        };
        let base = existing_doc();
        let mut doc = base.clone();
        for i in 1..=8 {
            doc = merge_documents(
                &doc,
                &base,
                &format!("补充{i}"),
                &format!("raw/{i}.md"),
                &format!("2025-10-{i:02}T00:00:00Z"),
                limits,
            );
            let headers = doc
                .body
                .lines()
                .filter(|l| l.trim_start().starts_with("### 补充"))
                .count();
            assert!(headers <= 1, "第 {i} 轮出现 {headers} 个「补充」标题: {}", doc.body);
        }
        // 所有内容都在
        for i in 1..=8 {
            assert!(doc.body.contains(&format!("补充{i}")), "丢了补充{i}");
        }
    }

    #[test]
    fn append_section_counting_and_consolidation_are_stable() {
        assert_eq!(count_append_sections("普通正文"), 0);
        assert_eq!(count_append_sections("### 追加（2025-10-01）\n内容"), 1);
        assert_eq!(
            count_append_sections("### 追加（x）\nA\n### 补充（累计 2 条）\nB"),
            2
        );
        // 无来源标记时也会汇总（切分点含段标题），但不虚报条数
        assert_eq!(
            consolidate_appends("正文\n### 追加（x）\n内容"),
            "正文\n\n### 补充\n\n内容"
        );
        // 完全没有追加内容 → 原样返回
        assert_eq!(consolidate_appends("只有正文"), "只有正文");
    }

    /// sources 按 resource 去重，同资源保留 last_modified 最新者。
    #[test]
    fn merge_dedupes_sources_keeping_newest() {
        let mut incoming = doc("decisions/use-jwt", "Decision", "T", "D", &[], "");
        incoming.frontmatter.sources = Some(vec![
            SourceEntry {
                resource: "raw/a.md".into(),
                title: Some("A 新".into()),
                author: None,
                usage_count: None,
                last_modified: Some("2025-11-01T00:00:00Z".into()), // 更新
            },
            SourceEntry {
                resource: "raw/c.md".into(),
                title: Some("C".into()),
                author: None,
                usage_count: None,
                last_modified: Some("2025-10-12T00:00:00Z".into()),
            },
        ]);

        let merged = merge_documents(&existing_doc(), &incoming, "", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);
        let sources = merged.frontmatter.sources.as_ref().unwrap();

        assert_eq!(sources.len(), 3, "raw/a.md 应去重: {sources:?}");
        assert_eq!(sources.iter().map(|s| s.resource.as_str()).collect::<Vec<_>>(), vec!["raw/a.md", "raw/b.md", "raw/c.md"]);
        let a = sources.iter().find(|s| s.resource == "raw/a.md").unwrap();
        assert_eq!(a.last_modified.as_deref(), Some("2025-11-01T00:00:00Z"), "保留最新 last_modified");
        assert_eq!(a.title.as_deref(), Some("A"), "原标题保留（不覆盖）");
    }

    /// UPDATE 只在正文**追加**，绝不重写已有正文。
    #[test]
    fn merge_appends_body_without_rewriting() {
        let incoming = doc("decisions/use-jwt", "Decision", "新标题", "", &[], "这是新概念的正文");
        let merged = merge_documents(&existing_doc(), &incoming, "补充：支持多端无状态鉴权", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);

        assert!(
            merged.body.starts_with("第一版正文，必须原样保留。"),
            "已有正文必须原样保留: {}",
            merged.body
        );
        assert!(merged.body.contains("### 追加（2025-10-12）"), "{}", merged.body);
        assert!(merged.body.contains("补充：支持多端无状态鉴权"), "应追加 merge_notes");
        assert!(
            merged.body.contains("<!-- source: raw/c.md @ 2025-10-12T03:00:00Z -->"),
            "应带来源标记: {}",
            merged.body
        );
        assert!(!merged.body.contains("这是新概念的正文"), "有 merge_notes 时不应把整段正文塞进去");
    }

    /// 幂等守卫：同一段追加内容已存在 → 不再重复追加（`--force` 反复重跑不膨胀）。
    #[test]
    fn merge_does_not_duplicate_identical_addition() {
        let existing = existing_doc();
        let first = merge_documents(&existing, &existing, "补充：支持多端无状态鉴权", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);
        let second = merge_documents(&first, &existing, "补充：支持多端无状态鉴权", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);
        assert_eq!(second.body, first.body, "重复追加应被守卫拦住: {}", second.body);
        assert_eq!(second.body.matches("### 追加").count(), 1);
        assert_eq!(second.body.matches("支持多端无状态鉴权").count(), 1);

        // 新内容仍要追加
        let third = merge_documents(&first, &existing, "又一条新信息", "raw/d.md", "2025-10-13T03:00:00Z", NO_LIMITS);
        assert_eq!(third.body.matches("### 追加").count(), 2, "{}", third.body);
        assert!(third.body.contains("又一条新信息"));
    }

    #[test]
    fn merge_without_merge_notes_uses_incoming_body() {
        let incoming = doc("decisions/use-jwt", "Decision", "T", "", &[], "新概念正文");
        let merged = merge_documents(&existing_doc(), &incoming, "   ", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);
        assert!(merged.body.contains("第一版正文，必须原样保留。"));
        assert!(merged.body.contains("新概念正文"));
    }

    #[test]
    fn merge_with_empty_addition_keeps_existing_and_marks_source() {
        let incoming = doc("decisions/use-jwt", "Decision", "T", "", &[], "   ");
        let merged = merge_documents(&existing_doc(), &incoming, "", "raw/c.md", "2025-10-12T03:00:00Z", NO_LIMITS);
        assert_eq!(merged.body.matches("第一版正文").count(), 1);
        assert!(merged.body.contains("<!-- source: raw/c.md"));
        assert!(!merged.body.contains("### 追加"));
    }

    #[test]
    fn merge_is_idempotent_for_tags_and_sources() {
        let incoming = doc("decisions/use-jwt", "Decision", "旧标题", "旧描述", &["auth"], "");
        let once = merge_documents(&existing_doc(), &incoming, "", "raw/b.md", "2025-10-11T00:00:00Z", NO_LIMITS);
        // 再合并一次（同资源）不应产生重复条目
        let twice = merge_documents(&once, &incoming, "", "raw/b.md", "2025-10-11T00:00:00Z", NO_LIMITS);
        assert_eq!(twice.frontmatter.tags.len(), 2, "{:?}", twice.frontmatter.tags);
        assert_eq!(twice.frontmatter.sources.as_ref().unwrap().len(), 2);
    }

    /// 等价判定：内容一致且来源都已记录 → 等价（使 `--force` 重跑幂等）。
    #[test]
    fn equivalence_requires_content_match_and_known_sources() {
        let base = existing_doc();

        // 同一来源、同样内容 → 等价
        let mut same = existing_doc();
        same.frontmatter.generated = Some(GeneratedInfo {
            by: "astorm-memory/0.1.0".into(),
            at: "2025-10-20T00:00:00Z".into(),
            model: Some("别的模型".into()),
        });
        assert!(
            documents_equivalent(&base, &same),
            "generated 每次都会变，不应影响等价判定"
        );

        // 正文不同 → 不等价
        let mut diff_body = base.clone();
        diff_body.body = "改了正文".into();
        assert!(!documents_equivalent(&base, &diff_body));

        // 标题/标签不同 → 不等价
        let mut diff_title = base.clone();
        diff_title.frontmatter.title = Some("新标题".into());
        assert!(!documents_equivalent(&base, &diff_title));
        let mut diff_tags = base.clone();
        diff_tags.frontmatter.tags.push("新标签".into());
        assert!(!documents_equivalent(&base, &diff_tags));

        // 新来源未记录 → 不等价（provenance 需要累积）
        let mut new_source = base.clone();
        new_source.frontmatter.sources = Some(vec![SourceEntry {
            resource: "raw/别处.md".into(),
            title: None,
            author: None,
            usage_count: None,
            last_modified: None,
        }]);
        assert!(
            !documents_equivalent(&base, &new_source),
            "另一会话再次确认同一结论时应 UPDATE 以累积来源"
        );

        // 来源是已有来源的子集 → 等价（重跑同一会话）
        let mut subset = base.clone();
        subset.frontmatter.sources = Some(vec![SourceEntry {
            resource: "raw/a.md".into(),
            title: None,
            author: None,
            usage_count: None,
            last_modified: None,
        }]);
        assert!(documents_equivalent(&base, &subset));
    }

    #[test]
    fn overlay_scoring_ignores_unrelated_docs() {
        let a = doc("x", "Decision", "认证方案选型", "选择无状态令牌", &["auth"], "");
        let b = doc("y", "Decision", "构建工具选型", "包管理器", &["build"], "");
        let c = doc("z", "Decision", "认证方案选型补充", "无状态令牌方案", &["auth"], "");
        assert!(overlap_score(&a, &c) >= 0.25, "相关概念应达阈值");
        assert!(overlap_score(&a, &b) < 0.25, "不相关概念不应达阈值");
    }

    #[test]
    fn feature_tokenizer_handles_cjk_bigrams_and_ascii_words() {
        let tokens = tokenize_features("认证 JWT use-jwt");
        assert!(tokens.contains("认证"), "{tokens:?}");
        assert!(tokens.contains("jwt"));
        assert!(tokens.contains("use"));
        assert!(tokens.contains("认证") && tokens.len() > 3);
    }

    #[test]
    fn parse_decisions_tolerates_bare_array_and_junk() {
        let parsed = parse_decisions(&json!([{"existing_concept_id": "a", "action": "UPDATE"}]));
        assert_eq!(parsed.len(), 1);
        let parsed = parse_decisions(&json!({"decisions": [{"action": "UPDATE"}]}));
        assert!(parsed.is_empty(), "缺 existing_concept_id 应丢弃");
        assert!(parse_decisions(&json!({"nope": 1})).is_empty());
    }
}
