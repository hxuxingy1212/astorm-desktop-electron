//! 管道编排：把五个阶段串成一次可取消、可增量、可报告的编译。
//!
//! ```text
//! 会话 JSONL
//!   → [Parser]    归一化为 Markdown 写入 raw/sessions/
//!   → [Extractor] LLM 提取原子记忆
//!   → [Compiler]  LLM 组装为 OKF 概念
//!   → [Merger]    BM25 候选召回 + LLM 判定 → CREATE/UPDATE/SKIP
//!   → [Writer]    原子落盘 + 重建 index.md + 追加 log.md
//! ```
//!
//! 不变量：
//! - **增量**：状态（`.okfstate.json`）指纹未变的会话直接跳过（size/mtime 快路径，不算哈希）
//! - **原子**：文件先写临时文件再 rename；索引在**末尾 commit 一次**；异常/取消 `rollback`
//! - **顺序**：先写文件成功、再更新索引（索引是可重建的派生数据，wiki 才是真相）
//! - **可取消**：任一阶段响应 `CancellationToken`；已落盘内容保留，报告标记 `cancelled`
//! - **部分失败不中断**：单会话提取/编译失败只计入 `failed_sessions`，其余会话照常处理

use std::collections::HashMap;

use tokio_util::sync::CancellationToken;

use super::compile::{self, CompileSource};
use super::concept::{dedupe_concept_id, ConceptDocument};
use super::extract;
use super::index::MemoryIndex;
use super::llm::MemoryLlm;
use super::merge::{hit_overlap_score, merge_documents, ConceptLimits, MergeAction, Merger};
use super::parser;
use super::paths::BundlePaths;
use super::state::{FileRecord, SourceStamp, SyncState, Incremental};
use super::types::{
    CandidateHit,
    CategoryTable, CompileReport, LogEntry, ProgressEvent, ProgressStage, SessionFailure,
};
use super::writer;
use crate::config::MemoryConfig;

/// 一个待编译的源会话。
#[derive(Debug, Clone)]
pub struct SessionInput {
    pub session_id: String,
    /// 会话标题（写入 raw frontmatter 与概念 `sources.title`）。
    pub title: Option<String>,
    /// 会话活动日志 `message.jsonl` 的原文。
    pub jsonl: String,
    /// 源文件指纹（增量快路径；调用方按 `message.jsonl` 取）。
    pub stamp: SourceStamp,
}

impl SessionInput {
    /// 由 `session.jsonl` 原文构造（指纹从内容取样，调用方若已知文件元数据应改用 [`Self::with_stamp`]）。
    pub fn new(session_id: impl Into<String>, jsonl: impl Into<String>) -> Self {
        let jsonl = jsonl.into();
        let stamp = SourceStamp::new(jsonl.len() as u64, 0);
        Self {
            session_id: session_id.into(),
            title: None,
            jsonl,
            stamp,
        }
    }

    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    pub fn with_stamp(mut self, stamp: SourceStamp) -> Self {
        self.stamp = stamp;
        self
    }
}

/// 编译选项。
#[derive(Debug, Clone, Default)]
pub struct CompileOptions {
    /// 忽略增量状态，强制重编译所有会话。
    pub force: bool,
    /// 只做确定性阶段（不调用 LLM）；配合 [`super::llm::DryRunLlm`] 用于审阅提示词。
    pub dry_run: bool,
}

/// 进度回调（阶段 / 当前 / 总数 / 说明）。
pub type ProgressFn<'a> = &'a (dyn Fn(ProgressEvent) + Send + Sync);

/// 空进度回调（测试与库调用省事）。
pub fn no_progress(_: ProgressEvent) {}

