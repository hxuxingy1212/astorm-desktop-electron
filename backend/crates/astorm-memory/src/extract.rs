//! Extractor：LLM 从归一化会话 Markdown 中提取原子记忆单元。
//!
//! - 正文超 `raw_char_budget` 时**按消息边界分块**（`### [mNN]`）多次提取后合并，
//!   避免长会话被截断而丢记忆；单块仍超预算则按字符硬切。
//! - `observed_at` 由 `frag_ids` → 锚点时间戳**确定性推导**（不接受 LLM 自造时间）。
//! - 二遍自查（`extract_self_critique`，默认关）把已提取结果连同原文再喂一次补漏，成本约 2×。
//! - 去重按 `event_text` 归一化键（大小写/首尾空白不敏感）。

use std::collections::HashSet;

use tokio_util::sync::CancellationToken;

use super::llm::MemoryLlm;
use super::parser::{observed_at_from, RawSession};
use super::prompt;
use super::types::{CategoryTable, MemoryUnit};
use crate::config::MemoryConfig;

/// 提取阶段产出。
#[derive(Debug, Clone, Default)]
pub struct ExtractOutcome {
    pub units: Vec<MemoryUnit>,
    /// LLM 调用次数（分块 + 自查）。
    pub calls: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 被跳过的无效单元数（缺 event_text 等）。
    pub invalid_units: usize,
    /// 分块数（>1 表示发生了分块提取）。
    pub chunks: usize,
    /// 精选前的候选单元数（诊断过抽：真实测试中 2 个会话曾产出 126 条）。
    pub units_before_curation: usize,
    /// 因 `confidence` 低于阈值被丢弃的单元数。
    pub dropped_low_confidence: usize,
    /// 因 `max_units_per_session` 被截断到的上限（未触发为 None）。
    pub capped_to: Option<usize>,
}

/// 从单个会话的归一化结果中提取记忆单元。
pub async fn extract_session(
    llm: &dyn MemoryLlm,
    raw: &RawSession,
    categories: &CategoryTable,
    cfg: &MemoryConfig,
    cancel: &CancellationToken,
) -> Result<ExtractOutcome, String> {
    let system = prompt::extract_system(categories);
    let chunks = chunk_markdown(&raw.markdown, cfg.raw_char_budget);
    let mut out = ExtractOutcome {
        chunks: chunks.len(),
        ..Default::default()
    };
    let mut seen: HashSet<String> = HashSet::new();

    for (index, chunk) in chunks.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err("已取消".into());
        }
        let chunk_hint = (chunks.len() > 1).then_some((index + 1, chunks.len()));
        let user = prompt::extract_user(chunk, chunk_hint);
        let outcome = llm.complete_json(&system, &user, cancel.clone()).await?;
        out.calls += 1;
        out.prompt_tokens += u64::from(outcome.usage.prompt);
        out.completion_tokens += u64::from(outcome.usage.completion);

        let mut batch = parse_units(&outcome.value, &mut out.invalid_units);
        if cfg.extract_self_critique {
            let extra = self_critique(llm, &batch, chunk, &mut out, cancel).await?;
            batch.extend(extra);
        }
        for mut unit in batch {
            fill_observed_at(&mut unit, &raw.anchors);
            if seen.insert(unit.dedup_key()) {
                out.units.push(unit);
            }
        }
    }

    // ② 精选（去噪 + 合并同主题）→ 置信度过滤 → 产出上限
    out.units_before_curation = out.units.len();
    if cfg.curate && !out.units.is_empty() {
        let originals = std::mem::take(&mut out.units);
        let mut curated =
            curate_units(llm, &originals, raw, cfg.curate_units_per_call, &mut out, cancel).await?;
        carry_over_confidence(&mut curated, &originals);
        // 跨批归并：候选超过单批上限时首轮是**分批**精选的，跨批的同主题碎片不会被合并
        // （真实数据里同一会话因此对同一概念追加了 5 次）。此处再做**一次整体**归并。
        if originals.len() > cfg.curate_units_per_call && curated.len() > 1 {
            let pass2 = curate_units(llm, &curated, raw, curated.len(), &mut out, cancel).await?;
            let mut merged = pass2;
            carry_over_confidence(&mut merged, &originals);
            curated = merged;
        }
        let mut seen_again: HashSet<String> = HashSet::new();
        out.units = curated
            .into_iter()
            .filter(|u| seen_again.insert(u.dedup_key()))
            .collect();
    }
    if cfg.min_confidence > 0.0 {
        let before = out.units.len();
        out.units.retain(|u| u.confidence >= cfg.min_confidence);
        out.dropped_low_confidence = before - out.units.len();
    }
    if cfg.max_units_per_session > 0 && out.units.len() > cfg.max_units_per_session {
        // 超限时保留置信度最高的若干条
        out.units.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out.units.truncate(cfg.max_units_per_session);
        out.capped_to = Some(cfg.max_units_per_session);
    }

    Ok(out)
}

