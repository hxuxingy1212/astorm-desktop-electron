//! 长期记忆（memory_*）DTO 镜像。
//!
//! 定义全部在 `astorm-core` 门面（`facade::memory`）与 `astorm-memory`，此处只做
//! `#[napi(object)]` 镜像与字段改名，**不含任何业务判断**。

use astorm_core::facade::memory::{
    ConceptSummary as CoreSummary, IssueLevel, LogEntry as CoreLogEntry,
    MemoryCategoryStat as CoreCategoryStat, MemoryConceptDetail as CoreDetail,
    MemoryRawSource as CoreRawSource, MemorySourceRef as CoreSourceRef, MemoryStatus as CoreStatus,
    ValidateReport as CoreValidateReport,
};
use napi_derive::napi;
use serde::Serialize;

/// 记忆库状态（记忆页顶部状态卡）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    /// Bundle 根绝对路径。
    pub root: String,
    /// 记忆功能是否启用。
    pub enabled: bool,
    /// 会话结束后是否自动增量编译。
    pub auto_compile: bool,
    /// 是否有编译正在进行。
    pub running: bool,
    /// 概念总数。
    pub concept_total: u32,
    /// 待编译会话数（不花 token）。
    pub pending: u32,
    /// 上次编译成功时间（RFC3339）。
    pub last_sync: Option<String>,
    /// 分类统计（按分类表顺序）。
    pub categories: Vec<MemoryCategoryStat>,
    /// 已落盘概念的解析错误（损坏文件）。
    pub parse_errors: Vec<String>,
}

impl From<CoreStatus> for MemoryStatus {
    fn from(v: CoreStatus) -> Self {
        Self {
            root: v.root,
            enabled: v.enabled,
            auto_compile: v.auto_compile,
            running: v.running,
            concept_total: v.concept_total as u32,
            pending: v.pending as u32,
            last_sync: v.last_sync,
            categories: v.categories.into_iter().map(Into::into).collect(),
            parse_errors: v.parse_errors,
        }
    }
}

/// 单个分类的统计。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCategoryStat {
    /// 分类 id（提取阶段的 `event_type` 取值）。
    pub id: String,
    /// 目标目录（相对 `wiki/`）。
    pub dir: String,
    /// OKF `type` 值。
    ///
    /// `#[napi(object)]` 只认 Rust 字段名（→ `typeName`），serde 的 `rename` 不参与，
    /// 故用 `js_name` 显式对齐前端字段名 `type`（OKF frontmatter 的键名）。
    #[napi(js_name = "type")]
    #[serde(rename = "type")]
    pub type_name: String,
    /// 语义说明。
    pub hint: String,
    /// 该分类下的概念数。
    pub count: u32,
}

impl From<CoreCategoryStat> for MemoryCategoryStat {
    fn from(v: CoreCategoryStat) -> Self {
        Self {
            id: v.id,
            dir: v.dir,
            type_name: v.type_name,
            hint: v.hint,
            count: v.count as u32,
        }
    }
}

/// 概念列表项。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConcept {
    /// Concept ID（相对 `wiki/` 去 `.md`）。
    pub concept_id: String,
    /// 分类目录名（空 = wiki 根）。
    pub dir: String,
    /// 分类 id（目录未配置分类时为空串）。
    pub category_id: String,
    /// OKF `type` 值。
    ///
    /// `#[napi(object)]` 只认 Rust 字段名（→ `typeName`），serde 的 `rename` 不参与，
    /// 故用 `js_name` 显式对齐前端字段名 `type`（OKF frontmatter 的键名）。
    #[napi(js_name = "type")]
    #[serde(rename = "type")]
    pub type_name: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// 观察日期（frontmatter `observed_at`）。
    pub observed_at: Option<String>,
    /// 最后写入时间（unix 秒）。
    pub updated_at: i64,
    /// 检索相关度（仅检索结果有值）。
    pub score: Option<f64>,
    /// 命中字段（`title` / `description` / `tags` / `body`；仅检索结果有值）。
    pub matched: Vec<String>,
}

impl From<CoreSummary> for MemoryConcept {
    fn from(v: CoreSummary) -> Self {
        Self {
            concept_id: v.concept_id,
            dir: v.dir,
            category_id: v.category_id,
            type_name: v.type_name,
            title: v.title,
            description: v.description,
            tags: v.tags,
            observed_at: v.observed_at,
            updated_at: v.updated_at,
            score: v.score.map(|s| s as f64),
            matched: v.matched,
        }
    }
}