/// 执行一次编译（增量；`options.force` 则全量）。
#[allow(clippy::too_many_arguments)]
pub async fn compile_bundle(
    paths: &BundlePaths,
    index: &MemoryIndex,
    llm: &dyn MemoryLlm,
    cfg: &MemoryConfig,
    categories: &CategoryTable,
    sessions: &[SessionInput],
    options: &CompileOptions,
    progress: ProgressFn<'_>,
    cancel: &CancellationToken,
) -> Result<CompileReport, String> {
    paths.ensure_dirs(&categories.dirs())?;

    let mut report = CompileReport {
        total_sessions: sessions.len(),
        ..Default::default()
    };
    let mut state = SyncState::load(&paths.state_file());
    let mut merger = Merger::new();
    let total = sessions.len();

    // ① 解析阶段：归一化 + 增量筛选
    let mut to_process: Vec<(usize, parser::RawSession, String)> = Vec::new();
    for (idx, session) in sessions.iter().enumerate() {
        progress(ProgressEvent::new(
            ProgressStage::Parse,
            idx + 1,
            total,
            format!("归一化会话 {}", short(&session.session_id)),
        ));
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        // 跳过条件必须同时满足「指纹未变」**与**「确实编译过」：
        // 否则「先 parse 再 compile」会把所有会话跳过，一个概念都产不出来。
        if !options.force
            && state.check(&session.session_id, session.stamp) == Incremental::Skip
            && state.is_compiled(&session.session_id)
        {
            report.skipped_sessions += 1;
            continue;
        }
        let sha = parser::sha256_hex(session.jsonl.as_bytes());
        // 慢路径同样要求「已编译」：否则仅 parse 过的会话会被哈希判定跳过（见 needs_compile）
        if !options.force && state.matches_hash(&session.session_id, &sha) && state.is_compiled(&session.session_id)
        {
            report.skipped_sessions += 1;
            continue;
        }

        let parsed = parser::parse_jsonl(&session.jsonl);
        if parsed.bad_lines > 0 {
            report.warnings.push(format!(
                "会话 {} 有 {} 行无法解析（已跳过）",
                short(&session.session_id),
                parsed.bad_lines
            ));
        }
        let transcript = parser::SessionTranscript {
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            messages: parsed.messages,
        };
        match parser::normalize(&transcript, &session.jsonl, cfg.tool_result_chars) {
            Some(raw) => to_process.push((idx, raw, sha)),
            None => {
                // 无对话内容（只有 system / 空会话）：记录状态避免每次重扫
                report.skipped_sessions += 1;
                state.record(empty_record(session, &sha));
            }
        }
    }

    if report.cancelled {
        state.save(&paths.state_file())?;
        return Ok(report);
    }

    // ② 提取 / 编译 / 合并 / 写入
    for (done, (_, raw, sha)) in to_process.iter().enumerate() {
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let session_id = raw.session_id.clone();
        let resource = format!("raw/sessions/{}.md", raw.stem);

        // 写入 raw（不可变来源层；同一会话重复处理会覆盖为最新归一化结果）
        writer::atomic_write(&paths.raw_file(&raw.stem), &raw.markdown)?;

        progress(ProgressEvent::new(
            ProgressStage::Extract,
            done + 1,
            to_process.len(),
            format!("提取记忆 {}", short(&session_id)),
        ));
        let extracted = match extract::extract_session(llm, raw, categories, cfg, cancel).await {
            Ok(outcome) => outcome,
            Err(e) => {
                if cancel.is_cancelled() {
                    report.cancelled = true;
                    break;
                }
                // 单会话失败不中断整体；不记录状态，下次可重试
                report.failed_sessions.push(SessionFailure {
                    session_id: session_id.clone(),
                    stage: "extract".into(),
                    message: e,
                });
                continue;
            }
        };
        // 精选效果诊断：候选 → 精选后 的收敛比（真实数据上从 ~63 条/会话降到 ~5 条）
        progress(ProgressEvent::new(
            ProgressStage::Extract,
            done + 1,
            to_process.len(),
            format!(
                "提取完成：候选 {} → 精选 {} 条（去低置信 {}，分块 {}）",
                extracted.units_before_curation,
                extracted.units.len(),
                extracted.dropped_low_confidence,
                extracted.chunks
            ),
        ));
        report.extracted_units += extracted.units.len();
        report.calls += extracted.calls;
        report.prompt_tokens += extracted.prompt_tokens;
        report.completion_tokens += extracted.completion_tokens;

        progress(ProgressEvent::new(
            ProgressStage::Compile,
            done + 1,
            to_process.len(),
            format!("编译概念（{} 个记忆单元）", extracted.units.len()),
        ));
        let source = CompileSource {
            resource: resource.clone(),
            title: raw_title(raw),
            anchors: raw.anchors.clone(),
            last_modified: raw
                .time_range
                .and_then(|(_, max)| parser::iso_ts(max))
                .or_else(|| Some(parser::now_rfc3339())),
        };
        let compiled = match compile::compile_units(llm, &extracted.units, categories, &source, cfg, cancel).await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                if cancel.is_cancelled() {
                    report.cancelled = true;
                    break;
                }
                report.failed_sessions.push(SessionFailure {
                    session_id: session_id.clone(),
                    stage: "compile".into(),
                    message: e,
                });
                continue;
            }
        };
        report.compiled_concepts += compiled.documents.len();
        report.calls += compiled.calls;
        report.prompt_tokens += compiled.prompt_tokens;
        report.completion_tokens += compiled.completion_tokens;
        report.warnings.extend(compiled.warnings.clone());
        report.warnings.extend(compiled.invalid.iter().map(|e| format!("无效产出: {e}")));

        // 合并 + 落盘
        let mut created_here: Vec<String> = Vec::new();
        let mut updated_here: Vec<String> = Vec::new();
        let mut logs: Vec<LogEntry> = Vec::new();
        let at = parser::now_rfc3339();

        for doc in compiled.documents {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            progress(ProgressEvent::new(
                ProgressStage::Merge,
                done + 1,
                to_process.len(),
                format!("合并 {}", doc.concept_id),
            ));
            let load_existing = |id: &str| load_for_update(paths, &merger, id).ok().flatten();
            let decision = match merger.decide(llm, index, &doc, cfg, cancel, &load_existing).await {
                Ok(decision) => decision,
                Err(e) => {
                    report.warnings.push(format!("概念 {} 合并失败: {e}", doc.concept_id));
                    continue;
                }
            };
            report.warnings.extend(decision.warnings.clone());
            report.prompt_tokens += decision.prompt_tokens;
            report.completion_tokens += decision.completion_tokens;

            match decision.action {
                MergeAction::Skip => {
                    report.skipped.push(doc.concept_id.clone());
                    logs.push(log_entry(&at, "SKIP", &doc.concept_id, &resource, &decision.reason));
                }
                MergeAction::Create => {
                    // 概念 ID 冲突避让：磁盘 / 索引 / 本批次已产出都要看
                    let exists = |id: &str| {
                        merger.overlay_get(id).is_some()
                            || paths.concept_path(id).exists()
                            || index.get(id).ok().flatten().is_some()
                    };
                    let final_id = dedupe_concept_id(&doc.concept_id, exists);
                    let mut doc = doc;
                    doc.concept_id = final_id;
                    writer::write_concept(paths, &doc)?;
                    index.upsert(&doc, parser::now_secs())?;
                    merger.track(doc.clone());
                    report.created.push(doc.concept_id.clone());
                    created_here.push(doc.concept_id.clone());
                    logs.push(log_entry(&at, "CREATE", &doc.concept_id, &resource, &decision.reason));
                }
                MergeAction::Update => {
                    let target = decision
                        .target
                        .clone()
                        .unwrap_or_else(|| doc.concept_id.clone());
                    let existing = load_for_update(paths, &merger, &target)?;
                    let merged = match existing {
                        Some(existing) => {
                            merge_documents(&existing, &doc, &decision.merge_notes, &resource, &at, ConceptLimits::from_config(cfg))
                        }
                        // 目标文件缺失（索引与磁盘不一致）→ 降级为新建，避免丢内容
                        None => {
                            report.warnings.push(format!(
                                "合并目标 {target} 在磁盘上不存在，已改为新建 {}",
                                doc.concept_id
                            ));
                            doc.clone()
                        }
                    };
                    writer::write_concept(paths, &merged)?;
                    index.upsert(&merged, parser::now_secs())?;
                    merger.track(merged.clone());
                    report.updated.push(merged.concept_id.clone());
                    updated_here.push(merged.concept_id.clone());
                    logs.push(log_entry(
                        &at,
                        "UPDATE",
                        &merged.concept_id,
                        &resource,
                        &decision.reason,
                    ));
                }
            }
        }

        // 写入阶段：重建索引 + 追加变更历史（每个会话处理完落一次，崩溃不丢已记录内容）
        progress(ProgressEvent::new(
            ProgressStage::Write,
            done + 1,
            to_process.len(),
            "重建索引与变更历史".to_string(),
        ));
        writer::append_log(paths, &logs)?;
        writer::rebuild_indexes(paths)?;

        state.record(FileRecord {
            session_id: session_id.clone(),
            sha256: sha.clone(),
            size: sessions
                .iter()
                .find(|s| s.session_id == session_id)
                .map(|s| s.stamp.size)
                .unwrap_or(0),
            mtime: sessions
                .iter()
                .find(|s| s.session_id == session_id)
                .map(|s| s.stamp.mtime)
                .unwrap_or(0),
            processed_at: at.clone(),
            compiled_at: Some(at.clone()),
            concepts_created: created_here,
            concepts_updated: updated_here,
        });
        report.processed_sessions += 1;
    }

    // ③ 收尾：
    //    1) 重建本轮写入过概念的「相关」小节（**只能链接索引里已有的概念** ⇒ 永不悬空）；
    //    2) 再把编译器自造的悬空相对链接降级为纯文本。
    //    顺序不能反：先有「相关」小节，降级才能一并检查它们（它们本就指向真实文件，会原样保留）。
    if !report.cancelled {
        let touched: Vec<String> = report
            .created
            .iter()
            .chain(report.updated.iter())
            .cloned()
            .collect();
        if !touched.is_empty() && cfg.related_concepts > 0 {
            // 成功不写 warning（那是正常产出，不是问题）；失败才提示且不影响已落盘内容
            if let Err(e) = refresh_related_sections(paths, index, &merger, &touched, cfg.related_concepts) {
                report
                    .warnings
                    .push(format!("「相关」小节生成失败（不影响已落盘内容）: {e}"));
            }
        }
        match writer::repair_relative_links(paths) {
            Ok((_, 0)) => {}
            Ok((_, n)) => report
                .warnings
                .push(format!("悬空相对链接已降级为纯文本: {n} 处")),
            Err(e) => report.warnings.push(format!("链接修复失败（不影响已落盘内容）: {e}")),
        }
    }

    if cancel.is_cancelled() {
        report.cancelled = true;
        let _ = index.rollback();
    } else if let Err(e) = index.commit() {
        report.warnings.push(format!("索引提交失败（可稍后重建）: {e}"));
    }
    if report.failed_sessions.is_empty() {
        state.last_sync = Some(parser::now_rfc3339());
    }
    state.save(&paths.state_file())?;

    progress(ProgressEvent::new(
        ProgressStage::Write,
        total,
        total,
        if report.cancelled {
            "已取消".to_string()
        } else {
            format!(
                "完成：新建 {} / 更新 {} / 跳过 {}",
                report.created.len(),
                report.updated.len(),
                report.skipped.len()
            )
        },
    ));
    Ok(report)
}

