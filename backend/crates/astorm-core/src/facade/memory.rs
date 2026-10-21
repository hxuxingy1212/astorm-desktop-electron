//! 记忆门面：把**应用设定**解析为 `astorm-memory` 的配置与输入，驱动 OKF 编译管道。
//!
//! 职责边界（模块解耦）：记忆语义（归一化 / 提示词 / 提取 / 编译 / 合并 / 索引 / 校验）
//! 全部在 `astorm-memory` crate，本模块**不含任何记忆逻辑**，只做三件「应用集成」的事：
//!
//! 1. **配置解析**：`AppConfig.memory` → `MemoryConfig` / `CategoryTable`
//! 2. **输入组织**：`<data>/session/<id>/message.jsonl` + sqlite 里的会话标题 → `SessionInput`
//! 3. **宿主适配**：复用应用**已配置**的 LLM 客户端（密钥已解密）、把进度转发到全局事件总线、
//!    在应用 runtime 上异步执行并支持整体取消
//!
//! Bundle 根 = `<用户目录>/memory`（[`AppDir::memory_dir`]），与知识库（`knowledge/`）无关。
//! 索引落在 Bundle 内部（`<bundle>/.okf-index`）——纯派生数据、随 Bundle 同生共死；因此
//! 这里**按操作临时打开索引**而不长驻句柄（tantivy 目录带独占锁，长驻会与 CLI 互斥）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use astorm_memory::config::MemoryConfig;
use astorm_memory::concept::ConceptDocument;
use astorm_memory::index::MemoryIndex;
use astorm_memory::llm::ChatLlm;
use astorm_memory::paths::{BundlePaths, INDEX_DIR_NAME};
use astorm_memory::pipeline::{self, CompileOptions, SessionInput};
use astorm_memory::state::{SourceStamp, SyncState};
use astorm_memory::types::{CategoryTable, ProgressEvent, ProgressStage};
use astorm_memory::validate::validate_bundle;
use astorm_memory::writer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::app::{App, AppState};
use crate::util::text::truncate_chars;

// 视图类型直接复用记忆 crate 的定义（再导出，避免 napi 层另建一套平行结构而漂移）。
pub use astorm_memory::types::{CompileReport, ConceptSummary, LogEntry};
pub use astorm_memory::validate::{IssueLevel, ValidateIssue, ValidateReport};

/// 记忆事件的 `type` 值（前端 `type === 'memory'` 分发）。
pub const MEMORY_EVENT: &str = "memory";

/// `memory` 工具的注册名（会话工具集 / 注入门禁都以它为准）。
pub const MEMORY_TOOL_ID: &str = "memory";
/// 注入块里单条标题 / 描述的字符上限。
const DIGEST_TITLE_CHARS: usize = 60;
const DIGEST_DESC_CHARS: usize = 90;
/// 注入块整体字符上限（兜底：即便条数已限，字段也可能偏长）。
const DIGEST_MAX_CHARS: usize = 1_200;

// ---------------------------------------------------------------------------
// 请求 / 视图 DTO（napi 层镜像为 `#[napi(object)]`）
// ---------------------------------------------------------------------------

/// 编译请求（前端两个按钮 → 同一条命令）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MemoryCompileRequest {
    /// 忽略增量状态，强制重编译全部会话。
    pub force: bool,
    /// 只跑确定性阶段（不调用 LLM），用于审阅提示词。
    pub dry_run: bool,
}

/// 记忆库状态（记忆页顶部状态卡）。
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
    pub concept_total: usize,
    /// 待编译会话数（源指纹与状态比对，不花 token）。
    pub pending: usize,
    /// 上次编译成功时间（RFC3339）。
    pub last_sync: Option<String>,
    /// 分类统计（按分类表顺序）。
    pub categories: Vec<MemoryCategoryStat>,
    /// 已落盘概念的解析错误（损坏文件；不阻断其他概念）。
    pub parse_errors: Vec<String>,
}

/// 单个分类的统计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCategoryStat {
    /// 分类 id（提取阶段的 `event_type` 取值）。
    pub id: String,
    /// 目标目录（相对 `wiki/`；空 = wiki 根）。
    pub dir: String,
    /// OKF `type` 值。
    pub type_name: String,
    /// 语义说明。
    pub hint: String,
    /// 该分类下的概念数。
    pub count: usize,
}

