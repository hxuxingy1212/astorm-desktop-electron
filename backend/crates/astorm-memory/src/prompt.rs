//! 提示词：OKF 速查前缀（P0）+ 提取（P1）/ 二遍自查（P1'）/ 编译（P2）/ 合并判定（P3）。
//!
//! 对方案文档的 6 处修正（见 crate 文档与 CHANGELOG 式注释）：
//! 1. 提取提示词的字段与 `MemoryUnit` 结构体对齐（`time_range` → `confidence`，取值集由分类表给出）
//! 2. `frag_ids` 明确定义为归一化正文里的 `[mNN]` 锚点
//! 3. **提取阶段不注入 OKF 速查**（其产出在 OKF 之前，注入会把输出带向「概念形状」，
//!    损害「最大化细粒度」目标）；仅编译与判定注入
//! 4. 判定阶段只输出 `UPDATE` / `SKIP`（`CREATE` 由「无候选 / 空 decisions」判定，
//!    消除文档中 CREATE 以 `existing_concept_id` 为键却无处安放的歧义）
//! 5. 判定 payload 携带候选的 frontmatter + 正文摘要（否则无法判断「是否已完全覆盖」）
//! 6. 重试 / 退避 / JSON 围栏剥离均已由 `llm-provider` 与 [`crate::llm::ChatLlm`] 承担
//!
//! 取值集与目录映射表**由分类表动态生成**（配置驱动分类 ⇒ 新增分类无需改提示词代码）。

use super::types::{CandidateHit, CategoryTable, MemoryUnit};
use crate::concept::ConceptDocument;

/// P0：OKF v0.2 速查（≤300 token；仅编译与判定注入）。
pub const OKF_QUICK_REFERENCE: &str = r#"## OKF v0.2 Quick Reference

**Bundle**: A directory of Markdown files. Reserved filenames
`index.md` and `log.md` MUST NOT be used as concept documents.

**Concept**: One non-reserved .md file. Its identity (Concept ID)
is its file path without the .md suffix.

**Concept document structure**:
---
type: <REQUIRED, non-empty string>
title: <recommended>
description: <recommended>
tags: [<recommended>]
sources: <v0.2 provenance>
generated: <v0.2 trust>
status: <v0.2 lifecycle>
stale_after: <v0.2 freshness>
---
<free-form Markdown body>

**Links**: Use relative markdown links only:
  ./other.md or ../concepts/x.md
Bundle-absolute links (/concepts/x.md) are discouraged.
Never invent pseudo-schemes (session:, astorm:, date-only targets):
link only to real .md concepts in the bundle.

**Type values**: Descriptive, not enumerated. Use self-explanatory
values. Consumers MUST tolerate unknown types."#;

/// 判定阶段送给模型的候选正文摘要长度（字符）。
const JUDGE_EXCERPT_CHARS: usize = 800;