/// 仅归一化（不调用 LLM）：把会话写入 `raw/sessions/`。
///
/// 对应方案文档的 `parse` 子命令；返回 `(处理会话数, 跳过会话数, 告警)`。
pub fn parse_sessions(
    paths: &BundlePaths,
    cfg: &MemoryConfig,
    sessions: &[SessionInput],
    force: bool,
) -> Result<(usize, usize, Vec<String>), String> {
    paths.ensure_dirs(&[])?;
    let mut state = SyncState::load(&paths.state_file());
    let (mut processed, mut skipped) = (0usize, 0usize);
    let mut warnings = Vec::new();

    for session in sessions {
        if !force && state.check(&session.session_id, session.stamp) == Incremental::Skip {
            skipped += 1;
            continue;
        }
        let sha = parser::sha256_hex(session.jsonl.as_bytes());
        if !force && state.matches_hash(&session.session_id, &sha) {
            skipped += 1;
            continue;
        }
        let parsed = parser::parse_jsonl(&session.jsonl);
        if parsed.bad_lines > 0 {
            warnings.push(format!(
                "会话 {} 有 {} 行无法解析（已跳过）",
                short(&session.session_id),
                parsed.bad_lines
            ));
        }
        let transcript = parser::SessionTranscript {
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            messages: parsed.messages,
        };
        match parser::normalize(&transcript, &session.jsonl, cfg.tool_result_chars) {
            Some(raw) => {
                writer::atomic_write(&paths.raw_file(&raw.stem), &raw.markdown)?;
                state.record(FileRecord {
                    session_id: session.session_id.clone(),
                    sha256: sha.clone(),
                    size: session.stamp.size,
                    mtime: session.stamp.mtime,
                    processed_at: parser::now_rfc3339(),
                    compiled_at: None, // 只归一化，未编译
                    concepts_created: Vec::new(),
                    concepts_updated: Vec::new(),
                });
                processed += 1;
            }
            None => {
                skipped += 1;
                state.record(empty_record(session, &sha));
            }
        }
    }
    state.save(&paths.state_file())?;
    Ok((processed, skipped, warnings))
}

/// 加载合并目标（优先内存 overlay，其次磁盘）。
fn load_for_update(
    paths: &BundlePaths,
    merger: &Merger,
    concept_id: &str,
) -> Result<Option<ConceptDocument>, String> {
    if let Some(doc) = merger.overlay_get(concept_id) {
        return Ok(Some(doc.clone()));
    }
    let path = paths.concept_path(concept_id);
    match std::fs::read_to_string(&path) {
        Ok(content) => ConceptDocument::from_markdown(concept_id, &content).map(Some),
        Err(_) => Ok(None),
    }
}

/// raw 的标题（写入来源 `sources.title`）。
fn raw_title(raw: &parser::RawSession) -> Option<String> {
    // frontmatter 里可能没有 title；用文件名主干兜底
    Some(raw.stem.clone()).filter(|s| !s.is_empty())
}

/* ---------- 「相关」小节重建 ---------- */

/// 「相关」链接的主题重叠下限（比合并下限 `candidate_min_overlap` 宽松）。
///
/// 合并要求「可并入同一概念」（0.25），链接只要求「值得互看」。但门槛不能是 0：
/// 候选池非空时 BM25 总会返回点什么，0 门槛会把无关概念也链上（见 `refresh_related_sections`）。
const RELATED_MIN_OVERLAP: f32 = 0.1;

/// 候选池大小：多取一些再按相关性门槛筛，保证 `limit` 条能用满。
fn link_pool(limit: usize) -> usize {
    limit + 4
}

/// 两个概念的共同标签（用于「相关」链接注解与相关性判定）。
fn shared_tags(doc: &ConceptDocument, hit: &CandidateHit) -> Vec<String> {
    doc.frontmatter
        .tags
        .iter()
        .filter(|t| hit.tags.iter().any(|x| x == *t))
        .cloned()
        .collect()
}

