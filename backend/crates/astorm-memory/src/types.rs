//! 记忆管道的中间类型：原子记忆单元、分类查表、编译报告、进度事件、日志条目。
//!
//! 这些类型是**管道内部契约**（提取 → 编译 → 合并 → 写入）；对前端的 DTO 镜子在 napi 层
//! （`astorm-napi/src/memory/dto.rs`）——core 不依赖 napi。

use serde::{Deserialize, Serialize};

use crate::config::{MemoryCategory, MemoryConfig};

/// 未给出 `confidence` 时的中性默认值。
fn default_confidence() -> f32 {
    0.5
}

/// 原子记忆单元：**一条自包含的声明**（Extractor 产出，Compiler 消费）。
///
/// 粒度要求「一条一个声明」；`frag_ids` 是归一化 raw 正文里的消息锚点（`m07`），
/// 支撑该声明的原文，供回溯与去重。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryUnit {
    /// 一条自包含的声明（规范化单句）。
    pub event_text: String,
    /// 支撑该声明的片段锚点（来自 raw 正文的 `[mNN]` 标签，逐字复制）。
    #[serde(default)]
    pub frag_ids: Vec<String>,
    /// 记忆分类 id（[`MemoryCategory::id`]；LLM 输出未知值时由 [`CategoryTable::resolve`] 兜底）。
    #[serde(default)]
    pub event_type: String,
    /// LLM 自评置信度（`[0,1]`）。
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    /// 观察日期（ISO `YYYY-MM-DD`）；由 `frag_ids` → 消息 `createdAt` **确定性推导**，
    /// 故只序列化（供编译阶段写入 frontmatter）不反序列化（LLM 不得自造时间）。
    #[serde(default, skip_deserializing)]
    pub observed_at: Option<String>,
}

impl MemoryUnit {
    /// 用于批内 / 跨批去重的归一化键（忽略大小写与首尾空白）。
    pub fn dedup_key(&self) -> String {
        self.event_text.trim().to_lowercase()
    }
}

/// 分类查表：包装规范化后的分类表，提供 id 查表与**未知值兜底**。
///
/// OKF 消费者必须容忍未知 type，故 LLM 返回未知 `event_type` 时**不丢弃该记忆**，
/// 而是回退到 `other` 分类（无则第一条）。
#[derive(Debug, Clone)]
pub struct CategoryTable {
    categories: Vec<MemoryCategory>,
}

impl CategoryTable {
    /// 直接以**已规范化**的分类表构建（空表回退默认表）。
    pub fn new(categories: Vec<MemoryCategory>) -> Self {
        let cfg = MemoryConfig {
            categories,
            ..MemoryConfig::default()
        };
        Self {
            categories: cfg.effective_categories(),
        }
    }

    /// 由记忆配置构建（内部走 [`MemoryConfig::effective_categories`] 规范化）。
    pub fn from_config(cfg: &MemoryConfig) -> Self {
        Self {
            categories: cfg.effective_categories(),
        }
    }

    /// 全部分类（升序外的配置顺序，即提示词取值集的呈现顺序）。
    pub fn all(&self) -> &[MemoryCategory] {
        &self.categories
    }

    /// 是否为空（构造时已回退默认表，故恒非空——保留给防御性判断）。
    pub fn is_empty(&self) -> bool {
        self.categories.is_empty()
    }

    /// 按 id 查表（大小写不敏感、去首尾空白）。
    pub fn get(&self, id: &str) -> Option<&MemoryCategory> {
        let needle = id.trim().to_lowercase();
        self.categories.iter().find(|c| c.id == needle)
    }

    /// 按 id 解析分类；未知 / 空 id → 兜底分类（`other` 优先，否则第一条）。
    pub fn resolve(&self, id: &str) -> &MemoryCategory {
        self.get(id).unwrap_or_else(|| self.fallback())
    }

    /// 兜底分类（`other` 优先，否则第一条；分类表恒非空）。
    pub fn fallback(&self) -> &MemoryCategory {
        self.categories
            .iter()
            .find(|c| c.id == "other")
            .or_else(|| self.categories.first())
            .expect("分类表非空（构造时已回退默认表）")
    }