/// P1：提取阶段的 system 提示词（**不含 OKF 速查**；`event_type` 取值集由分类表生成）。
///
/// **耐久性硬门槛放在最前**：真实全量测试证明，只靠文末一句建议性约束会严重过抽
/// （2 个会话产出 126 条，其中 43 条是同一第三方库 README 的碎片、`decisions` 为 0 条）。
/// 因此这里把「不要提取什么」设为 **reject 规则**，并给出明确的反例清单与产出预算。
pub fn extract_system(categories: &CategoryTable) -> String {
    let options = extract_options(categories);
    format!(
        r#"You are an information extraction engine. Extract the DURABLE memory
units from one conversation session. Return only valid JSON.

## Hard gate — do NOT extract these (even though they are factual)
- Contents, structure or metadata of third-party projects / docs / pages
  that the session merely READ: file paths, directory listings, install
  or build commands, licenses, homepages, version numbers, API or wire
  protocol field lists, changelogs, README summaries.
  (If the session was mostly reading someone else's repo, it yields
  almost NOTHING worth remembering.)
- Mechanical traces of the agent itself: which tools were called, raw
  command output, approvals or forms that were shown, tooling errors.
- Opaque identifiers: request ids, tool_call ids, session ids, uuids,
  hashes, file paths, timestamps.
- Temporary state, progress, TODOs and one-off schedules: these expire
  and are NOT memory.
- Greetings, pleasantries, and one-off chatter.

Ask yourself for every candidate: "would a FUTURE, DIFFERENT session
still act on this?" If no, do not extract it.

## What to extract (durable memory)
- decisions made and their rationale ("we chose X over Y because …");
- user preferences and working style;
- standing instructions or constraints the user imposed;
- the user's own project conventions (layout, naming, workflow, toolchain);
- reusable procedures: command sequences, checklists, debugging flows;
- hard constraints imposed by the environment or external systems;
- stable facts about the user's own project / environment / toolchain;
- relationships between named entities that outlive the session.

## Granularity
- One unit = one independently queryable claim.
- But do NOT shred one topic into fragments: a coherent topic together
  with its rationale is ONE unit. Units sharing a subject must be merged.
- Budget: aim for 5–25 units for a whole session. Producing dozens of
  units means you are transcribing the session instead of extracting
  memory. Fewer, better units are strongly preferred.

## Fields
- Top-level key: "events".
- Each unit must include:
  - "event_text": a single canonical sentence describing one claim.
  - "frag_ids": supporting anchors copied VERBATIM from the bracketed
    labels in the transcript (e.g. ["m07","m08"]). Never invent one.
  - "event_type": one of {options}.
  - "confidence": a number in [0,1] — your own estimate that this is
    durable, reusable memory (not merely that it appeared in the text).
- Do NOT invent timestamps or facts not present in the transcript.
- Ignore the `【会话环境】`-style environment noise if any remains."#
    )
}

/// P1.5：精选阶段的 system 提示词（第二遍 LLM；去噪 + 合并同主题）。
///
/// 输入是**同一会话已抽出的候选单元**，输出是保留集（同 schema）。
/// 不重写好的单元、不引入新信息，只做取舍与归并。
pub fn curate_system() -> String {
    r#"You are a memory curator. You receive candidate memory units that
were extracted from ONE session, and you return the KEPT set.

Be ruthless about durability, and de-duplicate aggressively:
1. DROP every unit that fails the durability gate below.
2. MERGE units about the same topic into ONE unit (union their
   frag_ids; keep the clearest wording; do not lose a distinct claim).
3. KEEP the remaining units as they are — do not rewrite good units and
   do NOT invent new information.

## Durability gate — drop these
- Third-party project / doc / page details the session merely read:
  file paths, directory listings, install or build commands, licenses,
  homepages, version numbers, API or protocol field lists, changelogs.
- Mechanical traces of the agent: tool calls, raw output, approvals or
  forms shown, tooling errors.
- Opaque identifiers: request ids, tool_call ids, session ids, uuids,
  hashes, timestamps.
- Temporary state, progress, TODOs and one-off schedules: these expire
  and are NOT memory.
- Greetings, pleasantries, one-off chatter.
Keep only what a FUTURE, DIFFERENT session would still act on:
decisions + rationale, user preferences, standing instructions,
project conventions, reusable procedures, environmental constraints,
stable facts about the user's own project.

## Output
- Target 5–25 units. Fewer is better than noise.
- If the session truly has no durable content, return an empty array.
- JSON with top-level key "events"; per unit keep the input fields
  "event_text" and "frag_ids", plus "event_type" and "confidence"
  (confidence = your own durability estimate for the kept unit)."#
        .to_string()
}

/// P1.5 的 user 内容：会话上下文摘要 + 候选单元。
pub fn curate_user(session_context: &str, units: &[MemoryUnit]) -> String {
    let list = serde_json::to_string_pretty(units).unwrap_or_else(|_| "[]".into());
    format!(
        "Session context (excerpt):\n{}\n\nCandidate memory units ({}):\n{list}",
        truncate_chars(session_context, 1_200),
        units.len()
    )
}

/// 提取阶段的取值集（`- id: hint`）。
fn extract_options(categories: &CategoryTable) -> String {
    let lines: Vec<String> = categories
        .all()
        .iter()
        .map(|c| {
            if c.hint.is_empty() {
                format!("\"{}\"", c.id)
            } else {
                format!("\"{}\" ({})", c.id, c.hint)
            }
        })
        .collect();
    format!("[{}]", lines.join(", "))
}

