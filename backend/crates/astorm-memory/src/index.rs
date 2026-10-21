//! `MemoryIndex`：概念全文索引（Tantivy + jieba 中文分词），供合并候选召回与记忆页检索。
//!
//! 索引是**派生数据**：`wiki/` 目录才是真相，索引可随时 [`MemoryIndex::rebuild`] 重建。
//! 与 `skill/index.rs` 同范式（`schema_version` 不匹配即重建、jieba 索引/查询两侧分词器一致、
//! `delete_term` + `add_document` 实现 upsert、手动切词 + 跨字段 `Should` BooleanQuery）。
//!
//! 字段：`concept_id` 精确匹配（STRING），`type` / `tags` / `sources` STRING|STORED，
//! `title` / `description` / `body_excerpt` 走 jieba 分词。`body_excerpt` 截断存储，
//! **同时**用于合并判定的 payload 与检索结果摘要，避免判定阶段再读盘。

use std::path::Path;
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, INDEXED, STORED, STRING,
};
use tantivy::tokenizer::{LowerCaser, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};
use tantivy_jieba::JiebaTokenizer;

use super::concept::ConceptDocument;
use super::paths::BundlePaths;
use super::types::{CandidateHit, ConceptSummary};
use crate::writer::load_all_concepts;

/// 文本字段分词器名（索引 / 查询两侧必须一致）。
const TOKENIZER_NAME: &str = "memory_content";
/// IndexWriter 内存预算。
const WRITER_HEAP_BYTES: usize = 15_000_000;
/// 索引 schema 版本（变更 schema 后递增，触发索引目录重建）。
const SCHEMA_VERSION: &str = "1";
/// schema 版本文件名（本管道索引的标记之一）。
const SCHEMA_VERSION_FILE: &str = "schema_version";
/// tantivy 自身的元数据文件（判断目录是否已是 tantivy 索引）。
const TANTIVY_META_FILE: &str = "meta.json";
/// `body_excerpt` 截断字符数（兼顾合并判定 payload 与结果摘要）。
const BODY_EXCERPT_CHARS: usize = 2_000;
/// `list` / `rebuild` 的返回上限。
const LIST_LIMIT: usize = 50_000;

/// 构建 analyzer（`search_mode=false` 用于落盘索引，`true` 用于查询分词）。
fn create_analyzer(search_mode: bool) -> TextAnalyzer {
    TextAnalyzer::builder(JiebaTokenizer::with_search_mode(search_mode))
        .filter(LowerCaser)
        .build()
}