/// 概念详情（记忆页右侧）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConceptDetail {
    pub concept_id: String,
    pub dir: String,
    pub category_id: String,
    pub type_name: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub status: Option<String>,
    pub stale_after: Option<String>,
    pub observed_at: Option<String>,
    /// 支撑锚点（`raw/sessions/*.md` 里的 `### [mNN]` 标签）：前端据此渲染「依据」并跳回原文。
    ///
    /// 这是**权威**依据集（`observed_at` 就是由它推导的）。正文里模型自述的「依据：mNN」
    /// 属于自由文本，锚点数量/内容都可能与 frag_ids 不一致，故 UI 一律以本字段为准。
    pub frag_ids: Vec<String>,
    pub sources: Vec<MemorySourceRef>,
    /// 生成者（`generated.by`）。
    pub generated_by: Option<String>,
    /// 生成时间（`generated.at`）。
    pub generated_at: Option<String>,
    /// 生成模型（`generated.model`）。
    pub model: Option<String>,
    /// 最后写入时间（unix 秒）。
    pub updated_at: i64,
    /// 完整 Markdown 原文（含 frontmatter；前端预览区渲染正文）。
    pub markdown: String,
}

/// 概念的来源引用（`sources[]`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySourceRef {
    /// 相对 Bundle 的资源路径（如 `raw/sessions/2026-09-25-a1f9.md`）。
    pub resource: String,
    pub title: Option<String>,
    pub last_modified: Option<String>,
}

/// 源会话（`raw/sessions/*.md`，记忆页抽屉）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRawSource {
    /// 文件名主干（`raw` 文件的稳定标识）。
    pub stem: String,
    /// 相对 Bundle 的资源路径。
    pub resource: String,
    pub size: i64,
    /// 最后修改时间（unix 秒）。
    pub updated_at: i64,
}

// ---------------------------------------------------------------------------
// 门面
// ---------------------------------------------------------------------------

/// 记忆门面（无持久状态，仅持「运行中编译的取消令牌」）。
pub struct MemoryImpl {
    /// 运行中编译的取消令牌（同一时刻至多一次编译；`None` = 空闲）。
    running: Mutex<Option<CancellationToken>>,
}

impl MemoryImpl {
    /// 构建门面（Bundle 目录按需创建；不在构造期触碰磁盘）。
    pub fn new() -> Self {
        Self {
            running: Mutex::new(None),
        }
    }

    // ---------- 路径与配置 ----------

