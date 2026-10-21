//! SkillIndex：技能全文索引（Tantivy + jieba 中文分词）与发现 / 导入。
//!
//! 索引 SKILL.md（`---` frontmatter + 正文），支持按名称 / 描述 / 正文全文检索；
//! 技能来源三处：项目 `.astorm/skills`（cwd 向上逐层）、用户全局 `~/.config/astorm/skills`、
//! 用户导入目录（`AppDir.skills_dir`）。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::Deserialize;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, INDEXED, STORED,
    STRING,
};
use tantivy::tokenizer::{LowerCaser, TextAnalyzer};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use tantivy_jieba::JiebaTokenizer;

use super::{DiscoveredSkill, SkillDoc, SkillSummary};

/// 技能内容 / 描述字段使用的分词器名（jieba 搜索模式 + 小写化；索引 / 查询两侧必须一致）。
const TOKENIZER_NAME: &str = "skill_content";
/// IndexWriter 内存预算。
const WRITER_HEAP_BYTES: usize = 15_000_000;
/// `list` 最大返回条数。
const LIST_LIMIT: usize = 10_000;
/// 索引 schema 版本（变更 schema 后递增，触发索引目录重建）。
const SCHEMA_VERSION: &str = "1";

/// 构建 analyzer（`search_mode=false` 用于落盘索引，`true` 用于查询分词）。
fn create_analyzer(search_mode: bool) -> TextAnalyzer {
    TextAnalyzer::builder(JiebaTokenizer::with_search_mode(search_mode))
        .filter(LowerCaser)
        .build()
}

/// 技能索引 schema：`name` 精确匹配（STRING），`description` / `prompt` 走自定义分词（jieba），
/// 其余元数据 STRING|STORED，`enabled` 布尔，`updated_at` 可排序。
fn build_schema() -> Schema {
    let content_indexing = TextFieldIndexing::default()
        .set_tokenizer(TOKENIZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let mut sb = Schema::builder();
    sb.add_text_field("name", STRING | STORED);
    sb.add_text_field(
        "description",
        TextOptions::default()
            .set_indexing_options(content_indexing.clone())
            .set_stored(),
    );
    sb.add_text_field(
        "prompt",
        TextOptions::default()
            .set_indexing_options(content_indexing)
            .set_stored(),
    );
    sb.add_text_field("category", STRING | STORED);
    sb.add_text_field("tags", STRING | STORED);
    sb.add_text_field("version", STRING | STORED);
    sb.add_text_field("author", STRING | STORED);
    sb.add_text_field("source", STRING | STORED);
    sb.add_text_field("source_path", STRING | STORED);
    sb.add_bool_field("enabled", STORED);
    sb.add_i64_field("updated_at", INDEXED | STORED);
    sb.build()
}

/// SKILL.md frontmatter（YAML `---` 块；`metadata.category` / `metadata.tags` 进分类与标签）。
#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    metadata: Option<HashMap<String, String>>,
}

/// 技能全文索引（进程内由 [`super::manager::SkillManager`] 包装为 `Arc<Mutex<...>>`）。
pub struct SkillIndex {
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
    name_field: Field,
    description_field: Field,
    prompt_field: Field,
    category_field: Field,
    tags_field: Field,
    version_field: Field,
    author_field: Field,
    source_field: Field,
    source_path_field: Field,
    enabled_field: Field,
    updated_at_field: Field,
    /// 用户导入目录（`AppDir.skills_dir`）。
    user_dir: PathBuf,
}