/// P1 的 user 内容：raw 会话 Markdown（可选分块说明）。
pub fn extract_user(raw_markdown: &str, chunk: Option<(usize, usize)>) -> String {
    match chunk {
        Some((index, total)) => format!(
            "This is part {index} of {total} of a single session transcript. \
Extract units only from this part.\n\n{raw_markdown}"
        ),
        None => raw_markdown.to_string(),
    }
}

/// P1'：二遍自查的 system 提示词（默认关闭；开启后成本约 2×）。
pub const EXTRACT_SELF_CHECK_SYSTEM: &str = r#"You are a memory-unit quality auditor. You are given a session
transcript and a list of memory units already extracted from it.

Return only valid JSON with top-level key "events", containing the
units that the first pass MISSED or got wrong:
- atomic, queryable claims present in the transcript but absent from
  the list;
- claims that are not self-contained and should be split again.

Do NOT repeat units that are already covered. Return an empty
"events" array if nothing is missing. Same per-unit fields as before
("event_text", "frag_ids", "event_type", "confidence")."#;

/// P1' 的 user 内容。
pub fn extract_self_check_user(units: &[MemoryUnit], raw_markdown: &str) -> String {
    let list = serde_json::to_string_pretty(units).unwrap_or_else(|_| "[]".into());
    format!("Already extracted:\n{list}\n\nSession transcript:\n{raw_markdown}")
}

/// P2：编译阶段的 system 提示词（OKF 速查 + 编译器角色 + **动态目录映射表**）。
pub fn compile_system(categories: &CategoryTable) -> String {
    let mapping = categories.prompt_lines();
    format!(
        r#"You are an OKF Concept Compiler. You transform raw conversation
memory units into valid OKF concept documents.

{OKF_QUICK_REFERENCE}

Memory category mapping (choose the concept_id prefix by event_type):
{mapping}

Rules:
- Produce exactly ONE concept document per memory unit.
- Echo the unit's "event_type" VERBATIM as "event_type" (do not change
  it) — the caller uses it to apply the category prefix.
- Echo the unit's "frag_ids" VERBATIM as "frag_ids" — the caller uses
  them to derive the observation date and to keep the concept traceable
  back to individual messages.
- "type" is REQUIRED and must be non-empty. Use the mapped type value
  for the unit's event_type.
- Body is free-form Markdown: state the claim, then the supporting
  evidence. Keep it short; do not pad. Write in the language of the
  conversation (Chinese conversations get Chinese bodies).
- "sources" must list the raw session resource with resource, title,
  and last_modified where available.
- Do NOT emit "generated" — the caller fills it in.
- Carry "observed_at" through verbatim when the unit provides it.
- Use relative markdown links (./other.md, ../concepts/x.md) only.
  NEVER invent link schemes for source sessions: no "session:",
  "/session:", "astorm:" pseudo-links, and no date-only link targets
  (e.g. [原文](/session:2026-09-27) is WRONG). Cite a source session by
  plain text only (e.g. "依据：2026-09-27 的会话") — the caller appends
  the correct relative "原文" link itself.
- concept_id: a short descriptive slug, lowercase, hyphen-separated.
  CJK characters may be kept verbatim when the claim is Chinese.
  Use only the final path segment; the category prefix is applied by
  the caller and will be normalized.

Output JSON with top-level key "concepts", an array of:
{{ "event_type": "decision",
   "frag_ids": ["m07", "m08"],
   "concept_id": "use-jwt",
   "frontmatter": {{ "type": "...", "title": "...",
                    "description": "...", "tags": ["..."],
                    "observed_at": "YYYY-MM-DD" }},
   "body": "..." }}"#
    )
}

/// P2 的 user 内容。
pub fn compile_user(units: &[MemoryUnit]) -> String {
    let list = serde_json::to_string_pretty(units).unwrap_or_else(|_| "[]".into());
    format!("Memory units to compile:\n{list}")
}