/// 为本轮写入过的概念重建正文末尾的「相关」小节（概念互链 + 原文链接）。
///
/// 为什么放在整轮收尾、而不是编译时：编译是**单次调用、无工具**的，模型看不到 Bundle 里
/// 有哪些概念，它自造的跨概念链接必然是猜的（真实首次全量编译就产出过悬空链接，随后被
/// `repair_relative_links` 降级）。这里改为**确定性**生成：候选来自 `Merger::candidates`
/// （BM25 + 本轮 overlay），且写链接前再确认目标文件真的存在 ⇒ 结构上不可能产生悬空链接。
///
/// 注意 `min_overlap = 0.0`：那套重叠度下限是给**合并**用的（防止不同主题碎片被并进同一概念），
/// 对「相关」链接过严 —— 链接只要求相关，不要求可合并。
///
/// 幂等：小节按标题整段替换并固定落在正文末尾，重复编译不会堆叠（`--force` 重跑也稳定）。
///
/// 返回被改写的概念数。
fn refresh_related_sections(
    paths: &BundlePaths,
    index: &MemoryIndex,
    merger: &Merger,
    ids: &[String],
    limit: usize,
) -> Result<usize, String> {
    let mut changed = 0usize;
    for id in ids {
        let path = paths.concept_path(id);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = ConceptDocument::from_markdown(id, &content) else {
            continue;
        };

        // ① 概念互链：候选来自 BM25（已提交概念）+ 本轮 overlay（同轮兄弟概念）
        let mut links: Vec<writer::RelatedLink> = Vec::new();
        for hit in merger.candidates(index, &doc, link_pool(limit), 0.0)? {
            if links.len() >= limit || hit.concept_id == doc.concept_id {
                continue;
            }
            // 目标必须真的在磁盘上（索引/overlay 都可能领先于删除）——这是「永不悬空」的最后一道保证
            if !paths.concept_path(&hit.concept_id).is_file() {
                continue;
            }
            let shared = shared_tags(&doc, &hit);
            // 相关性门槛：**共同标签**（同一主题的直接证据）或足够高的词重叠。
            // 不能只要「索引非空就互链」——那样任何两条无关概念都会被链在一起（真实 e2e 里
            // 「偏好中文注释」一度链到了「认证方案用 JWT」，只因索引里有它）。
            // 阈值刻意比合并下限（`candidate_min_overlap`，默认 0.25）宽松：链接只要求相关，
            // 不要求可合并；而共同标签足以独立成立（同一个项目的不同侧面本就相关）。
            if shared.is_empty() && hit_overlap_score(&doc, &hit) < RELATED_MIN_OVERLAP {
                continue;
            }
            links.push(writer::RelatedLink {
                concept_id: hit.concept_id,
                title: hit.title,
                shared_tags: shared,
            });
        }

        // ② 原文链接：frontmatter.sources[].resource 即 `raw/sessions/x.md`
        let sources: Vec<writer::RelatedSource> = doc
            .frontmatter
            .sources
            .as_ref()
            .map(|list| {
                list.iter()
                    .map(|s| writer::RelatedSource {
                        title: s.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| {
                            std::path::Path::new(&s.resource)
                                .file_stem()
                                .and_then(|x| x.to_str())
                                .unwrap_or(&s.resource)
                                .to_string()
                        }),
                        href: writer::relative_raw_link(&doc.dir(), &s.resource),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let section = writer::render_related_section(&links, &doc.dir(), &sources);
        let body = writer::replace_related_section(&doc.body, &section);
        if body == doc.body {
            continue;
        }
        let mut updated = doc.clone();
        updated.body = body;
        writer::write_concept(paths, &updated)?;
        // 正文变了 ⇒ 同步索引（否则索引里的 body_excerpt 与正文不一致，检索会漏新链接）
        index.upsert(&updated, parser::now_secs())?;
        changed += 1;
    }
    Ok(changed)
}

/// 空会话的状态记录（避免每次重扫）。
fn empty_record(session: &SessionInput, sha: &str) -> FileRecord {
    FileRecord {
        session_id: session.session_id.clone(),
        sha256: sha.into(),
        size: session.stamp.size,
        mtime: session.stamp.mtime,
        processed_at: parser::now_rfc3339(),
        compiled_at: None,
        concepts_created: Vec::new(),
        concepts_updated: Vec::new(),
    }
}

fn log_entry(at: &str, action: &str, concept_id: &str, resource: &str, reason: &str) -> LogEntry {
    LogEntry {
        at: at.to_string(),
        action: action.to_string(),
        concept_id: concept_id.to_string(),
        resource: resource.to_string(),
        reason: reason.to_string(),
    }
}

/// 会话 ID 短号（日志/进度显示）。
fn short(session_id: &str) -> String {
    session_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect()
}

/// 从目录扫描会话（`<sessions_dir>/<session_id>/message.jsonl`）。
///
/// 返回按 `session_id` 排序的输入列表；`stamp` 取自文件元数据（增量快路径用）。
pub fn scan_session_dir(sessions_dir: &std::path::Path) -> Result<Vec<SessionInput>, String> {
    let entries = std::fs::read_dir(sessions_dir).map_err(|e| {
        format!("读取会话目录失败({}): {e}", sessions_dir.display())
    })?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(session_id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        let live = dir.join("message.jsonl");
        if !live.is_file() {
            continue;
        }
        let Ok(jsonl) = std::fs::read_to_string(&live) else {
            continue;
        };
        let stamp = SourceStamp::of_file(&live);
        out.push(
            SessionInput::new(session_id, jsonl)
                .with_stamp(stamp)
                .with_title(read_session_title(sessions_dir, &entry.file_name().to_string_lossy())),
        );
    }
    out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(out)
}

/// 会话标题：从 `<sessions_dir>/../sqlite.db` 读代价高，这里先用空（由应用侧传入更准）。
fn read_session_title(_sessions_dir: &std::path::Path, _session_id: &str) -> Option<String> {
    None
}

/// 统计待编译会话数（**不读文件内容、不算哈希**）。
pub fn count_pending(paths: &BundlePaths, sessions: &[SessionInput]) -> usize {
    let state = SyncState::load(&paths.state_file());
    let stamps: Vec<(String, SourceStamp)> = sessions
        .iter()
        .map(|s| (s.session_id.clone(), s.stamp))
        .collect();
    state.count_pending(&stamps)
}

/// 只读会话目录的**指纹**（不读 `message.jsonl` 内容）—— 供「待编译 N 个」这类
/// 轻量查询使用（打开页面即调用，不应把全部会话正文读进内存）。
///
/// 目录不存在视为空（首次安装尚无会话），不报错。
pub fn scan_session_stamps(
    sessions_dir: &std::path::Path,
) -> Result<Vec<(String, SourceStamp)>, String> {
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(sessions_dir)
        .map_err(|e| format!("读取会话目录失败({}): {e}", sessions_dir.display()))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(session_id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        let live = dir.join("message.jsonl");
        if !live.is_file() {
            continue;
        }
        out.push((session_id, SourceStamp::of_file(&live)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}


/// 概念统计（数量 / 按 type 分组）。
pub fn concept_stats(paths: &BundlePaths) -> HashMap<String, usize> {
    let loaded = writer::load_all_concepts(paths);
    let mut stats: HashMap<String, usize> = HashMap::new();
    for (_, _, doc) in loaded.concepts {
        *stats.entry(doc.frontmatter.concept_type).or_insert(0) += 1;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{PromptStage, ScriptedLlm};
    use crate::validate::validate_bundle;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        index_dir: PathBuf,
        paths: BundlePaths,
        index: MemoryIndex,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.index_dir);
        }
    }

    /// 独立的会话来源目录（放 Bundle **外面**，否则触发 Bundle 归属守卫）。
    fn session_dir(tag: &str) -> (PathBuf, impl Fn()) {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "astorm_mem_sessions_{tag}_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cleanup_dir = dir.clone();
        (dir, move || {
            let _ = std::fs::remove_dir_all(&cleanup_dir);
        })
    }

    fn fixture(tag: &str) -> Fixture {        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("astorm_mem_pipe_{tag}_{}_{n}", std::process::id()));
        let index_dir = std::env::temp_dir().join(format!("astorm_mem_pipeidx_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&index_dir);
        let paths = BundlePaths::new(&root);
        let index = MemoryIndex::new(&index_dir).unwrap();
        Fixture {
            root,
            index_dir,
            paths,
            index,
        }
    }

    /// 一段真实形态的会话 JSONL（含 system / 用户 / 助手 / 工具调用 / 工具结果）。
    fn sample_jsonl() -> String {
        let msgs = json!([
            {"id": "s1:0", "sessionId": "sess-a1f9", "role": "system", "content": "你是 astorm……", "toolCalls": null, "seq": 0, "createdAt": 1760000000},
            {"id": "s1:1", "sessionId": "sess-a1f9", "role": "user", "content": "认证方案怎么选？\n\n【会话环境】\n- 工作区：/w/x\n- 可用工具：bash", "toolCalls": null, "seq": 1, "createdAt": 1760000100},
            {"id": "s1:2", "sessionId": "sess-a1f9", "role": "assistant", "content": "建议用 JWT。", "toolCalls": [{"id": "call_1", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"grep -r jwt\"}"}}], "seq": 2, "createdAt": 1760000200},
            {"id": "s1:tool:call_1", "sessionId": "sess-a1f9", "role": "tool", "content": "文件列表……", "toolCalls": null, "seq": 3, "createdAt": 1760000300, "toolCallId": "call_1", "metadata": {"status": "ok"}},
            {"id": "s1:4", "sessionId": "sess-a1f9", "role": "user", "content": "那就用 JWT 吧", "toolCalls": null, "seq": 4, "createdAt": 1760000400}
        ]);
        msgs.as_array()
            .unwrap()
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn input() -> SessionInput {
        SessionInput::new("sess-a1f9", sample_jsonl())
            .with_stamp(SourceStamp::new(sample_jsonl().len() as u64, 1760000400))
            .with_title(Some("认证方案讨论".into()))
    }

    fn scripted() -> ScriptedLlm {
        ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [
                    {"event_text": "认证方案选择 JWT 而非 session", "frag_ids": ["m01", "m02"], "event_type": "decision", "confidence": 0.95},
                    {"event_text": "用户偏好中文注释", "frag_ids": ["m02"], "event_type": "preference", "confidence": 0.7}
                ]}),
            )
            .with(
                PromptStage::Curate,
                json!({"events": [
                    {"event_text": "认证方案选择 JWT 而非 session", "frag_ids": ["m01"], "event_type": "decision", "confidence": 0.95},
                    {"event_text": "用户偏好中文注释", "frag_ids": ["m02"], "event_type": "preference", "confidence": 0.7}
                ]}),
            )
            .with(
                PromptStage::Compile,
                json!({"concepts": [
                    {"event_type": "decision", "concept_id": "use-jwt",
                     "frontmatter": {"type": "Decision", "title": "认证方案用 JWT", "description": "无状态鉴权选型", "tags": ["auth"]},
                     "body": "选择 JWT 而非 session。"},
                    {"event_type": "preference", "concept_id": "chinese-comments",
                     "frontmatter": {"type": "Preference", "title": "偏好中文注释", "tags": ["style"]},
                     "body": "用户偏好中文注释。"}
                ]}),
            )
    }

    async fn run(
        f: &Fixture,
        llm: &dyn MemoryLlm,
        sessions: &[SessionInput],
        options: &CompileOptions,
    ) -> CompileReport {
        run_with_config(f, llm, sessions, options, MemoryConfig::default()).await
    }

    /// 带自定义配置的编译（分类表跟随配置，避免两处各写一份默认值）。
    async fn run_with_config(
        f: &Fixture,
        llm: &dyn MemoryLlm,
        sessions: &[SessionInput],
        options: &CompileOptions,
        cfg: MemoryConfig,
    ) -> CompileReport {
        let table = CategoryTable::new(cfg.categories.clone());
        compile_bundle(&f.paths, &f.index, llm, &cfg, &table, sessions, options, &no_progress, &CancellationToken::new())
            .await
            .unwrap()
    }

    /// 端到端：JSONL → raw/ + wiki/ + index.md + log.md + 状态文件，且通过 OKF 校验。
    #[tokio::test]
    async fn end_to_end_produces_valid_bundle() {
        let f = fixture("e2e");
        let report = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;

        assert_eq!(report.total_sessions, 1);
        assert_eq!(report.processed_sessions, 1);
        assert_eq!(report.extracted_units, 2);
        assert_eq!(report.compiled_concepts, 2);
        assert_eq!(report.created.len(), 2, "{:?}", report.created);
        assert!(report.failed_sessions.is_empty(), "{:?}", report.failed_sessions);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        // 分类目录由 event_type 决定
        assert!(report.created.contains(&"decisions/use-jwt".to_string()), "{:?}", report.created);
        assert!(
            report.created.contains(&"preferences/chinese-comments".to_string()),
            "{:?}",
            report.created
        );

        // 落盘产物齐全
        let raw = std::fs::read_dir(f.paths.raw_sessions_dir()).unwrap().count();
        assert_eq!(raw, 1, "应写出 1 个 raw 会话");
        assert!(f.paths.root_index_path().is_file(), "应重建根索引");
        assert!(f.paths.log_path().is_file(), "应写出变更历史");
        assert!(f.paths.state_file().is_file(), "应写出增量状态");
        assert!(f.paths.index_path("decisions").is_file());

        // OKF 合规
        let validation = validate_bundle(&f.paths);
        assert!(validation.is_ok(), "Bundle 应通过校验: {:?}", validation.issues);
        assert_eq!(validation.warnings(), 0, "{:?}", validation.issues);

        // 索引可召回
        assert_eq!(f.index.count(), 2);
        let doc = ConceptDocument::from_markdown(
            "decisions/use-jwt",
            &std::fs::read_to_string(f.paths.concept_path("decisions/use-jwt")).unwrap(),
        )
        .unwrap();
        assert_eq!(doc.frontmatter.concept_type, "Decision");
        assert_eq!(doc.frontmatter.title.as_deref(), Some("认证方案用 JWT"));
        assert_eq!(doc.frontmatter.observed_at.as_deref(), Some("2025-10-09"));
        // 来源是本管道的 raw 文件
        let sources = doc.frontmatter.sources.as_ref().unwrap();
        assert!(sources[0].resource.starts_with("raw/sessions/"), "{sources:?}");
        // 环境块噪声未进入概念正文（由归一化阶段截断）
        let hits = f.index.search("工作区", 10).unwrap();
        assert!(hits.is_empty(), "环境块噪声不应被索引到: {hits:?}");
        // 但正文关键词可检索
        assert_eq!(f.index.search("JWT", 10).unwrap().len(), 1);
    }

    /// 集成：编译器自造悬空链接 → 收尾自动降级为纯文本，validate 无 Error。
    #[tokio::test]
    async fn dangling_links_are_repaired_so_bundle_stays_valid() {
        let f = fixture("links");
        let llm = ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [
                    {"event_text": "决策甲", "event_type": "decision"},
                    {"event_text": "决策乙", "event_type": "decision"}
                ]}),
            )
            .with(
                PromptStage::Curate,
                json!({"events": [
                    {"event_text": "决策甲", "event_type": "decision", "confidence": 0.9},
                    {"event_text": "决策乙", "event_type": "decision", "confidence": 0.9}
                ]}),
            )
            .with(
                PromptStage::Compile,
                json!({"concepts": [
                    {"event_type": "decision", "concept_id": "alpha",
                     "frontmatter": {"title": "甲"},
                     "body": "见 [乙](./beta.md) 与 [不存在](./ghost.md)，外链 [官网](https://example.com) 与 [锚点](#x) 保留。"},
                    {"event_type": "decision", "concept_id": "beta",
                     "frontmatter": {"title": "乙"}, "body": "正文"}
                ]}),
            );
        let report = run(&f, &llm, &[input()], &CompileOptions::default()).await;

        assert!(
            report.warnings.iter().any(|w| w.contains("悬空相对链接")),
            "应报告修复: {:?}",
            report.warnings
        );
        let body = std::fs::read_to_string(f.paths.concept_path("decisions/alpha")).unwrap();
        // 能解析的链接保留
        assert!(body.contains("[乙](./beta.md)"), "可解析链接应保留: {body}");
        // 悬空链接降级为纯文本（文字保留）
        assert!(!body.contains("ghost.md"), "悬空链接语法应被去掉: {body}");
        assert!(body.contains("不存在"), "链接文字应保留: {body}");
        // 外链与锚点不动
        assert!(body.contains("[官网](https://example.com)"), "{body}");
        assert!(body.contains("[锚点](#x)"), "{body}");

        let validation = validate_bundle(&f.paths);
        assert!(validation.is_ok(), "修复后不应有 Error: {:?}", validation.issues);

        // 再次强制编译：LLM 会重新吐出同样的悬空链接，故修复会再次触发（这是正确的）；
        // 关键是 Bundle 仍然合规，且正文不因重复追加而膨胀。
        let again = run(
            &f,
            &llm,
            &[input()],
            &CompileOptions {
                force: true,
                dry_run: false,
            },
        )
        .await;
        assert!(validate_bundle(&f.paths).is_ok(), "重跑后仍应合规");
        let body2 = std::fs::read_to_string(f.paths.concept_path("decisions/alpha")).unwrap();
        assert_eq!(
            body2.matches("### 追加").count(),
            0,
            "不应产生追加段: {body2}"
        );
        assert!(!body2.contains("ghost.md"), "重跑后仍应保持修复: {body2}");
        assert!(body2.contains("[乙](./beta.md)"), "{body2}");
        assert!(again.created.is_empty() && again.updated.is_empty(), "{again:?}");
    }

    /// 「相关」小节：只链接**已存在**的概念 + 原文链接，且重复编译逐字稳定。
    ///
    /// 这是「链接从哪来」的核心测试：模型看不到 Bundle 里有哪些概念，所以链接必须由
    /// 管道确定性生成；这里同时钉住「不悬空 / 不自指 / 幂等 / 原文可解析」四条。
    #[tokio::test]
    async fn related_section_links_existing_concepts_and_is_stable() {
        let f = fixture("related");
        let llm = ScriptedLlm::new()
            .with(
                PromptStage::Extract,
                json!({"events": [
                    {"event_text": "认证方案选择 JWT", "frag_ids": ["m01"], "event_type": "decision", "confidence": 0.95},
                    {"event_text": "JWT 密钥需要定期轮换", "frag_ids": ["m02"], "event_type": "fact", "confidence": 0.9}
                ]}),
            )
            .with(
                PromptStage::Curate,
                json!({"events": [
                    {"event_text": "认证方案选择 JWT", "frag_ids": ["m01"], "event_type": "decision", "confidence": 0.95},
                    {"event_text": "JWT 密钥需要定期轮换", "frag_ids": ["m02"], "event_type": "fact", "confidence": 0.9}
                ]}),
            )
            .with(
                PromptStage::Compile,
                json!({"concepts": [
                    {"event_type": "decision", "concept_id": "use-jwt",
                     "frontmatter": {"type": "Decision", "title": "认证方案用 JWT", "description": "无状态鉴权选型，JWT 令牌", "tags": ["auth", "jwt"]},
                     "body": "选择 JWT 而非 session。"},
                    {"event_type": "fact", "concept_id": "jwt-rotation",
                     "frontmatter": {"type": "Fact", "title": "JWT 密钥轮换", "description": "JWT 密钥轮换与撤销", "tags": ["auth", "jwt"]},
                     "body": "密钥按周期轮换。"}
                ]}),
            );

        let report = run(&f, &llm, &[input()], &CompileOptions::default()).await;
        assert!(report.failed_sessions.is_empty(), "{:?}", report.failed_sessions);

        let a = std::fs::read_to_string(f.paths.concept_path("decisions/use-jwt")).unwrap();
        let b = std::fs::read_to_string(f.paths.concept_path("concepts/jwt-rotation")).unwrap();

        // 互链：跨目录用 ../ 形式
        assert!(a.contains("## 相关"), "{a}");
        assert!(a.contains("[JWT 密钥轮换](../concepts/jwt-rotation.md)"), "{a}");
        assert!(b.contains("[认证方案用 JWT](../decisions/use-jwt.md)"), "{b}");
        assert!(a.contains("共同标签："), "应给出共同标签注解: {a}");
        // 原文链接（raw 在 wiki 之外，退两级；raw 无 frontmatter title 时以文件主干当标题）
        assert!(
            a.contains("- 原文：[2025-10-09-sessa1f9](../../raw/sessions/2025-10-09-sessa1f9.md)"),
            "{a}"
        );
        // 不自指
        assert!(!a.contains("use-jwt.md)"), "不得链接自己: {a}");
        assert_eq!(a.matches("## 相关").count(), 1, "{a}");

        // 不悬空：合规 + 链接修复一个都不用降级
        let issues = validate_bundle(&f.paths).issues;
        assert!(
            !issues.iter().any(|i| i.message.contains("不存在") || i.message.contains("越出")),
            "{issues:?}"
        );
        let (_, degraded) = writer::repair_relative_links(&f.paths).unwrap();
        assert_eq!(degraded, 0, "生成的链接必须全部可解析");

        // 幂等：强制重跑逐字不变，且不堆叠小节
        let _ = run(
            &f,
            &llm,
            &[input()],
            &CompileOptions {
                force: true,
                dry_run: false,
            },
        )
        .await;
        let a2 = std::fs::read_to_string(f.paths.concept_path("decisions/use-jwt")).unwrap();
        assert_eq!(a2, a, "「相关」小节不得随重跑变化");
        assert_eq!(a2.matches("## 相关").count(), 1, "{a2}");
    }

    /// 相关链接条数受 `related_concepts` 约束；0 = 完全不生成该小节。
    #[tokio::test]
    async fn related_concepts_limit_is_respected() {
        let f = fixture("related0");
        let mut cfg = MemoryConfig::default();
        cfg.related_concepts = 0;
        let report = run_with_config(&f, &scripted(), &[input()], &CompileOptions::default(), cfg).await;
        assert!(report.failed_sessions.is_empty(), "{:?}", report.failed_sessions);
        let body = std::fs::read_to_string(f.paths.concept_path("decisions/use-jwt")).unwrap();
        assert!(!body.contains("## 相关"), "关闭时不得生成小节: {body}");
    }

    /// 增量：第二次运行指纹未变 → 全部跳过，**零 LLM 调用**。
    #[tokio::test]
    async fn second_run_is_incremental_and_makes_no_llm_calls() {
        let f = fixture("incr");
        let first = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;
        assert_eq!(first.processed_sessions, 1);

        let llm = ScriptedLlm::new(); // 严格模式：任何调用都会报错
        let second = run(&f, &llm, &[input()], &CompileOptions::default()).await;

        assert_eq!(second.processed_sessions, 0);
        assert_eq!(second.skipped_sessions, 1);
        assert!(llm.prompts().is_empty(), "增量跳过不应调用 LLM");
        assert!(second.created.is_empty() && second.updated.is_empty());
        // 状态文件被更新
        let state = SyncState::load(&f.paths.state_file());
        assert_eq!(state.len(), 1);
        assert!(state.last_sync.is_some());
    }

    /// 回归：`parse` 之后再 `compile` 不得把所有会话跳过（曾因共用增量状态而出错）。
    #[tokio::test]
    async fn parse_then_compile_does_not_skip_everything() {
        let f = fixture("parseThenCompile");
        let sessions = [input()];
        let (processed, skipped, _) = parse_sessions(&f.paths, &MemoryConfig::default(), &sessions, false).unwrap();
        assert_eq!((processed, skipped), (1, 0));
        // 仅 parse 过 → 仍属待编译
        assert_eq!(count_pending(&f.paths, &sessions), 1, "parse 后应仍待编译");

        let report = run(&f, &scripted(), &sessions, &CompileOptions::default()).await;
        assert_eq!(report.processed_sessions, 1, "不应被增量状态跳过: {report:?}");
        assert_eq!(report.created.len(), 2, "{:?}", report.created);

        // 编译完成后再跑 → 才应跳过
        assert_eq!(count_pending(&f.paths, &sessions), 0, "编译后应不再待编译");
        let again = run(&f, &ScriptedLlm::new(), &sessions, &CompileOptions::default()).await;
        assert_eq!(again.skipped_sessions, 1);
    }

    /// `--force` 重跑：不新增概念、body 不重复追加（幂等）。
    #[tokio::test]
    async fn force_rerun_is_idempotent() {
        let f = fixture("force");
        let _ = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;
        let body_before = std::fs::read_to_string(f.paths.concept_path("decisions/use-jwt")).unwrap();

        let report = run(
            &f,
            &scripted(),
            &[input()],
            &CompileOptions {
                force: true,
                dry_run: false,
            },
        )
        .await;

        assert!(report.created.is_empty(), "强制重跑不应新建: {:?}", report.created);
        assert!(report.updated.is_empty(), "内容未变不应更新: {:?}", report.updated);
        assert_eq!(report.skipped.len(), 2, "应全部判为已覆盖: {:?}", report.skipped);
        // 判定为 SKIP 记入 log.md（report.skipped 存概念 ID，原因在日志/单测层验证）
        let logs = crate::writer::read_log(&f.paths, 20);
        assert_eq!(logs.iter().filter(|e| e.action == "SKIP").count(), 2, "{logs:?}");

        let body_after = std::fs::read_to_string(f.paths.concept_path("decisions/use-jwt")).unwrap();
        assert_eq!(body_before, body_after, "body 不得重复追加");
        assert_eq!(f.index.count(), 2, "索引不应累积重复文档");
    }

    /// 真实数据形态：跳过 system、截断环境块、工具结果截断、锚点时间戳。
    #[tokio::test]
    async fn normalization_is_applied_to_raw_output() {
        let f = fixture("normalize");
        let _ = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;

        let raw_dir = f.paths.raw_sessions_dir();
        let raw_file = std::fs::read_dir(&raw_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .next()
            .unwrap();
        let raw = std::fs::read_to_string(&raw_file).unwrap();

        assert!(raw.starts_with("---\nsession_id: sess-a1f9\n"));
        assert!(raw.contains("source_kind: astorm-session"));
        assert!(raw.contains("title: 认证方案讨论"));
        assert!(raw.contains("### [m01] user"));
        assert!(raw.contains("调用 `bash`"), "工具调用应渲染:\n{raw}");
        assert!(raw.contains("**工具结果** `call_1`"), "工具结果应渲染:\n{raw}");
        assert!(!raw.contains("你是 astorm"), "system 不得进入 raw:\n{raw}");
        assert!(!raw.contains("【会话环境】"), "环境块应被截断:\n{raw}");
        assert!(!raw.contains("/w/x"), "工作区噪声不得进入 raw:\n{raw}");
        // 锚点顺序对应过滤后的消息顺序（m01=用户、m02=助手）
        assert!(raw.contains("认证方案怎么选？"));
        assert!(raw.contains("那就用 JWT 吧"));
    }

    /// 第 N 次提取调用失败的替身（严格按阶段分派，仅在 Extract 阶段按次数注入失败）。
    struct FailNthExtract {
        inner: ScriptedLlm,
        fail_on: usize,
        seen: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MemoryLlm for FailNthExtract {
        async fn complete_json(
            &self,
            system: &str,
            user: &str,
            cancel: CancellationToken,
        ) -> Result<crate::llm::LlmJsonOutcome, String> {
            if crate::llm::detect_stage(system) == PromptStage::Extract {
                let n = self.seen.fetch_add(1, Ordering::SeqCst) + 1;
                if n == self.fail_on {
                    return Err("模拟提取失败".into());
                }
            }
            self.inner.complete_json(system, user, cancel).await
        }

        fn model_label(&self) -> String {
            self.inner.model_label()
        }
    }

    /// 单会话提取失败不中断其他会话，且失败会话不写入状态（下次可重试）。
    #[tokio::test]
    async fn session_failure_does_not_abort_others() {
        let f = fixture("partial");
        let first = input();
        let mut second = SessionInput::new("sess-b2c3", sample_jsonl())
            .with_stamp(SourceStamp::new(sample_jsonl().len() as u64, 1760000600));
        second.jsonl = sample_jsonl().replace("sess-a1f9", "sess-b2c3");

        let llm = FailNthExtract {
            inner: scripted(),
            fail_on: 1, // 第一个会话提取失败
            seen: AtomicUsize::new(0),
        };
        let report = compile_bundle(
            &f.paths,
            &f.index,
            &llm,
            &MemoryConfig::default(),
            &CategoryTable::new(MemoryConfig::default().categories),
            &[first, second],
            &CompileOptions::default(),
            &no_progress,
            &CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(report.total_sessions, 2);
        assert_eq!(report.processed_sessions, 1, "第二个会话应处理成功: {report:?}");
        assert_eq!(report.failed_sessions.len(), 1);
        assert_eq!(report.failed_sessions[0].session_id, "sess-a1f9");
        assert_eq!(report.failed_sessions[0].stage, "extract");
        assert!(report.failed_sessions[0].message.contains("模拟提取失败"));
        assert_eq!(report.created.len(), 2, "成功的会话应如常产出: {:?}", report.created);

        // 失败会话不写状态（下次重试）；成功会话写状态
        let state = SyncState::load(&f.paths.state_file());
        assert_eq!(state.len(), 1, "只应记录成功会话: {state:?}");
        assert!(state.get("sess-b2c3").is_some());
        assert!(state.get("sess-a1f9").is_none(), "失败会话不应记录，以便重试");
        assert!(state.last_sync.is_none(), "存在失败会话时不更新 last_sync");

        // Bundle 仍然自洽
        assert!(validate_bundle(&f.paths).is_ok());
    }

    /// 取消：报告标记 cancelled，已落盘内容保留，索引丢弃未提交写入。
    #[tokio::test]
    async fn cancellation_is_reported_and_bundle_stays_consistent() {
        let f = fixture("cancel");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let llm = ScriptedLlm::new();
        let report = compile_bundle(
            &f.paths,
            &f.index,
            &llm,
            &MemoryConfig::default(),
            &CategoryTable::new(MemoryConfig::default().categories),
            &[input()],
            &CompileOptions::default(),
            &no_progress,
            &cancel,
        )
        .await
        .unwrap();

        assert!(report.cancelled);
        assert_eq!(report.processed_sessions, 0);
        assert!(llm.prompts().is_empty(), "取消后不应发起 LLM 调用");
        assert!(f.index.is_empty(), "取消后索引应无未提交写入");
    }

    /// `--dry-run`：确定性阶段照跑（raw 落盘），LLM 阶段只记录提示词、不产出概念。
    #[tokio::test]
    async fn dry_run_records_prompts_without_creating_concepts() {
        let f = fixture("dry");
        let llm = crate::llm::DryRunLlm::new();
        let report = run(
            &f,
            &llm,
            &[input()],
            &CompileOptions {
                force: false,
                dry_run: true,
            },
        )
        .await;

        assert_eq!(report.processed_sessions, 1);
        assert_eq!(report.extracted_units, 0, "dry-run 不产出记忆单元");
        assert!(report.created.is_empty());
        // raw 仍然落盘（归一化是确定性阶段）
        assert_eq!(std::fs::read_dir(f.paths.raw_sessions_dir()).unwrap().count(), 1);
        // 提示词被完整捕获
        let prompts = llm.prompts();
        assert!(!prompts.is_empty());
        let stages: Vec<PromptStage> = prompts.iter().map(|p| p.stage).collect();
        assert!(stages.contains(&PromptStage::Extract), "{stages:?}");
        // 提取提示词里带上了真实会话内容
        assert!(prompts[0].user.contains("认证方案怎么选？"), "{}", prompts[0].user);
        // 提取提示词不含环境噪声
        assert!(!prompts[0].user.contains("/w/x"), "环境噪声不应进提示词");
    }

    /// 增量快路径：指纹一致时不读内容（此处用「内容变了但 size/mtime 相同」验证快路径短路）。
    #[tokio::test]
    async fn fingerprint_fast_path_skips_without_hashing() {
        let f = fixture("fast");
        let _ = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;

        // 内容不同但 size/mtime 与记录一致 → 快路径判为未变，跳过
        let mut tampered = input();
        tampered.jsonl = "x".repeat(input().jsonl.len());
        assert_eq!(tampered.jsonl.len(), input().jsonl.len());
        let report = run(&f, &ScriptedLlm::new(), &[tampered], &CompileOptions::default()).await;
        assert_eq!(report.skipped_sessions, 1, "指纹一致应跳过");
    }

    /// 慢路径：size/mtime 变化但内容哈希未变 → 仍跳过，但需算一次哈希。
    #[tokio::test]
    async fn touched_file_with_same_hash_is_skipped() {
        let f = fixture("touched");
        let _ = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;

        let mut touched = input();
        touched.stamp = SourceStamp::new(input().stamp.size, input().stamp.mtime + 999);
        let report = run(&f, &ScriptedLlm::new(), &[touched], &CompileOptions::default()).await;
        assert_eq!(report.skipped_sessions, 1, "内容未变应跳过: {report:?}");
    }

    /// 无对话内容的会话：跳过且记录状态（不被反复重扫）。
    #[tokio::test]
    async fn contentless_session_is_recorded_as_skipped() {
        let f = fixture("empty");
        let only_system = r#"{"role":"system","content":"提示词","seq":0,"createdAt":1760000000}"#.to_string();
        let session = SessionInput::new("sess-empty0", only_system.clone())
            .with_stamp(SourceStamp::new(only_system.len() as u64, 1760000000));

        let report = run(&f, &ScriptedLlm::new(), &[session.clone()], &CompileOptions::default()).await;
        assert_eq!(report.skipped_sessions, 1);
        assert_eq!(report.processed_sessions, 0);
        assert_eq!(SyncState::load(&f.paths.state_file()).len(), 1, "空会话也应记录状态");

        // 第二次仍然跳过（不再重扫）
        let again = run(&f, &ScriptedLlm::new(), &[session], &CompileOptions::default()).await;
        assert_eq!(again.skipped_sessions, 1);
    }

    /// 类型冲突（同 ID 不同 type）→ 另建文件并告警，不覆盖已有概念。
    #[tokio::test]
    async fn type_conflict_on_same_id_creates_separate_file() {
        let f = fixture("conflict");
        // 先初始化（建立 Bundle 归属标记），再手工放一个同 ID 但 type 不同的概念
        f.paths.ensure_dirs(&["decisions".into()]).unwrap();
        let existing = "---\ntype: Fact\ntitle: 手工事实\n---\n\n手工正文\n";
        let path = f.paths.concept_path("decisions/use-jwt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, existing).unwrap();
        f.index.upsert(&ConceptDocument::from_markdown("decisions/use-jwt", existing).unwrap(), 1).unwrap();
        f.index.commit().unwrap();

        let report = run(&f, &scripted(), &[input()], &CompileOptions::default()).await;

        assert!(
            report.warnings.iter().any(|w| w.contains("类型冲突")),
            "应告警类型冲突: {:?}",
            report.warnings
        );
        // 手工概念未被覆盖
        assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        // 新概念以避让 ID 落盘
        assert!(
            report.created.iter().any(|c| c.starts_with("decisions/use-jwt-")),
            "{:?}",
            report.created
        );
    }

    /// 多会话：按 session_id 排序处理，raw 与概念都完整落盘。
    #[tokio::test]
    async fn handles_multiple_sessions() {
        let f = fixture("multi");
        let a = input();
        let mut b = SessionInput::new("sess-b2c3", sample_jsonl())
            .with_stamp(SourceStamp::new(sample_jsonl().len() as u64, 1760000600))
            .with_title(Some("第二次讨论".into()));
        b.jsonl = sample_jsonl().replace("sess-a1f9", "sess-b2c3");

        let report = run(&f, &scripted(), &[a, b], &CompileOptions::default()).await;

        assert_eq!(report.total_sessions, 2);
        assert_eq!(report.processed_sessions, 2);
        assert_eq!(std::fs::read_dir(f.paths.raw_sessions_dir()).unwrap().count(), 2);
        // 第二个会话的同名概念走 UPSERT（同 ID）→ 不重复新建
        assert_eq!(report.created.len(), 2, "同 ID 概念应合并而非重复: {:?}", report.created);
        assert_eq!(report.updated.len(), 2, "第二个会话应更新已有概念: {:?}", report.updated);
        assert!(validate_bundle(&f.paths).is_ok());
    }

    #[test]
    fn count_pending_and_stats() {
        let f = fixture("pending");
        let sessions = vec![input()];
        assert_eq!(count_pending(&f.paths, &sessions), 1, "无状态时应全部待编译");
        assert!(concept_stats(&f.paths).is_empty());
        let _ = &f.root;
    }

    /// `scan_session_dir` 读真实目录结构。
    #[test]
    fn scan_reads_real_session_layout() {
        let f = fixture("scan");
        let dir = f.root.join("session/sess-a1f9");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("message.jsonl"), sample_jsonl()).unwrap();
        std::fs::write(dir.join("summary_001.md"), "摘要").unwrap(); // 非消息文件应被忽略
        std::fs::create_dir_all(f.root.join("session/empty-sid")).unwrap(); // 无消息文件应跳过

        let sessions = scan_session_dir(&f.paths.root().join("session")).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "sess-a1f9");
        assert!(sessions[0].jsonl.contains("认证方案怎么选"));
        assert!(sessions[0].stamp.size > 0, "应取到文件指纹");
        assert!(sessions[0].stamp.mtime > 0, "应取到 mtime（快路径依赖）");
    }

    #[test]
    fn short_id_helper() {
        assert_eq!(short("sess-a1f9-xxxx"), "sessa1f9");
        assert_eq!(short("---"), "");
    }

    /// 轻量指纹扫描：不读正文也能列出会话；异常目录被忽略；目录缺失不报错。
    #[test]
    fn session_stamps_are_scanned_without_reading_content() {
        let (dir, cleanup) = session_dir("stamps");
        let pass = dir.join("sess-a1f9");
        std::fs::create_dir_all(&pass).unwrap();
        std::fs::write(pass.join("message.jsonl"), sample_jsonl()).unwrap();
        std::fs::create_dir_all(dir.join("empty-sid")).unwrap(); // 无消息文件 → 跳过
        std::fs::write(dir.join("stray.txt"), "x").unwrap(); // 非目录 → 跳过

        let stamps = scan_session_stamps(&dir).unwrap();
        assert_eq!(stamps.len(), 1);
        assert_eq!(stamps[0].0, "sess-a1f9");
        assert!(stamps[0].1.size > 0 && stamps[0].1.mtime > 0);

        // 目录不存在 = 空列表（首次安装尚无会话），不是错误
        assert!(scan_session_stamps(&dir.join("nope")).unwrap().is_empty());
        cleanup();
    }

    /// 待编译计数：仅归一化过（未编译）的会话仍算待编译；编译后才归零。
    #[test]
    fn pending_count_requires_a_real_compile() {
        let f = fixture("pending");
        let (dir, cleanup) = session_dir("pending");
        let pass = dir.join("sess-a1f9");
        std::fs::create_dir_all(&pass).unwrap();
        std::fs::write(pass.join("message.jsonl"), sample_jsonl()).unwrap();

        // 待编译计数 = 目录指纹（不读正文）× 增量状态；门面同样用这两个原语组合
        let pending = |f: &Fixture, dir: &PathBuf| {
            SyncState::load(&f.paths.state_file())
                .count_pending(&scan_session_stamps(dir).unwrap())
        };
        assert_eq!(pending(&f, &dir), 1);

        // 只归一化：指纹记下了，但没有 compiled_at → 仍待编译
        // （用 scan_session_dir 取真实指纹：假的 mtime 会让快路径永远判定为「需重算」）
        let sessions = scan_session_dir(&dir).unwrap();
        assert_eq!(sessions.len(), 1);
        parse_sessions(&f.paths, &MemoryConfig::default(), &sessions, false).unwrap();
        assert_eq!(pending(&f, &dir), 1, "只 parse 过不算编译过");

        // 标记为已编译 → 归零
        let mut state = SyncState::load(&f.paths.state_file());
        if let Some(rec) = state.get("sess-a1f9").cloned() {
            state.record(FileRecord {
                compiled_at: Some("2026-01-01T00:00:00Z".into()),
                ..rec
            });
        }
        state.save(&f.paths.state_file()).unwrap();
        assert_eq!(pending(&f, &dir), 0, "编译过就该归零");
        cleanup();
    }
}