impl SkillIndex {
    /// 打开 / 创建索引：`data_dir` 为索引目录（schema 版本不匹配时重建），`user_dir` 为用户技能目录。
    pub fn new(data_dir: &Path, user_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(user_dir).map_err(|e| format!("创建用户技能目录失败: {e}"))?;
        let schema_version = data_dir.join("schema_version");
        let up_to_date = fs::read_to_string(&schema_version).map(|v| v.trim() == SCHEMA_VERSION).unwrap_or(false);
        if !up_to_date {
            let _ = fs::remove_dir_all(data_dir);
            fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
            fs::write(&schema_version, SCHEMA_VERSION).map_err(|e| e.to_string())?;
        }

        let schema = build_schema();
        let index = Index::create_in_dir(data_dir, schema)
            .or_else(|_| Index::open_in_dir(data_dir))
            .map_err(|e| format!("打开/创建技能索引失败: {e}"))?;
        index
            .tokenizers()
            .register(TOKENIZER_NAME, create_analyzer(false));
        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| format!("构建索引 writer 失败: {e}"))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| format!("构建索引 reader 失败: {e}"))?;

        let schema = index.schema();
        let name_field = schema.get_field("name").map_err(|e| format!("schema 缺 name: {e}"))?;
        let description_field = schema.get_field("description").map_err(|e| format!("schema 缺 description: {e}"))?;
        let prompt_field = schema.get_field("prompt").map_err(|e| format!("schema 缺 prompt: {e}"))?;
        let category_field = schema.get_field("category").map_err(|e| format!("schema 缺 category: {e}"))?;
        let tags_field = schema.get_field("tags").map_err(|e| format!("schema 缺 tags: {e}"))?;
        let version_field = schema.get_field("version").map_err(|e| format!("schema 缺 version: {e}"))?;
        let author_field = schema.get_field("author").map_err(|e| format!("schema 缺 author: {e}"))?;
        let source_field = schema.get_field("source").map_err(|e| format!("schema 缺 source: {e}"))?;
        let source_path_field = schema.get_field("source_path").map_err(|e| format!("schema 缺 source_path: {e}"))?;
        let enabled_field = schema.get_field("enabled").map_err(|e| format!("schema 缺 enabled: {e}"))?;
        let updated_at_field = schema.get_field("updated_at").map_err(|e| format!("schema 缺 updated_at: {e}"))?;

        Ok(Self {
            writer: Mutex::new(writer),
            reader,
            name_field,
            description_field,
            prompt_field,
            category_field,
            tags_field,
            version_field,
            author_field,
            source_field,
            source_path_field,
            enabled_field,
            updated_at_field,
            user_dir: user_dir.to_path_buf(),
        })
    }

    /// 索引（更新）一个技能：先按 `name` 删除旧档再写入。
    pub fn index_skill(&mut self, skill: &SkillDoc) -> Result<(), String> {
        let writer = self.writer.lock().map_err(|e| e.to_string())?;
        writer.delete_term(Term::from_field_text(self.name_field, &skill.name));
        writer
            .add_document(doc!(
                self.name_field => skill.name.clone(),
                self.description_field => skill.description.clone(),
                self.prompt_field => skill.prompt.clone(),
                self.category_field => skill.category.clone(),
                self.tags_field => skill.tags.join(", "),
                self.version_field => skill.version.clone(),
                self.author_field => skill.author.clone(),
                self.source_field => skill.source.clone(),
                self.source_path_field => skill.source_path.clone(),
                self.enabled_field => skill.enabled,
                self.updated_at_field => skill.updated_at,
            ))
            .map_err(|e| format!("写入技能索引失败: {e}"))?;
        Ok(())
    }

    /// 按名称移除技能。
    pub fn remove_skill(&mut self, name: &str) -> Result<(), String> {
        let writer = self.writer.lock().map_err(|e| e.to_string())?;
        writer.delete_term(Term::from_field_text(self.name_field, name));
        Ok(())
    }

    /// commit 并 reload 索引（写入可见）。
    pub fn commit(&mut self) -> Result<(), String> {
        let mut writer = self.writer.lock().map_err(|e| e.to_string())?;
        writer.commit().map_err(|e| format!("索引 commit 失败: {e}"))?;
        self.reader.reload().map_err(|e| format!("索引 reload 失败: {e}"))
    }

    /// 全文检索：按名称 / 描述命中（相关度降序）。
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<SkillSummary>, String> {
        self.search_fields(query_str, limit, &[self.name_field, self.description_field])
    }

    /// 全文检索：仅按正文（`prompt`）命中。
    pub fn search_content(&self, query_str: &str, limit: usize) -> Result<Vec<SkillSummary>, String> {
        self.search_fields(query_str, limit, &[self.prompt_field])
    }

    /// 按名称读取完整技能。
    pub fn get_by_name(&self, name: &str) -> Result<Option<SkillDoc>, String> {
        let query = TermQuery::new(
            Term::from_field_text(self.name_field, name),
            IndexRecordOption::Basic,
        );
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .map_err(|e| format!("技能检索失败: {e}"))?;
        if let Some((_, addr)) = top.into_iter().next() {
            let doc = searcher
                .doc::<TantivyDocument>(addr)
                .map_err(|e| format!("读取技能文档失败: {e}"))?;
            return Ok(Some(self.doc_to_full(&doc)));
        }
        Ok(None)
    }

    /// 按名称列表取摘要（空列表 → 空；逐个查询，缺失跳过）。
    pub fn list_by_names(&self, names: &[String]) -> Result<Vec<SkillSummary>, String> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let mut out = Vec::new();
        for name in names {
            let query = TermQuery::new(
                Term::from_field_text(self.name_field, name),
                IndexRecordOption::Basic,
            );
            let top = searcher
                .search(&query, &TopDocs::with_limit(1).order_by_score())
                .map_err(|e| format!("技能检索失败: {e}"))?;
            if let Some((_, addr)) = top.into_iter().next() {
                let doc = searcher
                    .doc::<TantivyDocument>(addr)
                    .map_err(|e| format!("读取技能文档失败: {e}"))?;
                if let Some(s) = self.doc_to_summary(&doc) {
                    out.push(s);
                }
            }
        }
        Ok(out)
    }

    /// 列出全部技能（可按启用状态 / 分类过滤）。
    pub fn list(&self, enabled_only: bool, category: Option<&str>) -> Result<Vec<SkillSummary>, String> {
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&AllQuery, &TopDocs::with_limit(LIST_LIMIT).order_by_score())
            .map_err(|e| format!("技能检索失败: {e}"))?;
        let mut out = Vec::new();
        for (score, addr) in top {
            let doc = searcher
                .doc::<TantivyDocument>(addr)
                .map_err(|e| format!("读取技能文档失败: {e}"))?;
            if let Some(mut s) = self.doc_to_summary(&doc) {
                if enabled_only && !s.enabled {
                    continue;
                }
                if let Some(c) = category {
                    if s.category != c {
                        continue;
                    }
                }
                s.score = Some(score);
                out.push(s);
            }
        }
        Ok(out)
    }

    /// 解析 SKILL.md 内容 → 技能文档。
    pub fn parse_skill_content(content: &str, path: &Path) -> Result<SkillDoc, String> {
        let content = content.strip_prefix('\u{feff}').unwrap_or(content).trim_start();
        if !content.starts_with("---") {
            return Err(format!("SKILL.md 缺少 frontmatter（必须以 --- 开头）: {}", path.display()));
        }
        let rest = &content[3..];
        let sep = rest
            .find("\n---")
            .or_else(|| rest.find("\r\n---"))
            .ok_or_else(|| format!("SKILL.md frontmatter 未闭合（缺少 ---）: {}", path.display()))?;
        let yaml_block = &rest[..sep];
        let body = &rest[sep + 4..];

        let fm: Frontmatter = serde_yaml::from_str(yaml_block)
            .map_err(|e| format!("frontmatter 解析失败（{}）: {e}", path.display()))?;
        let metadata = fm.metadata.unwrap_or_default();
        let category = metadata.get("category").cloned().unwrap_or_else(|| "general".into());
        let tags: Vec<String> = metadata
            .get("tags")
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Ok(SkillDoc {
            name: fm.name,
            description: fm.description,
            prompt: body.trim().to_string(),
            category,
            tags,
            version: fm.version.unwrap_or_else(|| "1.0.0".into()),
            author: fm.author.unwrap_or_default(),
            source: "file".into(),
            source_path: path.to_string_lossy().into_owned(),
            enabled: true,
            updated_at: Utc::now().timestamp(),
        })
    }

    /// 解析 SKILL.md 文件 → 技能文档。
    pub fn parse_skill_file(path: &Path) -> Result<SkillDoc, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        Self::parse_skill_content(&content, path)
    }

    /// 扫描全部搜索目录，列出合法与损坏的技能文件（按名称去重）。
    pub fn discover(&self) -> Result<Vec<DiscoveredSkill>, String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for dir in self.get_search_dirs() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().into_owned();
                let skill_file = path.join("SKILL.md");
                if !skill_file.is_file() {
                    continue;
                }
                let path_str = skill_file.to_string_lossy().into_owned();
                match Self::parse_skill_file(&skill_file) {
                    Ok(doc) => {
                        // 已在索引中的技能跳过(重启/重复发现不覆盖 enabled 等索引态;
                        // 与 astorm-desktop 的「仅返回未索引技能」语义一致)
                        if self.get_by_name(&doc.name)?.is_some() {
                            continue;
                        }
                        if !seen.insert(doc.name.clone()) {
                            continue;
                        }
                        out.push(DiscoveredSkill {
                            name: doc.name,
                            description: doc.description,
                            path: path_str,
                            error: None,
                        });
                    }
                    Err(e) => {
                        // 损坏技能按目录名去重（导入后不会因重命名被反复发现）
                        if !seen.insert(dir_name.clone()) {
                            continue;
                        }
                        out.push(DiscoveredSkill {
                            name: dir_name,
                            description: String::new(),
                            path: path_str,
                            error: Some(e),
                        });
                    }
                }
            }
        }
        Ok(out)
    }