/// 精选一批候选单元（第二遍 LLM）：只做取舍与归并，不引入新信息。
async fn curate_units(
    llm: &dyn MemoryLlm,
    units: &[MemoryUnit],
    raw: &RawSession,
    batch_size: usize,
    out: &mut ExtractOutcome,
    cancel: &CancellationToken,
) -> Result<Vec<MemoryUnit>, String> {
    let context = session_context(raw);
    let system = prompt::curate_system();
    let batch = batch_size.max(1);
    let mut kept: Vec<MemoryUnit> = Vec::new();
    for chunk in units.chunks(batch) {
        if cancel.is_cancelled() {
            return Err("已取消".into());
        }
        let user = prompt::curate_user(&context, chunk);
        let outcome = llm.complete_json(&system, &user, cancel.clone()).await?;
        out.calls += 1;
        out.prompt_tokens += u64::from(outcome.usage.prompt);
        out.completion_tokens += u64::from(outcome.usage.completion);
        for mut unit in parse_units(&outcome.value, &mut out.invalid_units) {
            fill_observed_at(&mut unit, &raw.anchors);
            kept.push(unit);
        }
    }
    Ok(kept)
}

/// 精选结果继承了原单元的 `confidence`（按 `event_text` 归一化键匹配取较大者）。
///
/// 必要性：精选器若省略 `confidence`，反序列化会落到默认 0.5，会被 `min_confidence`
/// 阈值整体误杀。
fn carry_over_confidence(kept: &mut [MemoryUnit], original: &[MemoryUnit]) {
    let mut map: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for u in original {
        let key = u.dedup_key();
        let entry = map.entry(key).or_insert(u.confidence);
        *entry = entry.max(u.confidence);
    }
    for u in kept.iter_mut() {
        if let Some(c) = map.get(&u.dedup_key()) {
            u.confidence = u.confidence.max(*c);
        }
    }
}

/// 送给精选器的会话上下文摘要（raw 正文前 1200 字符；够判断主题，不必重发全文）。
fn session_context(raw: &RawSession) -> String {
    raw.markdown.chars().take(1_200).collect()
}

/// 二遍自查：把已提取单元连同原文再喂一次，合并补漏结果。
async fn self_critique(
    llm: &dyn MemoryLlm,
    extracted: &[MemoryUnit],
    chunk: &str,
    out: &mut ExtractOutcome,
    cancel: &CancellationToken,
) -> Result<Vec<MemoryUnit>, String> {
    let user = prompt::extract_self_check_user(extracted, chunk);
    let outcome = llm
        .complete_json(prompt::EXTRACT_SELF_CHECK_SYSTEM, &user, cancel.clone())
        .await?;
    out.calls += 1;
    out.prompt_tokens += u64::from(outcome.usage.prompt);
    out.completion_tokens += u64::from(outcome.usage.completion);
    Ok(parse_units(&outcome.value, &mut out.invalid_units))
}

/// 解析 `{"events": [...]}` 契约（容忍 LLM 直接给数组或字段名偏差）。
fn parse_units(value: &serde_json::Value, invalid: &mut usize) -> Vec<MemoryUnit> {
    let array = value
        .get("events")
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());
    let Some(array) = array else {
        return Vec::new();
    };
    let mut units = Vec::new();
    for item in array {
        match serde_json::from_value::<MemoryUnit>(item.clone()) {
            Ok(unit) if !unit.event_text.trim().is_empty() => units.push(unit),
            _ => *invalid += 1,
        }
    }
    units
}