    /// 分类目录名集合（去重、去空；用于预建骨架目录）。
    pub fn dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = self
            .categories
            .iter()
            .map(|c| c.dir.clone())
            .filter(|d| !d.is_empty())
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// 提示词用取值集：`- <id>（<type>）: <hint>` 逐行（编译阶段的目录映射用）。
    pub fn prompt_lines(&self) -> String {
        self.categories
            .iter()
            .map(|c| {
                let dir = if c.dir.is_empty() {
                    "（wiki 根）".to_string()
                } else {
                    format!("{}/", c.dir)
                };
                if c.hint.is_empty() {
                    format!("- {}（type: {}）→ {}", c.id, c.type_name, dir)
                } else {
                    format!("- {}（type: {}）→ {}  —— {}", c.id, c.type_name, dir, c.hint)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 合并候选命中（BM25 召回 + 批内 overlay）。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateHit {
    /// 已有概念 ID。
    pub concept_id: String,
    /// 相关度评分（BM25；overlay 命中为确定性打分）。
    pub score: f32,
    /// OKF `type` 值（用于类型冲突判定）。
    pub type_name: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// 正文摘要（截断；供合并判定判断「是否已完全覆盖」）。
    pub body_excerpt: String,
}

/// 概念摘要（列表 / 检索结果 / 索引重建）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConceptSummary {
    /// Concept ID（相对 `wiki/` 去 `.md`）。
    pub concept_id: String,
    /// 分类目录名（空 = wiki 根）。
    pub dir: String,
    /// 分类 id（由目录反查；目录未配置分类时为空串）。
    pub category_id: String,
    /// OKF `type` 值。
    pub type_name: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// 观察日期（frontmatter 扩展字段 `observed_at`）。
    pub observed_at: Option<String>,
    /// 最后写入时间（unix 秒）。
    pub updated_at: i64,
    /// 检索相关度（仅检索结果有值）。
    pub score: Option<f32>,
    /// 命中字段（`title` / `description` / `tags` / `body`；仅检索结果有值）。
    pub matched: Vec<String>,
}

/// 单个会话处理失败记录（不中断整体编译）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFailure {
    pub session_id: String,
    /// 失败阶段：`parse` / `extract` / `compile` / `merge` / `write`。
    pub stage: String,
    pub message: String,
}

/// 一次编译的汇总报告（`memoryCompile` 的结果与页面展示）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileReport {
    /// 扫描到的源会话总数。
    pub total_sessions: usize,
    /// 实际处理的会话数（哈希变化或 `force`）。
    pub processed_sessions: usize,
    /// 哈希未变而跳过的会话数。
    pub skipped_sessions: usize,
    /// 处理失败的会话（部分失败不中断整体）。
    pub failed_sessions: Vec<SessionFailure>,
    /// 提取出的记忆单元数。
    pub extracted_units: usize,
    /// 编译产出的概念数（含无效被拦截前的产出）。
    pub compiled_concepts: usize,
    /// 新建的概念 ID。
    pub created: Vec<String>,
    /// 更新的概念 ID。
    pub updated: Vec<String>,
    /// 判定为已覆盖而跳过的概念 ID。
    pub skipped: Vec<String>,
    /// 告警（类型冲突、无效产出、索引不一致等）。
    pub warnings: Vec<String>,
    /// LLM 调用次数。
    pub calls: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 是否被取消（取消时已落盘的内容保留）。
    pub cancelled: bool,
}

/// 进度阶段（与页面上的阶段指示一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    /// 归一化来源会话。
    Parse,
    /// LLM 提取原子记忆。
    Extract,
    /// LLM 编译为 OKF 概念。
    Compile,
    /// 语义比对与 upsert（含 BM25 召回）。
    Merge,
    /// 落盘与索引维护。
    Write,
}

impl ProgressStage {
    /// 阶段标识（事件 JSON 的 `stage` 值）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgressStage::Parse => "parse",
            ProgressStage::Extract => "extract",
            ProgressStage::Compile => "compile",
            ProgressStage::Merge => "merge",
            ProgressStage::Write => "write",
        }
    }
}