/// 技能名必须是**单个安全路径段**。
///
/// 技能目录直接由 frontmatter `name` / 设置请求拼接（`user_dir.join(name)`），
/// 未校验时 `../../x` 或 `C:\Windows\...` 会逃出技能根目录；Windows 上
/// `C:/...` 作为绝对路径还会让 `join` 丢弃前缀。
pub fn validate_skill_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("技能名称不能为空".into());
    }
    if n == "." || n == ".." {
        return Err("技能名称不能是 . 或 ..".into());
    }
    if n.chars().any(|c| c.is_control()) {
        return Err("技能名称不能包含控制字符".into());
    }
    if n.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err("技能名称不能包含路径分隔符或非法字符".into());
    }
    if n.ends_with('.') || n.ends_with(' ') {
        return Err("技能名称不能以点或空格结尾".into());
    }
    if n.len() > 128 {
        return Err("技能名称过长".into());
    }
    // Windows 设备名（大小写不敏感，带扩展名同样命中）
    let stem = n.split('.').next().unwrap_or(n).to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err("技能名称与 Windows 保留设备名冲突".into());
    }
    Ok(())
}

    /// 从文件导入技能：复制到用户技能目录（SKILL.md + 附属白名单子目录）并索引。
    /// 重复导入时先清空目标目录（幂等覆盖，不残留已删除的旧文件）。
    pub fn import_from_path(&mut self, path: &Path) -> Result<SkillDoc, String> {
        let mut doc = Self::parse_skill_file(path)?;
        Self::validate_skill_name(&doc.name)?;
        let dest_dir = self.user_dir.join(&doc.name);
        let dest = dest_dir.join("SKILL.md");
        // 源已位于目标位置（用户技能目录内重导入）时跳过复制，仅刷新索引
        if path != dest {
            copy_skill_tree(path, &dest_dir)?;
        }
        doc.source = "user".into();
        doc.source_path = dest.to_string_lossy().into_owned();
        doc.updated_at = Utc::now().timestamp();
        self.index_skill(&doc)?;
        Ok(doc)
    }

    /// 从压缩包（zip / tar.gz）导入技能：解压到临时目录后逐个导入。
    pub fn import_from_archive(&mut self, archive_path: &Path) -> Result<Vec<SkillDoc>, String> {
        let tmp = std::env::temp_dir().join(format!("skill_extract_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let outcome = (|| {
            match archive_path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "zip" => {
                    let file = fs::File::open(archive_path)
                        .map_err(|e| format!("打开压缩包失败: {e}"))?;
                    let mut archive =
                        zip::ZipArchive::new(file).map_err(|e| format!("读取 zip 失败: {e}"))?;
                    archive
                        .extract(&tmp)
                        .map_err(|e| format!("解压 zip 失败: {e}"))?;
                }
                "tar.gz" | "tgz" => {
                    let file = fs::File::open(archive_path)
                        .map_err(|e| format!("打开压缩包失败: {e}"))?;
                    let gz = flate2::read::GzDecoder::new(file);
                    let mut archive = tar::Archive::new(gz);
                    archive
                        .unpack(&tmp)
                        .map_err(|e| format!("解压 tar.gz 失败: {e}"))?;
                }
                _ => return Err("不支持的压缩格式（仅支持 zip / tar.gz）".into()),
            }
            let mut files = Vec::new();
            Self::collect_skill_files(&tmp, &mut files);
            let mut docs = Vec::new();
            for f in files {
                if let Ok(doc) = self.import_from_path(&f) {
                    docs.push(doc);
                }
            }
            Ok(docs)
        })();
        let _ = fs::remove_dir_all(&tmp);
        outcome
    }

    /// 用户技能目录。
    pub fn user_skills_dir(&self) -> PathBuf {
        self.user_dir.clone()
    }

    /// 搜索目录：项目 `.astorm/skills`（cwd 向上逐层）+ 全局配置 + 用户导入目录。
    fn get_search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            let mut cur = cwd;
            loop {
                dirs.push(cur.join(".astorm").join("skills"));
                if !cur.pop() {
                    break;
                }
            }
        }
        if let Some(home) = home_dir() {
            dirs.push(home.join(".config").join("astorm").join("skills"));
        }
        dirs.push(self.user_dir.clone());
        dirs
    }

    fn search_fields(
        &self,
        query_str: &str,
        limit: usize,
        fields: &[Field],
    ) -> Result<Vec<SkillSummary>, String> {
        let tokens = self.tokenize(query_str);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let query = self.boolean_query(&tokens, fields);
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| format!("技能检索失败: {e}"))?;
        let mut out = Vec::new();
        for (score, addr) in top {
            let doc = searcher
                .doc::<TantivyDocument>(addr)
                .map_err(|e| format!("读取技能文档失败: {e}"))?;
            if let Some(mut s) = self.doc_to_summary(&doc) {
                s.score = Some(score);
                out.push(s);
            }
        }
        Ok(out)
    }

    fn tokenize(&self, query_str: &str) -> Vec<String> {
        let mut analyzer = create_analyzer(true);
        let mut stream = analyzer.token_stream(query_str);
        let mut tokens = Vec::new();
        while let Some(token) = stream.next() {
            let text = token.text.to_lowercase();
            if !text.is_empty() {
                tokens.push(text);
            }
        }
        tokens
    }

    fn boolean_query(&self, tokens: &[String], fields: &[Field]) -> BooleanQuery {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for field in fields {
            for token in tokens {
                clauses.push((
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_text(*field, token),
                        IndexRecordOption::WithFreqsAndPositions,
                    )),
                ));
            }
        }
        BooleanQuery::new(clauses)
    }

    fn doc_to_summary(&self, doc: &TantivyDocument) -> Option<SkillSummary> {
        Some(SkillSummary {
            name: str_of(doc, self.name_field),
            description: str_of(doc, self.description_field),
            category: str_of(doc, self.category_field),
            version: str_of(doc, self.version_field),
            source: str_of(doc, self.source_field),
            source_path: str_of(doc, self.source_path_field),
            enabled: bool_of(doc, self.enabled_field),
            score: None,
        })
    }

    fn doc_to_full(&self, doc: &TantivyDocument) -> SkillDoc {
        SkillDoc {
            name: str_of(doc, self.name_field),
            description: str_of(doc, self.description_field),
            prompt: str_of(doc, self.prompt_field),
            category: str_of(doc, self.category_field),
            tags: str_of(doc, self.tags_field)
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
            version: str_of(doc, self.version_field),
            author: str_of(doc, self.author_field),
            source: str_of(doc, self.source_field),
            source_path: str_of(doc, self.source_path_field),
            enabled: bool_of(doc, self.enabled_field),
            updated_at: i64_of(doc, self.updated_at_field),
        }
    }

    /// 递归收集目录下所有 `SKILL.md` 路径。
    fn collect_skill_files(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::collect_skill_files(&p, out);
                } else if p.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                    out.push(p);
                }
            }
        }
    }
}