/// 索引 schema。
fn build_schema() -> Schema {
    let content_indexing = TextFieldIndexing::default()
        .set_tokenizer(TOKENIZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let text = || {
        TextOptions::default()
            .set_indexing_options(content_indexing.clone())
            .set_stored()
    };
    let mut sb = Schema::builder();
    sb.add_text_field("concept_id", STRING | STORED);
    sb.add_text_field("type", STRING | STORED);
    sb.add_text_field("title", text());
    sb.add_text_field("description", text());
    sb.add_text_field("body_excerpt", text());
    sb.add_text_field("tags", STRING | STORED);
    sb.add_text_field("sources", STRING | STORED);
    sb.add_i64_field("updated_at", INDEXED | STORED);
    sb.build()
}

/// 检索词 → token 列表（jieba 搜索模式 + 小写），与索引分词**完全同源**。
///
/// **必须过滤掉不含字母/数字/CJK 的 token**：jieba 会把空白与标点也切成 token（实测
/// `"napi dll"` → `["napi", " ", "dll"]`），若把 `" "` 当查询词，凡是正文含空格的文档
/// 都会被命中 —— 跨文档假召回。`is_alphanumeric()` 对 CJK 亦为真，故中文词不受影响。
///
/// 公开导出的理由：调用方在**索引不可用**时会退化为关键词扫描，那时必须用同一套分词，
/// 否则「整串查询当子串匹配」会让自然语言提问一条都命不中（真实事故：问句里的
/// 「我们之前认证方案是怎么定的」不可能作为子串出现在任何概念正文里）。
pub fn tokenize_query(query_str: &str) -> Vec<String> {
    let mut analyzer = create_analyzer(true);
    let mut stream = analyzer.token_stream(query_str);
    let mut tokens = Vec::new();
    while let Some(token) = stream.next() {
        let text = token.text.to_lowercase();
        if text.chars().any(|c| c.is_alphanumeric()) {
            tokens.push(text);
        }
    }
    tokens
}

/// 概念全文索引（进程内由调用方以 `Arc` 共享；索引目录默认 `<bundle>/.okf-index`）。
pub struct MemoryIndex {
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
    f_concept_id: Field,
    f_type: Field,
    f_title: Field,
    f_description: Field,
    f_body: Field,
    f_tags: Field,
    f_sources: Field,
    f_updated_at: Field,
}

impl MemoryIndex {
    /// 打开**已存在**的索引；不存在则返回 `None`。
    ///
    /// **绝不创建、绝不清空任何目录** —— 供只读命令（status / list / search）使用。
    pub fn open_existing(index_dir: &Path) -> Result<Option<Self>, String> {
        if !index_dir.join(SCHEMA_VERSION_FILE).is_file() {
            return Ok(None);
        }
        Self::open(index_dir, false).map(Some)
    }

    /// 打开 / 创建索引：`index_dir` 为索引目录（`schema_version` 不匹配时重建）。
    ///
    /// **安全语义**（避免 `--index-dir` 指错路径时清空用户数据）：只在以下情况重建目录：
    /// - 目录不存在或为空
    /// - 目录内含本管道的标记（`schema_version`）或已是 tantivy 索引（`meta.json`）
    ///
    /// 若目录**非空且不含任何索引标记**，判为「别人的目录」→ 返回错误而**不删除**。
    pub fn new(index_dir: &Path) -> Result<Self, String> {
        Self::open(index_dir, true)
    }

    fn open(index_dir: &Path, create: bool) -> Result<Self, String> {
        let version_file = index_dir.join(SCHEMA_VERSION_FILE);
        let has_version = version_file.is_file();
        let up_to_date = std::fs::read_to_string(&version_file)
            .map(|v| v.trim() == SCHEMA_VERSION)
            .unwrap_or(false);
        let is_tantivy = index_dir.join(TANTIVY_META_FILE).is_file();
        let empty = !index_dir.is_dir()
            || std::fs::read_dir(index_dir)
                .map(|mut it| it.next().is_none())
                .unwrap_or(true);

        if !up_to_date {
            if !create {
                return Err(format!("索引不可用: {}", index_dir.display()));
            }
            if !empty && !has_version && !is_tantivy {
                return Err(format!(
                    "索引目录非空且不像本管道的索引，拒绝清空以免误删数据: {}\n\
                     如确认要在此建立索引，请先自行备份并清空该目录，或用 --index-dir 指向别处。",
                    index_dir.display()
                ));
            }
            let _ = std::fs::remove_dir_all(index_dir);
            std::fs::create_dir_all(index_dir)
                .map_err(|e| format!("创建记忆索引目录失败({}): {e}", index_dir.display()))?;
            std::fs::write(&version_file, SCHEMA_VERSION)
                .map_err(|e| format!("写入记忆索引版本失败: {e}"))?;
        }

        let schema = build_schema();
        let index = Index::create_in_dir(index_dir, schema)
            .or_else(|_| Index::open_in_dir(index_dir))
            .map_err(|e| format!("打开/创建记忆索引失败: {e}"))?;
        index
            .tokenizers()
            .register(TOKENIZER_NAME, create_analyzer(false));
        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| format!("构建记忆索引 writer 失败: {e}"))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| format!("构建记忆索引 reader 失败: {e}"))?;

        let schema = index.schema();
        let field = |name: &str| {
            schema
                .get_field(name)
                .map_err(|e| format!("记忆索引 schema 缺字段 {name}: {e}"))
        };
        Ok(Self {
            writer: Mutex::new(writer),
            reader,
            f_concept_id: field("concept_id")?,
            f_type: field("type")?,
            f_title: field("title")?,
            f_description: field("description")?,
            f_body: field("body_excerpt")?,
            f_tags: field("tags")?,
            f_sources: field("sources")?,
            f_updated_at: field("updated_at")?,
        })
    }

    /// 索引（更新）一个概念：先按 `concept_id` 删旧档再写入。
    ///
    /// 供合并判定与检索使用的 `type` 取 frontmatter 的 `type`（用于类型冲突判定）。
    pub fn upsert(&self, doc: &ConceptDocument, updated_at: i64) -> Result<(), String> {
        let writer = self.writer.lock().map_err(|_| "记忆索引写锁中毒".to_string())?;
        writer.delete_term(Term::from_field_text(self.f_concept_id, &doc.concept_id));
        writer
            .add_document(doc!(
                self.f_concept_id => doc.concept_id.clone(),
                self.f_type => doc.frontmatter.concept_type.clone(),
                self.f_title => doc.frontmatter.title.clone().unwrap_or_default(),
                self.f_description => doc.frontmatter.description.clone().unwrap_or_default(),
                self.f_body => truncate_chars(&doc.body, BODY_EXCERPT_CHARS),
                self.f_tags => doc.frontmatter.tags.join(", "),
                self.f_sources => source_resources(doc).join(", "),
                self.f_updated_at => updated_at,
            ))
            .map_err(|e| format!("写入记忆索引失败: {e}"))?;
        Ok(())
    }

    /// 按概念 ID 移除。
    pub fn remove(&self, concept_id: &str) -> Result<(), String> {
        let writer = self.writer.lock().map_err(|_| "记忆索引写锁中毒".to_string())?;
        writer.delete_term(Term::from_field_text(self.f_concept_id, concept_id));
        Ok(())
    }

    /// commit 并 reload（写入对检索可见）。
    pub fn commit(&self) -> Result<(), String> {
        let mut writer = self.writer.lock().map_err(|_| "记忆索引写锁中毒".to_string())?;
        writer.commit().map_err(|e| format!("记忆索引 commit 失败: {e}"))?;
        self.reader
            .reload()
            .map_err(|e| format!("记忆索引 reload 失败: {e}"))
    }

    /// 丢弃未 commit 的写入（异常 / 取消路径：已落盘文件才是真相）。
    pub fn rollback(&self) -> Result<(), String> {
        let mut writer = self.writer.lock().map_err(|_| "记忆索引写锁中毒".to_string())?;
        writer.rollback().map_err(|e| format!("记忆索引 rollback 失败: {e}"))?;
        Ok(())
    }

    /// 索引中的概念数。
    pub fn count(&self) -> usize {
        self.reader.searcher().num_docs() as usize
    }

    /// 索引是否为空（用于「wiki 有内容但索引为空 ⇒ 自动重建」的对账）。
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// 按概念 ID 精确取摘要（不存在返回 None）。
    pub fn get(&self, concept_id: &str) -> Result<Option<ConceptSummary>, String> {
        let query = TermQuery::new(
            Term::from_field_text(self.f_concept_id, concept_id),
            IndexRecordOption::Basic,
        );
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .map_err(|e| format!("记忆索引检索失败: {e}"))?;
        let Some((score, addr)) = top.into_iter().next() else {
            return Ok(None);
        };
        let doc = searcher
            .doc::<TantivyDocument>(addr)
            .map_err(|e| format!("读取记忆索引文档失败: {e}"))?;
        let mut summary = self.doc_to_summary(&doc);
        summary.score = Some(score);
        Ok(Some(summary))
    }

    /// 列出全部概念（相关度无意义，仅作遍历；按 `concept_id` 排序）。
    pub fn list(&self) -> Result<Vec<ConceptSummary>, String> {
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&AllQuery, &TopDocs::with_limit(LIST_LIMIT).order_by_score())
            .map_err(|e| format!("记忆索引遍历失败: {e}"))?;
        let mut out = Vec::new();
        for (_, addr) in top {
            let doc = searcher
                .doc::<TantivyDocument>(addr)
                .map_err(|e| format!("读取记忆索引文档失败: {e}"))?;
            out.push(self.doc_to_summary(&doc));
        }
        out.sort_by(|a, b| a.concept_id.cmp(&b.concept_id));
        Ok(out)
    }

    /// 全文检索（标题 / 描述 / 标签 / 正文摘要），命中字段写入 `matched`。
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<ConceptSummary>, String> {
        let tokens = self.tokenize(query_str);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let fields = [self.f_title, self.f_description, self.f_tags, self.f_body];
        let query = self.boolean_query(&tokens, &fields);
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| format!("记忆检索失败: {e}"))?;
        let mut out = Vec::new();
        for (score, addr) in top {
            let doc = searcher
                .doc::<TantivyDocument>(addr)
                .map_err(|e| format!("读取记忆索引文档失败: {e}"))?;
            let mut summary = self.doc_to_summary(&doc);
            summary.score = Some(score);
            summary.matched = matched_fields(&doc, &tokens, &[
                (self.f_title, "title"),
                (self.f_description, "description"),
                (self.f_tags, "tags"),
                (self.f_body, "body"),
            ]);
            out.push(summary);
        }
        Ok(out)
    }

    /// 合并候选召回：以新概念的 标题 + 描述 + 标签 + 正文 为查询，返回 top N（**排除自身**）。
    pub fn candidates(&self, concept: &ConceptDocument, limit: usize) -> Result<Vec<CandidateHit>, String> {
        let query_text = candidate_query_text(concept);
        let tokens = self.tokenize(&query_text);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        // 多取若干以备过滤掉自身
        let over_fetch = limit.saturating_add(1);
        let fields = [self.f_title, self.f_description, self.f_tags, self.f_body];
        let query = self.boolean_query(&tokens, &fields);
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(over_fetch).order_by_score())
            .map_err(|e| format!("记忆候选检索失败: {e}"))?;
        let mut out = Vec::new();
        for (score, addr) in top {
            let doc = searcher
                .doc::<TantivyDocument>(addr)
                .map_err(|e| format!("读取记忆索引文档失败: {e}"))?;
            let concept_id = str_of(&doc, self.f_concept_id);
            if concept_id == concept.concept_id {
                continue; // 自身由「同 ID → 直接 UPDATE」路径处理，不进候选
            }
            out.push(CandidateHit {
                concept_id,
                score,
                type_name: str_of(&doc, self.f_type),
                title: str_of(&doc, self.f_title),
                description: str_of(&doc, self.f_description),
                tags: split_tags(&str_of(&doc, self.f_tags)),
                body_excerpt: str_of(&doc, self.f_body),
            });
        }
        out.truncate(limit);
        Ok(out)
    }

    /// 全量重建索引：清空后按 `wiki/` 实际内容重灌（索引是派生数据，随时可重建）。
    ///
    /// 返回重建后的概念数；解析失败的文件跳过（由 validate 负责报告）。
    pub fn rebuild(&self, paths: &BundlePaths) -> Result<usize, String> {
        {
            let writer = self.writer.lock().map_err(|_| "记忆索引写锁中毒".to_string())?;
            writer
                .delete_all_documents()
                .map_err(|e| format!("清空记忆索引失败: {e}"))?;
        }
        let loaded = load_all_concepts(paths);
        for (_, updated_at, doc) in &loaded.concepts {
            self.upsert(doc, *updated_at)?;
        }
        self.commit()?;
        Ok(loaded.concepts.len())
    }

    /// 检索词 → token 列表（jieba 搜索模式 + 小写）。
    fn tokenize(&self, query_str: &str) -> Vec<String> {
        tokenize_query(query_str)
    }

    /// 跨字段 `Should` 组合查询（任一字段命中任一 token 即召回）。
    fn boolean_query(&self, tokens: &[String], fields: &[Field]) -> BooleanQuery {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for field in fields {
            for token in tokens {
                clauses.push((
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_text(*field, token),
                        IndexRecordOption::WithFreqs,
                    )) as Box<dyn Query>,
                ));
            }
        }
        BooleanQuery::new(clauses)
    }

    fn doc_to_summary(&self, doc: &TantivyDocument) -> ConceptSummary {
        let concept_id = str_of(doc, self.f_concept_id);
        let dir = concept_id
            .split_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        ConceptSummary {
            concept_id,
            dir,
            category_id: String::new(), // 由调用方按分类表回填
            type_name: str_of(doc, self.f_type),
            title: str_of(doc, self.f_title),
            description: str_of(doc, self.f_description),
            tags: split_tags(&str_of(doc, self.f_tags)),
            observed_at: None, // 摘要行不索引该扩展字段，详情经 memory_show 读文件
            updated_at: i64_of(doc, self.f_updated_at),
            score: None,
            matched: Vec::new(),
        }
    }
}