/// 进度事件（管道 → 门面 → 事件总线 → 记忆页）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressEvent {
    pub stage: ProgressStage,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

impl ProgressEvent {
    pub fn new(stage: ProgressStage, current: usize, total: usize, message: impl Into<String>) -> Self {
        Self {
            stage,
            current,
            total,
            message: message.into(),
        }
    }
}

/// 变更历史条目（`wiki/log.md` 一行）。
///
/// 行格式（无 frontmatter、无引号包裹，便于逐行解析）：
/// ```text
/// - 2026-09-25T10:00:00Z CREATE decisions/use-jwt ← raw/sessions/2026-09-25-a1f9.md
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// 操作时间（RFC3339）。
    pub at: String,
    /// 操作类型：`CREATE` / `UPDATE` / `SKIP`。
    pub action: String,
    pub concept_id: String,
    /// 来源 raw 会话文件（相对 bundle 根，如 `raw/sessions/xxx.md`）。
    pub resource: String,
    /// 判定理由（`UPDATE` / `SKIP` 时的 LLM 说明）。
    #[serde(default)]
    pub reason: String,
}

impl LogEntry {
    /// 序列化为 `log.md` 一行（`reason` 不落盘，避免破坏逐行解析）。
    pub fn to_line(&self) -> String {
        format!(
            "- {} {} {} ← {}",
            self.at, self.action, self.concept_id, self.resource
        )
    }