/// P3：合并判定阶段的 system 提示词（OKF 速查 + 判定器角色）。
///
/// 只输出 `UPDATE` / `SKIP`：`CREATE` 由上层按「无候选」「空 decisions」判定。
pub fn judge_system() -> String {
    format!(
        r#"You are a memory consolidation judge. Given a NEW concept and a
list of EXISTING concepts from a knowledge base, decide the
relationship for each EXISTING concept.

{OKF_QUICK_REFERENCE}

Rules:
- The NEW concept describes the same fact/decision/preference as an
  EXISTING one                              -> UPDATE.
- The NEW concept adds information to an EXISTING concept -> UPDATE.
- The NEW concept is fully covered by an EXISTING concept and adds
  nothing new                               -> SKIP.
- Nothing in the list is related to the NEW concept
                                            -> return an empty
                                               "decisions" array.
- When evidence CONFLICTS, never SKIP and never silently pick one
  side: return UPDATE and state the conflict in "merge_notes".
- "merge_notes" (UPDATE only): the additional claim(s) to append, in
  one or two sentences. Do NOT rewrite existing prose.

Output JSON with top-level key "decisions":
[{{ "existing_concept_id": "...",
   "action": "UPDATE" | "SKIP",
   "reason": "...",
   "merge_notes": "..." }}]"#
    )
}

/// P3 的 user 内容：新概念 + 候选列表（frontmatter + 正文摘要）。
pub fn judge_user(new_concept: &ConceptDocument, candidates: &[CandidateHit]) -> String {
    let mut out = String::from("NEW concept:\n");
    out.push_str(&render_concept_for_judge(new_concept));
    out.push_str("\n\nEXISTING concepts:\n");
    for hit in candidates {
        out.push_str(&format!(
            "- concept_id: {}\n  type: {}\n  title: {}\n  description: {}\n  tags: [{}]\n  body_excerpt: {}\n",
            hit.concept_id,
            hit.type_name,
            hit.title,
            hit.description,
            hit.tags.join(", "),
            truncate_chars(&hit.body_excerpt, JUDGE_EXCERPT_CHARS).replace('\n', " ")
        ));
    }
    out
}