    /// Bundle 根目录：`<用户目录>/memory`（用户目录 = `AppConfig.workspace`）。
    pub fn root(&self) -> PathBuf {
        App::try_global()
            .map(|app| app.dir().memory_dir)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("astorm").join("memory"))
    }

    /// Bundle 路径集。
    pub fn paths(&self) -> BundlePaths {
        BundlePaths::new(self.root())
    }

    /// 生效的记忆配置（共享热更值：前端保存后经 `App::update_config` 刷新）。
    pub fn config(&self) -> MemoryConfig {
        App::try_global()
            .and_then(|app| app.memory_config())
            .unwrap_or_default()
    }

    /// 分类表（由配置规范化而来）。
    pub fn categories(&self) -> CategoryTable {
        CategoryTable::from_config(&self.config())
    }

    /// 会话目录（源 JSONL 所在：`<data>/session`）。
    fn sessions_dir(&self) -> PathBuf {
        App::try_global()
            .map(|app| app.dir().data_dir.join("session"))
            .unwrap_or_else(|| self.root().join("sessions"))
    }

    /// 待编译会话数（**与编译用同一来源集合**，不读会话正文、不花 token）。
    ///
    /// 必须复用同一套排除规则：否则「待编译 N」会把子会话算进去 —— 子会话每跑一次
    /// 指纹就变一次，那个数字将永远归不了零。
    fn count_pending(&self) -> Result<usize, String> {
        let excluded = excluded_sessions();
        let stamps: Vec<(String, SourceStamp)> = pipeline::scan_session_stamps(&self.sessions_dir())?
            .into_iter()
            .filter(|(id, _)| !excluded.contains(id))
            .collect();
        Ok(SyncState::load(&self.paths().state_file()).count_pending(&stamps))
    }

    /// 是否正在编译。
    pub fn is_running(&self) -> bool {
        self.running.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    // ---------- 会话侧注入（agent 集成）----------

    /// 环境块里的「长期记忆」能力行（会话拿得到 `memory` 工具且库里有概念时才给）。
    ///
    /// 只做一次目录扫描（不解析概念、不碰索引），随每个用户消息调用一次。
    pub fn capability_line(&self, tool_list: &[String]) -> Option<String> {
        if !tool_list.iter().any(|t| t == MEMORY_TOOL_ID) || !self.config().enabled {
            return None;
        }
        let total = writer::scan_concepts(&self.paths()).len();
        (total > 0).then(|| {
            format!(
                "- 长期记忆：{total} 个概念（用 {MEMORY_TOOL_ID} 工具检索：search/read/list）"
            )
        })
    }

    /// 与当前用户消息相关的记忆块（`【相关记忆】`），无命中或任何异常都返回 `None`。
    ///
    /// 设计要点：
    /// - 用**当前用户消息原文**做 BM25 检索，因此注入内容与这一问相关（而非「最近 N 条」噪声）
    /// - **无命中就不注入**：宁缺勿噪，不拿无关记忆占提示词预算
    /// - **任何错误（含编译占用索引、索引未建）一律 `None`**：注入是可选的增益，
    ///   绝不能因为它让会话失败或变慢
    /// - 注入文本由 `astorm-memory` 归一化时按 `【相关记忆】` 标记整体剥离，**不会回流成记忆**
    pub fn digest_for(&self, user_text: &str, tool_list: &[String]) -> Option<String> {
        if !tool_list.iter().any(|t| t == MEMORY_TOOL_ID) {
            return None;
        }
        let cfg = self.config();
        if !cfg.enabled || !cfg.inject_memories || cfg.injected_concepts == 0 {
            return None;
        }
        let query = user_text.trim();
        if query.is_empty() {
            return None;
        }
        let hits = self.search(query, cfg.injected_concepts).ok()?;
        if hits.is_empty() {
            return None;
        }

        let mut block = String::from(
            "【相关记忆】\n以下是跨会话沉淀的记忆，可能与本次提问相关（摘要仅供参考；\
             需要完整正文用 memory 工具的 read + concept_id）：",
        );
        for hit in &hits {
            let line = format!(
                "\n- {} [{}] {}",
                hit.concept_id,
                hit.type_name,
                truncate_chars(&hit.title, DIGEST_TITLE_CHARS)
            );
            let line = if hit.description.trim().is_empty() {
                line
            } else {
                format!("{line} — {}", truncate_chars(hit.description.trim(), DIGEST_DESC_CHARS))
            };
            if block.chars().count() + line.chars().count() > DIGEST_MAX_CHARS {
                break;
            }
            block.push_str(&line);
        }
        Some(block)
    }

    // ---------- 只读查询 ----------

    /// 状态卡：概念数 / 待编译数 / 分类统计 / 上次编译时间。
    pub fn status(&self) -> Result<MemoryStatus, String> {
        let cfg = self.config();
        let paths = self.paths();
        let loaded = writer::load_all_concepts(&paths);
        let table = CategoryTable::from_config(&cfg);
        let counts = type_counts(&loaded.concepts, &table);

        let categories = table
            .all()
            .iter()
            .map(|c| MemoryCategoryStat {
                id: c.id.clone(),
                dir: c.dir.clone(),
                type_name: c.type_name.clone(),
                hint: c.hint.clone(),
                count: counts.get(&c.id).copied().unwrap_or(0),
            })
            .collect();

        let pending = self.count_pending().unwrap_or(0);
        Ok(MemoryStatus {
            root: self.root().to_string_lossy().into_owned(),
            enabled: cfg.enabled,
            auto_compile: cfg.auto_compile,
            running: self.is_running(),
            concept_total: loaded.concepts.len(),
            pending,
            last_sync: SyncState::load(&paths.state_file()).last_sync,
            categories,
            parse_errors: loaded
                .errors
                .iter()
                .map(|(id, e)| format!("{id}: {e}"))
                .collect(),
        })
    }

    /// 概念列表（**直接读盘**，不依赖索引 → 索引未建也能列出）。
    pub fn list_concepts(&self) -> Result<Vec<ConceptSummary>, String> {
        let paths = self.paths();
        let table = CategoryTable::from_config(&self.config());
        let loaded = writer::load_all_concepts(&paths);
        let mut out: Vec<ConceptSummary> = loaded
            .concepts
            .iter()
            .map(|(id, updated_at, doc)| summary_of(id, *updated_at, doc, &table))
            .collect();
        out.sort_by(|a, b| a.concept_id.cmp(&b.concept_id));
        Ok(out)
    }

    /// 概念详情（含完整 Markdown 原文）。
    pub fn show_concept(&self, concept_id: &str) -> Result<MemoryConceptDetail, String> {
        let paths = self.paths();
        let id = concept_id.trim();
        if id.is_empty() {
            return Err("概念 ID 不能为空".into());
        }
        // 读路径准入：id 可能来自模型 / 用户输入，必须挡住 `../` 穿越（`concept_path` 是按段 push 的）
        let path = paths.concept_path_checked(id)?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取概念失败（{}）: {e}", path.display()))?;
        let doc = ConceptDocument::from_markdown(id, &content)?;
        let table = CategoryTable::from_config(&self.config());
        let fm = &doc.frontmatter;
        Ok(MemoryConceptDetail {
            concept_id: doc.concept_id.clone(),
            dir: doc.dir(),
            category_id: category_id_of(&doc.dir(), &fm.concept_type, &table),
            type_name: fm.concept_type.clone(),
            title: fm.title.clone().unwrap_or_else(|| doc.slug().to_string()),
            description: fm.description.clone().unwrap_or_default(),
            tags: fm.tags.clone(),
            status: fm.status.clone(),
            stale_after: fm.stale_after.clone(),
            observed_at: fm.observed_at.clone(),
            frag_ids: fm.frag_ids.clone(),
            sources: fm
                .sources
                .as_ref()
                .map(|list| {
                    list.iter()
                        .map(|s| MemorySourceRef {
                            resource: s.resource.clone(),
                            title: s.title.clone(),
                            last_modified: s.last_modified.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            generated_by: fm.generated.as_ref().map(|g| g.by.clone()),
            generated_at: fm.generated.as_ref().map(|g| g.at.clone()),
            model: fm.generated.as_ref().and_then(|g| g.model.clone()),
            updated_at: mtime_secs(&path),
            markdown: content,
        })
    }

    /// 全文检索（索引优先；索引缺失时退化为子串扫描 —— 用户不该为了搜索先重建索引）。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ConceptSummary>, String> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 200);
        let paths = self.paths();
        let table = CategoryTable::from_config(&self.config());
        if let Some(index) = self.open_index_readonly()? {
            let mut hits = index.search(q, limit)?;
            for hit in &mut hits {
                hit.category_id = category_id_of(&hit.dir, &hit.type_name, &table);
            }
            if !hits.is_empty() {
                return Ok(hits);
            }
        }
        // 索引缺失 / 未命中 → **关键词扫描**兜底（同一个分词器，退化成迷你 BM25）
        Ok(keyword_scan(&paths, &table, q, limit))
    }

    /// OKF 合规校验。
    pub fn validate(&self) -> ValidateReport {
        validate_bundle(&self.paths())
    }

    /// 变更历史（`wiki/log.md`，倒序；`limit` 为条数上限）。
    pub fn read_log(&self, limit: usize) -> Vec<LogEntry> {
        writer::read_log(&self.paths(), limit.clamp(1, 2_000))
    }

    /// 源会话列表（`raw/sessions/`）。
    pub fn list_raw_sources(&self) -> Result<Vec<MemoryRawSource>, String> {
        let paths = self.paths();
        let dir = paths.raw_sessions_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("读取来源目录失败（{}）: {e}", dir.display()))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            let size = entry.metadata().map(|m| m.len() as i64).unwrap_or(0);
            out.push(MemoryRawSource {
                resource: format!("raw/sessions/{stem}.md"),
                stem,
                size,
                updated_at: mtime_secs(&path),
            });
        }
        out.sort_by(|a, b| b.stem.cmp(&a.stem));
        Ok(out)
    }

    /// 读取源会话（归一化后的 Markdown 原文）。
    pub fn read_raw_source(&self, stem: &str) -> Result<String, String> {
        let stem = stem.trim();
        if stem.is_empty() {
            return Err("来源标识不能为空".into());
        }
        // 读路径准入（同 show_concept）：只接受单一文件名
        let path = self.paths().raw_file_checked(stem)?;
        std::fs::read_to_string(&path)
            .map_err(|e| format!("读取来源失败（{}）: {e}", path.display()))
    }

    // ---------- 写操作 ----------

    /// 重建全文索引（wiki 是真相，索引纯派生）。
    pub fn reindex(&self) -> Result<usize, String> {
        let paths = self.paths();
        paths.ensure_writable()?;
        if paths.concept_files().is_empty() {
            return Ok(0);
        }
        let index = self.open_index_for_write()?;
        index.rebuild(&paths)
    }

    /// 取消运行中的编译。
    ///
    /// 返回 `true` 表示确有任务被取消；已落盘内容保留（管道按会话原子落盘），
    /// 未提交的索引改动由管道自身 `rollback`。
    pub fn cancel(&self) -> bool {
        let Ok(guard) = self.running.lock() else {
            return false;
        };
        match guard.as_ref() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// 启动一次编译（**异步**：立即返回，进度与结果经事件总线推送）。
    ///
    /// 错误只在「无法启动」时同步返回（未启用 / 已有任务 / 未配置模型）；
    /// 管道内部的失败走 `kind = "failed"` 事件。
    pub fn compile(&self, request: MemoryCompileRequest) -> Result<(), String> {
        let app = App::try_global().ok_or("应用未初始化：请先调用 start")?;
        let cfg = self.config();
        if !cfg.enabled {
            return Err("记忆功能已关闭：请在「设置 → 记忆」中启用后再编译".into());
        }
        // LLM 在启动前就构建：缺少供应商 / 模型的提示要立刻返回给用户，
        // 而不是抛进后台任务里只留一条事件。
        let llm = build_llm(&cfg)?;
        let token = CancellationToken::new();
        {
            let mut running = self
                .running
                .lock()
                .map_err(|_| "记忆门面状态锁中毒".to_string())?;
            if running.is_some() {
                return Err("已有记忆编译正在进行中（可先取消）".into());
            }
            *running = Some(token.clone());
        }

        let state = Arc::clone(app);
        let me = Arc::clone(&app.memory);
        app.runtime.handle().spawn(async move {
            emit(&state, "started", None, 0, 0, "开始编译记忆…");
            let outcome =
                run_pipeline(&state, &cfg, llm, request.force, request.dry_run, &token).await;
            match outcome {
                Ok(report) => {
                    let message = if report.cancelled {
                        format!(
                            "已取消：新建 {} / 更新 {} / 跳过 {}",
                            report.created.len(),
                            report.updated.len(),
                            report.skipped.len()
                        )
                    } else {
                        format!(
                            "完成：会话 {} 个（跳过 {}），记忆单元 {} → 概念 {}（新建 {} / 更新 {} / 已覆盖 {}）",
                            report.processed_sessions,
                            report.skipped_sessions,
                            report.extracted_units,
                            report.compiled_concepts,
                            report.created.len(),
                            report.updated.len(),
                            report.skipped.len(),
                        )
                    };
                    let kind = if report.cancelled { "cancelled" } else { "finished" };
                    emit_with_report(&state, kind, &message, &report);
                }
                Err(e) => {
                    tracing::warn!("[memory] 编译失败: {e}");
                    emit(&state, "failed", None, 0, 0, &e);
                }
            }
            if let Ok(mut running) = me.running.lock() {
                *running = None;
            }
        });
        Ok(())
    }

    /// 会话结束时的自动增量钩子（`autoCompile` 开启时由会话收尾阶段调用）。
    ///
    /// **永不阻塞、永不报错**：自动编译是后台增益，失败只记日志（用户没主动要求，
    /// 不该被弹窗打断）。已有任务在跑或未启用时直接跳过。
    pub fn auto_compile_after_session(&self, session_id: &str) {
        let Some(app) = App::try_global() else {
            return;
        };
        let cfg = self.config();
        if !cfg.enabled || !cfg.auto_compile {
            return;
        }
        // 不编译子会话：它们是委派执行的内部轨迹，正文噪声大且结论已回到父会话
        let is_child = app
            .storage
            .get_session(session_id)
            .ok()
            .flatten()
            .map(|r| matches!(r.session_type, astorm_common::types::record::SessionType::Child))
            .unwrap_or(false);
        if is_child {
            return;
        }
        if self.is_running() {
            return;
        }
        let request = MemoryCompileRequest {
            force: false,
            dry_run: false,
        };
        if let Err(e) = self.compile(request) {
            tracing::info!("[memory] 会话 {session_id} 结束后跳过自动编译: {e}");
        }
    }

    // ---------- 索引打开 ----------

    /// 只读打开（不存在返回 `None`；**不创建、不重建任何目录**）。
    fn open_index_readonly(&self) -> Result<Option<MemoryIndex>, String> {
        let dir = self.paths().root().join(INDEX_DIR_NAME);
        MemoryIndex::open_existing(&dir)
    }

    /// 写入打开（不存在则创建）；索引为空而 wiki 有内容时自动对账重建。
    fn open_index_for_write(&self) -> Result<MemoryIndex, String> {
        let paths = self.paths();
        let dir = paths.root().join(INDEX_DIR_NAME);
        let index = MemoryIndex::new(&dir).map_err(|e| format!("打开记忆索引失败: {e}"))?;
        if index.is_empty() && !paths.concept_files().is_empty() {
            let n = index.rebuild(&paths)?;
            tracing::info!("[memory] wiki 有内容但索引为空，已对账重建 {n} 个概念");
        }
        Ok(index)
    }
}

impl Default for MemoryImpl {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 管道执行
// ---------------------------------------------------------------------------

/// 跑一次完整管道（含「Bundle 归属守卫」与来源收集）。
async fn run_pipeline(
    app: &Arc<AppState>,
    cfg: &MemoryConfig,
    llm: ChatLlm,
    force: bool,
    dry_run: bool,
    token: &CancellationToken,
) -> Result<CompileReport, String> {
    let paths = BundlePaths::new(app.dir().memory_dir);
    // 归属守卫：拒绝把编译产物写进「装着别的东西」的目录（防止覆盖用户既有内容）
    paths.ensure_writable()?;
    let index = MemoryIndex::new(&paths.root().join(INDEX_DIR_NAME))
        .map_err(|e| format!("打开记忆索引失败: {e}"))?;
    if index.is_empty() && !paths.concept_files().is_empty() {
        index.rebuild(&paths)?;
    }

    let sessions = load_sessions(app)?;
    if sessions.is_empty() {
        emit(app, "stage", Some("parse"), 0, 0, "没有可编译的会话");
    }
    let table = CategoryTable::from_config(cfg);
    let options = CompileOptions { force, dry_run };
    let state = Arc::clone(app);
    let progress = move |ev: ProgressEvent| {
        emit(
            &state,
            "stage",
            Some(ev.stage.as_str()),
            ev.current,
            ev.total,
            &ev.message,
        );
    };
    pipeline::compile_bundle(
        &paths,
        &index,
        &llm,
        cfg,
        &table,
        &sessions,
        &options,
        &progress,
        token,
    )
    .await
}

/// 收集来源会话：`<data>/session/<id>/message.jsonl` + sqlite 里的会话标题与类型。
///
/// **子会话（`SessionType::Child`）整体排除**：那是 `sub_session` 工具委派执行的内部轨迹，
/// 工具调用与原始输出密度极高，而结论已经回到父会话里；喂进记忆既涨体积又引噪声。
/// 这是**来源集合**层面的过滤，所以手动「增量编译」与自动编译都生效
/// （`auto_compile_after_session` 那侧的同类判断只是「别为子会话收尾白白唤醒一次」）。
/// 记录缺失（目录在、库里没登记）时**保留**该会话 —— 宁可多编译一个，也不静默丢用户对话。
fn load_sessions(app: &Arc<AppState>) -> Result<Vec<SessionInput>, String> {
    let dir = app.dir().data_dir.join("session");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let excluded = excluded_sessions();
    let mut sessions = pipeline::scan_session_dir(&dir)?;
    sessions.retain(|s| !excluded.contains(&s.session_id));
    // 标题带上后概念的 `sources.title` 才是人读的名字
    let titles: HashMap<String, String> = app
        .storage
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            r.title
                .filter(|t| !t.trim().is_empty())
                .map(|t| (r.id.to_string(), t))
        })
        .collect();
    for session in &mut sessions {
        if let Some(title) = titles.get(&session.session_id) {
            session.title = Some(title.clone());
        }
    }
    Ok(sessions)
}

/// 不进记忆的会话 id 集合（`SessionType::Child`）。
///
/// 子会话是 `sub_session` 工具委派执行的内部轨迹：工具调用与原始输出密度极高，
/// 结论已经回到父会话里，喂进记忆既涨体积又引噪声。**编译与「待编译」计数必须共用本函数**，
/// 否则两处口径不一致（计数把子会话算进去 → 永远归不了零）。
fn excluded_sessions() -> std::collections::HashSet<String> {
    App::try_global()
        .map(|app| {
            app.storage
                .list_sessions()
                .unwrap_or_default()
                .into_iter()
                .filter(|r| {
                    matches!(
                        r.session_type,
                        astorm_common::types::record::SessionType::Child
                    )
                })
                .map(|r| r.id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 由应用已配置的供应商 / 模型构建记忆 LLM 客户端。
///
/// 复用 [`crate::app::LlmRegistry`]：密钥在 `start` 时已由主进程解密传入，
/// 记忆编译不需要也不应再持有一套凭据。
fn build_llm(cfg: &MemoryConfig) -> Result<ChatLlm, String> {
    let app = App::try_global().ok_or("应用未初始化：请先调用 start")?;
    let registry = app.llm();
    if registry.is_empty() {
        return Err("尚未配置 LLM 供应商：请先在「设置 → 模型」中添加供应商与模型".into());
    }
    let provider = cfg.provider.clone();
    let client = registry.client_for(provider.as_deref()).ok_or_else(|| {
        format!(
            "记忆编译指定的供应商不存在: {}",
            provider.as_deref().unwrap_or("(默认)")
        )
    })?;
    let model_ref = cfg
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| registry.default_model())
        .ok_or("记忆编译缺少模型：请在「设置 → 记忆」中选择编译模型，或配置默认模型")?;
    let model = registry.resolve_model_name(provider.as_deref(), &model_ref);
    Ok(ChatLlm::new(client, model))
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

/// 广播一条记忆事件（全局事件，`session_id` 为空；前端按 `type === 'memory'` 分发）。
fn emit(app: &AppState, kind: &str, stage: Option<&str>, current: usize, total: usize, message: &str) {
    app.emit(json!({
        "type": MEMORY_EVENT,
        "kind": kind,
        "stage": stage,
        "current": current,
        "total": total,
        "message": message,
        "report": Value::Null,
        "session_id": Value::Null,
        "timestamp": now_ms(),
    }));
}

/// 带编译报告的事件（`finished` / `cancelled` 携带完整结果，前端直接渲染结果卡）。
fn emit_with_report(app: &AppState, kind: &str, message: &str, report: &CompileReport) {
    app.emit(json!({
        "type": MEMORY_EVENT,
        "kind": kind,
        "stage": ProgressStage::Write.as_str(),
        "current": report.processed_sessions,
        "total": report.total_sessions,
        "message": message,
        "report": serde_json::to_value(report).unwrap_or(Value::Null),
        "session_id": Value::Null,
        "timestamp": now_ms(),
    }));
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 无索引时的关键词兜底检索：用**与索引同一个分词器**切词，按命中 token 数降序。
///
/// 为什么不能拿整串查询当子串匹配：真实提问是「我们之前认证方案是怎么定的」这种句子，
/// 它不可能作为子串出现在任何概念正文里 —— 那样兜底会一条都命不中，注入通道在索引缺失时
/// 静默失效。分词后按 token 命中数排序，行为与 BM25 路径大体一致（只是没有 IDF/长度归一）。
fn keyword_scan(
    paths: &BundlePaths,
    table: &CategoryTable,
    query: &str,
    limit: usize,
) -> Vec<ConceptSummary> {
    let tokens = astorm_memory::index::tokenize_query(query);
    if tokens.is_empty() {
        return Vec::new();
    }
    let loaded = writer::load_all_concepts(paths);
    let mut scored: Vec<(usize, ConceptSummary)> = loaded
        .concepts
        .iter()
        .filter_map(|(id, updated_at, doc)| {
            let fm = &doc.frontmatter;
            // 分字段加权：标题/标签命中比正文命中更能说明「这条概念讲的就是这件事」
            let title = fm.title.clone().unwrap_or_default().to_lowercase();
            let desc = fm.description.clone().unwrap_or_default().to_lowercase();
            let tags = fm.tags.join(" ").to_lowercase();
            let body = doc.body.to_lowercase();
            let mut score = 0usize;
            for token in &tokens {
                if title.contains(token.as_str()) {
                    score += 3;
                }
                if tags.contains(token.as_str()) {
                    score += 2;
                }
                if desc.contains(token.as_str()) {
                    score += 2;
                }
                if body.contains(token.as_str()) {
                    score += 1;
                }
            }
            (score > 0).then(|| {
                let mut summary = summary_of(id, *updated_at, doc, table);
                summary.matched = vec!["keyword".into()];
                (score, summary)
            })
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.concept_id.cmp(&b.1.concept_id)));
    scored
        .into_iter()
        .take(limit)
        .map(|(score, mut summary)| {
            summary.score = Some(score as f32);
            summary
        })
        .collect()
}

/// 概念文档 → 列表项（`category_id` 由分类表按「目录 + type」回填）。
fn summary_of(
    concept_id: &str,
    updated_at: i64,
    doc: &ConceptDocument,
    table: &CategoryTable,
) -> ConceptSummary {
    let dir = doc.dir();
    let fm = &doc.frontmatter;
    ConceptSummary {
        concept_id: concept_id.to_string(),
        category_id: category_id_of(&dir, &fm.concept_type, table),
        dir,
        type_name: fm.concept_type.clone(),
        title: fm.title.clone().unwrap_or_else(|| doc.slug().to_string()),
        description: fm.description.clone().unwrap_or_default(),
        tags: fm.tags.clone(),
        observed_at: fm.observed_at.clone(),
        updated_at,
        score: None,
        matched: Vec::new(),
    }
}

/// 目录 + type → 分类 id。
///
/// **必须先按 (目录, type) 精确匹配**：默认分类表里 `fact` / `other` 共用 `concepts/`
/// 目录，只看目录会让 `/concepts/` 下的 Other 概念被算成 Fact（统计与筛选取值都错）。
/// 精确未命中再退化到「目录匹配」（存量 `type: Status` 等已删分类的老概念因此落到
/// `fact`），最后为空串（人工放入的目录不算任何已配置分类）。
fn category_id_of(dir: &str, type_name: &str, table: &CategoryTable) -> String {
    table
        .all()
        .iter()
        .find(|c| c.dir == dir && c.type_name == type_name)
        .or_else(|| table.all().iter().find(|c| c.dir == dir))
        .map(|c| c.id.clone())
        .unwrap_or_default()
}

/// 按**分类 id** 聚合概念数，与 [`category_id_of`] / 列表项的 `category_id` 同口径。
///
/// 不能按 `(dir, type)` 精确聚合：`concepts/` 被 `fact` / `other` 共用（外加存量
/// `Status`），精确键会让这些概念在分类计数里凭空消失，而列表筛选仍按
/// `category_id_of` 把它们算进 `fact` —— 于是「chip 计数」与「点进去看到的条数」对不上。
/// 落不到任何分类的概念（配置改过、目录已不是分类目录）不计入分类，但仍算在 `concept_total` 里。
fn type_counts(
    concepts: &[(String, i64, ConceptDocument)],
    table: &CategoryTable,
) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for (_, _, doc) in concepts {
        let id = category_id_of(&doc.dir(), &doc.frontmatter.concept_type, table);
        if id.is_empty() {
            continue;
        }
        *out.entry(id).or_insert(0) += 1;
    }
    out
}

/// 文件最后修改时间（unix 秒；取不到为 0）。
fn mtime_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 当前时间（epoch ms）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