fn str_of(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// 随 SKILL.md 一并复制到用户技能目录的附属子目录白名单（与技能目录规范一致）。
const SKILL_ASSET_DIRS: [&str; 3] = ["scripts", "references", "assets"];

/// 复制技能文件树：SKILL.md 本身 + 同级白名单附属子目录 → `dest_dir`。
/// 目标目录已存在时先清空（重复导入幂等）；白名单外的同级文件 / 目录不复制。
fn copy_skill_tree(skill_file: &Path, dest_dir: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(dest_dir);
    fs::create_dir_all(dest_dir).map_err(|e| format!("创建技能目录失败: {e}"))?;
    fs::copy(skill_file, dest_dir.join("SKILL.md"))
        .map_err(|e| format!("复制技能文件失败: {e}"))?;
    if let Some(parent) = skill_file.parent() {
        for sub in SKILL_ASSET_DIRS {
            let src = parent.join(sub);
            if src.is_dir() {
                copy_dir_all(&src, &dest_dir.join(sub))
                    .map_err(|e| format!("复制附属目录 {sub}/ 失败: {e}"))?;
            }
        }
    }
    Ok(())
}

/// 递归复制目录。
fn copy_dir_all(src: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn bool_of(doc: &TantivyDocument, field: Field) -> bool {
    doc.get_first(field).and_then(|v| v.as_bool()).unwrap_or(true)
}

fn i64_of(doc: &TantivyDocument, field: Field) -> i64 {
    doc.get_first(field).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// 用户主目录（`HOME` 优先，回退 `USERPROFILE`）。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}