/// 以新概念的关键字段构造候选召回查询文本。
fn candidate_query_text(doc: &ConceptDocument) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = &doc.frontmatter.title {
        parts.push(t.clone());
    }
    if let Some(d) = &doc.frontmatter.description {
        parts.push(d.clone());
    }
    parts.extend(doc.frontmatter.tags.iter().cloned());
    parts.push(truncate_chars(&doc.body, 600));
    parts.join("\n")
}

/// 概念 frontmatter 的来源资源列表。
fn source_resources(doc: &ConceptDocument) -> Vec<String> {
    doc.frontmatter
        .sources
        .as_ref()
        .map(|list| list.iter().map(|s| s.resource.clone()).collect())
        .unwrap_or_default()
}

/// 命中的字段名（对已存文本做大小写不敏感子串判定，供 UI 标注）。
fn matched_fields(
    doc: &TantivyDocument,
    tokens: &[String],
    fields: &[(Field, &'static str)],
) -> Vec<String> {
    let mut out = Vec::new();
    for (field, name) in fields {
        let text = str_of(doc, *field).to_lowercase();
        if tokens.iter().any(|t| text.contains(t.as_str())) {
            out.push((*name).to_string());
        }
    }
    out
}

/// 按字符截断（UTF-8 边界安全）。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

fn str_of(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_default()
}

fn i64_of(doc: &TantivyDocument, field: Field) -> i64 {
    doc.get_first(field).and_then(|v| v.as_i64()).unwrap_or(0)
}

fn split_tags(joined: &str) -> Vec<String> {
    joined
        .split(", ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concept::ConceptFrontmatter;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> (PathBuf, MemoryIndex) {
        let dir = std::env::temp_dir().join(format!("astorm_mem_index_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let idx = MemoryIndex::new(&dir).unwrap();
        (dir, idx)
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

    #[test]
    fn upsert_commit_get_round_trip() {
        let (dir, idx) = tmp("roundtrip");
        idx.upsert(&doc("decisions/use-jwt", "Decision", "用 JWT", "认证选型", &["auth"], "正文"), 100)
            .unwrap();
        assert!(idx.is_empty(), "未 commit 前 reader 不可见");
        idx.commit().unwrap();
        assert_eq!(idx.count(), 1);

        let got = idx.get("decisions/use-jwt").unwrap().unwrap();
        assert_eq!(got.type_name, "Decision");
        assert_eq!(got.title, "用 JWT");
        assert_eq!(got.tags, vec!["auth"]);
        assert_eq!(got.updated_at, 100);
        assert_eq!(got.dir, "decisions");
        assert!(idx.get("decisions/nope").unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同 ID 再 upsert 应覆盖而非追加（delete_term + add_document）。
    #[test]
    fn upsert_same_id_overwrites() {
        let (dir, idx) = tmp("upsert");
        idx.upsert(&doc("concepts/x", "Fact", "旧", "", &[], "旧正文"), 1).unwrap();
        idx.commit().unwrap();
        idx.upsert(&doc("concepts/x", "Fact", "新", "", &[], "新正文"), 2).unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.count(), 1, "同 ID 不应产生两条");
        assert_eq!(idx.get("concepts/x").unwrap().unwrap().title, "新");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_deletes_from_index() {
        let (dir, idx) = tmp("remove");
        idx.upsert(&doc("concepts/x", "Fact", "X", "", &[], "正文"), 1).unwrap();
        idx.commit().unwrap();
        idx.remove("concepts/x").unwrap();
        idx.commit().unwrap();
        assert!(idx.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 中文 jieba 分词召回（关键词检索的核心能力）。
    #[test]
    fn search_matches_chinese_with_jieba() {
        let (dir, idx) = tmp("cjk");
        idx.upsert(
            &doc("decisions/use-jwt", "Decision", "认证方案选型", "选择无状态令牌", &[], "决定使用 JWT 而非 session"),
            1,
        )
        .unwrap();
        idx.upsert(
            &doc("concepts/napi-dll", "Fact", "后端形态", "动态库", &[], "后端是 napi dll，必须改名 .node"),
            1,
        )
        .unwrap();
        idx.commit().unwrap();

        // 中文词汇命中
        let hits = idx.search("认证", 10).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].concept_id, "decisions/use-jwt");
        assert!(hits[0].score.is_some());
        assert!(hits[0].matched.contains(&"title".to_string()), "{:?}", hits[0].matched);

        // 正文命中（title/desc 都没有「无状态」三字之外的词）
        let hits = idx.search("session", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].matched.contains(&"body".to_string()), "{:?}", hits[0].matched);

        // 另一个文档：验证跨文档不会因空白 token 假命中（jieba 会把空格切成 token）
        let hits = idx.search("napi dll", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].concept_id, "concepts/napi-dll");

        // 无关键词为空查询
        assert!(idx.search("", 10).unwrap().is_empty());
        assert!(idx.search("   ", 10).unwrap().is_empty());
        // 无命中
        assert!(idx.search("完全无关的词组xyz", 10).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 候选召回排除自身（自身由「同 ID → UPDATE」路径处理）。
    #[test]
    fn candidates_exclude_self_and_rank_related_first() {
        let (dir, idx) = tmp("cand");
        let existing = doc("decisions/use-jwt", "Decision", "认证方案选型", "选无状态令牌", &[], "JWT 而非 session");
        idx.upsert(&existing, 1).unwrap();
        idx.upsert(&doc("concepts/unrelated", "Fact", "构建工具", "用 pnpm", &[], "包管理器"), 1).unwrap();
        idx.commit().unwrap();

        // 与已有概念高度相关的新概念
        let new_doc = doc("decisions/auth-2", "Decision", "认证方案选型补充", "无状态令牌", &[], "继续用 JWT");
        let hits = idx.candidates(&new_doc, 8).unwrap();
        assert!(!hits.is_empty(), "应召回相关候选");
        assert!(
            hits.iter().all(|h| h.concept_id != "decisions/auth-2"),
            "候选必须排除自身: {hits:?}"
        );
        assert_eq!(hits[0].concept_id, "decisions/use-jwt", "最相关的应排首位: {hits:?}");

        // 新概念自身已在索引中时，也不得把自己当候选
        idx.upsert(&new_doc, 2).unwrap();
        idx.commit().unwrap();
        let hits = idx.candidates(&new_doc, 8).unwrap();
        assert!(hits.iter().all(|h| h.concept_id != "decisions/auth-2"), "{hits:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidates_empty_for_unrelated_new_concept() {
        let (dir, idx) = tmp("cand2");
        idx.upsert(&doc("concepts/a", "Fact", "构建工具", "pnpm", &[], "包管理器"), 1).unwrap();
        idx.commit().unwrap();
        let new_doc = doc("decisions/x", "Decision", "k8s 集群选型", "容器编排", &[], "选择 k3s");
        let hits = idx.candidates(&new_doc, 8).unwrap();
        assert!(hits.is_empty(), "无关概念不应召回候选: {hits:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_matches_wiki_content() {
        let root = std::env::temp_dir().join(format!("astorm_mem_index_rebuild_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = BundlePaths::new(&root);
        paths.ensure_dirs(&["decisions".into(), "concepts".into()]).unwrap();

        let write = |id: &str, content: &str| {
            let p = paths.concept_path(id);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        };
        write("decisions/a", "---\ntype: Decision\ntitle: A\n---\n\n正文 A\n");
        write("concepts/b", "---\ntype: Fact\ntitle: B\n---\n\n正文 B\n");
        paths.ensure_dirs(&[]).unwrap();
        std::fs::write(paths.wiki_dir().join("concepts/index.md"), "# 索引\n").unwrap();

        let idx_dir = std::env::temp_dir().join(format!("astorm_mem_index_rebuild_idx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&idx_dir);
        let idx = MemoryIndex::new(&idx_dir).unwrap();
        let n = idx.rebuild(&paths).unwrap();
        assert_eq!(n, 2, "索引应只含 2 个概念（保留文件 index.md 不算）");
        assert_eq!(idx.count(), 2);
        assert!(idx.get("decisions/a").unwrap().is_some());
        assert!(idx.get("concepts/b").unwrap().is_some());
        assert!(idx.get("concepts/index").unwrap().is_none(), "保留文件不得索引");

        // 二次重建幂等
        assert_eq!(idx.rebuild(&paths).unwrap(), 2);
        assert_eq!(idx.count(), 2, "重建应清空后重灌，不重复累积");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&idx_dir);
    }

    #[test]
    fn schema_version_change_rebuilds_dir() {
        let dir = std::env::temp_dir().join(format!("astorm_mem_index_ver_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let idx = MemoryIndex::new(&dir).unwrap();
            idx.upsert(&doc("concepts/x", "Fact", "X", "", &[], "正文"), 1).unwrap();
            idx.commit().unwrap();
            assert_eq!(idx.count(), 1);
        }
        // 伪造成旧版本 → 下次打开应清空重建
        std::fs::write(dir.join("schema_version"), "0").unwrap();
        let idx = MemoryIndex::new(&dir).unwrap();
        assert!(idx.is_empty(), "版本不匹配应重建（清空）索引");
        assert_eq!(std::fs::read_to_string(dir.join("schema_version")).unwrap(), SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_discards_uncommitted() {
        let (dir, idx) = tmp("rollback");
        idx.upsert(&doc("concepts/x", "Fact", "X", "", &[], "正文"), 1).unwrap();
        idx.rollback().unwrap();
        assert!(idx.is_empty(), "rollback 后未提交文档应被丢弃");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_sorted_by_concept_id_and_excerpt_truncated() {
        let (dir, idx) = tmp("list");
        let long_body = "中".repeat(BODY_EXCERPT_CHARS + 500);
        idx.upsert(&doc("concepts/z", "Fact", "Z", "", &[], "短"), 1).unwrap();
        idx.upsert(&doc("concepts/a", "Fact", "A", "", &[], &long_body), 1).unwrap();
        idx.commit().unwrap();

        let all = idx.list().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].concept_id, "concepts/a", "应按 concept_id 排序");

        // 正文摘要被截断存储
        let hits = idx.candidates(&doc("concepts/probe", "Fact", "中", "", &[], "中"), 8).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].body_excerpt.chars().count() <= BODY_EXCERPT_CHARS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 临时诊断：打印 jieba 搜索模式下的分词结果（排查召回异常）。
    #[test]
    fn tokenize_drops_whitespace_and_punctuation_tokens() {
        let (dir, idx) = tmp("dbg");
        // 回归：jieba 会把空格/标点也切成 token，必须被过滤（否则跨文档假命中）
        let tokens = idx.tokenize("napi dll");
        assert_eq!(tokens, vec!["napi", "dll"], "{tokens:?}");
        assert!(!tokens.iter().any(|t| t.trim().is_empty()));
        let tokens = idx.tokenize("认证，方案。选型!");
        assert!(
            tokens.iter().all(|t| t.chars().any(|c| c.is_alphanumeric())),
            "{tokens:?}"
        );
        assert!(tokens.contains(&"认证".to_string()), "{tokens:?}");
        // 纯标点查询 → 无 token
        assert!(idx.tokenize("  ，。!  ").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_chars_is_utf8_safe() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 3), "abc");
        assert_eq!(truncate_chars("中文测试", 2), "中文");
        assert_eq!(truncate_chars("", 0), "");
    }

}