/// 用锚点时间戳填充 `observed_at`（仅在 LLM 未提供时；实际上 LLM 的值总被 `skip_deserializing` 丢弃）。
fn fill_observed_at(unit: &mut MemoryUnit, anchors: &std::collections::BTreeMap<String, i64>) {
    if unit.observed_at.is_none() {
        unit.observed_at = observed_at_from(anchors, &unit.frag_ids);
    }
}

/// 按字符预算分块：优先在消息边界（`### [mNN]`）切分；单块仍超预算则按字符硬切。
pub fn chunk_markdown(markdown: &str, budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    if markdown.chars().count() <= budget {
        return vec![markdown.to_string()];
    }

    // ① 以消息块边界切分（保留 frontmatter 与头部块到第一段）
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in markdown.split_inclusive('\n') {
        if line.starts_with("### [") && !current.is_empty() {
            segments.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        segments.push(current);
    }

    // ② 组装到预算内；单个段超预算则硬切
    let mut chunks: Vec<String> = Vec::new();
    let mut buffer = String::new();
    let flush = |buffer: &mut String, chunks: &mut Vec<String>| {
        if !buffer.trim().is_empty() {
            chunks.push(std::mem::take(buffer));
        } else {
            buffer.clear();
        }
    };
    for segment in segments {
        if segment.chars().count() > budget {
            flush(&mut buffer, &mut chunks);
            for part in hard_split(&segment, budget) {
                chunks.push(part);
            }
            continue;
        }
        if buffer.chars().count() + segment.chars().count() > budget {
            flush(&mut buffer, &mut chunks);
        }
        buffer.push_str(&segment);
    }
    flush(&mut buffer, &mut chunks);
    if chunks.is_empty() {
        vec![markdown.to_string()]
    } else {
        chunks
    }
}

/// 按字符硬切（UTF-8 边界安全）。
fn hard_split(text: &str, budget: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= budget {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{PromptStage, ScriptedLlm};
    use crate::parser::{self, SessionTranscript, TranscriptMessage};
    use serde_json::json;

    fn table() -> CategoryTable {
        CategoryTable::new(MemoryConfig::default().categories)
    }

    /// 提取**机制**类用例的配置：关掉精选 / 置信度阈值 / 产出上限，
    /// 使断言只反映提取本身的行为（这三项另有专测）。
    fn mech_cfg() -> MemoryConfig {
        MemoryConfig {
            curate: false,
            min_confidence: 0.0,
            max_units_per_session: 0,
            ..MemoryConfig::default()
        }
    }

    fn transcript_with(n: usize, body_len: usize) -> SessionTranscript {
        let mut messages = Vec::new();
        for i in 0..n {
            messages.push(TranscriptMessage {
                id: format!("id-{i}"),
                session_id: "s1".into(),
                role: if i % 2 == 0 { "user".into() } else { "assistant".into() },
                content: format!("消息{i}") + &"补".repeat(body_len),
                tool_calls: serde_json::Value::Null,
                seq: i as i64,
                created_at: 1_760_000_100 + i as i64 * 60,
                tool_call_id: None,
                metadata: None,
            });
        }
        SessionTranscript::new("s1", messages)
    }

    fn raw_of(n: usize, body_len: usize) -> (parser::RawSession, SessionTranscript) {
        let t = transcript_with(n, body_len);
        let jsonl = serde_json::to_string(&t.messages).unwrap();
        let raw = parser::normalize(&t, &jsonl, 200).unwrap();
        (raw, t)
    }

    #[tokio::test]
    async fn extracts_units_and_fills_observed_at_from_anchors() {
        let (raw, _) = raw_of(3, 10);
        let llm = ScriptedLlm::new().with(
            PromptStage::Extract,
            json!({"events": [
                {"event_text": "选了 JWT", "frag_ids": ["m01"], "event_type": "decision", "confidence": 0.9},
                {"event_text": "偏好中文注释", "frag_ids": ["m02"], "event_type": "preference", "confidence": 0.8}
            ]}),
        );
        let out = extract_session(&llm, &raw, &table(), &mech_cfg(), &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(out.units.len(), 2);
        assert_eq!(out.calls, 1);
        assert_eq!(out.chunks, 1);
        // observed_at 由锚点时间戳推导（m01 → 2025-10-09）
        assert_eq!(out.units[0].observed_at.as_deref(), Some("2025-10-09"));
        assert_eq!(out.units[0].event_type, "decision");
        assert_eq!(out.units[1].event_type, "preference");
        assert!(out.prompt_tokens > 0);
    }

    #[tokio::test]
    async fn dedups_identical_claims_and_skips_invalid() {
        let (raw, _) = raw_of(2, 5);
        let llm = ScriptedLlm::new().with(
            PromptStage::Extract,
            json!({"events": [
                {"event_text": "选了 JWT", "event_type": "decision"},
                {"event_text": "  选了 JWT  ", "event_type": "decision"},
                {"event_text": "", "event_type": "fact"},
                {"event_type": "fact"}
            ]}),
        );
        let out = extract_session(&llm, &raw, &table(), &mech_cfg(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.units.len(), 1, "同一声明（去空白/大小写）应去重: {:?}", out.units);
        assert_eq!(out.invalid_units, 2, "缺 event_text 的应计入无效");
        assert_eq!(out.units[0].confidence, 0.5, "缺 confidence 走默认");
    }

    /// 超过字符预算 → 按消息边界分块，多次调用后合并。
    #[tokio::test]
    async fn chunks_long_sessions_by_message_boundary() {
        let (raw, _) = raw_of(12, 400); // 每条约 400+ 字符
        assert!(raw.markdown.chars().count() > 1_000);

        let llm = ScriptedLlm::new().with(
            PromptStage::Extract,
            json!({"events": [{"event_text": "单元", "event_type": "fact"}]}),
        );
        let cfg = MemoryConfig {
            raw_char_budget: 900,
            ..mech_cfg()
        };
        let out = extract_session(&llm, &raw, &table(), &cfg, &CancellationToken::new())
            .await
            .unwrap();

        assert!(out.chunks > 1, "应发生分块，实际 {}", out.chunks);
        assert_eq!(out.calls, out.chunks, "每块一次调用");
        // 相同 event_text 跨块去重
        assert_eq!(out.units.len(), 1, "跨块重复声明应去重");
        // 分块提示词应标明位置
        let prompts = llm.prompts();
        assert!(prompts[1].user.contains("part 2 of"), "{}", prompts[1].user);
    }

    #[tokio::test]
    async fn self_critique_runs_second_pass_and_merges() {
        let (raw, _) = raw_of(3, 10);
        let llm = ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [{"event_text": "第一条", "event_type": "fact"}]}),
            )
            .with(
                PromptStage::ExtractSelfCheck,
                json!({"events": [
                    {"event_text": "遗漏的第二条", "event_type": "fact"},
                    {"event_text": "第一条", "event_type": "fact"}
                ]}),
            );
        let cfg = MemoryConfig {
            extract_self_critique: true,
            ..mech_cfg()
        };
        let out = extract_session(&llm, &raw, &table(), &cfg, &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(out.calls, 2, "应发生两遍调用");
        assert_eq!(out.units.len(), 2, "补漏结果应合并且去重: {:?}", out.units);
        assert!(out.units.iter().any(|u| u.event_text == "遗漏的第二条"));
        assert_eq!(llm.calls(PromptStage::ExtractSelfCheck), 1);
    }

    #[tokio::test]
    async fn self_critique_disabled_by_default() {
        let (raw, _) = raw_of(2, 5);
        let llm = ScriptedLlm::new()
            .with(PromptStage::Extract, json!({"events": []}))
            .with(PromptStage::ExtractSelfCheck, json!({"events": []}));
        let out = extract_session(&llm, &raw, &table(), &mech_cfg(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(llm.calls(PromptStage::ExtractSelfCheck), 0, "默认不应自查");
        assert_eq!(out.calls, 1);
    }

    #[tokio::test]
    async fn tolerates_missing_events_key_and_llm_error() {
        let (raw, _) = raw_of(2, 5);
        // 缺 events 键 → 空结果，不报错
        let llm = ScriptedLlm::new().with(PromptStage::Extract, json!({"wrong": []}));
        let out = extract_session(&llm, &raw, &table(), &mech_cfg(), &CancellationToken::new())
            .await
            .unwrap();
        assert!(out.units.is_empty());

        // 未预置响应（严格模式）→ 报错上抛，由管道计入失败会话
        let strict = ScriptedLlm::new();
        let err = extract_session(&strict, &raw, &table(), &mech_cfg(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.contains("Extract"), "{err}");
    }

    #[tokio::test]
    async fn cancelled_token_aborts_before_second_chunk() {
        let (raw, _) = raw_of(12, 400);
        let llm = ScriptedLlm::new().with(PromptStage::Extract, json!({"events": []}));
        let cfg = MemoryConfig {
            raw_char_budget: 900,
            ..mech_cfg()
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = extract_session(&llm, &raw, &table(), &cfg, &cancel).await.unwrap_err();
        assert_eq!(err, "已取消");
        assert_eq!(llm.calls(PromptStage::Extract), 0, "取消后不应发起调用");
    }

    fn cfg_with(curate: bool, min_conf: f32, cap: usize) -> MemoryConfig {
        MemoryConfig {
            curate,
            min_confidence: min_conf,
            max_units_per_session: cap,
            ..MemoryConfig::default()
        }
    }

    /// 精选：以「保留集」**替换**候选集（不是追加）。
    #[tokio::test]
    async fn curation_replaces_candidates_with_kept_set() {
        let (raw, _) = raw_of(3, 10);
        let llm = ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [
                    {"event_text": "第三方库的 bin 入口", "event_type": "fact", "confidence": 0.9},
                    {"event_text": "第三方库的 LICENSE 是 MIT", "event_type": "fact", "confidence": 0.9},
                    {"event_text": "认证方案选 JWT 而非 session", "event_type": "decision", "confidence": 0.9}
                ]}),
            )
            .with(
                PromptStage::Curate,
                json!({"events": [
                    {"event_text": "认证方案选 JWT 而非 session", "event_type": "decision", "confidence": 0.95}
                ]}),
            );
        let out = extract_session(&llm, &raw, &table(), &cfg_with(true, 0.0, 0), &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(out.units_before_curation, 3, "应记录精选前数量");
        assert_eq!(out.units.len(), 1, "应只保留精选结果: {:?}", out.units);
        assert!(out.units[0].event_text.contains("JWT"));
        assert_eq!(llm.calls(PromptStage::Curate), 1);
    }

    /// 精选器省略 confidence 时，必须继承原单元的置信度（否则会被阈值整体误杀）。
    #[tokio::test]
    async fn curation_preserves_original_confidence_when_omitted() {
        let (raw, _) = raw_of(2, 5);
        let llm = ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [{"event_text": "认证方案选 JWT", "event_type": "decision", "confidence": 0.88}]}),
            )
            .with(
                PromptStage::Curate,
                // 故意不返回 confidence
                json!({"events": [{"event_text": "认证方案选 JWT", "event_type": "decision"}]}),
            );
        let out = extract_session(&llm, &raw, &table(), &cfg_with(true, 0.6, 0), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.units.len(), 1, "不应被阈值误杀: {:?}", out.units);
        assert!((out.units[0].confidence - 0.88).abs() < 1e-6, "应继承 0.88");
        assert_eq!(out.dropped_low_confidence, 0);
    }

    #[tokio::test]
    async fn low_confidence_units_are_dropped() {
        let (raw, _) = raw_of(3, 10);
        let llm = ScriptedLlm::new().with(
            PromptStage::Extract,
            json!({"events": [
                {"event_text": "高置信", "event_type": "fact", "confidence": 0.9},
                {"event_text": "低置信", "event_type": "fact", "confidence": 0.2}
            ]}),
        );
        let out = extract_session(&llm, &raw, &table(), &cfg_with(false, 0.6, 0), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.units.len(), 1);
        assert_eq!(out.units[0].event_text, "高置信");
        assert_eq!(out.dropped_low_confidence, 1);
    }

    /// 产出上限：超限时保留置信度最高的若干条。
    #[tokio::test]
    async fn session_unit_cap_keeps_highest_confidence() {
        let (raw, _) = raw_of(3, 10);
        let llm = ScriptedLlm::new().with(
            PromptStage::Extract,
            json!({"events": [
                {"event_text": "低", "event_type": "fact", "confidence": 0.3},
                {"event_text": "中", "event_type": "fact", "confidence": 0.6},
                {"event_text": "高", "event_type": "fact", "confidence": 0.95}
            ]}),
        );
        let out = extract_session(&llm, &raw, &table(), &cfg_with(false, 0.0, 2), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.capped_to, Some(2));
        assert_eq!(out.units.len(), 2);
        let texts: Vec<&str> = out.units.iter().map(|u| u.event_text.as_str()).collect();
        assert!(texts.contains(&"高") && texts.contains(&"中"), "{texts:?}");
        assert!(!texts.contains(&"低"), "应淘汰最低置信度: {texts:?}");
    }

    /// 精选默认开启；显式关闭时不应发起第二遍调用。
    #[tokio::test]
    async fn curation_can_be_disabled() {
        assert!(MemoryConfig::default().curate, "精选应默认开启");
        let (raw, _) = raw_of(2, 5);
        let llm = ScriptedLlm::new().with(
            PromptStage::Extract,
            json!({"events": [{"event_text": "单元", "event_type": "fact", "confidence": 0.9}]}),
        );
        let out = extract_session(&llm, &raw, &table(), &cfg_with(false, 0.0, 0), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(llm.calls(PromptStage::Curate), 0, "关闭后不应精选");
        assert_eq!(out.units_before_curation, 1);
        assert_eq!(out.units.len(), 1);
    }

    /// 精选器返回空数组（会话无耐久内容）→ 最终为空，不报错。
    #[tokio::test]
    async fn curation_can_return_empty_for_contentless_sessions() {
        let (raw, _) = raw_of(2, 5);
        let llm = ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [{"event_text": "寒暄", "event_type": "other", "confidence": 0.9}]}),
            )
            .with(PromptStage::Curate, json!({"events": []}));
        let out = extract_session(&llm, &raw, &table(), &cfg_with(true, 0.0, 0), &CancellationToken::new())
            .await
            .unwrap();
        assert!(out.units.is_empty());
        assert_eq!(out.units_before_curation, 1);
        assert!(!out.capped_to.is_some());
    }

    #[test]
    fn chunking_keeps_short_input_whole() {
        let chunks = chunk_markdown("短内容", 1_000);
        assert_eq!(chunks, vec!["短内容"]);
    }

    #[test]
    fn chunking_splits_at_message_boundaries() {
        let md = format!(
            "---\nfrontmatter\n---\n\n# 对话\n\n{}",
            (1..=6)
                .map(|i| format!("### [m{i:02}] user\n\n{}\n", "内容".repeat(50)))
                .collect::<String>()
        );
        let chunks = chunk_markdown(&md, 300);
        assert!(chunks.len() > 1, "{}", chunks.len());
        // 每个分块都不超预算（允许段边界带来的小幅超出由硬切兜底，这里应严格不超）
        for chunk in &chunks {
            assert!(
                chunk.chars().count() <= 300,
                "分块超预算: {} 字符",
                chunk.chars().count()
            );
        }
        // 所有消息块都在某个分块里出现过
        for i in 1..=6 {
            let anchor = format!("[m{i:02}]");
            assert!(chunks.iter().any(|c| c.contains(&anchor)), "丢消息 {anchor}");
        }
    }

    #[test]
    fn chunking_hard_splits_oversized_single_message() {
        let md = format!("### [m01] user\n\n{}", "字".repeat(1_000));
        let chunks = chunk_markdown(&md, 100);
        assert!(chunks.len() >= 10, "{}", chunks.len());
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 100);
        }
        // 内容不丢失（拼接后字符数与原文一致）
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert_eq!(total, md.chars().count());
    }

    #[test]
    fn hard_split_is_utf8_safe() {
        let parts = hard_split("中文测试", 2);
        assert_eq!(parts, vec!["中文", "测试"]);
    }
}