/// 概念详情（含完整 Markdown 原文）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConceptDetail {
    pub concept_id: String,
    pub dir: String,
    pub category_id: String,
    #[napi(js_name = "type")]
    #[serde(rename = "type")]
    pub type_name: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub status: Option<String>,
    pub stale_after: Option<String>,
    pub observed_at: Option<String>,
    /// 支撑锚点（`### [mNN]`）：前端据此渲染「依据」小丸并跳回原文对应行。
    /// 权威依据集（`observed_at` 即由它推导），不采信正文里模型自述的「依据：mNN」。
    pub frag_ids: Vec<String>,
    pub sources: Vec<MemorySourceRef>,
    pub generated_by: Option<String>,
    pub generated_at: Option<String>,
    pub model: Option<String>,
    pub updated_at: i64,
    /// 完整 Markdown 原文（含 frontmatter）。
    pub markdown: String,
}

impl From<CoreDetail> for MemoryConceptDetail {
    fn from(v: CoreDetail) -> Self {
        Self {
            concept_id: v.concept_id,
            dir: v.dir,
            category_id: v.category_id,
            type_name: v.type_name,
            title: v.title,
            description: v.description,
            tags: v.tags,
            status: v.status,
            stale_after: v.stale_after,
            observed_at: v.observed_at,
            frag_ids: v.frag_ids,
            sources: v.sources.into_iter().map(Into::into).collect(),
            generated_by: v.generated_by,
            generated_at: v.generated_at,
            model: v.model,
            updated_at: v.updated_at,
            markdown: v.markdown,
        }
    }
}

/// 概念的来源引用。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySourceRef {
    pub resource: String,
    pub title: Option<String>,
    pub last_modified: Option<String>,
}

impl From<CoreSourceRef> for MemorySourceRef {
    fn from(v: CoreSourceRef) -> Self {
        Self {
            resource: v.resource,
            title: v.title,
            last_modified: v.last_modified,
        }
    }
}

/// 源会话（`raw/sessions/*.md`）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRawSource {
    pub stem: String,
    pub resource: String,
    pub size: i64,
    pub updated_at: i64,
}

impl From<CoreRawSource> for MemoryRawSource {
    fn from(v: CoreRawSource) -> Self {
        Self {
            stem: v.stem,
            resource: v.resource,
            size: v.size,
            updated_at: v.updated_at,
        }
    }
}

/// 变更历史条目（`wiki/log.md` 一行）。
///
/// **不含 `reason`**：`log.md` 的行格式是 `- <时间> <动作> <概念ID> ← <来源>`，出于体积考虑
/// 不落变更原因，`LogEntry::parse_line` 读回来恒为空串（`MemoryLogEntry` 不再透出这个空字段）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryLogEntry {
    /// 时间（RFC3339）。
    pub at: String,
    /// 动作：`CREATE` / `UPDATE` / `SKIP`。
    pub action: String,
    pub concept_id: String,
    /// 来源资源（`raw/sessions/xxx.md`）。
    pub resource: String,
}

impl From<CoreLogEntry> for MemoryLogEntry {
    fn from(v: CoreLogEntry) -> Self {
        Self {
            at: v.at,
            action: v.action,
            concept_id: v.concept_id,
            resource: v.resource,
        }
    }
}

/// 单条校验问题。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIssue {
    /// "error" | "warning"
    pub level: String,
    pub concept_id: String,
    pub message: String,
}

/// OKF 合规校验报告。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryValidateReport {
    pub checked: u32,
    pub errors: Vec<MemoryIssue>,
    pub warnings: Vec<MemoryIssue>,
}

impl From<CoreValidateReport> for MemoryValidateReport {
    fn from(v: CoreValidateReport) -> Self {
        let map = |list: Vec<astorm_core::facade::memory::ValidateIssue>, level: &str| {
            list.into_iter()
                .map(|i| MemoryIssue {
                    level: level.to_string(),
                    concept_id: i.concept_id,
                    message: i.message,
                })
                .collect::<Vec<_>>()
        };
        let (errors, warnings): (Vec<_>, Vec<_>) =
            v.issues.into_iter().partition(|i| i.level == IssueLevel::Error);
        Self {
            checked: v.checked as u32,
            errors: map(errors, "error"),
            warnings: map(warnings, "warning"),
        }
    }
}