    /// 从 `log.md` 一行解析（不匹配返回 None）。
    pub fn parse_line(line: &str) -> Option<Self> {
        let rest = line.trim().strip_prefix("- ")?;
        let mut parts = rest.split_whitespace();
        let at = parts.next()?.to_string();
        let action = parts.next()?.to_string();
        let concept_id = parts.next()?.to_string();
        // 第 4 段必须是箭头
        if parts.next()? != "←" {
            return None;
        }
        let resource = parts.next()?.to_string();
        Some(Self {
            at,
            action,
            concept_id,
            resource,
            reason: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_table_resolves_unknown_to_other() {
        let table = CategoryTable::new(MemoryConfig::default().categories);
        assert_eq!(table.resolve("decision").id, "decision");
        assert_eq!(table.resolve("DECISION").id, "decision", "大小写不敏感");
        assert_eq!(table.resolve("  fact  ").id, "fact", "去首尾空白");
        assert_eq!(table.resolve("episodic").id, "other", "未知值兜底到 other");
        assert_eq!(table.resolve("").id, "other", "空值兜底到 other");
    }

    #[test]
    fn category_table_without_other_falls_back_to_first() {
        let table = CategoryTable::new(vec![MemoryCategory {
            id: "note".into(),
            dir: "notes".into(),
            type_name: "Note".into(),
            hint: String::new(),
        }]);
        assert_eq!(table.resolve("unknown").id, "note");
        assert_eq!(table.fallback().id, "note");
    }

    #[test]
    fn empty_category_config_falls_back_to_defaults() {
        let table = CategoryTable::new(Vec::new());
        assert_eq!(table.all().len(), MemoryConfig::default_categories().len());
        assert_eq!(table.resolve("decision").dir, "decisions");
    }

    #[test]
    fn category_dirs_dedup_and_drop_empty() {
        let table = CategoryTable::new(MemoryConfig::default().categories);
        // 9 个分类 → 8 个目录（concepts 被 fact/other 共用）
        assert_eq!(
            table.dirs(),
            vec![
                "concepts",
                "constraints",
                "conventions",
                "decisions",
                "entities",
                "instructions",
                "preferences",
                "procedures"
            ]
        );
        // 空 dir（wiki 根）不出现在预建目录列表
        let root_table = CategoryTable::new(vec![MemoryCategory {
            id: "misc".into(),
            dir: String::new(),
            type_name: "Misc".into(),
            hint: String::new(),
        }]);
        assert!(root_table.dirs().is_empty());
    }

    #[test]
    fn prompt_lines_cover_every_category() {
        let table = CategoryTable::new(MemoryConfig::default().categories);
        let lines = table.prompt_lines();
        for c in table.all() {
            assert!(lines.contains(&c.id), "取值集缺 {}", c.id);
            assert!(lines.contains(&c.type_name), "取值集缺 type {}", c.type_name);
        }
        // wiki 根分类的目录提示
        let root_table = CategoryTable::new(vec![MemoryCategory {
            id: "misc".into(),
            dir: String::new(),
            type_name: "Misc".into(),
            hint: "杂项".into(),
        }]);
        assert!(root_table.prompt_lines().contains("wiki 根"));
    }

    #[test]
    fn memory_unit_ignores_llm_invented_observed_at() {
        // LLM 输出里带 observed_at 也不生效（skip_deserializing）
        let json = r#"{
            "event_text": "选了 JWT",
            "frag_ids": ["m07"],
            "event_type": "decision",
            "confidence": 0.9,
            "observed_at": "1900-01-01"
        }"#;
        let unit: MemoryUnit = serde_json::from_str(json).unwrap();
        assert_eq!(unit.event_type, "decision");
        assert_eq!(unit.confidence, 0.9);
        assert_eq!(unit.observed_at, None, "observed_at 必须由代码推导，不接受 LLM 值");
    }

    #[test]
    fn memory_unit_defaults_are_lenient() {
        let unit: MemoryUnit = serde_json::from_str(r#"{"event_text":"只有正文"}"#).unwrap();
        assert!(unit.frag_ids.is_empty());
        assert_eq!(unit.event_type, "");
        assert_eq!(unit.confidence, 0.5);
        assert_eq!(unit.dedup_key(), "只有正文");
        assert_eq!(unit.dedup_key(), MemoryUnit {
            event_text: "  只有正文  ".into(),
            ..unit.clone()
        }
        .dedup_key(), "去重键应忽略首尾空白");
    }

    #[test]
    fn memory_unit_round_trips_with_observed_at() {
        let unit = MemoryUnit {
            event_text: "事实".into(),
            frag_ids: vec!["m01".into()],
            event_type: "fact".into(),
            confidence: 0.8,
            observed_at: Some("2026-09-25".into()),
        };
        let json = serde_json::to_string(&unit).unwrap();
        assert!(json.contains("observed_at"), "observed_at 需序列化给编译阶段: {json}");
        let back: MemoryUnit = serde_json::from_str(&json).unwrap();
        assert_eq!(back.observed_at, None, "回读时由代码重新推导");
        assert_eq!(back.event_text, unit.event_text);
    }

    #[test]
    fn log_entry_round_trips() {
        let entry = LogEntry {
            at: "2026-09-25T10:00:00Z".into(),
            action: "CREATE".into(),
            concept_id: "decisions/use-jwt".into(),
            resource: "raw/sessions/2026-09-25-a1f9.md".into(),
            reason: String::new(),
        };
        let line = entry.to_line();
        assert_eq!(
            line,
            "- 2026-09-25T10:00:00Z CREATE decisions/use-jwt ← raw/sessions/2026-09-25-a1f9.md"
        );
        let parsed = LogEntry::parse_line(&line).unwrap();
        assert_eq!(parsed.at, entry.at);
        assert_eq!(parsed.action, entry.action);
        assert_eq!(parsed.concept_id, entry.concept_id);
        assert_eq!(parsed.resource, entry.resource);
    }

    #[test]
    fn log_entry_rejects_non_entry_lines() {
        assert!(LogEntry::parse_line("# 变更历史").is_none());
        assert!(LogEntry::parse_line("").is_none());
        assert!(LogEntry::parse_line("- 2026-09-25T10:00:00Z CREATE decisions/x").is_none());
        // 缺少箭头
        assert!(LogEntry::parse_line("- ts CREATE id raw/x.md").is_none());
    }

    #[test]
    fn progress_stage_labels_match_event_contract() {
        assert_eq!(ProgressStage::Parse.as_str(), "parse");
        assert_eq!(ProgressStage::Extract.as_str(), "extract");
        assert_eq!(ProgressStage::Compile.as_str(), "compile");
        assert_eq!(ProgressStage::Merge.as_str(), "merge");
        assert_eq!(ProgressStage::Write.as_str(), "write");
    }
}