/// 把新概念渲染为判定输入（frontmatter 关键字段 + 正文摘要）。
fn render_concept_for_judge(doc: &ConceptDocument) -> String {
    format!(
        "concept_id: {}\ntype: {}\ntitle: {}\ndescription: {}\ntags: [{}]\nbody:\n{}",
        doc.concept_id,
        doc.frontmatter.concept_type,
        doc.frontmatter.title.clone().unwrap_or_default(),
        doc.frontmatter.description.clone().unwrap_or_default(),
        doc.frontmatter.tags.join(", "),
        truncate_chars(&doc.body, JUDGE_EXCERPT_CHARS)
    )
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concept::ConceptFrontmatter;
    use crate::config::MemoryConfig;

    fn table() -> CategoryTable {
        CategoryTable::new(MemoryConfig::default().categories)
    }

    fn unit(text: &str, ty: &str) -> MemoryUnit {
        MemoryUnit {
            event_text: text.into(),
            frag_ids: vec!["m07".into()],
            event_type: ty.into(),
            confidence: 0.9,
            observed_at: Some("2025-10-09".into()),
        }
    }

    fn concept(id: &str, type_name: &str, title: &str) -> ConceptDocument {
        ConceptDocument::new(
            id,
            ConceptFrontmatter {
                concept_type: type_name.into(),
                title: Some(title.into()),
                description: Some("描述".into()),
                tags: vec!["auth".into()],
                ..Default::default()
            },
            "正文内容",
        )
    }

    /// 提取阶段**不得**注入 OKF 速查（否则会把输出带向「概念形状」）。
    #[test]
    fn extract_prompt_has_no_okf_prefix() {
        let p = extract_system(&table());
        assert!(!p.contains("OKF v0.2 Quick Reference"), "提取阶段不应注入 OKF 速查");
        assert!(!p.contains("Concept ID"), "不应出现 OKF 概念术语");
        assert!(p.contains("\"events\""), "应声明输出契约");
        assert!(p.contains("frag_ids"));
        assert!(p.contains("confidence"));
        assert!(!p.contains("time_range"), "字段应与 MemoryUnit 结构体对齐");
        // 耐久性硬门槛必须在最前，且有明确反例清单与产出预算
        let gate = p.find("Hard gate").expect("应有硬门槛");
        let fields = p.find("## Fields").expect("应有字段说明");
        assert!(gate < fields, "硬门槛应出现在字段说明之前");
        assert!(p.contains("third-party"), "应点名第三方资料");
        assert!(p.contains("licenses"), "应点名许可证这类元数据");
        assert!(p.contains("5–25 units"), "应给出产出预算");
    }

    /// 删除 `status` 分类不够 —— 必须在 reject 清单里点名，否则模型仍会抽会过期的东西。
    #[test]
    fn extract_prompt_rejects_temporary_state() {
        let p = extract_system(&table());
        let gate = p.find("Hard gate").expect("应有硬门槛");
        let fields = p.find("## Fields").expect("应有字段说明");
        let temp = p.find("Temporary state").expect("硬门槛应点名临时状态/进度/待办");
        assert!(gate < temp && temp < fields, "临时状态应在硬门槛段内");
        assert!(p.contains("are NOT memory"));
        // 新增分类的抽取信号也要出现，否则模型不知道这些该抽
        for needle in ["reusable procedures", "hard constraints", "project conventions"] {
            assert!(p.contains(needle), "抽取信号缺 {needle}");
        }
    }

    /// 取值集由分类表动态生成 ⇒ 配置里加分类无需改提示词代码。
    #[test]
    fn extract_prompt_options_come_from_category_table() {
        let p = extract_system(&table());
        for id in [
            "decision",
            "preference",
            "instruction",
            "convention",
            "procedure",
            "constraint",
            "fact",
            "relationship",
            "other",
        ] {
            assert!(p.contains(&format!("\"{id}\"")), "取值集缺 {id}: {p}");
        }
        assert!(p.contains("一次性的选择及其理由"), "应带上 hint 帮助模型判断");
        assert!(
            !p.contains("\"status\""),
            "临时状态/进度已不属耐久记忆，不得再是取值集: {p}"
        );
        assert!(!p.contains("episodic"), "不得残留方案文档的旧取值集");

        // 自定义分类立即生效
        let custom = CategoryTable::new(vec![crate::config::MemoryCategory {
            id: "runbook".into(),
            dir: "runbooks".into(),
            type_name: "Runbook".into(),
            hint: "运维步骤".into(),
        }]);
        let p = extract_system(&custom);
        assert!(p.contains("\"runbook\""));
        assert!(p.contains("运维步骤"));
        assert!(!p.contains("\"decision\""), "配置替换后不应再出现默认分类");
    }

    #[test]
    fn curate_prompt_is_ruthless_and_merge_oriented() {
        let p = curate_system();
        assert!(p.contains("memory curator"));
        assert!(p.contains("DROP"), "应有丢弃指令");
        assert!(p.contains("MERGE"), "应有合并同主题指令");
        assert!(p.contains("do NOT invent new information"));
        assert!(p.contains("empty array"), "应允许「无耐久内容」");
        assert!(p.contains("5–25 units"));
        // 与提取提示词共用同一套反例
        for needle in ["licenses", "tool_call ids", "Temporary state", "Greetings"] {
            assert!(p.contains(needle), "精选提示词缺反例 {needle}");
        }

        let user = curate_user("会话上下文", &[unit("选了 JWT", "decision")]);
        assert!(user.contains("Session context"));
        assert!(user.contains("选了 JWT"));
        assert!(user.contains("Candidate memory units (1)"));
    }

    #[test]
    fn extract_user_marks_chunk_position() {
        let single = extract_user("# 会话\n\n内容", None);
        assert!(single.starts_with("# 会话"));
        let chunked = extract_user("内容", Some((2, 3)));
        assert!(chunked.starts_with("This is part 2 of 3"), "{chunked}");
        assert!(chunked.contains("Extract units only from this part."));
    }

    #[test]
    fn self_check_prompt_lists_already_extracted() {
        let units = vec![unit("选了 JWT", "decision")];
        let user = extract_self_check_user(&units, "原文");
        assert!(user.contains("选了 JWT"));
        assert!(user.contains("原文"));
        assert!(EXTRACT_SELF_CHECK_SYSTEM.contains("\"events\""));
        assert!(EXTRACT_SELF_CHECK_SYSTEM.contains("empty"), "应允许「无遗漏」");
    }

    /// 编译与判定**必须**注入 OKF 速查 + 动态目录映射。
    #[test]
    fn compile_prompt_has_okf_prefix_and_dynamic_mapping() {
        let p = compile_system(&table());
        assert!(p.contains("OKF v0.2 Quick Reference"), "编译阶段应注入 OKF 速查");
        assert!(p.contains("type: <REQUIRED, non-empty string>"));
        assert!(p.contains("Consumers MUST tolerate unknown types"));
        // 动态映射表
        for id in [
            "decision",
            "preference",
            "instruction",
            "convention",
            "procedure",
            "constraint",
            "fact",
            "relationship",
        ] {
            assert!(p.contains(id), "映射表缺 {id}");
        }
        assert!(p.contains("decisions/"), "映射表应给出目录");
        assert!(p.contains("procedures/"), "新分类应给出目录");
        assert!(p.contains("type: Entity"), "relationship 的 type 应为 Entity");
        // 注意 OKF 速查里合法地含有 `status:` frontmatter 键，故只查映射表行
        assert!(
            !p.contains("- status（type:"),
            "status 已不是默认分类，不应出现在映射表"
        );
        assert!(p.contains("Do NOT emit \"generated\""), "generated 由调用方填");
        assert!(p.contains("observed_at"), "应要求透传 observed_at");
        assert!(
            p.contains("NEVER invent link schemes"),
            "应禁止模型自造 session:/伪链接，避免前端把引用当外链"
        );

        // 自定义分类的映射立即生效
        let custom = CategoryTable::new(vec![crate::config::MemoryCategory {
            id: "note".into(),
            dir: "notes".into(),
            type_name: "Note".into(),
            hint: "随手记".into(),
        }]);
        let p = compile_system(&custom);
        assert!(p.contains("notes/"));
        assert!(p.contains("note"));
    }

    #[test]
    fn compile_user_serializes_units() {
        let user = compile_user(&[unit("选了 JWT", "decision")]);
        assert!(user.contains("选了 JWT"));
        assert!(user.contains("decision"));
        assert!(user.contains("observed_at"), "应把推导出的观察日期传给编译阶段");
    }

    /// 判定阶段只允许 UPDATE / SKIP —— CREATE 由上层判定，消除歧义。
    #[test]
    fn judge_prompt_excludes_create_action() {
        let p = judge_system();
        assert!(p.contains("OKF v0.2 Quick Reference"));
        assert!(p.contains("\"UPDATE\" | \"SKIP\""), "动作集应只含 UPDATE / SKIP");
        assert!(!p.contains("\"CREATE\""), "不得让模型输出 CREATE");
        assert!(p.contains("empty\n                                               \"decisions\""), "应允许空数组表示无关");
        assert!(p.contains("never SKIP") || p.contains("never SKIP and never silently"), "冲突不得 SKIP");
        assert!(p.contains("Do NOT rewrite existing prose"));
    }

    #[test]
    fn judge_user_includes_new_concept_and_candidates() {
        let new_doc = concept("decisions/use-jwt", "Decision", "用 JWT");
        let candidates = vec![CandidateHit {
            concept_id: "decisions/auth".into(),
            score: 1.5,
            type_name: "Decision".into(),
            title: "认证方案".into(),
            description: "旧决策".into(),
            tags: vec!["auth".into()],
            body_excerpt: "很长的正文".repeat(500),
        }];
        let user = judge_user(&new_doc, &candidates);
        assert!(user.contains("NEW concept:"));
        assert!(user.contains("EXISTING concepts:"));
        assert!(user.contains("decisions/use-jwt"));
        assert!(user.contains("decisions/auth"));
        assert!(user.contains("body_excerpt:"));
        // 候选正文被截断（否则长 wiki 会把上下文撑爆）
        assert!(user.chars().count() < 3_000, "候选正文应被截断: {} 字符", user.chars().count());
    }

    #[test]
    fn judge_user_handles_no_candidates() {
        let user = judge_user(&concept("decisions/x", "Decision", "X"), &[]);
        assert!(user.contains("EXISTING concepts:"));
        assert!(!user.contains("body_excerpt"));
    }

    #[test]
    fn truncate_is_utf8_safe() {
        assert_eq!(truncate_chars("中文测试", 2), "中文");
        assert_eq!(truncate_chars("ab", 5), "ab");
    }
}
